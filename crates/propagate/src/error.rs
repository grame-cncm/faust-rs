//! Propagation diagnostics and typed error reporting.
//!
//! Errors in this module cover flat-box validation, arity mismatches,
//! unsupported propagation families, and AD-specific coherence failures. They
//! also map to repository diagnostic codes for compiler-facing reporting.

use super::*;

/// Errors returned by box-to-signal propagation and arity inference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropagateError {
    UnsupportedBox {
        node: TreeId,
        kind: &'static str,
    },
    InvalidIntegerValue {
        node: TreeId,
        field: &'static str,
    },
    NegativeIntegerValue {
        field: &'static str,
        value: i64,
    },
    IntegerTooLarge {
        field: &'static str,
        value: usize,
    },
    InputArityMismatch {
        node: TreeId,
        expected: usize,
        got: usize,
    },
    OutputArityMismatch {
        node: TreeId,
        expected: usize,
        got: usize,
    },
    SeqArityMismatch {
        node: TreeId,
        left_outputs: usize,
        right_inputs: usize,
    },
    SplitArityMismatch {
        node: TreeId,
        left_outputs: usize,
        right_inputs: usize,
    },
    MergeArityMismatch {
        node: TreeId,
        left_outputs: usize,
        right_inputs: usize,
    },
    RecArityMismatch {
        node: TreeId,
        left_inputs: usize,
        left_outputs: usize,
        right_inputs: usize,
        right_outputs: usize,
    },
    FadSeedArity {
        node: TreeId,
        outputs: usize,
    },
    /// A seed of `fad` or `rad` is computed from another seed of the same
    /// call. A seed is differentiated as an independent variable, so the
    /// other seeds' lanes would be zero wherever the body reads it: the
    /// program is refused with the two spellings that mean something.
    AdDependentSeed(Box<DependentSeed>),
    RadBodyArity {
        node: TreeId,
        outputs: usize,
    },
    RadSeedArity {
        node: TreeId,
        outputs: usize,
    },
    /// An AD transform produced a signal tree whose de Bruijn references are
    /// not locally bound by enclosing recursive groups.
    DeBruijnCoherence {
        /// Name of the transform that produced the incoherent tree.
        pass: &'static str,
        /// Description of the first violation reported by `tlib`.
        detail: String,
    },
    RadUnsupportedNode {
        node: TreeId,
        kind: &'static str,
    },
    /// `fad` reached a clock-domain boundary node
    /// (`Seq`/`Clocked`/`TempVar`/`PermVar`/`ZeroPad`/`OnDemand`/
    /// `Upsampling`/`Downsampling`) it cannot differentiate yet.
    ///
    /// Roadmap P0.4: this replaces the silent `zero_tangent` fallback — a
    /// learning loop across an `ondemand` boundary must fail loudly instead of
    /// compiling gradients that are silently zero (the "correctness cliff" of
    /// `porting/ondemand-fad-rad-cohabitation-2026-06-10-en.md` §4). Phase B
    /// (roadmap P5) relaxes this by implementing the boundary dual rules.
    FadUnsupportedNode {
        node: TreeId,
        kind: &'static str,
    },
    /// The augmented twin `fad` built for a clocked block still reaches a
    /// `Clocked` wrapper of the original block's domain, or of a domain
    /// nested in it: a signal of the twin would live in two sibling domains,
    /// and the two blocks would share that state. An invariant of the
    /// forward-mode transformer, checked after every block augmentation;
    /// its failure is a defect of the compiler, not of the program.
    FadTwinKeepsOriginalDomain {
        node: TreeId,
        domain: u32,
    },
}

impl Display for PropagateError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedBox { node, kind } => {
                write!(f, "unsupported box node {} ({kind})", node.as_u32())
            }
            Self::InvalidIntegerValue { node, field } => {
                write!(
                    f,
                    "invalid integer value for `{field}` at node {}",
                    node.as_u32()
                )
            }
            Self::NegativeIntegerValue { field, value } => {
                write!(f, "negative integer value for `{field}`: {value}")
            }
            Self::IntegerTooLarge { field, value } => {
                write!(f, "integer value too large for `{field}`: {value}")
            }
            Self::InputArityMismatch {
                node,
                expected,
                got,
            } => write!(
                f,
                "input arity mismatch at node {}: expected {expected}, got {got}",
                node.as_u32()
            ),
            Self::OutputArityMismatch {
                node,
                expected,
                got,
            } => write!(
                f,
                "output arity mismatch at node {}: expected {expected}, got {got}",
                node.as_u32()
            ),
            Self::SeqArityMismatch {
                node,
                left_outputs,
                right_inputs,
            } => write!(
                f,
                "sequential composition mismatch at node {}: left outputs ({left_outputs}) != right inputs ({right_inputs})",
                node.as_u32()
            ),
            Self::SplitArityMismatch {
                node,
                left_outputs,
                right_inputs,
            } => write!(
                f,
                "split composition mismatch at node {}: left outputs ({left_outputs}) must divide right inputs ({right_inputs})",
                node.as_u32()
            ),
            Self::MergeArityMismatch {
                node,
                left_outputs,
                right_inputs,
            } => write!(
                f,
                "merge composition mismatch at node {}: left outputs ({left_outputs}) must be a multiple of right inputs ({right_inputs})",
                node.as_u32()
            ),
            Self::RecArityMismatch {
                node,
                left_inputs,
                left_outputs,
                right_inputs,
                right_outputs,
            } => write!(
                f,
                "recursive composition mismatch at node {}: right inputs ({right_inputs}) <= left outputs ({left_outputs}) and right outputs ({right_outputs}) <= left inputs ({left_inputs}) are required",
                node.as_u32()
            ),
            Self::FadSeedArity { node, outputs } => write!(
                f,
                "fad seed at node {} must produce at least 1 output, got {outputs}",
                node.as_u32()
            ),
            Self::AdDependentSeed(details) => write!(
                f,
                "{} seed {} `{}` is computed from {}",
                details.mode,
                details.seed,
                details.seed_text,
                dependent_seed_list(&details.depends_on)
            ),
            Self::RadBodyArity { node, outputs } => write!(
                f,
                "rad body at node {} must produce at least 1 output, got {outputs}",
                node.as_u32()
            ),
            Self::RadSeedArity { node, outputs } => write!(
                f,
                "rad seeds at node {} must produce at least 1 output, got {outputs}",
                node.as_u32()
            ),
            Self::DeBruijnCoherence { pass, detail } => {
                write!(f, "De Bruijn coherence error in {pass} transform: {detail}")
            }
            Self::RadUnsupportedNode { node, kind } => write!(
                f,
                "rad cannot differentiate signal node {} ({kind})",
                node.as_u32()
            ),
            Self::FadUnsupportedNode { node, kind } => write!(
                f,
                "fad cannot yet differentiate across an ondemand/upsampling/downsampling \
                 clock boundary: signal node {} ({kind}); keep the whole differentiated \
                 expression inside one clock domain (FAD Phase A) until boundary \
                 crossing (Phase B) lands",
                node.as_u32()
            ),
            Self::FadTwinKeepsOriginalDomain { node, domain } => write!(
                f,
                "fad: the augmented copy of clocked block {} still reaches a signal of the \
                 original's clock domain #{domain}; this is a defect of the compiler",
                node.as_u32()
            ),
        }
    }
}

impl std::error::Error for PropagateError {}

impl From<FlatBoxBuildError> for PropagateError {
    fn from(value: FlatBoxBuildError) -> Self {
        match value {
            FlatBoxBuildError::UnexpectedPostEvalBox { node, kind } => {
                Self::UnsupportedBox { node, kind }
            }
        }
    }
}

/// Converts propagation errors into structured diagnostics used by the compiler facade.
impl ToDiagnostic for PropagateError {
    fn to_diagnostic(&self) -> Diagnostic {
        let message = self.to_string();
        match self {
            Self::UnsupportedBox { .. } => {
                Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_UNSUPPORTED_BOX, message)
                    .with_note(
                        "cause: encountered box node family is not supported in current propagation phase",
                    )
                    .with_help("evaluate box expression first or add propagation support for this node family")
            }
            Self::InputArityMismatch { expected, got, .. }
            | Self::OutputArityMismatch { expected, got, .. } => {
                Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_ARITY_MISMATCH, message)
                    .with_note("cause: propagated bus width differs from required arity")
                    .with_note(
                        "rule: input/output arities at composition boundary must match target signature",
                    )
                    .with_note(format!("expected {expected}, got {got}"))
                    .with_help("adjust composition so input/output bus widths match")
            }
            Self::SeqArityMismatch {
                left_outputs,
                right_inputs,
                ..
            } => Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_ARITY_MISMATCH, message)
                .with_note("cause: sequential composition bus widths do not match")
                .with_note("rule: seq(A, B) requires outputs(A) == inputs(B)")
                .with_note(format!(
                    "sequential composition requires left outputs ({left_outputs}) == right inputs ({right_inputs})"
                ))
                .with_note(format!(
                    "computed: {left_outputs} == {right_inputs} -> {}",
                    left_outputs == right_inputs
                ))
                .with_note(format!(
                    "suggested target: make outputs(A) and inputs(B) equal (common target: {})",
                    left_outputs.max(right_inputs)
                ))
                .with_help("for `A : B`, enforce outputs(A) == inputs(B)")
                .with_help("fix: adjust A or B channel count to same bus width")
                .with_help("template: process = A : B; // outputs(A) == inputs(B)"),
            Self::SplitArityMismatch {
                left_outputs,
                right_inputs,
                ..
            } => Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_ARITY_MISMATCH, message)
                .with_note("cause: split composition divisibility rule is violated")
                .with_note("rule: split(A, B) requires inputs(B) % outputs(A) == 0")
                .with_note(format!(
                    "split composition requires right inputs ({right_inputs}) to be divisible by left outputs ({left_outputs})"
                ))
                .with_note(if *left_outputs == 0 {
                    "computed: divisor outputs(A)=0 is invalid".to_owned()
                } else {
                    format!(
                        "computed: {right_inputs} % {left_outputs} = {}",
                        right_inputs % left_outputs
                    )
                })
                .with_note(if *left_outputs == 0 {
                    "suggested target: outputs(A) must be > 0 before divisibility can be satisfied".to_owned()
                } else {
                    let next = right_inputs
                        .saturating_add(left_outputs - 1)
                        .checked_div(*left_outputs)
                        .expect("left_outputs is non-zero in split arity target")
                        * left_outputs;
                    format!(
                        "suggested target: set inputs(B) to {next} (next multiple of outputs(A)={left_outputs})"
                    )
                })
                .with_help("for `A <: B`, enforce inputs(B) % outputs(A) == 0")
                .with_help("fix: make B inputs a multiple of A outputs")
                .with_help("template: process = A <: B; // inputs(B) % outputs(A) == 0"),
            Self::MergeArityMismatch {
                left_outputs,
                right_inputs,
                ..
            } => Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_ARITY_MISMATCH, message)
                .with_note("cause: merge composition multiple rule is violated")
                .with_note("rule: merge(A, B) requires outputs(A) % inputs(B) == 0")
                .with_note(format!(
                    "merge composition requires left outputs ({left_outputs}) to be a multiple of right inputs ({right_inputs})"
                ))
                .with_note(if *right_inputs == 0 {
                    "computed: divisor inputs(B)=0 is invalid".to_owned()
                } else {
                    format!(
                        "computed: {left_outputs} % {right_inputs} = {}",
                        left_outputs % right_inputs
                    )
                })
                .with_note(if *right_inputs == 0 {
                    "suggested target: inputs(B) must be > 0 before multiple constraints can be satisfied".to_owned()
                } else {
                    let next = left_outputs
                        .saturating_add(right_inputs - 1)
                        .checked_div(*right_inputs)
                        .expect("right_inputs is non-zero in merge arity target")
                        * right_inputs;
                    format!(
                        "suggested target: set outputs(A) to {next} (next multiple of inputs(B)={right_inputs})"
                    )
                })
                .with_help("for `A :> B`, enforce outputs(A) % inputs(B) == 0")
                .with_help("fix: make A outputs a multiple of B inputs")
                .with_help("template: process = A :> B; // outputs(A) % inputs(B) == 0"),
            Self::RecArityMismatch {
                left_inputs,
                left_outputs,
                right_inputs,
                right_outputs,
                ..
            } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_RECURSION_MISMATCH,
                message,
            )
            .with_note("cause: recursive feedback arity constraints are not satisfied")
            .with_note(
                "rule: rec(A, B) requires right_inputs <= left_outputs and right_outputs <= left_inputs",
            )
            .with_note(format!(
                "required: right_inputs ({right_inputs}) <= left_outputs ({left_outputs}) and right_outputs ({right_outputs}) <= left_inputs ({left_inputs})"
            ))
            .with_note(format!(
                "computed: {} <= {} is {}, {} <= {} is {}",
                right_inputs,
                left_outputs,
                right_inputs <= left_outputs,
                right_outputs,
                left_inputs,
                right_outputs <= left_inputs
            ))
            .with_note(format!(
                "suggested target: set outputs(A) >= {} and inputs(A) >= {}",
                right_inputs, right_outputs
            ))
            .with_help(
                "for `A ~ B`, enforce inputs(B) <= outputs(A) and outputs(B) <= inputs(A)",
            )
            .with_help("fix: reduce B feedback bus or widen matching A arities")
            .with_help(
                "template: process = A ~ B; // inputs(B)<=outputs(A), outputs(B)<=inputs(A)",
            ),
            Self::InvalidIntegerValue { field, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: integer-valued propagation field has invalid runtime representation")
            .with_note(format!("invalid integer for field `{field}`")),
            Self::NegativeIntegerValue { field, value } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_GENERIC_FAILURE,
                message,
            )
            .with_note(
                "cause: field constrained to non-negative integer received a negative value",
            )
            .with_note(format!("field `{field}` is negative: {value}")),
            Self::IntegerTooLarge { field, value } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: integer field exceeds propagation-supported bounds")
            .with_note(format!("field `{field}` exceeds target range: {value}")),
            Self::FadSeedArity { outputs, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_ARITY_MISMATCH,
                message,
            )
            .with_note("cause: fad seed expression must produce at least 1 output signal")
            .with_note(format!("seed produced {outputs} output(s)")),
            Self::AdDependentSeed(details) => {
                let DependentSeed {
                    node,
                    mode,
                    seed,
                    seed_text,
                    depends_on,
                } = details.as_ref();
                let derivative = if *mode == "rad" { "adjoint" } else { "tangent" };
                let lanes = dependent_lane_list(depends_on);
                let names = depends_on
                    .iter()
                    .map(|(_, text)| format!("`{text}`"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                let dropped = depends_on
                    .iter()
                    .map(|(_, text)| text.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                Diagnostic::new(
                    Severity::Error,
                    Stage::Propagate,
                    codes::PROP_AD_DEPENDENT_SEED,
                    message,
                )
                .with_note(
                    "cause: a seed is differentiated as an independent variable; what computes it is a constant for every lane",
                )
                .with_note(format!(
                    "{lanes} would be 0 wherever the body reads `{seed_text}`, since the {derivative} of {names} does not pass through a seed"
                ))
                .with_help(format!(
                    "to differentiate with respect to {names}, drop `{seed_text}` from the seed list: {mode}(…, ({dropped}))"
                ))
                .with_help(format!(
                    "to differentiate with respect to `{seed_text}` as one quantity, seed it alone: {mode}(…, {seed_text})"
                ))
                .with_fact("node", u64::from(node.as_u32()))
                .with_fact("mode", *mode)
                .with_fact("seed", *seed as u64)
                .with_fact(
                    "depends_on",
                    depends_on
                        .iter()
                        .map(|(lane, _)| lane.to_string())
                        .collect::<Vec<String>>(),
                )
            }
            Self::RadBodyArity { outputs, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_ARITY_MISMATCH,
                message,
            )
            .with_note("cause: rad body expression must produce at least 1 output signal")
            .with_note(format!("body produced {outputs} output(s)")),
            Self::RadSeedArity { outputs, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_ARITY_MISMATCH,
                message,
            )
            .with_note("cause: rad seeds expression must produce at least 1 output signal")
            .with_note(format!("seeds produced {outputs} output(s)")),
            Self::DeBruijnCoherence { pass, detail } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_GENERIC_FAILURE,
                message,
            )
            .with_note(format!(
                "cause: {pass} produced a de Bruijn reference outside its local recursive scope"
            ))
            .with_note(detail.as_str())
            .with_help("this indicates an internal AD transform bug; preserve the failing DSP as a regression fixture"),
            Self::RadUnsupportedNode { kind, .. } => {
                // Tailor the diagnostic by family so users get an actionable
                // explanation instead of a generic "unsupported" wall.
                let mut diag = Diagnostic::new(
                    Severity::Error,
                    Stage::Propagate,
                    codes::PROP_UNSUPPORTED_BOX,
                    message,
                );
                match *kind {
                    "delay-or-prefix" => {
                        diag = diag
                            .with_note(
                                "cause: reverse-mode AD of a delay or prefix would require a non-causal transpose `adj_x[n] += adj_y[n+1]`",
                            )
                            .with_note(
                                "rule: the local symbolic RAD sweep delegates temporal nodes to the BlockReverseAD fallback, which supplies the required finite block tape",
                            )
                            .with_help(
                                "if this diagnostic is surfaced directly, preserve the DSP as a fallback-dispatch regression",
                            );
                    }
                    "recursive-linear-transpose" => {
                        diag = diag
                            .with_note(
                                "cause: RAD reached a linear time-invariant recursive feedback; exact reverse mode needs the phase-E1 linear-transpose path",
                            )
                            .with_note(
                                "rule: the ReverseTimeRec fast path is dormant; public RAD routes recursive bodies through BlockReverseAD",
                            )
                            .with_help(
                                "if this diagnostic is surfaced directly, preserve the DSP as a fallback-dispatch regression",
                            );
                    }
                    "recursive-block-linear-time-varying" => {
                        diag = diag
                            .with_note(
                                "cause: RAD reached a linear but time-varying recursive feedback; exact reverse mode needs block coefficient replay",
                            )
                            .with_note(
                                "rule: the specialized phase-E2 replay path is not enabled; public RAD uses BlockReverseAD for this class",
                            )
                            .with_help(
                                "if this diagnostic is surfaced directly, preserve the DSP as a fallback-dispatch regression",
                            );
                    }
                    "recursive-bptt-required" => {
                        diag = diag
                            .with_note(
                                "cause: RAD reached nonlinear recursive feedback; exact reverse mode requires finite-horizon BPTT",
                            )
                            .with_note(
                                "rule: the specialized phase-F path is not enabled; public RAD uses the generic BlockReverseAD finite-block fallback",
                            )
                            .with_help(
                                "if this diagnostic is surfaced directly, preserve the DSP as a fallback-dispatch regression",
                            );
                    }
                    "recursive-projection" => {
                        diag = diag
                            .with_note(
                                "cause: reverse-mode AD of a recursive feedback would require a non-causal transpose over an infinite stream",
                            )
                            .with_note(
                                "rule: the local symbolic RAD sweep delegates recursion / projection nodes to the BlockReverseAD fallback",
                            )
                            .with_help(
                                "if this diagnostic is surfaced directly, preserve the DSP as a fallback-dispatch regression",
                            );
                    }
                    "writable-table" | "writable-table-or-waveform-direct" => {
                        diag = diag
                            .with_note("cause: rad does not differentiate through mutable table state")
                            .with_help("read the table contents via rdtable(...) instead of writing/reading in the same expression");
                    }
                    "ffun" => {
                        diag = diag
                            .with_note(
                                "cause: rad recognizes only the unary chain-rule rules (tanh/sinh/cosh and inverses)",
                            )
                            .with_help(
                                "ensure the foreign function is unary and listed in the rad rule set, or use fad(...) which has the same coverage",
                            );
                    }
                    "soundfile" => {
                        diag = diag
                            .with_note("cause: rad does not differentiate through soundfile content or accessors")
                            .with_help("treat soundfile reads as constants in the differentiated expression");
                    }
                    "ondemand"
                    | "upsampling"
                    | "downsampling"
                    | "clocked wrapper"
                    | "clocked sequencing (Seq)"
                    | "clock-domain boundary variable (TempVar/PermVar/ZeroPad)" => {
                        diag = diag
                            .with_note(
                                "cause: rad reached ondemand/upsampling/downsampling clock-domain machinery; reverse mode across a clock boundary requires the clock-aware tape (roadmap P8)",
                            )
                            .with_help(
                                "keep the whole differentiated expression inside one clock domain, or use fad(...) once FAD Phase B (boundary crossing) lands",
                            );
                    }
                    _ => {
                        diag = diag
                            .with_note(format!(
                                "cause: rad has no differentiation rule for the `{kind}` signal family in this phase"
                            ))
                            .with_help(
                                "rewrite the differentiated expression to avoid this signal family, or use fad(...) when the case is a feed-forward derivative",
                            );
                    }
                }
                diag
            }
            Self::FadUnsupportedNode { kind, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_AD_CLOCK_BOUNDARY,
                message,
            )
            .with_note(format!(
                "cause: fad reached the clock-domain construct `{kind}`; differentiating \
                 across an ondemand/upsampling/downsampling boundary is not implemented yet"
            ))
            .with_note(
                "rule: tangents would be silently zero across the boundary, so propagation \
                 rejects the program instead (FAD Phase A supports differentiation strictly \
                 inside one clock domain)",
            )
            .with_help(
                "move the whole differentiated expression inside the clocked block \
                 (e.g. ondemand(fad(...)) instead of fad(ondemand(...), ...))",
            ),
            Self::FadTwinKeepsOriginalDomain { domain, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_GENERIC_FAILURE,
                message,
            )
            .with_note(format!(
                "cause: a signal of the augmented block is still annotated with clock \
                 domain #{domain} (the original block's, or one nested in it), so the two \
                 blocks would share its state"
            ))
            .with_help(
                "this is a defect of the forward-mode transformer (crates/propagate/src/forward_ad.rs, \
                 augment_block): please report the program",
            ),
        }
    }
}

/// The payload of [`PropagateError::AdDependentSeed`], boxed so that the error
/// stays small on every `Result` it travels through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependentSeed {
    /// The `fad`/`rad` box.
    pub node: TreeId,
    /// `"fad"` or `"rad"`.
    pub mode: &'static str,
    /// 1-based lane of the dependent seed.
    pub seed: usize,
    /// A short rendering of the dependent seed.
    pub seed_text: String,
    /// 1-based lanes and renderings of the seeds it is computed from.
    pub depends_on: Vec<(usize, String)>,
}

/// `seed 2 `x``, `seeds 2 `x` and 3 `y``, `seeds 1 `a`, 2 `b` and 4 `d``.
fn dependent_seed_list(depends_on: &[(usize, String)]) -> String {
    let items = depends_on
        .iter()
        .map(|(lane, text)| format!("{lane} `{text}`"))
        .collect::<Vec<_>>();
    match items.len() {
        0 => String::from("no seed"),
        1 => format!("seed {}", items[0]),
        n => format!("seeds {} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

/// `lane 2`, `lanes 2 and 3`, `lanes 1, 2 and 4`.
fn dependent_lane_list(depends_on: &[(usize, String)]) -> String {
    let items = depends_on
        .iter()
        .map(|(lane, _)| lane.to_string())
        .collect::<Vec<_>>();
    match items.len() {
        0 => String::from("no lane"),
        1 => format!("lane {}", items[0]),
        n => format!("lanes {} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}
