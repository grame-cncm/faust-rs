//! Faust-syntax serialization of box trees.
//!
//! # Source provenance (C++)
//! - `compiler/boxes/ppbox.hh`
//! - `compiler/boxes/ppbox.cpp`
//! - `boxpp::print(std::ostream&)` and `boxppShared::print(std::ostream&)`
//!
//! # Public API mapping status
//! `adapted`. The emitted text is the parity target; the implementation
//! differs from C++ in three deliberate ways, each documented at its site:
//! no process-global printer state, an explicit worklist instead of recursion,
//! and `Result` instead of exceptions.
//!
//! # The two printers
//! [`box_pp`] expands a box into one self-contained expression. A box tree is
//! a DAG after evaluation, so this expansion is worst-case exponential in the
//! node count; it exists for small sub-expressions and for the two node kinds
//! that cannot be shared (see below).
//!
//! [`box_pp_shared`] is the printer behind the `-e` option. Every composite
//! node is assigned an `ID_<n>` and emitted once as a top-level definition, so
//! output size is linear in the DAG size. Definitions are emitted in
//! first-completion order, which guarantees every `ID_<n>` is defined before
//! its first use and makes the result a valid Faust definition list.
//!
//! # Why abstractions are never shared
//! `Abstr` (`\x.(body)`) and `Symbolic` (`\(slot).(body)`) bind a variable
//! inside their body. Hoisting a sub-expression of that body into a top-level
//! `ID_<n> = ...;` definition would move an occurrence of the bound variable
//! out of its binder's scope, turning it free and changing the program's
//! meaning. Both node kinds therefore render their bodies through [`box_pp`],
//! matching `compiler/boxes/ppbox.cpp:550,717`.
//!
//! # Operator parenthesization
//! Composition operators carry a priority (`:` and `<:`/`:>` = 1, `,` = 2,
//! `~` = 4). A binary node is parenthesized when the priority of the context
//! it appears in exceeds its own — `streambinop`/`streambinopShared`,
//! `compiler/boxes/ppbox.cpp:174,187`. Under sharing, the priority observed at
//! a node's *first* visit is the one baked into its memoized definition text;
//! later uses print `ID_<n>`, which never needs parentheses. This is why the
//! same sub-diagram can appear as `ID_1 = (ID_0 : *)` in one program and
//! `ID_1 = ID_0 : *` in another.

use std::collections::HashMap;
use std::fmt::Write as _;

use tlib::{NodeKind, TreeArena};

use crate::{BoxId, BoxMatch, match_box};

/// Internal DSP computation precision, mirroring C++ `gGlobal->gFloatSize`.
///
/// It selects the numeric-literal suffix and, for `ffunction`, how many of the
/// four per-precision foreign names are printed.
///
/// # Deliberate deviation
/// C++ derives the literal suffix from `gOutputLang` as well as the float size
/// (`compiler/generator/floats.cpp:49`), so `-lang rust -e` drops the `f`
/// suffix that `-lang cpp -e` emits for the very same program. An expanded
/// `.dsp` is Faust source, not backend code, so the suffix here depends on the
/// precision alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatSize {
    /// Single precision: literals carry an `f` suffix.
    #[default]
    Single,
    /// Double precision: literals carry no suffix.
    Double,
    /// Quad precision: literals carry an `L` suffix.
    Quad,
}

impl FloatSize {
    /// Returns the numeric-literal suffix, mirroring C++ `inumix()`.
    #[must_use]
    pub fn num_suffix(self) -> &'static str {
        match self {
            Self::Single => "f",
            Self::Double => "",
            Self::Quad => "L",
        }
    }

    /// Returns how many foreign-function names one `ffunction` prints.
    ///
    /// C++ loops `i < gGlobal->gFloatSize` over the four-slot name list
    /// (`compiler/boxes/ppbox.cpp:288`), so single precision prints one name
    /// and double prints two separated by `|`.
    #[must_use]
    pub fn foreign_name_count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Double => 2,
            Self::Quad => 3,
        }
    }

    /// Returns whether literals must round-trip through `f32` rather than `f64`.
    #[must_use]
    fn rounds_through_f32(self) -> bool {
        self == Self::Single
    }
}

/// Failure to serialize one box.
///
/// # Deliberate deviation
/// C++ throws `faustexception` from inside a stream operator
/// (`compiler/boxes/ppbox.cpp:485,741`). Returning an error keeps the failure
/// on the call path and, more importantly, makes an unprintable node
/// impossible to ignore: a printer that degrades an unrecognized node to `_`
/// would emit text that compiles to a different program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoxPrintError {
    /// The node is not a box, or is a box shape this printer does not cover.
    NotAValidBox {
        /// Offending node.
        node: BoxId,
        /// Short structural description for the diagnostic.
        kind: &'static str,
    },
    /// A node payload had the wrong shape for its tag (malformed arena).
    MalformedNode {
        /// Offending node.
        node: BoxId,
        /// What the printer expected to find.
        expected: &'static str,
    },
}

impl std::fmt::Display for BoxPrintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAValidBox { node, kind } => {
                write!(f, "node {} is not a valid box ({kind})", node.as_u32())
            }
            Self::MalformedNode { node, expected } => {
                write!(
                    f,
                    "node {} is malformed: expected {expected}",
                    node.as_u32()
                )
            }
        }
    }
}

impl std::error::Error for BoxPrintError {}

/// One box program serialized with sharing.
///
/// `definitions` holds the `ID_<n> = ...;` lines in emission order and `root`
/// holds the expression the entry point is bound to. Callers assemble the
/// final document, because the entry-point name and any surrounding `declare`
/// header are not this module's concern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedBoxProgram {
    /// Top-level definitions, already terminated by `;` and ordered so that
    /// every identifier is defined before its first use.
    pub definitions: Vec<String>,
    /// Expression for the serialized root.
    pub root: String,
}

impl SharedBoxProgram {
    /// Renders the program as Faust source binding the root to `entry_point`.
    #[must_use]
    pub fn render(&self, entry_point: &str) -> String {
        let mut out = String::new();
        for definition in &self.definitions {
            out.push_str(definition);
            out.push('\n');
        }
        let _ = writeln!(out, "{entry_point} = {};", self.root);
        out
    }
}

// ── Operator priorities ───────────────────────────────────────────────────────

/// Priority of `:`, `<:` and `:>` (`compiler/boxes/ppbox.cpp:592-596`).
const PRIORITY_COMPOSE: u8 = 1;
/// Priority of `,` (`compiler/boxes/ppbox.cpp:598`).
const PRIORITY_PAR: u8 = 2;
/// Priority of `~` (`compiler/boxes/ppbox.cpp:600`).
const PRIORITY_REC: u8 = 4;
/// Priority used for a context that never parenthesizes its operand.
const PRIORITY_TOP: u8 = 0;

// ── Public entry points ───────────────────────────────────────────────────────

/// Serializes one box as a single self-contained Faust expression.
///
/// Shared sub-diagrams are expanded at every occurrence, so prefer
/// [`box_pp_shared`] for whole programs. `priority` is the priority of the
/// context the expression will sit in; pass `0` for a standalone expression.
///
/// # Errors
/// Returns [`BoxPrintError`] when the tree contains a node this printer does
/// not recognize as a box.
pub fn box_pp(
    arena: &TreeArena,
    node: BoxId,
    priority: u8,
    float_size: FloatSize,
) -> Result<String, BoxPrintError> {
    let mut printer = Printer::new(arena, float_size, Sharing::Off);
    printer.render(node, priority)
}

/// Serializes one box program with sharing, the printer behind `-e`.
///
/// # Errors
/// Returns [`BoxPrintError`] when the tree contains a node this printer does
/// not recognize as a box.
pub fn box_pp_shared(
    arena: &TreeArena,
    root: BoxId,
    float_size: FloatSize,
) -> Result<SharedBoxProgram, BoxPrintError> {
    let mut printer = Printer::new(arena, float_size, Sharing::On);
    let root_text = printer.render(root, PRIORITY_TOP)?;
    Ok(SharedBoxProgram {
        definitions: printer.definitions,
        root: root_text,
    })
}

// ── Real-literal formatting ───────────────────────────────────────────────────

/// Highest `%g` precision C++ tries before giving up (`MAX_PRECISION`).
const MAX_PRECISION: usize = 32;

/// Formats one real the way C++ `T(double)` does.
///
/// The C++ implementation (`compiler/generator/Text.cpp:227-300`) prints with
/// `%.*g` at increasing precision until the text round-trips back to the same
/// value at the target precision, appends `.0` when the result looks like an
/// integer, and appends the precision suffix. The shortest round-trip decimal
/// produced by Rust's `Display` is *not* the same text: `440.0` there is
/// `4.4e+02f` here, because `%g` switches to exponent form as soon as the
/// exponent reaches the chosen precision.
#[must_use]
pub fn format_real(value: f64, float_size: FloatSize) -> String {
    // C++ narrows to the target precision *before* formatting
    // (`float v = (float)n;`), so the digits describe the value the compiled
    // program will actually hold. Formatting the unnarrowed `f64` instead
    // stops one representation too early: `2*pi` prints as `6.2831853`, which
    // rounds to the same `f32` but is not the text C++ emits.
    let value = if float_size.rounds_through_f32() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "single precision deliberately narrows before formatting"
        )]
        let narrowed = value as f32;
        f64::from(narrowed)
    } else {
        value
    };

    let mut text = String::new();
    for precision in 1..=MAX_PRECISION {
        text = format_g(value, precision);
        if round_trips(&text, value, float_size) {
            break;
        }
    }
    if !text.contains(['.', 'e']) {
        text.push_str(".0");
    }
    text.push_str(float_size.num_suffix());
    text
}

/// Returns whether `text` parses back to exactly `value` at the target precision.
fn round_trips(text: &str, value: f64, float_size: FloatSize) -> bool {
    if float_size.rounds_through_f32() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "single precision compares the f32 round-trip, matching C++ strtof"
        )]
        let target = value as f32;
        text.parse::<f32>().is_ok_and(|parsed| parsed == target)
    } else {
        text.parse::<f64>().is_ok_and(|parsed| parsed == value)
    }
}

/// Formats one value the way C's `%.*g` does.
///
/// Rust has no `%g`, so the rule is reproduced explicitly: render with
/// `precision` significant digits, choose exponent form when the decimal
/// exponent is below `-4` or at least `precision`, strip trailing zeros from
/// the significand, and print the exponent with a sign and at least two
/// digits.
fn format_g(value: f64, precision: usize) -> String {
    if value == 0.0 {
        // `%g` prints an unsigned zero for -0.0 only through the significand
        // rules below; short-circuiting keeps the exponent computation off a
        // value whose logarithm is undefined.
        return if value.is_sign_negative() {
            "-0".to_owned()
        } else {
            "0".to_owned()
        };
    }
    if !value.is_finite() {
        // Unreachable for box literals, but a printer must not emit a value it
        // cannot describe; `%g` spells these `inf`/`nan`.
        return if value.is_nan() {
            "nan".to_owned()
        } else if value.is_sign_negative() {
            "-inf".to_owned()
        } else {
            "inf".to_owned()
        };
    }

    // The exponent `%g` tests is the one of the value *after* rounding to
    // `precision` significant digits, which is why it is read back from the
    // scientific rendering instead of computed from `log10`.
    let scientific = format!("{value:.*e}", precision.saturating_sub(1));
    let exponent = scientific
        .rsplit_once('e')
        .and_then(|(_, exp)| exp.parse::<i32>().ok())
        .unwrap_or(0);

    #[expect(
        clippy::cast_possible_wrap,
        clippy::cast_possible_truncation,
        reason = "precision is bounded by MAX_PRECISION"
    )]
    let precision_i32 = precision as i32;
    if exponent < -4 || exponent >= precision_i32 {
        let (mantissa, _) = scientific.split_once('e').unwrap_or((&scientific, "0"));
        let mantissa = strip_trailing_zeros(mantissa);
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    } else {
        let decimals = usize::try_from(precision_i32 - 1 - exponent).unwrap_or(0);
        strip_trailing_zeros(&format!("{value:.decimals$}"))
    }
}

/// Removes trailing fractional zeros, and a trailing `.`, from a decimal.
fn strip_trailing_zeros(text: &str) -> String {
    if !text.contains('.') {
        return text.to_owned();
    }
    let trimmed = text.trim_end_matches('0');
    trimmed.strip_suffix('.').unwrap_or(trimmed).to_owned()
}

// ── Printer ───────────────────────────────────────────────────────────────────

/// Whether composite nodes are hoisted into `ID_<n>` definitions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sharing {
    /// Every composite node is emitted once and referenced by identifier.
    On,
    /// Every occurrence is expanded in place.
    Off,
}

/// One step of the explicit traversal.
///
/// # Deliberate deviation
/// C++ recurses through `operator<<`, which puts one native stack frame per
/// box-tree level. Evaluated diagrams reach thousands of levels — a
/// 2000-stage `seq` is in the corpus — so the traversal is an explicit stack
/// here and only the output text lives on the heap.
enum Step {
    /// Produce the text used where `node` appears.
    Enter {
        node: BoxId,
        priority: u8,
        sharing: Sharing,
    },
    /// Combine the already-produced child texts into `node`'s own text.
    Build {
        node: BoxId,
        priority: u8,
        sharing: Sharing,
    },
}

/// Serialization state for one printing session.
///
/// # Deliberate deviation
/// C++ keeps the memo table and the definition trace in `gGlobal`
/// (`gBoxTable`, `gBoxTrace`, `gBoxCounter`), so two expansions in one process
/// interleave their identifier numbering. Here they are session-local.
struct Printer<'a> {
    arena: &'a TreeArena,
    float_size: FloatSize,
    sharing: Sharing,
    /// Composite node → assigned `ID_` number.
    table: HashMap<BoxId, u32>,
    /// `ID_<n> = ...;` lines in emission order.
    definitions: Vec<String>,
    /// Texts produced by completed steps, consumed by their parent's `Build`.
    values: Vec<String>,
    /// Pending traversal steps.
    steps: Vec<Step>,
}

impl<'a> Printer<'a> {
    /// Creates one printing session.
    fn new(arena: &'a TreeArena, float_size: FloatSize, sharing: Sharing) -> Self {
        Self {
            arena,
            float_size,
            sharing,
            table: HashMap::new(),
            definitions: Vec::new(),
            values: Vec::new(),
            steps: Vec::new(),
        }
    }

    /// Renders one node, running the traversal to completion.
    fn render(&mut self, node: BoxId, priority: u8) -> Result<String, BoxPrintError> {
        let sharing = self.sharing;
        self.steps.push(Step::Enter {
            node,
            priority,
            sharing,
        });
        while let Some(step) = self.steps.pop() {
            match step {
                Step::Enter {
                    node,
                    priority,
                    sharing,
                } => self.enter(node, priority, sharing)?,
                Step::Build {
                    node,
                    priority,
                    sharing,
                } => self.build(node, priority, sharing)?,
            }
        }
        self.values.pop().ok_or(BoxPrintError::MalformedNode {
            node,
            expected: "one rendered value",
        })
    }

    /// Handles one `Enter` step: emit an atom, reuse an identifier, or descend.
    fn enter(&mut self, node: BoxId, priority: u8, sharing: Sharing) -> Result<(), BoxPrintError> {
        if sharing == Sharing::On
            && let Some(id) = self.table.get(&node)
        {
            self.values.push(format!("ID_{id}"));
            return Ok(());
        }

        if let Some(atom) = self.atom_text(node)? {
            self.values.push(atom);
            return Ok(());
        }

        let children = self.render_children(node)?;
        self.steps.push(Step::Build {
            node,
            priority,
            sharing,
        });
        // Pushed in reverse so the traversal visits children left to right,
        // which is what fixes the `ID_` numbering order.
        for child in children.into_iter().rev() {
            self.steps.push(Step::Enter {
                node: child.node,
                priority: child.priority,
                sharing: child.sharing(sharing),
            });
        }
        Ok(())
    }

    /// Handles one `Build` step: assemble this node's text from its children.
    fn build(&mut self, node: BoxId, priority: u8, sharing: Sharing) -> Result<(), BoxPrintError> {
        let child_count = self.render_children(node)?.len();
        let at = self.values.len() - child_count;
        let parts = self.values.split_off(at);
        let text = self.compose(node, priority, sharing, &parts)?;

        if sharing == Sharing::On && shares_identifier(self.arena, node) {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "one program cannot reach 2^32 shared nodes"
            )]
            let id = self.definitions.len() as u32;
            self.definitions.push(format!("ID_{id} = {text};"));
            self.table.insert(node, id);
            self.values.push(format!("ID_{id}"));
        } else {
            self.values.push(text);
        }
        Ok(())
    }
}

/// One child to render, with the context it is rendered in.
struct ChildRef {
    /// Node to render.
    node: BoxId,
    /// Priority of the context the child appears in.
    priority: u8,
    /// Whether the child escapes sharing because it sits under a binder.
    unshared: bool,
}

impl ChildRef {
    /// Child rendered in the parent's context.
    fn new(node: BoxId, priority: u8) -> Self {
        Self {
            node,
            priority,
            unshared: false,
        }
    }

    /// Child rendered inside a binder, where hoisting would free a variable.
    fn bound(node: BoxId) -> Self {
        Self {
            node,
            priority: PRIORITY_TOP,
            unshared: true,
        }
    }

    /// Resolves the sharing mode this child is rendered under.
    fn sharing(&self, parent: Sharing) -> Sharing {
        if self.unshared { Sharing::Off } else { parent }
    }
}

/// Returns whether a composite node is hoisted into its own `ID_` definition.
///
/// Mirrors which branches of `boxppShared::print` use `BOX_INSERT_ID`. The
/// nodes excluded here print inline even under sharing: `Access` composes two
/// already-shared operands, and the pattern-matching nodes are debugging
/// shapes that never survive a successful evaluation.
fn shares_identifier(arena: &TreeArena, node: BoxId) -> bool {
    !matches!(
        match_box(arena, node),
        BoxMatch::Access(_, _)
            | BoxMatch::Abstr(_, _)
            | BoxMatch::Symbolic(_, _)
            | BoxMatch::Case(_)
            | BoxMatch::PatternVar(_)
    )
}

// ── Atoms ─────────────────────────────────────────────────────────────────────

impl Printer<'_> {
    /// Returns the inline text of `node` when it renders without children.
    ///
    /// `None` means the node is composite and must be descended into. An error
    /// means the node is not a box at all.
    fn atom_text(&self, node: BoxId) -> Result<Option<String>, BoxPrintError> {
        // Raw payload nodes appear as operands of box nodes (labels, foreign
        // names, list terminators) and have no `BoxMatch` shape of their own.
        match self.arena.kind(node) {
            Some(NodeKind::Nil) => return Ok(Some("()".to_owned())),
            Some(NodeKind::Symbol(name)) => return Ok(Some(name.to_string())),
            Some(NodeKind::StringLiteral(value)) => return Ok(Some(quote(value))),
            None => {
                return Err(BoxPrintError::NotAValidBox {
                    node,
                    kind: "dangling node",
                });
            }
            _ => {}
        }

        let text = match match_box(self.arena, node) {
            BoxMatch::Int(value) => value.to_string(),
            BoxMatch::Real(value) => format_real(value, self.float_size),
            BoxMatch::Wire => "_".to_owned(),
            BoxMatch::Cut => "!".to_owned(),
            BoxMatch::Ident(name) => name.to_owned(),
            BoxMatch::Slot(id) => format!("x{id}"),
            BoxMatch::Environment => "environment".to_owned(),
            other => match primitive_name(other) {
                Some(name) => name.to_owned(),
                None => return Ok(None),
            },
        };
        Ok(Some(text))
    }
}

/// Returns the source spelling of one nullary primitive box.
///
/// Mirrors `prim1name`/`prim2name`/`prim3name`/`prim4name`/`prim5name`
/// (`compiler/boxes/ppbox.cpp:40-163`) and the `xtended::name()` values of
/// `compiler/extended/*.hh`.
///
/// A handful of these names — `exp10`, `round`, `lowest`, `highest`,
/// `assertbounds` — are compiler-internal primitives with no source spelling,
/// exactly as in C++. A program that reaches one of them expands to text the
/// parser rejects; the round-trip checks in this module's tests are what
/// surface that, rather than a silent substitution.
fn primitive_name(shape: BoxMatch<'_>) -> Option<&'static str> {
    let name = match shape {
        // Binary composition primitives.
        BoxMatch::Add => "+",
        BoxMatch::Sub => "-",
        BoxMatch::Mul => "*",
        BoxMatch::Div => "/",
        BoxMatch::Rem => "%",
        BoxMatch::And => "&",
        BoxMatch::Or => "|",
        BoxMatch::Xor => "xor",
        BoxMatch::Lsh => "<<",
        BoxMatch::Rsh => ">>",
        BoxMatch::LRsh => ">>>",
        BoxMatch::Lt => "<",
        BoxMatch::Le => "<=",
        BoxMatch::Gt => ">",
        BoxMatch::Ge => ">=",
        BoxMatch::Eq => "==",
        BoxMatch::Ne => "!=",
        BoxMatch::Delay => "@",
        BoxMatch::Prefix => "prefix",
        BoxMatch::Attach => "attach",
        BoxMatch::Enable => "enable",
        BoxMatch::Control => "control",
        // Unary primitives.
        BoxMatch::Delay1 => "mem",
        BoxMatch::IntCast => "int",
        BoxMatch::FloatCast => "float",
        BoxMatch::Lowest => "lowest",
        BoxMatch::Highest => "highest",
        // Table and selection primitives.
        BoxMatch::ReadOnlyTable => "rdtable",
        BoxMatch::WriteReadTable => "rwtable",
        BoxMatch::Select2 => "select2",
        BoxMatch::Select3 => "select3",
        BoxMatch::AssertBounds => "assertbounds",
        // Extended math primitives.
        BoxMatch::Abs => "abs",
        BoxMatch::Acos => "acos",
        BoxMatch::Asin => "asin",
        BoxMatch::Atan => "atan",
        BoxMatch::Atan2 => "atan2",
        BoxMatch::Ceil => "ceil",
        BoxMatch::Cos => "cos",
        BoxMatch::Exp => "exp",
        BoxMatch::Exp10 => "exp10",
        BoxMatch::Floor => "floor",
        BoxMatch::Fmod => "fmod",
        BoxMatch::Log => "log",
        BoxMatch::Log10 => "log10",
        BoxMatch::Max => "max",
        BoxMatch::Min => "min",
        BoxMatch::Pow => "pow",
        BoxMatch::Remainder => "remainder",
        BoxMatch::Rint => "rint",
        BoxMatch::Round => "round",
        BoxMatch::Sin => "sin",
        BoxMatch::Sqrt => "sqrt",
        BoxMatch::Tan => "tan",
        _ => return None,
    };
    Some(name)
}

/// Wraps one label in a Faust string literal, escaping what would end it.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(character),
        }
    }
    out.push('"');
    out
}

// ── Children and composition ──────────────────────────────────────────────────

impl Printer<'_> {
    /// Returns the operands `node` renders, in C++ visiting order.
    ///
    /// The order fixes `ID_` numbering, so it follows the order the C++ stream
    /// expressions evaluate their `boxppShared(...)` operands in.
    fn render_children(&self, node: BoxId) -> Result<Vec<ChildRef>, BoxPrintError> {
        // A cons list renders as `(a,b,c)`; its spine is walked here rather
        // than treated as two operands so the elements land in source order.
        if matches!(self.arena.kind(node), Some(NodeKind::Cons)) {
            return Ok(list_elements(self.arena, node)
                .into_iter()
                .map(|element| ChildRef::new(element, PRIORITY_TOP))
                .collect());
        }

        let children = match match_box(self.arena, node) {
            BoxMatch::Seq(a, b) | BoxMatch::Split(a, b) | BoxMatch::Merge(a, b) => vec![
                ChildRef::new(a, PRIORITY_COMPOSE),
                ChildRef::new(b, PRIORITY_COMPOSE),
            ],
            BoxMatch::Par(a, b) => vec![
                ChildRef::new(a, PRIORITY_PAR),
                ChildRef::new(b, PRIORITY_PAR),
            ],
            BoxMatch::Rec(a, b) => vec![
                ChildRef::new(a, PRIORITY_REC),
                ChildRef::new(b, PRIORITY_REC),
            ],
            BoxMatch::Appl(fun, args) => vec![
                ChildRef::new(fun, PRIORITY_TOP),
                ChildRef::new(args, PRIORITY_TOP),
            ],
            BoxMatch::Access(a, b) => vec![
                ChildRef::new(a, PRIORITY_TOP),
                ChildRef::new(b, PRIORITY_TOP),
            ],
            // Both binders render their body unshared: hoisting any part of it
            // would take an occurrence of the bound variable out of scope.
            BoxMatch::Abstr(arg, body) => vec![ChildRef::bound(arg), ChildRef::bound(body)],
            BoxMatch::Symbolic(slot, body) => vec![ChildRef::bound(slot), ChildRef::bound(body)],
            BoxMatch::IPar(a, b, c)
            | BoxMatch::ISeq(a, b, c)
            | BoxMatch::ISum(a, b, c)
            | BoxMatch::IProd(a, b, c) => vec![
                ChildRef::new(a, PRIORITY_TOP),
                ChildRef::new(b, PRIORITY_TOP),
                ChildRef::new(c, PRIORITY_TOP),
            ],
            BoxMatch::Route(inputs, outputs, routes) => vec![
                ChildRef::new(inputs, PRIORITY_TOP),
                ChildRef::new(outputs, PRIORITY_TOP),
                ChildRef::new(routes, PRIORITY_TOP),
            ],
            BoxMatch::Ondemand(inner)
            | BoxMatch::Upsampling(inner)
            | BoxMatch::Downsampling(inner)
            | BoxMatch::Inputs(inner)
            | BoxMatch::Outputs(inner) => vec![ChildRef::new(inner, PRIORITY_TOP)],
            BoxMatch::ForwardAD(expr, seeds) | BoxMatch::ReverseAD(expr, seeds) => vec![
                ChildRef::new(expr, PRIORITY_TOP),
                ChildRef::new(seeds, PRIORITY_TOP),
            ],
            BoxMatch::Modulation(_ident, body) => vec![ChildRef::new(body, PRIORITY_TOP)],
            BoxMatch::Metadata(expr, _mdlist) => vec![ChildRef::new(expr, PRIORITY_TOP)],
            BoxMatch::VGroup(_, body) | BoxMatch::HGroup(_, body) | BoxMatch::TGroup(_, body) => {
                vec![ChildRef::new(body, PRIORITY_TOP)]
            }
            BoxMatch::VSlider(_, cur, min, max, step)
            | BoxMatch::HSlider(_, cur, min, max, step)
            | BoxMatch::NumEntry(_, cur, min, max, step) => vec![
                ChildRef::new(cur, PRIORITY_TOP),
                ChildRef::new(min, PRIORITY_TOP),
                ChildRef::new(max, PRIORITY_TOP),
                ChildRef::new(step, PRIORITY_TOP),
            ],
            BoxMatch::VBargraph(_, min, max) | BoxMatch::HBargraph(_, min, max) => vec![
                ChildRef::new(min, PRIORITY_TOP),
                ChildRef::new(max, PRIORITY_TOP),
            ],
            BoxMatch::Soundfile(_, chan) => vec![ChildRef::new(chan, PRIORITY_TOP)],
            BoxMatch::Waveform(values) => list_elements(self.arena, values)
                .into_iter()
                // Waveform samples print through the unshared path in C++
                // (`compiler/boxes/ppbox.cpp:690`), so a sample shared with the
                // rest of the diagram still appears literally inside the braces.
                .map(ChildRef::bound)
                .collect(),
            // Labels, foreign names and type codes are read directly during
            // composition rather than rendered as operands, because they are
            // payload rather than sub-diagrams.
            BoxMatch::Button(_)
            | BoxMatch::Checkbox(_)
            | BoxMatch::Component(_)
            | BoxMatch::Library(_)
            | BoxMatch::ImportFile(_)
            | BoxMatch::FConst(_, _, _)
            | BoxMatch::FVar(_, _, _)
            | BoxMatch::FFun(_)
            | BoxMatch::Case(_) => Vec::new(),
            other => return Err(unprintable(node, other)),
        };
        Ok(children)
    }

    /// Assembles the text of `node` from its already-rendered operands.
    fn compose(
        &self,
        node: BoxId,
        priority: u8,
        sharing: Sharing,
        parts: &[String],
    ) -> Result<String, BoxPrintError> {
        if matches!(self.arena.kind(node), Some(NodeKind::Cons)) {
            return Ok(format!("({})", parts.join(",")));
        }

        let separators = Separators::for_sharing(sharing);
        let text = match match_box(self.arena, node) {
            BoxMatch::Seq(_, _) => binop(parts, separators.seq, PRIORITY_COMPOSE, priority),
            BoxMatch::Split(_, _) => binop(parts, separators.split, PRIORITY_COMPOSE, priority),
            BoxMatch::Merge(_, _) => binop(parts, separators.merge, PRIORITY_COMPOSE, priority),
            BoxMatch::Par(_, _) => binop(parts, separators.par, PRIORITY_PAR, priority),
            BoxMatch::Rec(_, _) => binop(parts, separators.rec, PRIORITY_REC, priority),
            BoxMatch::Appl(_, _) => format!("{}{}", parts[0], parts[1]),
            BoxMatch::Access(_, _) => format!("{}.{}", parts[0], parts[1]),
            BoxMatch::Abstr(_, _) => format!("\\{}.({})", parts[0], parts[1]),
            BoxMatch::Symbolic(_, _) => format!("\\({}).({})", parts[0], parts[1]),
            BoxMatch::IPar(_, _, _) => format!("par({}, {}) {{{}}}", parts[0], parts[1], parts[2]),
            BoxMatch::ISeq(_, _, _) => format!("seq({}, {}) {{{}}}", parts[0], parts[1], parts[2]),
            BoxMatch::ISum(_, _, _) => format!("sum({}, {}) {{{}}}", parts[0], parts[1], parts[2]),
            BoxMatch::IProd(_, _, _) => {
                format!("prod({}, {}) {{{}}}", parts[0], parts[1], parts[2])
            }
            BoxMatch::Route(_, _, _) => {
                format!("route({},{},{})", parts[0], parts[1], parts[2])
            }
            BoxMatch::Ondemand(_) => format!("ondemand({})", parts[0]),
            BoxMatch::Upsampling(_) => format!("upsampling({})", parts[0]),
            // C++ never reaches a downsampling branch: `boxppShared::print`
            // tests `isBoxUpsampling` twice (`compiler/boxes/ppbox.cpp:615-617`)
            // and throws on `BoxDownsampling`, so `faust -e` cannot expand any
            // program containing one. The node is printed here.
            BoxMatch::Downsampling(_) => format!("downsampling({})", parts[0]),
            BoxMatch::Inputs(_) => format!("inputs({})", parts[0]),
            BoxMatch::Outputs(_) => format!("outputs({})", parts[0]),
            BoxMatch::ForwardAD(_, _) => format!("fad({}, {})", parts[0], parts[1]),
            BoxMatch::ReverseAD(_, _) => format!("rad({}, {})", parts[0], parts[1]),
            BoxMatch::Modulation(ident, _) => {
                format!("modulate({}).({})", self.raw_text(ident)?, parts[0])
            }
            // Mirrors C++ (`compiler/boxes/ppbox.cpp:660`), which drops the
            // metadata list and leaves a comment. Expression-level `declare`
            // metadata is therefore lost by expansion in both compilers; the
            // document header still carries the file-level `declare` set.
            BoxMatch::Metadata(_, _) => format!("{}/* md */", parts[0]),
            BoxMatch::VGroup(label, _) => {
                format!("vgroup({}, {})", self.label_text(label)?, parts[0])
            }
            BoxMatch::HGroup(label, _) => {
                format!("hgroup({}, {})", self.label_text(label)?, parts[0])
            }
            BoxMatch::TGroup(label, _) => {
                format!("tgroup({}, {})", self.label_text(label)?, parts[0])
            }
            BoxMatch::VSlider(label, _, _, _, _) => {
                format!("vslider({}, {})", self.label_text(label)?, parts.join(", "))
            }
            BoxMatch::HSlider(label, _, _, _, _) => {
                format!("hslider({}, {})", self.label_text(label)?, parts.join(", "))
            }
            BoxMatch::NumEntry(label, _, _, _, _) => {
                format!("nentry({}, {})", self.label_text(label)?, parts.join(", "))
            }
            BoxMatch::VBargraph(label, _, _) => {
                format!(
                    "vbargraph({}, {})",
                    self.label_text(label)?,
                    parts.join(", ")
                )
            }
            BoxMatch::HBargraph(label, _, _) => {
                format!(
                    "hbargraph({}, {})",
                    self.label_text(label)?,
                    parts.join(", ")
                )
            }
            BoxMatch::Soundfile(label, _) => {
                format!("soundfile({}, {})", self.label_text(label)?, parts[0])
            }
            BoxMatch::Waveform(_) => format!("waveform{{{}}}", parts.join(",")),
            BoxMatch::Button(label) => format!("button({})", self.label_text(label)?),
            BoxMatch::Checkbox(label) => format!("checkbox({})", self.label_text(label)?),
            BoxMatch::Component(label) => format!("component({})", self.label_text(label)?),
            BoxMatch::Library(label) => format!("library({})", self.label_text(label)?),
            BoxMatch::ImportFile(label) => format!("import({})", self.label_text(label)?),
            BoxMatch::FConst(kind, name, file) => format!(
                "fconstant({} {}, {})",
                self.foreign_type_text(kind)?,
                self.raw_text(name)?,
                self.raw_text(file)?
            ),
            BoxMatch::FVar(kind, name, file) => format!(
                "fvariable({} {}, {})",
                self.foreign_type_text(kind)?,
                self.raw_text(name)?,
                self.raw_text(file)?
            ),
            BoxMatch::FFun(descriptor) => self.foreign_function_text(descriptor)?,
            BoxMatch::Case(rules) => self.case_text(rules)?,
            other => return Err(unprintable(node, other)),
        };
        Ok(text)
    }
}

/// Composition-operator spellings, which differ between the two printers.
///
/// `boxpp` writes `,`, `<:`, `:>` and `~` unpadded while `boxppShared` pads
/// all four (`compiler/boxes/ppbox.cpp:311-319` against `:592-600`). The
/// difference is cosmetic — both re-parse identically — but reproducing it is
/// what lets an expansion be diffed against the C++ compiler's own output.
struct Separators {
    seq: &'static str,
    split: &'static str,
    merge: &'static str,
    par: &'static str,
    rec: &'static str,
}

impl Separators {
    /// Returns the spellings the given printer uses.
    fn for_sharing(sharing: Sharing) -> Self {
        match sharing {
            Sharing::On => Self {
                seq: " : ",
                split: " <: ",
                merge: " :> ",
                par: ", ",
                rec: " ~ ",
            },
            Sharing::Off => Self {
                seq: " : ",
                split: "<:",
                merge: ":>",
                par: ",",
                rec: "~",
            },
        }
    }
}

/// Joins two operands with `op`, parenthesizing when the context binds tighter.
///
/// Mirrors `streambinop`/`streambinopShared`
/// (`compiler/boxes/ppbox.cpp:174,187`).
fn binop(parts: &[String], op: &str, own_priority: u8, context_priority: u8) -> String {
    let joined = format!("{}{op}{}", parts[0], parts[1]);
    if context_priority > own_priority {
        format!("({joined})")
    } else {
        joined
    }
}

/// Returns the elements of one cons list, or an empty vector for a non-list.
fn list_elements(arena: &TreeArena, list: BoxId) -> Vec<BoxId> {
    let mut out = Vec::new();
    let mut cursor = list;
    while let Some(NodeKind::Cons) = arena.kind(cursor) {
        let Some(head) = arena.hd(cursor) else { break };
        out.push(head);
        let Some(tail) = arena.tl(cursor) else { break };
        cursor = tail;
    }
    out
}

/// Builds the error for a box shape this printer cannot express in Faust source.
///
/// The shapes routed here are the ones whose payload lives in an evaluator
/// side-table or in an environment tree with no source syntax: printing a
/// placeholder for them, as C++ does for `closure[...]`, `PM[...]` and
/// `with { <tree dump> }`, produces text the parser rejects. They cannot occur
/// in a successfully evaluated `process`.
fn unprintable(node: BoxId, shape: BoxMatch<'_>) -> BoxPrintError {
    let kind = match shape {
        BoxMatch::WithLocalDef(_, _) => "unevaluated `with { ... }`",
        BoxMatch::ModifLocalDef(_, _) => "unevaluated local-definition modifier",
        BoxMatch::WithRecDef(_, _, _) => "unevaluated `letrec`",
        BoxMatch::Closure(_) => "evaluator closure handle",
        BoxMatch::PatternMatcher(_) => "partially applied pattern matcher",
        BoxMatch::PatternVar(_) => "pattern variable",
        BoxMatch::Ffunction(_, _, _) => "bare foreign-function descriptor",
        BoxMatch::Unknown => "unrecognized node",
        _ => "unsupported box",
    };
    BoxPrintError::NotAValidBox { node, kind }
}

// ── Payload rendering ─────────────────────────────────────────────────────────

impl Printer<'_> {
    /// Returns a UI label or file reference as a quoted Faust string.
    ///
    /// Mirrors C++ `tree2quotedstr`.
    fn label_text(&self, node: BoxId) -> Result<String, BoxPrintError> {
        match self.arena.kind(node) {
            Some(NodeKind::StringLiteral(value) | NodeKind::Symbol(value)) => Ok(quote(value)),
            _ => match match_box(self.arena, node) {
                BoxMatch::Ident(name) => Ok(quote(name)),
                _ => Err(BoxPrintError::MalformedNode {
                    node,
                    expected: "a string label",
                }),
            },
        }
    }

    /// Returns one payload node exactly as C++ streams a `Tree`.
    ///
    /// A symbol prints bare and a string prints quoted, which is why the same
    /// `ffunction` renders its include file as `<math.h>` but its (empty)
    /// library file as `""`.
    fn raw_text(&self, node: BoxId) -> Result<String, BoxPrintError> {
        match self.arena.kind(node) {
            Some(NodeKind::Symbol(value)) => Ok(value.to_string()),
            Some(NodeKind::StringLiteral(value)) => Ok(quote(value)),
            Some(NodeKind::Int(value)) => Ok(value.to_string()),
            _ => match match_box(self.arena, node) {
                BoxMatch::Ident(name) => Ok(name.to_owned()),
                BoxMatch::Int(value) => Ok(value.to_string()),
                _ => Err(BoxPrintError::MalformedNode {
                    node,
                    expected: "a symbol, string or integer payload",
                }),
            },
        }
    }

    /// Returns the source spelling of one foreign type code.
    ///
    /// Mirrors C++ `type2str` (`compiler/boxes/ppbox.cpp:216`), including its
    /// empty spelling for the wildcard code.
    fn foreign_type_text(&self, node: BoxId) -> Result<&'static str, BoxPrintError> {
        let code = match self.arena.kind(node) {
            Some(NodeKind::Int(value)) => *value,
            _ => {
                return Err(BoxPrintError::MalformedNode {
                    node,
                    expected: "a foreign type code",
                });
            }
        };
        Ok(match code {
            0 => "int",
            1 => "float",
            _ => "",
        })
    }

    /// Renders one `ffunction` from its descriptor.
    ///
    /// The descriptor's signature list is `cons(return_type, cons(names,
    /// argument_types))`, so the argument count is the list length minus two —
    /// the same arithmetic as C++ `ffarity`. Only the first
    /// [`FloatSize::foreign_name_count`] names are printed, joined by `|`.
    fn foreign_function_text(&self, descriptor: BoxId) -> Result<String, BoxPrintError> {
        let BoxMatch::Ffunction(signature, incfile, libfile) = match_box(self.arena, descriptor)
        else {
            return Err(BoxPrintError::MalformedNode {
                node: descriptor,
                expected: "a foreign-function descriptor",
            });
        };
        let signature = list_elements(self.arena, signature);
        let (return_type, names) = match signature.split_first() {
            Some((return_type, rest)) => match rest.split_first() {
                Some((names, _)) => (*return_type, *names),
                None => {
                    return Err(BoxPrintError::MalformedNode {
                        node: descriptor,
                        expected: "a foreign signature with a name list",
                    });
                }
            },
            None => {
                return Err(BoxPrintError::MalformedNode {
                    node: descriptor,
                    expected: "a non-empty foreign signature",
                });
            }
        };

        let mut out = format!("ffunction({}", self.foreign_type_text(return_type)?);
        let name_slots = list_elements(self.arena, names);
        let mut separator = ' ';
        for slot in name_slots.iter().take(self.float_size.foreign_name_count()) {
            let _ = write!(out, "{separator}{}", self.raw_text(*slot)?);
            separator = '|';
        }

        let mut separator = '(';
        for argument in signature.iter().skip(2) {
            let _ = write!(out, "{separator}{}", self.foreign_type_text(*argument)?);
            separator = ',';
        }
        out.push(')');
        let _ = write!(
            out,
            ",{},{})",
            self.raw_text(incfile)?,
            self.raw_text(libfile)?
        );
        Ok(out)
    }

    /// Renders one `case { ... }` rule list.
    ///
    /// Rules are printed through the unshared path, mirroring C++ `printRule`
    /// (`compiler/boxes/ppbox.cpp:504`): a rule's left-hand side binds pattern
    /// variables used by its right-hand side, so neither side may be hoisted.
    fn case_text(&self, rules: BoxId) -> Result<String, BoxPrintError> {
        let mut out = String::from("case {");
        for rule in list_elements(self.arena, rules) {
            let Some(NodeKind::Cons) = self.arena.kind(rule) else {
                return Err(BoxPrintError::MalformedNode {
                    node: rule,
                    expected: "a `pattern => body` rule pair",
                });
            };
            let (Some(patterns), Some(body)) = (self.arena.hd(rule), self.arena.tl(rule)) else {
                return Err(BoxPrintError::MalformedNode {
                    node: rule,
                    expected: "a well-formed rule pair",
                });
            };
            let mut separator = '(';
            for pattern in list_elements(self.arena, patterns) {
                let _ = write!(
                    out,
                    "{separator}{}",
                    box_pp(self.arena, pattern, PRIORITY_TOP, self.float_size)?
                );
                separator = ',';
            }
            if separator == '(' {
                out.push('(');
            }
            let _ = write!(
                out,
                ") => {}; ",
                box_pp(self.arena, body, PRIORITY_TOP, self.float_size)?
            );
        }
        out.push('}');
        Ok(out)
    }
}
