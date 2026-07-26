//! `codebox` backend: RNBO / `gen~` code generation from FIR.
//!
//! Codebox is the textual language of Cycling '74 RNBO's `codebox~` object.
//! This backend emits a flat codebox file an RNBO patch can import.
//!
//! # Source provenance (C++)
//! - `compiler/generator/codebox/codebox_code_container.cpp` (module assembly)
//! - `compiler/generator/codebox/codebox_instructions.hh` (the emitter)
//!
//! Mapping status: `adapted`. The emitted *syntax* follows the C++ backend, but
//! not the emitted *structure*: faust-rs lowers signals to FIR differently (a
//! recursion becomes a scalar here and a shift buffer there), so byte parity
//! with `faust -lang codebox` is neither achievable nor a goal. The contract is
//! numerical equivalence, as for every other backend — see
//! `porting/codebox-backend-port-plan-2026-07-26-en.md` §5.2.
//!
//! # What the target language forces
//! - **Identifiers may not end with a digit**, so every emitted name carries a
//!   `_cb` suffix ([`codebox_var_name`]). Faust names variables `fRec0`,
//!   `IOTA0`, `fSlow1`…, so this is not optional.
//! - **Storage classes are syntactic**: `@state` for values that persist across
//!   calls, `let` for locals. A `@state` scalar must carry an initialiser.
//! - **Arrays are constructed, not annotated**:
//!   `@state t_cb = new FixedFloatArray(8);`, filled element by element in
//!   `dspsetup`.
//! - **One sample at a time**: `compute(i0, …)` returns a list of outputs.
//! - **No soundfiles**: rejected with a typed error, as upstream does.
//!
//! # Expected lowering
//! The module must have been lowered with external control and the one-sample
//! processing API: `control` supplies codebox's `control()`, and `frame`
//! supplies the body of `compute()`. Selecting that lowering is the caller's
//! job; this module reports what it finds.

use std::fmt::Write as _;

use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

/// Suffix appended to every emitted identifier, because codebox rejects
/// identifiers ending in a digit.
const VAR_SUFFIX: &str = "_cb";

/// Codebox backend options.
#[derive(Clone, Debug, Default)]
pub struct CodeboxOptions {
    /// Emit `double`-shaped float literals (`0.5`) instead of `0.5f`.
    ///
    /// Codebox has a single `number` type, so this changes only literal
    /// spelling — the same thing `-double` does in the C++ backend, and nothing
    /// more.
    pub double_precision: bool,
}

/// Stable error codes for this backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// The FIR root did not decode to a module.
    RootNotModule,
    /// A construct codebox cannot express.
    Unsupported,
}

impl CodegenErrorCode {
    /// Returns the stable machine-readable code string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-CBOX-0001",
            Self::Unsupported => "FRS-CGEN-CBOX-0002",
        }
    }
}

/// Typed codebox backend error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodegenError {
    /// Stable machine-readable code.
    pub code: CodegenErrorCode,
    /// Human-readable message.
    pub message: String,
}

impl CodegenError {
    /// Builds an error with the given code and message.
    #[must_use]
    pub fn new(code: CodegenErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CodegenError {}

/// Decoded FIR module root.
struct ModuleView {
    dsp_struct: FirId,
    globals: FirId,
    functions: FirId,
    num_inputs: usize,
    num_outputs: usize,
}

/// Where a declaration is being emitted, which decides `@state` versus `let`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The `// Fields` section: persistent state.
    Fields,
    /// Inside a function body: locals.
    Body,
}

/// Appends the `_cb` suffix to a Faust identifier.
///
/// C++ parity: `codeboxVarName`.
#[must_use]
pub fn codebox_var_name(name: &str) -> String {
    format!("{name}{VAR_SUFFIX}")
}

/// Generates codebox code from a FIR module root.
///
/// # Errors
/// - `FRS-CGEN-CBOX-0001` when `module` does not decode to a FIR module.
/// - `FRS-CGEN-CBOX-0002` for a construct codebox cannot express.
pub fn generate_codebox_module(
    store: &FirStore,
    module: FirId,
    options: &CodeboxOptions,
) -> Result<String, CodegenError> {
    let view = decode_module(store, module)?;
    let mut out = String::new();

    let _ = writeln!(out, "// Code generated with faust-rs");
    let _ = writeln!(out, "// Additional functions");
    // Params come from the UI, in phase C2.
    let _ = writeln!(out, "// Params");

    let _ = writeln!(out, "// Globals");
    for stmt in block_items(store, view.globals) {
        if matches!(match_fir(store, stmt), FirMatch::DeclareFun { .. }) {
            emit_stmt(store, &mut out, options, stmt, 0, Phase::Body)?;
        }
    }

    let _ = writeln!(out, "// Fields");
    for stmt in block_items(store, view.dsp_struct) {
        emit_stmt(store, &mut out, options, stmt, 0, Phase::Fields)?;
    }
    for stmt in block_items(store, view.globals) {
        if matches!(match_fir(store, stmt), FirMatch::DeclareVar { .. }) {
            emit_stmt(store, &mut out, options, stmt, 0, Phase::Fields)?;
        }
    }
    let _ = writeln!(out, "@state fUpdated : Int = 0;");

    emit_dspsetup(store, &mut out, options, &view)?;
    emit_control(store, &mut out, options, &view)?;
    emit_update(&mut out);
    emit_compute(store, &mut out, options, &view)?;
    emit_top_level(&mut out, &view);

    Ok(out)
}

/// Emits `dspsetup()`, the single init entry point RNBO calls on start and on
/// sample-rate change.
///
/// It folds what the DSP API splits across `classInit`,
/// `instanceResetUserInterface`, `instanceClear` and `instanceConstants`, in
/// that order — the order the C++ container uses — with literal array contents
/// written first, since arrays are declared constructed-but-empty in the field
/// section.
fn emit_dspsetup(
    store: &FirStore,
    out: &mut String,
    options: &CodeboxOptions,
    view: &ModuleView,
) -> Result<(), CodegenError> {
    let _ = writeln!(out, "// Init");
    let _ = writeln!(out, "function dspsetup() {{");
    let _ = writeln!(out, "\tfUpdated = true;");

    emit_array_initialisers(store, out, options, view.dsp_struct)?;
    emit_array_initialisers(store, out, options, view.globals)?;

    for name in [
        "classInit",
        "instanceResetUserInterface",
        "instanceClear",
        "instanceConstants",
    ] {
        if let Some(body) = find_function_body(store, view.functions, name) {
            emit_block_items(store, out, options, body, 1, Phase::Body)?;
        }
    }

    let _ = writeln!(out, "}}");
    Ok(())
}

/// Writes the contents of every literal array declared in `block`.
///
/// C++ parity: `CodeboxInitArraysVisitor`. Only literal tables (waveforms) are
/// emitted here; zero-filling arrives through the FIR's own `instanceClear`
/// loops on the ordinary statement path.
fn emit_array_initialisers(
    store: &FirStore,
    out: &mut String,
    options: &CodeboxOptions,
    block: FirId,
) -> Result<(), CodegenError> {
    for stmt in block_items(store, block) {
        let FirMatch::DeclareVar {
            name,
            typ,
            init: Some(init),
            ..
        } = match_fir(store, stmt)
        else {
            continue;
        };
        if !matches!(typ, FirType::Array(..)) {
            continue;
        }
        let target = codebox_var_name(&name);
        for (index, element) in array_literal_elements(store, options, init)
            .into_iter()
            .enumerate()
        {
            let _ = writeln!(out, "\t{target}[{index}] = {element};");
        }
    }
    Ok(())
}

/// Renders a literal array's elements, or nothing when `value` is not a literal
/// table.
fn array_literal_elements(store: &FirStore, options: &CodeboxOptions, value: FirId) -> Vec<String> {
    match match_fir(store, value) {
        FirMatch::Int32Array { values, .. } => values.iter().map(ToString::to_string).collect(),
        FirMatch::Float32Array { values, .. } => values
            .iter()
            .map(|v| float_literal(f64::from(*v), options))
            .collect(),
        FirMatch::Float64Array { values, .. }
        | FirMatch::QuadArray { values, .. }
        | FirMatch::FixedPointArray { values, .. } => {
            values.iter().map(|v| float_literal(*v, options)).collect()
        }
        _ => Vec::new(),
    }
}

/// Emits `control()`, the block-rate work RNBO runs only when a parameter moved.
fn emit_control(
    store: &FirStore,
    out: &mut String,
    options: &CodeboxOptions,
    view: &ModuleView,
) -> Result<(), CodegenError> {
    let _ = writeln!(out, "// Control");
    let _ = writeln!(out, "function control() {{");
    if let Some(body) = find_function_body(store, view.functions, "control") {
        emit_block_items(store, out, options, body, 1, Phase::Body)?;
    }
    let _ = writeln!(out, "}}");
    Ok(())
}

/// Emits `update()`.
///
/// C1 emits the skeleton only: the per-parameter dirty checks and the argument
/// list come from the UI in phase C2. The `fUpdated` protocol is already right —
/// `control()` runs once after any parameter moved — there is simply nothing to
/// watch yet.
fn emit_update(out: &mut String) {
    let _ = writeln!(out, "// Update parameters");
    let _ = writeln!(out, "function update() {{");
    let _ = writeln!(out, "\tif (fUpdated) {{ fUpdated = false; control(); }}");
    let _ = writeln!(out, "}}");
}

/// Emits `compute()`, the one-sample body.
///
/// The FIR `frame` function reads `inputs[n]` and writes `outputs[n]`; codebox
/// takes one argument per input and returns a list instead. The two are bridged
/// by `input<n>_cb` / `output<n>_cb` locals declared here — which is also why
/// the field pass skips FIR's own `input`/`output` declarations.
fn emit_compute(
    store: &FirStore,
    out: &mut String,
    options: &CodeboxOptions,
    view: &ModuleView,
) -> Result<(), CodegenError> {
    let _ = writeln!(out, "// Compute one frame");
    let args: Vec<String> = (0..view.num_inputs).map(|i| format!("i{i}")).collect();
    let _ = writeln!(out, "function compute({}) {{", args.join(","));

    for i in 0..view.num_inputs {
        let _ = writeln!(out, "\tlet input{i}_cb : number = i{i};");
    }
    for i in 0..view.num_outputs {
        let _ = writeln!(out, "\tlet output{i}_cb : number = 0;");
    }

    if let Some(body) = find_function_body(store, view.functions, "frame") {
        emit_block_items(store, out, options, body, 1, Phase::Body)?;
    }

    let returned: Vec<String> = (0..view.num_outputs)
        .map(|i| format!("output{i}_cb"))
        .collect();
    let _ = writeln!(out, "\treturn [{}];", returned.join(","));
    let _ = writeln!(out, "}}");
    Ok(())
}

/// Emits the file-scope wiring RNBO evaluates once per sample.
fn emit_top_level(out: &mut String, view: &ModuleView) {
    let _ = writeln!(out, "// Update parameters");
    let _ = writeln!(out, "update();");
    let _ = writeln!(out, "// Compute one frame");
    let ins: Vec<String> = (0..view.num_inputs)
        .map(|i| format!("in{}", i + 1))
        .collect();
    let _ = writeln!(out, "outputs = compute({});", ins.join(","));
    let _ = writeln!(
        out,
        "// Write the outputs: audio ones and bargraph as additional audio signals"
    );
    for index in 0..view.num_outputs {
        let _ = writeln!(out, "out{} = outputs[{index}];", index + 1);
    }
}

/// Emits every statement of a block body.
fn emit_block_items(
    store: &FirStore,
    out: &mut String,
    options: &CodeboxOptions,
    block: FirId,
    indent: usize,
    phase: Phase,
) -> Result<(), CodegenError> {
    for stmt in block_items(store, block) {
        emit_stmt(store, out, options, stmt, indent, phase)?;
    }
    Ok(())
}

/// Emits one statement.
fn emit_stmt(
    store: &FirStore,
    out: &mut String,
    options: &CodeboxOptions,
    stmt: FirId,
    indent: usize,
    phase: Phase,
) -> Result<(), CodegenError> {
    let tab = "\t".repeat(indent);
    match match_fir(store, stmt) {
        // FIR provenance labels are comments, not code.
        FirMatch::Label(_) | FirMatch::NullStatement => Ok(()),

        FirMatch::DeclareVar {
            name,
            typ,
            access,
            init,
        } => {
            // `input`/`output` become `compute` locals instead; see `emit_compute`.
            if name.starts_with("input") || name.starts_with("output") {
                return Ok(());
            }
            let var = codebox_var_name(&name);
            if let FirType::Array(_, size) = &typ {
                let _ = writeln!(out, "{tab}@state {var} = new FixedFloatArray({size});");
                return Ok(());
            }
            let persistent = matches!(access, AccessType::Struct | AccessType::Static);
            let keyword = if phase == Phase::Fields || persistent {
                "@state"
            } else {
                "let"
            };
            let _ = write!(out, "{tab}{keyword} {var} : {}", emit_type(&typ));
            if let Some(init) = init {
                let value = emit_value(store, options, init)?;
                let _ = write!(out, " = {value}");
            } else if phase == Phase::Fields || persistent {
                // A `@state` scalar must be initialised.
                let _ = write!(out, " = 0");
            }
            let _ = writeln!(out, ";");
            Ok(())
        }

        FirMatch::StoreVar { name, value, .. } => {
            let value = emit_value(store, options, value)?;
            let _ = writeln!(out, "{tab}{} = {value};", store_name(&name));
            Ok(())
        }

        FirMatch::StoreTable {
            name, index, value, ..
        } => {
            let index = emit_value(store, options, index)?;
            let value = emit_value(store, options, value)?;
            let _ = writeln!(
                out,
                "{tab}{};",
                table_assignment(&name, &index, &value, options)
            );
            Ok(())
        }

        FirMatch::Block(items) => {
            for item in items {
                emit_stmt(store, out, options, item, indent, phase)?;
            }
            Ok(())
        }

        FirMatch::SimpleForLoop {
            var,
            upper,
            body,
            is_reverse,
        } => {
            let counter = codebox_var_name(&var);
            let upper = emit_value(store, options, upper)?;
            if is_reverse {
                let _ = writeln!(
                    out,
                    "{tab}for (let {counter} : Int = ({upper} - 1); ({counter} >= 0); {counter} = isub({counter}, 1)) {{"
                );
            } else {
                let _ = writeln!(
                    out,
                    "{tab}for (let {counter} : Int = 0; ({counter} < {upper}); {counter} = iadd({counter}, 1)) {{"
                );
            }
            emit_stmt_or_block(store, out, options, body, indent + 1, Phase::Body)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }

        FirMatch::WhileLoop { cond, body } => {
            let cond = emit_value(store, options, cond)?;
            let _ = writeln!(out, "{tab}while ({cond}) {{");
            emit_stmt_or_block(store, out, options, body, indent + 1, phase)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }

        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            let cond = emit_value(store, options, cond)?;
            let _ = writeln!(out, "{tab}if ({cond}) {{");
            emit_stmt_or_block(store, out, options, then_block, indent + 1, phase)?;
            if let Some(else_block) = else_block {
                let _ = writeln!(out, "{tab}}} else {{");
                emit_stmt_or_block(store, out, options, else_block, indent + 1, phase)?;
            }
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }

        FirMatch::DeclareFun {
            name, args, body, ..
        } => emit_declare_fun(store, out, options, &name, &args, body, indent),

        FirMatch::Return(value) => {
            match value {
                Some(value) => {
                    let value = emit_value(store, options, value)?;
                    let _ = writeln!(out, "{tab}return {value};");
                }
                None => {
                    let _ = writeln!(out, "{tab}return;");
                }
            }
            Ok(())
        }

        // A dropped value has no effect codebox can express.
        FirMatch::Drop(_) => Ok(()),

        FirMatch::AddSoundfile { label, .. } => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            format!("soundfile '{label}' is not available in codebox"),
        )),

        // UI statements arrive in phase C2. Skipping them keeps C1's output
        // valid rather than half-formed.
        FirMatch::OpenBox { .. }
        | FirMatch::CloseBox
        | FirMatch::AddButton { .. }
        | FirMatch::AddSlider { .. }
        | FirMatch::AddBargraph { .. }
        | FirMatch::AddMetaDeclare { .. } => Ok(()),

        other => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            format!("statement {other:?} is not supported by the codebox backend"),
        )),
    }
}

/// Emits a body that may be a single statement or a block, without braces.
fn emit_stmt_or_block(
    store: &FirStore,
    out: &mut String,
    options: &CodeboxOptions,
    body: FirId,
    indent: usize,
    phase: Phase,
) -> Result<(), CodegenError> {
    match match_fir(store, body) {
        FirMatch::Block(items) => {
            for item in items {
                emit_stmt(store, out, options, item, indent, phase)?;
            }
            Ok(())
        }
        _ => emit_stmt(store, out, options, body, indent, phase),
    }
}

/// Emits a function declaration.
fn emit_declare_fun(
    store: &FirStore,
    out: &mut String,
    options: &CodeboxOptions,
    name: &str,
    args: &[NamedType],
    body: Option<FirId>,
    indent: usize,
) -> Result<(), CodegenError> {
    // The DSP API methods are re-shaped into codebox's own entry points by the
    // section emitters; emitting them again would duplicate their bodies.
    if is_dsp_api_method(name) {
        return Ok(());
    }
    let Some(body) = body else {
        // Prototype-only: codebox has no forward declarations.
        return Ok(());
    };
    let tab = "\t".repeat(indent);
    let args: Vec<String> = args
        .iter()
        .filter(|arg| arg.name != "dsp")
        .map(|arg| codebox_var_name(&arg.name))
        .collect();
    let _ = writeln!(out, "{tab}function {name}({}) {{", args.join(", "));
    emit_stmt_or_block(store, out, options, body, indent + 1, Phase::Body)?;
    let _ = writeln!(out, "{tab}}}");
    Ok(())
}

/// Whether `name` is a DSP API method the section emitters re-shape themselves.
fn is_dsp_api_method(name: &str) -> bool {
    const METHODS: [&str; 14] = [
        "metadata",
        "getNumInputs",
        "getNumOutputs",
        "getSampleRate",
        "classInit",
        "instanceConstants",
        "instanceResetUserInterface",
        "instanceClear",
        "instanceInit",
        "init",
        "buildUserInterface",
        "control",
        "frame",
        "compute",
    ];
    METHODS.contains(&name)
}

/// Emits one value expression.
fn emit_value(
    store: &FirStore,
    options: &CodeboxOptions,
    value: FirId,
) -> Result<String, CodegenError> {
    match match_fir(store, value) {
        FirMatch::Int32 { value, .. } => Ok(value.to_string()),
        FirMatch::Int64 { value, .. } => Ok(value.to_string()),
        FirMatch::Float32 { value, .. } => Ok(float_literal(f64::from(value), options)),
        FirMatch::Float64 { value, .. }
        | FirMatch::Quad { value, .. }
        | FirMatch::FixedPoint { value, .. } => Ok(float_literal(value, options)),
        FirMatch::Bool { value, .. } => Ok(if value { "true" } else { "false" }.to_owned()),

        FirMatch::LoadVar { name, .. } | FirMatch::LoadVarAddress { name, .. } => {
            Ok(load_name(&name))
        }

        FirMatch::LoadTable { name, index, .. } => {
            let index = emit_value(store, options, index)?;
            Ok(table_read(&name, &index))
        }

        // Codebox has a single `number` type, so a float cast is the identity.
        // Integer casts need `trunc()`, which phase C4 owns.
        FirMatch::Cast { value, .. } | FirMatch::Bitcast { value, .. } => {
            emit_value(store, options, value)
        }

        FirMatch::BinOp { op, lhs, rhs, .. } => {
            let lhs = emit_value(store, options, lhs)?;
            let rhs = emit_value(store, options, rhs)?;
            // Fully parenthesised: codebox precedence is not C's.
            Ok(format!("({lhs} {} {rhs})", emit_binop(op)))
        }

        FirMatch::Neg { value, .. } => {
            let value = emit_value(store, options, value)?;
            Ok(format!("(-1 * {value})"))
        }

        FirMatch::FunCall { name, args, .. } => {
            let mut rendered = Vec::with_capacity(args.len());
            for arg in args {
                rendered.push(emit_value(store, options, arg)?);
            }
            Ok(format!("{name}({})", rendered.join(", ")))
        }

        other => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            format!("value {other:?} is not supported by the codebox backend"),
        )),
    }
}

/// Renders a variable read.
///
/// `sample_rate` is special: codebox exposes the rate through a call rather than
/// a field. C++ parity: `visit(NamedAddress*)`.
fn load_name(name: &str) -> String {
    if name == "sample_rate" {
        "samplerate()".to_owned()
    } else {
        codebox_var_name(name)
    }
}

/// Renders a variable write target.
fn store_name(name: &str) -> String {
    codebox_var_name(name)
}

/// Renders a table read.
///
/// The one-sample `inputs`/`outputs` arrays are replaced by per-channel locals,
/// so `inputs[0]` reads `input0_cb` and the subscript disappears. Any other
/// table keeps its subscript.
fn table_read(name: &str, index: &str) -> String {
    match io_local(name, index) {
        Some(local) => local,
        None => format!("{}[{index}]", codebox_var_name(name)),
    }
}

/// Renders a table write, as a complete `lhs = rhs` assignment.
fn table_assignment(name: &str, index: &str, value: &str, _options: &CodeboxOptions) -> String {
    match io_local(name, index) {
        Some(local) => format!("{local} = {value}"),
        None => format!("{}[{index}] = {value}", codebox_var_name(name)),
    }
}

/// Maps `inputs[N]` / `outputs[N]` onto the `compute` locals, when the index is
/// the literal channel number the one-sample shape always uses.
fn io_local(name: &str, index: &str) -> Option<String> {
    let channel: usize = index.parse().ok()?;
    match name {
        "inputs" => Some(format!("input{channel}_cb")),
        "outputs" => Some(format!("output{channel}_cb")),
        _ => None,
    }
}

/// Maps a FIR binary operator to its codebox spelling.
///
/// Integer `+`, `*` and `%` need the wrapping helpers instead, which phase C4
/// owns; here every operator is infix.
fn emit_binop(op: FirBinOp) -> &'static str {
    match op {
        FirBinOp::Add => "+",
        FirBinOp::Sub => "-",
        FirBinOp::Mul => "*",
        FirBinOp::Div => "/",
        FirBinOp::Rem => "%",
        FirBinOp::Lsh => "<<",
        FirBinOp::ARsh | FirBinOp::LRsh => ">>",
        FirBinOp::Gt => ">",
        FirBinOp::Lt => "<",
        FirBinOp::Ge => ">=",
        FirBinOp::Le => "<=",
        FirBinOp::Eq => "==",
        FirBinOp::Ne => "!=",
        FirBinOp::And => "&",
        FirBinOp::Or => "|",
        FirBinOp::Xor => "^",
    }
}

/// Maps a FIR type to its codebox annotation.
///
/// Codebox has two: `Int` and `number`. Everything else collapses onto
/// `number`, as the C++ type manager does.
fn emit_type(typ: &FirType) -> &'static str {
    match typ {
        FirType::Int32 | FirType::Int64 | FirType::Bool => "Int",
        _ => "number",
    }
}

/// Formats a float literal, with the `f` suffix the reference uses in single
/// precision.
fn float_literal(value: f64, options: &CodeboxOptions) -> String {
    let mut text = if value.is_finite() && value == value.trunc() {
        format!("{value:.1}")
    } else {
        format!("{value}")
    };
    if !options.double_precision {
        text.push('f');
    }
    text
}

/// Returns the items of a FIR block, or an empty list for anything else.
fn block_items(store: &FirStore, block: FirId) -> Vec<FirId> {
    match match_fir(store, block) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

/// Finds the body of a named function declaration.
fn find_function_body(store: &FirStore, functions: FirId, wanted: &str) -> Option<FirId> {
    block_items(store, functions)
        .into_iter()
        .find_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } if name == wanted => Some(body),
            _ => None,
        })
}

/// Decodes the FIR module root.
fn decode_module(store: &FirStore, module: FirId) -> Result<ModuleView, CodegenError> {
    match match_fir(store, module) {
        FirMatch::Module {
            num_inputs,
            num_outputs,
            dsp_struct,
            globals,
            functions,
            ..
        } => Ok(ModuleView {
            dsp_struct,
            globals,
            functions,
            num_inputs,
            num_outputs,
        }),
        other => Err(CodegenError::new(
            CodegenErrorCode::RootNotModule,
            format!("expected FIR module root, got {other:?}"),
        )),
    }
}

/// Stable backend identifier, kept for tooling and report wiring.
pub const BACKEND_NAME: &str = "codebox";

/// Returns the stable backend identifier.
#[must_use]
pub fn backend_id() -> &'static str {
    BACKEND_NAME
}
