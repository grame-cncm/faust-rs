//! Mojo backend generation from FIR `Module` roots.
//!
//! # Source provenance (C++)
//! The backend exists only in the open upstream pull request
//! `grame-cncm/faust#1293`, pinned at `05ac86fb1`; the pinned reference
//! `8eebea429` has none.
//! - `compiler/generator/mojo/mojo_code_container.cpp` (`MojoCodeContainer`,
//!   `MojoScalarCodeContainer`): file layout, lifecycle, UI, metadata.
//! - `compiler/generator/mojo/mojo_instructions.cpp` (`MojoInstVisitor`,
//!   `MojoInitFieldsVisitor`): statements, expressions, field initializers.
//! - `compiler/generator/mojo/__mojo_utils.hh`: `snakeCase`,
//!   `formatCompilerOptions`.
//!
//! Plan and contract: `porting/mojo-backend-port-analysis-and-plan-2026-11-06-en.md`.
//!
//! # Generated contract
//! The generated unit has no imports: the PR's `architecture/mojo` supplies
//! the prelude (`S32`, `F64`, `FaustFloat`, `Arr`, `Ptr`, `ImmStreams`, ...)
//! and the `FaustDsp`, `FaustGui` and `FaustMeta` traits through
//! `<<includeclass>>`. The unit holds a `@fieldwise_init struct mydsp(FaustDsp)`
//! with a `null_val` zone (the zone of a `declare` on `0`), one field per DSP
//! state declaration, an `__init__(out dsp)` that initializes every field, and
//! the snake-case lifecycle methods of `dsp/dsp.mojo`.
//!
//! # Names
//! Every FIR variable name goes through the C++ `snakeCase` (camel case split
//! by `_`, a leading `f_` removed: `fRec0` -> `rec0`, `iRec1` -> `i_rec1`,
//! `fSampleRate` -> `sample_rate`). That rename is not injective, and the C++
//! backend does not check it. Here every rename is recorded per namespace
//! (struct fields, everything else) and a second FIR name reaching an
//! identifier already taken fails with `FRS-CGEN-MOJO-0004` instead of
//! merging two variables. A Mojo keyword, and in the local namespace a name
//! the emitted code calls, gets a trailing `_`.
//!
//! # Types and literals
//! Mojo has no implicit numeric conversion, so every operand whose FIR type
//! differs from the type its operation expects is converted explicitly, as
//! the Rust backend does. Literals are always typed (`S32(1)`, `F64(0.5)`,
//! `F32(0.5)`): an untyped float literal materializes as `Float64` and is
//! evaluated in arbitrary precision when two of them meet, both of which
//! would change results. Upstream writes double-precision literals bare.
//!
//! # C semantics kept where Mojo differs
//! Mojo integer `//` and `%` floor (Python semantics), real `%` is a floor
//! modulo and `round` rounds half to even. Integer division and remainder, a
//! real remainder (`fmod`) and C `round` therefore go through generic helpers
//! (`faust_idiv`, `faust_irem`, `faust_fmod`, `faust_round`) written at the
//! top of the unit, only when used; C `rint` is `std.math.round`. Upstream
//! maps them to `//`/`%`, `mod` and `round` (`mod` and `rint` from the
//! architecture's `help/math.mojo`), which differ from C on negative operands
//! and on halves. The generated unit calls `std.math` only.
//!
//! # Not supported
//! Soundfiles (the C++ visitor panics on them), `-quad` and fixed point, and
//! the C++-only `-gpu` path. Unsupported FIR nodes fail with
//! `FRS-CGEN-MOJO-0003`.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use crate::backends::codegen_error::{BackendError, CodegenErrorCode as BackendErrorCode};
use crate::backends::faust_api;
use crate::backends::literals::{Digits, NegativeZero, RealSpelling, StringDialect, quoted_string};
use crate::backends::module_view::{
    FunctionDecl, SkeletonCodes, block_stores_var, collect_defined_functions, decode_module,
    prepare_flat_module, section_items, unsupported_node,
};
use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, match_fir};

use names::Renamer;
#[cfg(test)]
use names::snake_case;

pub const BACKEND_NAME: &str = "mojo";

/// Internal real precision of the generated DSP (`-single` / `-double`).
pub type MojoRealType = super::RealPrecision;

/// Mojo backend options for module-first emission.
///
/// The same transport as `JuliaOptions`: the facade fills the metadata, the
/// compilation options and the JSON description; `Default` keeps fixture-level
/// tests self-contained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MojoOptions {
    /// Optional DSP struct name override for the FIR module name.
    pub class_name: Option<String>,
    /// Internal real precision (`F32` or `F64`).
    pub real_type: MojoRealType,
    /// Compilation options string printed in the banner and in `metadata`.
    ///
    /// `None` falls back to `-lang mojo -single` / `-lang mojo -double`.
    pub compile_options: Option<String>,
    /// Source-level DSP name reported by `metadata` and the banner.
    pub metadata_name: Option<String>,
    /// Source basename reported by `metadata`.
    pub metadata_filename: Option<String>,
    /// Non-identity compilation metadata replayed by `metadata`.
    pub metadata_entries: Vec<(String, String)>,
    /// Pre-rendered one-line JSON description returned by `get_json`.
    pub dsp_json: Option<String>,
    /// Emit the batch contract: one struct for N voices, its state laid out
    /// as a structure of arrays by `fir::batch::layout_voices` (GPU batch
    /// plan G3.5, `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`
    /// §3.5). An emitter option only (decision D3): no CLI surface.
    pub batch: bool,
    /// With `batch`, keep the `compute` state in locals for the sample loop
    /// (`fir::batch::promote_state`, run before the layout): every per-voice
    /// field `compute` reaches only as a scalar or at literal indices is read
    /// once at the head of `compute` and written back at its end, instead of
    /// going through the voice arenas at every sample, which Mojo does not
    /// keep in registers (plan
    /// `porting/batch-state-register-promotion-plan-2026-11-08-en.md`).
    /// `false` emits the arena accesses as they are, for A/B measurements.
    pub batch_registers: bool,
}

impl Default for MojoOptions {
    fn default() -> Self {
        Self {
            class_name: Some("mydsp".to_owned()),
            real_type: MojoRealType::Float32,
            compile_options: None,
            metadata_name: None,
            metadata_filename: None,
            metadata_entries: Vec::new(),
            dsp_json: None,
            batch: false,
            batch_registers: true,
        }
    }
}

/// Stable machine-readable error codes for the Mojo backend emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// Root FIR node is not a module (`FirMatch::Module`).
    RootNotModule,
    /// One module section is not a FIR block or canonical API signatures are invalid.
    InvalidModuleSection,
    /// The Mojo emitter does not support this FIR node (soundfiles included).
    UnsupportedNode,
    /// Two FIR names map to the same Mojo identifier after the snake-case rename.
    NameCollision,
}

impl CodegenErrorCode {
    /// Returns the stable machine-readable error code string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-MOJO-0001",
            Self::InvalidModuleSection => "FRS-CGEN-MOJO-0002",
            Self::UnsupportedNode => "FRS-CGEN-MOJO-0003",
            Self::NameCollision => "FRS-CGEN-MOJO-0004",
        }
    }
}

impl BackendErrorCode for CodegenErrorCode {
    fn as_str(&self) -> &'static str {
        Self::as_str(*self)
    }
}

impl SkeletonCodes for CodegenErrorCode {
    const ROOT_NOT_MODULE: Self = Self::RootNotModule;
    const INVALID_MODULE_SECTION: Self = Self::InvalidModuleSection;
    const UNSUPPORTED_NODE: Self = Self::UnsupportedNode;
}

/// One emission failure of this backend.
pub type CodegenError = BackendError<CodegenErrorCode>;

#[must_use]
/// Returns the stable backend identifier.
pub fn backend_id() -> &'static str {
    BACKEND_NAME
}

/// Mojo spelling of a real literal's digits. Special values never reach it:
/// [`Emitter::real_literal`] spells them `nan[f64]()` / `inf[f64]()`.
pub(crate) const MOJO_REAL: RealSpelling = RealSpelling {
    nan: "nan",
    pos_inf: "inf",
    neg_inf: "-inf",
    digits: Digits::Debug,
    negative_zero: NegativeZero::Normalize,
};

/// Generates Mojo code from a FIR module root.
///
/// # Errors
/// Returns [`CodegenError`] if the root is not a FIR module, if the module
/// contains a FIR node this emitter does not support, or if two FIR names map
/// to the same Mojo identifier.
pub fn generate_mojo_module(
    store: &FirStore,
    module: FirId,
    options: &MojoOptions,
) -> Result<String, CodegenError> {
    fir::on_compile_stack(|| generate_mojo_module_step(store, module, options))
}

fn generate_mojo_module_step(
    store: &FirStore,
    module: FirId,
    options: &MojoOptions,
) -> Result<String, CodegenError> {
    // Mojo has no nested container and `class_init` takes the DSP, so a table
    // generator is inlined with its state merged into the DSP struct, as the
    // C++ `MojoCodeContainer::writeClassInit` does with
    // `inlineSubcontainersFunCalls` and `mergeSubContainers`.
    let prepared = prepare_flat_module(
        store,
        module,
        fir::subcontainer::SubModuleStatePolicy::MergedStructFields,
        false,
    )
    .map_err(|message| {
        CodegenError::new(CodegenErrorCode::InvalidModuleSection, message).at_node(module)
    })?;
    let (store, module) = prepared.as_ref().map_or((store, module), |(s, m)| (s, *m));
    // Batch contract: the `compute` state promoted to locals, then the
    // per-voice state laid out as a structure of arrays, each output accepted
    // by its pass's independent checker.
    let promoted = if options.batch && options.batch_registers {
        Some(fir::batch::promote_state(store, module).map_err(|err| {
            CodegenError::new(
                CodegenErrorCode::InvalidModuleSection,
                format!("batch state promotion: {err}"),
            )
            .at_node(module)
        })?)
    } else {
        None
    };
    let (store, module) = promoted
        .as_ref()
        .map_or((store, module), |(s, m, _)| (s, *m));
    let batched = if options.batch {
        Some(fir::batch::layout_voices(store, module).map_err(|err| {
            CodegenError::new(
                CodegenErrorCode::InvalidModuleSection,
                format!("batch voice layout: {err}"),
            )
            .at_node(module)
        })?)
    } else {
        None
    };
    let layout = batched.as_ref().map(|(_, _, layout)| layout.clone());
    let (store, module) = batched
        .as_ref()
        .map_or((store, module), |(s, m, _)| (s, *m));
    // Runtime-filled and folded tables are read through `dsp.` like every
    // other state: `collect_fields` declares the static section as struct
    // fields too, as upstream declares every global as a struct field.
    let view = decode_module(store, module)?;
    let class_name = options
        .class_name
        .as_deref()
        .unwrap_or(view.name.as_str())
        .to_owned();
    let functions = collect_defined_functions(store, view.functions)?;

    let mut emitter = Emitter::new(store, options, layout.clone());
    let fields = emitter.collect_fields([view.dsp_struct, view.globals, view.static_decls])?;
    let has_sample_rate = fields.iter().any(|f| f.fir_name == "fSampleRate");

    let mut body = String::new();
    for f in &functions {
        if crate::backends::is_lifecycle_function(&f.name) {
            continue;
        }
        emitter.emit_helper_function(&mut body, f)?;
    }

    let _ = writeln!(body, "@fieldwise_init");
    let trait_name = if layout.is_some() {
        "FaustDspBatch"
    } else {
        "FaustDsp"
    };
    let _ = writeln!(body, "struct {class_name}({trait_name}):");
    let _ = writeln!(body, "    var null_val: FaustFloat");
    for field in &fields {
        let _ = writeln!(body, "    var {}: {}", field.mojo_name, field.type_text);
    }
    if !has_sample_rate {
        let _ = writeln!(body, "    var sample_rate: S32");
    }
    let _ = writeln!(body);

    let _ = writeln!(body, "    @always_inline");
    // A batch struct is built over one state buffer of `state_bytes(voices)`
    // bytes, which the constructor carves into its typed arenas, each aligned
    // to 8 bytes: a `Pointer` cannot be null in Mojo, and a host allocates
    // (possibly on a device) before it builds.
    if layout.is_some() {
        let _ = writeln!(
            body,
            "    def __init__(out dsp, voices: S32, state: Ptr[UInt8]):"
        );
        let _ = writeln!(body, "        var at = 0");
    } else {
        let _ = writeln!(body, "    def __init__(out dsp):");
    }
    let _ = writeln!(body, "        dsp.null_val = FaustFloat(0.0)");
    for field in &fields {
        match &field.param {
            Some(_) if field.fir_name == fir::batch::VOICES_FIELD => {
                let _ = writeln!(body, "        dsp.{} = voices", field.mojo_name);
            }
            Some(type_text) => {
                let elem = type_text
                    .strip_prefix("Ptr[")
                    .and_then(|t| t.strip_suffix(']'))
                    .unwrap_or(type_text);
                let name = &field.mojo_name;
                let _ = writeln!(
                    body,
                    "        dsp.{name} = state.unsafe_offset(at).unsafe_bitcast[{elem}]()"
                );
                let _ = writeln!(
                    body,
                    "        at += (Self.{name}_elements() * Int(voices) * size_of[{elem}]() + 7) // 8 * 8"
                );
            }
            None => {
                let _ = writeln!(
                    body,
                    "        dsp.{} = {}",
                    field.mojo_name, field.init_text
                );
            }
        }
    }
    if !has_sample_rate {
        let _ = writeln!(body, "        dsp.sample_rate = S32(0)");
    }
    let _ = writeln!(body);
    if let Some(layout) = &layout {
        emitter.emit_arena_sizes(&mut body, layout)?;
    }

    emitter.emit_api(
        &mut body,
        ApiInput {
            module_name: view.name.as_str(),
            num_inputs: view.num_inputs,
            num_outputs: view.num_outputs,
            functions: &functions,
            fields: &fields,
        },
    )?;

    let mut out = String::new();
    emit_banner(&mut out, options);
    let _ = writeln!(out);
    emit_helpers(&mut out, &emitter.helpers);
    out.push_str(&body);
    Ok(out)
}

/// One DSP state field: struct, global or static declaration.
#[derive(Debug, Clone)]
struct Field {
    /// The `__init__` parameter type for a field the batch struct is built
    /// over (`fVoices`, the arenas); `None` otherwise.
    param: Option<String>,
    fir_name: String,
    mojo_name: String,
    type_text: String,
    /// Constructor initializer (`__init__`).
    init_text: String,
    /// Explicit scalar initializer, replayed by a synthesized
    /// `instance_reset_user_interface` when the FIR module defines none.
    reset: Option<(FirType, FirId)>,
}

/// A C-semantics helper written at the top of the unit when used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Helper {
    IDiv,
    IRem,
    FMod,
    Round,
}

impl Helper {
    fn name(self) -> &'static str {
        match self {
            Self::IDiv => "faust_idiv",
            Self::IRem => "faust_irem",
            Self::FMod => "faust_fmod",
            Self::Round => "faust_round",
        }
    }

    fn source(self) -> &'static str {
        match self {
            // C truncates the quotient toward zero; Mojo `//` floors.
            Self::IDiv => {
                "@always_inline\n\
                 def faust_idiv[dt: DType](a: Scalar[dt], b: Scalar[dt]) -> Scalar[dt]:\n\
                 \x20   var q = a // b\n\
                 \x20   if (a % b != 0) and ((a < 0) != (b < 0)):\n\
                 \x20       q += 1\n\
                 \x20   return q\n"
            }
            // C gives the remainder the sign of the dividend; Mojo `%` gives
            // it the sign of the divisor.
            Self::IRem => {
                "@always_inline\n\
                 def faust_irem[dt: DType](a: Scalar[dt], b: Scalar[dt]) -> Scalar[dt]:\n\
                 \x20   var r = a % b\n\
                 \x20   if (r != 0) and ((r < 0) != (a < 0)):\n\
                 \x20       r -= b\n\
                 \x20   return r\n"
            }
            // C `fmod`: the sign of the dividend. Mojo `%` floors, which is
            // C `fmod` on two non-negative operands, so only the sign is
            // restored.
            Self::FMod => {
                "@always_inline\n\
                 def faust_fmod[dt: DType](x: Scalar[dt], y: Scalar[dt]) -> Scalar[dt]:\n\
                 \x20   return copysign(abs(x) % abs(y), x)\n"
            }
            // C `round`: halves away from zero; Mojo `round` goes to even.
            // `x - trunc(x)` is exact, so the test on the half is exact.
            Self::Round => {
                "@always_inline\n\
                 def faust_round[dt: DType](x: Scalar[dt]) -> Scalar[dt]:\n\
                 \x20   var t = trunc(x)\n\
                 \x20   if abs(x - t) >= 0.5:\n\
                 \x20       t += copysign(Scalar[dt](1), x)\n\
                 \x20   return t\n"
            }
        }
    }
}

fn emit_helpers(out: &mut String, helpers: &BTreeSet<Helper>) {
    for helper in helpers {
        out.push_str(helper.source());
        out.push('\n');
    }
}

/// Context-dependent receiver of `AddMetaDeclare`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitMode {
    Default,
    Metadata,
    Ui,
}

struct ApiInput<'a> {
    module_name: &'a str,
    num_inputs: usize,
    num_outputs: usize,
    functions: &'a [FunctionDecl],
    fields: &'a [Field],
}

/// Emission state for one module: the rename maps, the declared types used for
/// explicit conversions, and the helpers the emitted code calls.
struct Emitter<'a> {
    store: &'a FirStore,
    options: &'a MojoOptions,
    fields: Renamer,
    locals: Renamer,
    /// FIR type of every struct, global or static declaration, by FIR name.
    field_types: HashMap<String, FirType>,
    /// Lexical scopes of the function being emitted: FIR name to FIR type.
    scopes: Vec<HashMap<String, FirType>>,
    /// Number of loops around the statement being emitted.
    helpers: BTreeSet<Helper>,
    /// The voice layout in batch mode (`MojoOptions::batch`).
    batch: Option<fir::batch::VoiceLayout>,
}

impl<'a> Emitter<'a> {
    fn new(
        store: &'a FirStore,
        options: &'a MojoOptions,
        batch: Option<fir::batch::VoiceLayout>,
    ) -> Self {
        let mut fields = Renamer::new(false);
        fields.reserve("\0null_val", "null_val");
        let mut locals = Renamer::new(true);
        for param in ["dsp", "sample_rate", "count", "inputs", "outputs"] {
            locals.reserve(param, param);
        }
        if batch.is_some() {
            locals.reserve(fir::batch::VOICE_ARG, fir::batch::VOICE_ARG);
        }
        Self {
            store,
            options,
            fields,
            locals,
            field_types: HashMap::new(),
            scopes: Vec::new(),
            helpers: BTreeSet::new(),
            batch,
        }
    }

    // ---------------------------------------------------------------- fields

    fn collect_fields<const N: usize>(
        &mut self,
        sections: [FirId; N],
    ) -> Result<Vec<Field>, CodegenError> {
        let mut out = Vec::new();
        for section in sections {
            let Ok(items) = section_items::<CodegenErrorCode>("struct", section, self.store) else {
                continue;
            };
            for item in items {
                match match_fir(self.store, item) {
                    // A foreign variable (`fvariable`) is a host symbol, not state.
                    FirMatch::DeclareVar {
                        access: AccessType::Global,
                        ..
                    } => {}
                    FirMatch::DeclareVar {
                        name, typ, init, ..
                    } => {
                        let mojo_name = self.fields.get(&name)?;
                        self.field_types.insert(name.clone(), typ.clone());
                        let type_text = self.emit_type(&typ)?;
                        let init_text = match init {
                            Some(init) => self.emit_coerced(&typ, init)?,
                            None if matches!(typ, FirType::Ptr(_)) && self.batch.is_some() => {
                                String::new()
                            }
                            None => self.zero_value(&typ)?,
                        };
                        let reset = init
                            .filter(|_| !matches!(typ, FirType::Array(..)))
                            .map(|init| (typ.clone(), init));
                        let param = self.batch.as_ref().and_then(|layout| {
                            (name == fir::batch::VOICES_FIELD
                                || layout.arenas.iter().any(|a| a.name == name))
                            .then(|| type_text.clone())
                        });
                        out.push(Field {
                            param,
                            fir_name: name,
                            mojo_name,
                            type_text,
                            init_text,
                            reset,
                        });
                    }
                    FirMatch::DeclareTable {
                        name,
                        elem_type,
                        values,
                        ..
                    } => {
                        let mojo_name = self.fields.get(&name)?;
                        let typ = FirType::Array(Box::new(elem_type.clone()), values.len());
                        self.field_types.insert(name.clone(), typ.clone());
                        let type_text = self.emit_type(&typ)?;
                        let init_text = self.table_literal(&elem_type, &values)?;
                        out.push(Field {
                            param: None,
                            fir_name: name,
                            mojo_name,
                            type_text,
                            init_text,
                            reset: None,
                        });
                    }
                    _ => {}
                }
            }
        }
        Ok(out)
    }

    fn table_literal(
        &mut self,
        elem_type: &FirType,
        values: &[FirId],
    ) -> Result<String, CodegenError> {
        let mut rendered = Vec::with_capacity(values.len());
        for value in values {
            rendered.push(self.emit_coerced(elem_type, *value)?);
        }
        Ok(format!("[{}]", rendered.join(", ")))
    }

    // ------------------------------------------------------------------- API

    fn emit_api(&mut self, out: &mut String, input: ApiInput<'_>) -> Result<(), CodegenError> {
        let ApiInput {
            module_name,
            num_inputs,
            num_outputs,
            functions,
            fields,
        } = input;
        let sample_rate = self.fields.get("fSampleRate")?;
        let find = |name: &str| functions.iter().find(|f| f.name == name);

        method(out, "def get_sample_rate(imm dsp) -> S32:");
        let _ = writeln!(out, "        return dsp.{sample_rate}\n");
        method(out, "def get_num_outputs(imm dsp) -> S32:");
        let _ = writeln!(out, "        return S32({num_outputs})\n");
        method(out, "def get_num_inputs(imm dsp) -> S32:");
        let _ = writeln!(out, "        return S32({num_inputs})\n");

        let mut body = String::new();
        match find("staticInit") {
            Some(f) => self.emit_fun_body(&mut body, f, EmitMode::Default, None)?,
            None => write_pass(&mut body),
        }
        write_method(
            out,
            "def class_init(mut dsp, imm sample_rate: S32) -> None:",
            &body,
        );

        let mut body = String::new();
        let store_sr = format!("        dsp.{sample_rate} = sample_rate\n");
        match find("instanceConstants") {
            Some(f) => {
                let fir_body = f.body.expect("collect_defined_functions keeps bodies");
                let prefix = (!block_stores_var(self.store, fir_body, "fSampleRate"))
                    .then_some(store_sr.as_str());
                self.emit_fun_body(&mut body, f, EmitMode::Default, prefix)?;
            }
            None => body.push_str(&store_sr),
        }
        write_method(
            out,
            &self.voiced("def instance_constants(mut dsp, imm sample_rate: S32) -> None:"),
            &body,
        );

        let mut body = String::new();
        if let Some(f) = find("instanceResetUserInterface") {
            self.emit_fun_body(&mut body, f, EmitMode::Default, None)?;
        } else {
            for field in fields {
                if let Some((typ, init)) = &field.reset {
                    let value = self.emit_coerced(typ, *init)?;
                    let _ = writeln!(body, "        dsp.{} = {value}", field.mojo_name);
                }
            }
            if body.is_empty() {
                write_pass(&mut body);
            }
        }
        write_method(
            out,
            &self.voiced("def instance_reset_user_interface(mut dsp) -> None:"),
            &body,
        );

        let mut body = String::new();
        match find("instanceClear") {
            Some(f) => self.emit_fun_body(&mut body, f, EmitMode::Default, None)?,
            None => write_pass(&mut body),
        }
        write_method(
            out,
            &self.voiced("def instance_clear(mut dsp) -> None:"),
            &body,
        );

        // Batch: the instance lifecycle runs per voice, `class_init` once for
        // all voices (design §3.5).
        let voice = self.voice_arg();
        let voice_only = voice.trim_end_matches(", ");
        method(
            out,
            &self.voiced("def instance_init(mut dsp, imm sample_rate: S32) -> None:"),
        );
        let _ = writeln!(out, "        dsp.instance_constants({voice}sample_rate)");
        let _ = writeln!(
            out,
            "        dsp.instance_reset_user_interface({voice_only})"
        );
        let _ = writeln!(out, "        dsp.instance_clear({voice_only})\n");

        method(
            out,
            &self.voiced("def init(mut dsp, imm sample_rate: S32) -> None:"),
        );
        out.push_str("        dsp.class_init(sample_rate)\n");
        let _ = writeln!(out, "        dsp.instance_init({voice}sample_rate)\n");

        // C++ `MojoCodeContainer::writeGetJson`: `flattenJSON(generateJSONAux())`.
        method(out, "def get_json(imm dsp) -> String:");
        let _ = writeln!(
            out,
            "        return {}\n",
            mojo_string(self.options.dsp_json.as_deref().unwrap_or("{}"))
        );

        let mut body = String::new();
        match find("metadata").filter(|f| !is_empty_body(self.store, f)) {
            Some(f) => self.emit_fun_body(&mut body, f, EmitMode::Metadata, None)?,
            None => self.emit_compilation_metadata(&mut body, module_name),
        }
        write_method(
            out,
            "def metadata(imm dsp, mut meta: Some[FaustMeta]) -> None:",
            &body,
        );

        let mut body = String::new();
        match find("buildUserInterface") {
            Some(f) => self.emit_fun_body(&mut body, f, EmitMode::Ui, None)?,
            None => write_pass(&mut body),
        }
        write_method(
            out,
            "def build_user_interface(mut dsp, mut ui: Some[FaustGui]) -> None:",
            &body,
        );

        let mut body = String::new();
        match find("compute") {
            Some(f) => self.emit_fun_body(&mut body, f, EmitMode::Default, None)?,
            None => write_pass(&mut body),
        }
        write_method(
            out,
            &self.voiced(
                "def compute(mut dsp, var count: S32, var inputs: ImmStreams, var outputs: MutStreams) -> None:",
            ),
            &body,
        );
        // `write_method` ends every method with a blank line; the struct is last.
        out.truncate(out.trim_end_matches('\n').len() + 1);
        Ok(())
    }

    /// Replays the compilation metadata carried by the options, in C++ key
    /// order (C++ `MojoCodeContainer::writeMetadataFunc` walks the metadata set).
    fn emit_compilation_metadata(&self, out: &mut String, module_name: &str) {
        let options = self.options;
        let filename = options
            .metadata_filename
            .clone()
            .unwrap_or_else(|| format!("{module_name}.dsp"));
        let name = options
            .metadata_name
            .clone()
            .unwrap_or_else(|| module_name.to_owned());
        let mut entries = options.metadata_entries.clone();
        if !entries.iter().any(|(key, _)| key == "compile_options") {
            entries.push(("compile_options".to_owned(), compile_options(options)));
        }
        let ordered =
            crate::backends::c_family::ordered_compilation_metadata(&entries, filename, name);
        if ordered.is_empty() {
            write_pass(out);
        }
        for (key, value) in ordered {
            let _ = writeln!(
                out,
                "        meta.declare({}, {})",
                mojo_string(&key),
                mojo_string(&value)
            );
        }
    }

    /// Emits the body of one canonical DSP method, after validating its FIR
    /// signature. `prefix` is written first (the `instance_constants` store of
    /// the sample rate when the FIR body omits it).
    fn emit_fun_body(
        &mut self,
        out: &mut String,
        decl: &FunctionDecl,
        mode: EmitMode,
        prefix: Option<&str>,
    ) -> Result<(), CodegenError> {
        if decl.name != "staticInit" {
            // A batch method carries `voice` after `dsp`; the rest must be the
            // canonical signature.
            let (typ, args) = match &self.batch {
                Some(_) if fir::batch::INSTANCE_FUNCTIONS.contains(&decl.name.as_str()) => {
                    let mut args = decl.named_args.clone();
                    if args.len() > 1 {
                        args.remove(1);
                    }
                    let typ = match &decl.typ {
                        FirType::Fun { args: types, ret } if types.len() > 1 => {
                            let mut types = types.clone();
                            types.remove(1);
                            FirType::Fun {
                                args: types,
                                ret: ret.clone(),
                            }
                        }
                        other => other.clone(),
                    };
                    (typ, args)
                }
                _ => (decl.typ.clone(), decl.named_args.clone()),
            };
            faust_api::validate_canonical_dsp_api_signature(&decl.name, &typ, &args)
                .map_err(|msg| CodegenError::new(CodegenErrorCode::InvalidModuleSection, msg))?;
        }
        let body = decl
            .body
            .expect("collect_defined_functions keeps function definitions only");
        let start = out.len();
        if let Some(prefix) = prefix {
            out.push_str(prefix);
        }
        self.scopes.clear();
        let mut scope = HashMap::new();
        for arg in &decl.named_args {
            scope.insert(arg.name.clone(), arg.typ.clone());
        }
        self.scopes.push(scope);
        let mut mode = mode;
        self.emit_block_items(out, body, 2, &mut mode)?;
        self.scopes.clear();
        if out.len() == start {
            write_pass(out);
        }
        Ok(())
    }

    /// Emits a non-canonical FIR function as a top-level Mojo function.
    fn emit_helper_function(
        &mut self,
        out: &mut String,
        decl: &FunctionDecl,
    ) -> Result<(), CodegenError> {
        let FirType::Fun { args, ret } = &decl.typ else {
            return Err(CodegenError::new(
                CodegenErrorCode::InvalidModuleSection,
                format!("function `{}` has no function type", decl.name),
            ));
        };
        let mut params = Vec::with_capacity(args.len());
        let mut scope = HashMap::new();
        for (index, arg_type) in args.iter().enumerate() {
            let fir_name = decl
                .named_args
                .get(index)
                .map_or_else(|| format!("arg{index}"), |named| named.name.clone());
            let name = self.locals.get(&fir_name)?;
            params.push(format!("{name}: {}", self.emit_type(arg_type)?));
            scope.insert(fir_name, arg_type.clone());
        }
        let ret = self.emit_type(ret)?;
        let _ = writeln!(out, "@always_inline");
        let _ = writeln!(out, "def {}({}) -> {ret}:", decl.name, params.join(", "));
        let body = decl
            .body
            .expect("collect_defined_functions keeps function definitions only");
        let start = out.len();
        self.scopes.clear();
        self.scopes.push(scope);
        let mut mode = EmitMode::Default;
        self.emit_block_items(out, body, 1, &mut mode)?;
        self.scopes.clear();
        if out.len() == start {
            let _ = writeln!(out, "    pass");
        }
        let _ = writeln!(out);
        Ok(())
    }

    // ------------------------------------------------------------ statements

    /// Emits the statements of a FIR block, in the current scope.
    fn emit_block_items(
        &mut self,
        out: &mut String,
        block: FirId,
        indent: usize,
        mode: &mut EmitMode,
    ) -> Result<(), CodegenError> {
        let FirMatch::Block(items) = match_fir(self.store, block) else {
            return Err(unsupported_node("expected block", block, self.store));
        };
        for stmt in items {
            self.emit_stmt(out, stmt, indent, mode)?;
        }
        Ok(())
    }

    /// Emits a FIR block as the body of a Mojo compound statement: a new
    /// lexical scope, and `pass` when it emits nothing.
    fn emit_nested_block(
        &mut self,
        out: &mut String,
        block: FirId,
        indent: usize,
        mode: &mut EmitMode,
        bound: Option<(String, FirType)>,
    ) -> Result<(), CodegenError> {
        let mut scope = HashMap::new();
        if let Some((name, typ)) = bound {
            scope.insert(name, typ);
        }
        self.scopes.push(scope);
        let start = out.len();
        let result = self.emit_block_items(out, block, indent, mode);
        self.scopes.pop();
        result?;
        if out.len() == start {
            let _ = writeln!(out, "{}pass", tab(indent));
        }
        Ok(())
    }

    fn emit_stmt(
        &mut self,
        out: &mut String,
        stmt: FirId,
        indent: usize,
        mode: &mut EmitMode,
    ) -> Result<(), CodegenError> {
        let t = tab(indent);
        match match_fir(self.store, stmt) {
            FirMatch::DeclareVar {
                name, typ, init, ..
            } => {
                let target = self.locals.get(&name)?;
                let redeclared = self
                    .scopes
                    .last()
                    .is_some_and(|scope| scope.contains_key(&name));
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert(name.clone(), typ.clone());
                }
                let value = match init {
                    Some(init) => Some(self.emit_coerced(&typ, init)?),
                    None if matches!(typ, FirType::Array(..)) => Some(self.zero_value(&typ)?),
                    None => None,
                };
                if redeclared {
                    if let Some(value) = value {
                        let _ = writeln!(out, "{t}{target} = {value}");
                    }
                    return Ok(());
                }
                // Pointer locals take the origin of their source (`inputs` is
                // immutable, `outputs` mutable), which `Ptr[T]` would not name.
                match (value, &typ) {
                    (Some(value), FirType::Ptr(_)) => {
                        let _ = writeln!(out, "{t}var {target} = {value}");
                    }
                    (Some(value), _) => {
                        let _ =
                            writeln!(out, "{t}var {target}: {} = {value}", self.emit_type(&typ)?);
                    }
                    (None, _) => {
                        let _ = writeln!(out, "{t}var {target}: {}", self.emit_type(&typ)?);
                    }
                }
                Ok(())
            }
            FirMatch::DeclareTable {
                name,
                elem_type,
                values,
                ..
            } => {
                let target = self.locals.get(&name)?;
                let typ = FirType::Array(Box::new(elem_type.clone()), values.len());
                let type_text = self.emit_type(&typ)?;
                let literal = self.table_literal(&elem_type, &values)?;
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert(name, typ);
                }
                let _ = writeln!(out, "{t}var {target}: {type_text} = {literal}");
                Ok(())
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                let target = self.var_ref(&name, access)?;
                let value = match self.var_type(&name, access) {
                    Some(typ) => self.emit_coerced(&typ, value)?,
                    None => self.emit_value(value)?,
                };
                let _ = writeln!(out, "{t}{target} = {value}");
                Ok(())
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                let target = self.table_ref(&name, access, index)?;
                let value = match self.element_type(&name, access) {
                    Some(typ) => self.emit_coerced(&typ, value)?,
                    None => self.emit_value(value)?,
                };
                let _ = writeln!(out, "{t}{target} = {value}");
                Ok(())
            }
            FirMatch::Drop(value) => {
                let value = self.emit_value(value)?;
                let _ = writeln!(out, "{t}_ = {value}");
                Ok(())
            }
            FirMatch::Return(value) => {
                match value {
                    Some(value) => {
                        let value = self.emit_value(value)?;
                        let _ = writeln!(out, "{t}return {value}");
                    }
                    None => {
                        let _ = writeln!(out, "{t}return");
                    }
                }
                Ok(())
            }
            FirMatch::Block(_) => self.emit_block_items(out, stmt, indent, mode),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                let cond = self.emit_cond(cond)?;
                let _ = writeln!(out, "{t}if {cond}:");
                self.emit_nested_block(out, then_block, indent + 1, mode, None)?;
                if let Some(else_block) = else_block {
                    let _ = writeln!(out, "{t}else:");
                    self.emit_nested_block(out, else_block, indent + 1, mode, None)?;
                }
                Ok(())
            }
            FirMatch::Control { cond, stmt: inner } => {
                let cond = self.emit_cond(cond)?;
                let _ = writeln!(out, "{t}if {cond}:");
                self.scopes.push(HashMap::new());
                let start = out.len();
                let result = self.emit_stmt(out, inner, indent + 1, mode);
                self.scopes.pop();
                result?;
                if out.len() == start {
                    let _ = writeln!(out, "{}pass", tab(indent + 1));
                }
                Ok(())
            }
            FirMatch::Switch {
                cond,
                ref cases,
                default,
            } => {
                // Evaluated once, as C's `switch` evaluates it.
                let cond_type = value_type(self.store, cond);
                let cond = self.emit_value(cond)?;
                let switch = format!("switch{}", stmt.as_u32());
                let _ = writeln!(out, "{t}var {switch} = {cond}");
                let ctor = match cond_type {
                    Some(FirType::Int64) => "S64",
                    _ => "S32",
                };
                if cases.is_empty() {
                    if let Some(default) = default {
                        self.emit_nested_block(out, default, indent, mode, None)?;
                    }
                    return Ok(());
                }
                for (i, (value, block)) in cases.iter().enumerate() {
                    let keyword = if i == 0 { "if" } else { "elif" };
                    let _ = writeln!(out, "{t}{keyword} {switch} == {ctor}({value}):");
                    self.emit_nested_block(out, *block, indent + 1, mode, None)?;
                }
                if let Some(default) = default {
                    let _ = writeln!(out, "{t}else:");
                    self.emit_nested_block(out, default, indent + 1, mode, None)?;
                }
                Ok(())
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => {
                // `init` declares the loop variable; FIR expresses C-style
                // init/end/step triples, which `while` renders directly.
                let var_type = match match_fir(self.store, init) {
                    FirMatch::DeclareVar { typ, .. } => {
                        self.emit_stmt(out, init, indent, mode)?;
                        typ
                    }
                    _ => {
                        let value = self.emit_value(init)?;
                        let name = self.locals.get(&var)?;
                        let _ = writeln!(out, "{t}var {name} = {value}");
                        value_type(self.store, init).unwrap_or(FirType::Int32)
                    }
                };
                let name = self.locals.get(&var)?;
                let end = self.emit_coerced(&var_type, end)?;
                let step = self.emit_coerced(&var_type, step)?;
                let cmp = if is_reverse { ">" } else { "<" };
                let _ = writeln!(out, "{t}while {name} {cmp} {end}:");
                self.emit_nested_block(out, body, indent + 1, mode, None)?;
                let _ = writeln!(out, "{}{name} = {name} + {step}", tab(indent + 1));
                Ok(())
            }
            FirMatch::SimpleForLoop {
                var,
                upper,
                body,
                is_reverse,
            } => {
                let upper_type = value_type(self.store, upper).unwrap_or(FirType::Int32);
                let ctor = match upper_type {
                    FirType::Int64 => "S64",
                    _ => "S32",
                };
                let upper = self.emit_value(upper)?;
                let name = self.locals.get(&var)?;
                let bound_type = if ctor == "S64" {
                    FirType::Int64
                } else {
                    FirType::Int32
                };
                if is_reverse {
                    let _ = writeln!(
                        out,
                        "{t}for var {name} in range(({upper}) - {ctor}(1), {ctor}(-1), {ctor}(-1)):"
                    );
                } else {
                    let _ = writeln!(out, "{t}for var {name} in range({ctor}(0), {upper}):");
                }
                self.emit_nested_block(out, body, indent + 1, mode, Some((var, bound_type)))
            }
            FirMatch::WhileLoop { cond, body } => {
                let cond = self.emit_cond(cond)?;
                let _ = writeln!(out, "{t}while {cond}:");
                self.emit_nested_block(out, body, indent + 1, mode, None)
            }
            FirMatch::OpenBox { typ, label } => {
                let api = match typ {
                    fir::UiBoxType::Vertical => "open_vertical_box",
                    fir::UiBoxType::Horizontal => "open_horizontal_box",
                    fir::UiBoxType::Tab => "open_tab_box",
                };
                let _ = writeln!(out, "{t}ui.{api}({})", mojo_string(&label));
                Ok(())
            }
            FirMatch::CloseBox => {
                let _ = writeln!(out, "{t}ui.close_box()");
                Ok(())
            }
            FirMatch::AddButton { typ, label, var } => {
                let api = match typ {
                    fir::ButtonType::Button => "add_button",
                    fir::ButtonType::Checkbox => "add_check_button",
                };
                let zone = self.zone(&var)?;
                let _ = writeln!(out, "{t}ui.{api}({}, {zone})", mojo_string(&label));
                Ok(())
            }
            FirMatch::AddSlider {
                typ,
                label,
                var,
                init,
                lo,
                hi,
                step,
            } => {
                let api = match typ {
                    fir::SliderType::Horizontal => "add_horizontal_slider",
                    fir::SliderType::Vertical => "add_vertical_slider",
                    fir::SliderType::NumEntry => "add_num_entry",
                };
                let zone = self.zone(&var)?;
                let _ = writeln!(
                    out,
                    "{t}ui.{api}({}, {zone}, {}, {}, {}, {})",
                    mojo_string(&label),
                    faust_float(init),
                    faust_float(lo),
                    faust_float(hi),
                    faust_float(step),
                );
                Ok(())
            }
            FirMatch::AddBargraph {
                typ,
                label,
                var,
                lo,
                hi,
            } => {
                let api = match typ {
                    fir::BargraphType::Horizontal => "add_horizontal_bargraph",
                    fir::BargraphType::Vertical => "add_vertical_bargraph",
                };
                let zone = self.zone(&var)?;
                let _ = writeln!(
                    out,
                    "{t}ui.{api}({}, {zone}, {}, {})",
                    mojo_string(&label),
                    faust_float(lo),
                    faust_float(hi),
                );
                Ok(())
            }
            FirMatch::AddMetaDeclare { var, key, value } => {
                match mode {
                    EmitMode::Ui => {
                        let zone = if var == "0" {
                            "dsp.null_val".to_owned()
                        } else {
                            self.zone(&var)?
                        };
                        let _ = writeln!(
                            out,
                            "{t}ui.declare({zone}, {}, {})",
                            mojo_string(&key),
                            mojo_string(&value)
                        );
                    }
                    EmitMode::Default | EmitMode::Metadata => {
                        let _ = writeln!(
                            out,
                            "{t}meta.declare({}, {})",
                            mojo_string(&key),
                            mojo_string(&value)
                        );
                    }
                }
                Ok(())
            }
            FirMatch::NullStatement => {
                let _ = writeln!(out, "{t}pass");
                Ok(())
            }
            // C++ `MojoInstVisitor::visit(LabelInst*)` writes nothing.
            FirMatch::Label(_) | FirMatch::DeclareDependency { .. } => Ok(()),
            FirMatch::AddSoundfile { .. } => Err(soundfile_error().at_node(stmt)),
            _ => Err(unsupported_node("statement", stmt, self.store)),
        }
    }

    fn zone(&mut self, var: &str) -> Result<String, CodegenError> {
        // Batch: a control is per-voice arena state; `build_user_interface`
        // describes it, and a host sets voice v of control i at
        // `arena[voice_controls[i] * voices + v]` (design §6, D2).
        if self
            .batch
            .as_ref()
            .is_some_and(|layout| layout.placement(var).is_some())
        {
            return Ok("dsp.null_val".to_owned());
        }
        Ok(format!("dsp.{}", self.fields.get(var)?))
    }

    // ---------------------------------------------------------------- values

    /// Declared FIR type of a variable, from the scopes or the struct fields.
    fn var_type(&self, name: &str, access: AccessType) -> Option<FirType> {
        match access {
            AccessType::Struct | AccessType::Static => self.field_types.get(name).cloned(),
            _ => self
                .scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get(name).cloned()),
        }
    }

    fn element_type(&self, name: &str, access: AccessType) -> Option<FirType> {
        match self.var_type(name, access)? {
            FirType::Array(inner, _) | FirType::Ptr(inner) | FirType::Vector(inner, _) => {
                Some(*inner)
            }
            _ => None,
        }
    }

    fn var_ref(&mut self, name: &str, access: AccessType) -> Result<String, CodegenError> {
        match access {
            AccessType::Struct | AccessType::Static => {
                Ok(format!("dsp.{}", self.fields.get(name)?))
            }
            // A foreign variable keeps its host spelling.
            AccessType::Global => Ok(name.to_owned()),
            _ => self.locals.get(name),
        }
    }

    /// An indexed access: `a[i]` for an array, `p[unsafe_offset = i]` for a
    /// pointer (C++ `MojoInstVisitor::visit(IndexedAddress*)`).
    fn table_ref(
        &mut self,
        name: &str,
        access: AccessType,
        index: FirId,
    ) -> Result<String, CodegenError> {
        let base = self.var_ref(name, access)?;
        let index = self.emit_value(index)?;
        let is_pointer = match self.var_type(name, access) {
            Some(FirType::Ptr(_)) => true,
            Some(_) => false,
            None => matches!(access, AccessType::FunArgs),
        };
        if is_pointer {
            Ok(format!("{base}[unsafe_offset = {index}]"))
        } else {
            Ok(format!("{base}[{index}]"))
        }
    }

    /// Emits a value converted to `target` when its FIR type differs.
    fn emit_coerced(&mut self, target: &FirType, value: FirId) -> Result<String, CodegenError> {
        let source = value_type(self.store, value);
        // A literal is written directly in the target type.
        match (match_fir(self.store, value), target) {
            (FirMatch::Int32 { value, .. }, FirType::Int32) => return Ok(format!("S32({value})")),
            (FirMatch::Int32 { value, .. }, FirType::Int64) => return Ok(format!("S64({value})")),
            (FirMatch::Int64 { value, .. }, FirType::Int64) => return Ok(format!("S64({value})")),
            (
                FirMatch::Float32 { .. } | FirMatch::Float64 { .. } | FirMatch::Int32 { .. },
                FirType::Float32 | FirType::Float64 | FirType::FaustFloat,
            ) => {
                if let Some(real) = literal_real(self.store, value) {
                    return Ok(self.real_literal(target, real));
                }
            }
            _ => {}
        }
        let rendered = self.emit_value(value)?;
        if source.as_ref() == Some(target) {
            return Ok(rendered);
        }
        Ok(self.cast_rendered(target, source.as_ref(), &rendered))
    }

    fn cast_rendered(&self, target: &FirType, source: Option<&FirType>, rendered: &str) -> String {
        match target {
            FirType::Ptr(_)
            | FirType::Void
            | FirType::Obj
            | FirType::UI
            | FirType::Meta
            | FirType::Sound
            | FirType::Struct(..)
            | FirType::Fun { .. }
            | FirType::Array(..)
            | FirType::Vector(..) => rendered.to_owned(),
            FirType::Bool => match source {
                Some(FirType::Bool) => rendered.to_owned(),
                Some(FirType::Float32 | FirType::Float64 | FirType::FaustFloat) => {
                    format!("({rendered} != 0.0)")
                }
                _ => format!("({rendered} != 0)"),
            },
            _ => format!("{}({rendered})", self.scalar_name(target)),
        }
    }

    /// A real literal in the type `target`.
    fn real_literal(&self, target: &FirType, value: f64) -> String {
        let ctor = self.scalar_name(target);
        let dtype = match target {
            FirType::Float32 => "f32",
            FirType::FaustFloat => "dfaust",
            _ => "f64",
        };
        if value.is_nan() {
            return format!("nan[{dtype}]()");
        }
        if value.is_infinite() {
            let sign = if value < 0.0 { "-" } else { "" };
            return format!("{sign}inf[{dtype}]()");
        }
        let text = if matches!(target, FirType::Float32) {
            MOJO_REAL.format_narrowed(value)
        } else {
            MOJO_REAL.format(value)
        };
        format!("{ctor}({text})")
    }

    /// Emits a FIR value as a Mojo `Bool` condition.
    fn emit_cond(&mut self, cond: FirId) -> Result<String, CodegenError> {
        if let FirMatch::BinOp { op, lhs, rhs, .. } = match_fir(self.store, cond)
            && op.is_comparison()
        {
            return self.emit_comparison(op, lhs, rhs);
        }
        let rendered = self.emit_value(cond)?;
        Ok(match value_type(self.store, cond) {
            Some(FirType::Bool) => rendered,
            Some(FirType::Float32 | FirType::Float64 | FirType::FaustFloat) => {
                format!("({rendered} != 0.0)")
            }
            _ => format!("({rendered} != 0)"),
        })
    }

    fn emit_comparison(
        &mut self,
        op: FirBinOp,
        lhs: FirId,
        rhs: FirId,
    ) -> Result<String, CodegenError> {
        let lt = value_type(self.store, lhs);
        let rt = value_type(self.store, rhs);
        let target = match (&lt, &rt) {
            (Some(l), Some(r)) if numeric_rank(l) >= numeric_rank(r) => l.clone(),
            (Some(_), Some(r)) => r.clone(),
            (Some(l), None) => l.clone(),
            (None, Some(r)) => r.clone(),
            (None, None) => FirType::Int32,
        };
        let l = self.emit_coerced(&target, lhs)?;
        let r = self.emit_coerced(&target, rhs)?;
        let token = crate::backends::textual::c_like_token(op);
        Ok(format!("({l} {token} {r})"))
    }

    fn emit_value(&mut self, value: FirId) -> Result<String, CodegenError> {
        match match_fir(self.store, value) {
            FirMatch::Int32 { value, .. } => Ok(format!("S32({value})")),
            FirMatch::Int64 { value, .. } => Ok(format!("S64({value})")),
            FirMatch::Float32 { value, .. } => {
                Ok(self.real_literal(&FirType::Float32, f64::from(value)))
            }
            FirMatch::Float64 { value, .. } => Ok(self.real_literal(&FirType::Float64, value)),
            FirMatch::Bool { value, .. } => Ok(if value { "True" } else { "False" }.to_owned()),
            FirMatch::LoadVar { name, access, .. } => self.var_ref(&name, access),
            FirMatch::LoadVarAddress { name, access, .. } => {
                Ok(format!("Ptr(to={})", self.var_ref(&name, access)?))
            }
            FirMatch::LoadTable {
                name,
                access,
                index,
                ..
            } => self.table_ref(&name, access, index),
            FirMatch::TeeVar {
                name,
                access,
                value: inner,
                ..
            } => {
                if matches!(access, AccessType::Struct | AccessType::Static) {
                    return Err(unsupported_node("struct-field tee", value, self.store));
                }
                let target = self.locals.get(&name)?;
                let inner = match self.var_type(&name, access) {
                    Some(typ) => self.emit_coerced(&typ, inner)?,
                    None => self.emit_value(inner)?,
                };
                Ok(format!("({target} := {inner})"))
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => self.emit_binop(op, lhs, rhs, &typ),
            FirMatch::Neg { value: inner, typ } => {
                let rendered = self.emit_coerced(&typ, inner)?;
                Ok(format!("(-{rendered})"))
            }
            FirMatch::Cast { typ, value: inner } => self.emit_coerced(&typ, inner),
            FirMatch::Bitcast { typ, value: inner } => {
                let dtype = match typ {
                    FirType::Int32 => "s32",
                    FirType::Int64 => "s64",
                    FirType::Float32 => "f32",
                    FirType::Float64 => "f64",
                    FirType::FaustFloat => "dfaust",
                    _ => return Err(unsupported_node("bitcast", value, self.store)),
                };
                let rendered = self.emit_value(inner)?;
                Ok(format!("bitcast[{dtype}]({rendered})"))
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                typ,
            } => {
                let cond = self.emit_cond(cond)?;
                let then_rendered = self.emit_coerced(&typ, then_value)?;
                let else_rendered = self.emit_coerced(&typ, else_value)?;
                Ok(format!("({then_rendered} if {cond} else {else_rendered})"))
            }
            FirMatch::FunCall { name, args, typ } => self.emit_fun_call(&name, &args, &typ),
            FirMatch::LoadSoundfileLength { .. }
            | FirMatch::LoadSoundfileRate { .. }
            | FirMatch::LoadSoundfileChannels { .. }
            | FirMatch::LoadSoundfileBuffer { .. } => Err(soundfile_error().at_node(value)),
            _ => Err(unsupported_node("value", value, self.store)),
        }
    }

    fn emit_binop(
        &mut self,
        op: FirBinOp,
        lhs: FirId,
        rhs: FirId,
        typ: &FirType,
    ) -> Result<String, CodegenError> {
        if op.is_comparison() {
            let rendered = self.emit_comparison(op, lhs, rhs)?;
            return Ok(match typ {
                FirType::Bool => rendered,
                _ => format!("{}({rendered})", self.scalar_name(typ)),
            });
        }
        let is_int = matches!(typ, FirType::Int32 | FirType::Int64);
        let is_real = matches!(
            typ,
            FirType::Float32 | FirType::Float64 | FirType::FaustFloat
        );
        match op {
            FirBinOp::Div if is_int => self.helper_call(Helper::IDiv, typ, &[lhs, rhs]),
            FirBinOp::Rem if is_int => self.helper_call(Helper::IRem, typ, &[lhs, rhs]),
            FirBinOp::Rem if is_real => self.helper_call(Helper::FMod, typ, &[lhs, rhs]),
            FirBinOp::LRsh => {
                let (unsigned, signed) = if matches!(typ, FirType::Int64) {
                    ("U64", "S64")
                } else {
                    ("U32", "S32")
                };
                let l = self.emit_coerced(typ, lhs)?;
                let r = self.emit_coerced(typ, rhs)?;
                Ok(format!("{signed}({unsigned}({l}) >> {unsigned}({r}))"))
            }
            _ => {
                let l = self.emit_coerced(typ, lhs)?;
                let r = self.emit_coerced(typ, rhs)?;
                let token = crate::backends::textual::c_like_token(op);
                Ok(format!("({l} {token} {r})"))
            }
        }
    }

    fn helper_call(
        &mut self,
        helper: Helper,
        typ: &FirType,
        args: &[FirId],
    ) -> Result<String, CodegenError> {
        self.helpers.insert(helper);
        let mut rendered = Vec::with_capacity(args.len());
        for arg in args {
            rendered.push(self.emit_coerced(typ, *arg)?);
        }
        Ok(format!("{}({})", helper.name(), rendered.join(", ")))
    }

    /// Emits one FIR call with Mojo `std.math` spellings.
    ///
    /// Names arrive normalized by FIR lowering (`sinf`, `fmin`, `min_i`, ...);
    /// the FIR result type carries the precision, so the C `f` suffix is
    /// dropped. Unknown names (FIR helpers, foreign functions) are emitted
    /// verbatim.
    fn emit_fun_call(
        &mut self,
        name: &str,
        args: &[FirId],
        typ: &FirType,
    ) -> Result<String, CodegenError> {
        let base = name.strip_prefix("std::").unwrap_or(name);
        let base = match base.strip_suffix('f') {
            Some(stem) if KNOWN_REAL_FNS.contains(&stem) => stem,
            _ => base,
        };
        let math = match base {
            "min_i" | "min_f" | "min_" | "fmin" | "min" => Some("min"),
            "max_i" | "max_f" | "max_" | "fmax" | "max" => Some("max"),
            "fabs" | "abs" | "labs" => Some("abs"),
            "round" => {
                return self.helper_call(Helper::Round, typ, args);
            }
            // `std.math` has no `rint`; its `round` rounds half to even, which
            // is C `rint` in the default rounding mode.
            "rint" => Some("round"),
            "fmod" => {
                return self.helper_call(Helper::FMod, typ, args);
            }
            "exp10" => {
                let x = self.emit_coerced(typ, args[0])?;
                return Ok(format!("pow({}, {x})", self.real_literal(typ, 10.0)));
            }
            "isnan" | "isinf" | "isfinite" => {
                let arg = self.emit_value(args[0])?;
                return Ok(format!("{}({base}({arg}))", self.scalar_name(typ)));
            }
            other if KNOWN_REAL_FNS.contains(&other) => Some(other),
            _ => None,
        };
        let mut rendered = Vec::with_capacity(args.len());
        if let Some(math) = math {
            for arg in args {
                rendered.push(self.emit_coerced(typ, *arg)?);
            }
            return Ok(format!("{math}({})", rendered.join(", ")));
        }
        for arg in args {
            rendered.push(self.emit_value(*arg)?);
        }
        Ok(format!("{name}({})", rendered.join(", ")))
    }

    // ----------------------------------------------------------------- types

    /// The Mojo constructor of a scalar FIR type.
    fn scalar_name(&self, typ: &FirType) -> &'static str {
        match typ {
            FirType::Int32 => "S32",
            FirType::Int64 => "S64",
            FirType::Float32 => "F32",
            FirType::FaustFloat => "FaustFloat",
            FirType::Bool => "Bool",
            _ => "F64",
        }
    }

    fn emit_type(&self, typ: &FirType) -> Result<String, CodegenError> {
        Ok(match typ {
            FirType::Int32 => "S32".to_owned(),
            FirType::Int64 => "S64".to_owned(),
            FirType::Float32 => "F32".to_owned(),
            FirType::Float64 => "F64".to_owned(),
            FirType::FaustFloat => "FaustFloat".to_owned(),
            FirType::Bool => "Bool".to_owned(),
            FirType::Void => "None".to_owned(),
            FirType::Ptr(inner) => match inner.as_ref() {
                FirType::Ptr(_) => "MutStreams".to_owned(),
                inner => format!("Ptr[{}]", self.emit_type(inner)?),
            },
            FirType::Array(inner, size) => format!("Arr[{}, {size}]", self.emit_type(inner)?),
            FirType::Quad | FirType::FixedPoint => {
                return Err(CodegenError::new(
                    CodegenErrorCode::UnsupportedNode,
                    "the Mojo backend supports -single and -double only (no -quad or fixed point)",
                ));
            }
            FirType::Sound => return Err(soundfile_error()),
            other => {
                return Err(CodegenError::new(
                    CodegenErrorCode::UnsupportedNode,
                    format!("Mojo has no spelling for the FIR type {other:?}"),
                ));
            }
        })
    }

    /// The zero of a field or local declared without a value.
    fn zero_value(&self, typ: &FirType) -> Result<String, CodegenError> {
        Ok(match typ {
            FirType::Bool => "False".to_owned(),
            FirType::Array(inner, size) => {
                let elem = self.emit_type(inner)?;
                let fill = match inner.as_ref() {
                    FirType::Bool => "False".to_owned(),
                    FirType::Int32 | FirType::Int64 => format!("{elem}(0)"),
                    _ => format!("{elem}(0.0)"),
                };
                format!("Arr[{elem}, {size}](fill={fill})")
            }
            FirType::Int32 | FirType::Int64 => format!("{}(0)", self.scalar_name(typ)),
            FirType::Float32 | FirType::Float64 | FirType::FaustFloat => {
                format!("{}(0.0)", self.scalar_name(typ))
            }
            other => {
                return Err(CodegenError::new(
                    CodegenErrorCode::UnsupportedNode,
                    format!("no Mojo zero for a field of FIR type {other:?}"),
                ));
            }
        })
    }
}

/// Real math functions spelled the same in C and Mojo `std.math`, once their
/// single-precision `f` suffix is dropped.
const KNOWN_REAL_FNS: &[&str] = &[
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atan2",
    "atanh",
    "cbrt",
    "ceil",
    "copysign",
    "cos",
    "cosh",
    "erf",
    "erfc",
    "exp",
    "exp2",
    "exp10",
    "fabs",
    "floor",
    "fmax",
    "fmin",
    "fmod",
    "log",
    "log10",
    "log2",
    "pow",
    "remainder",
    "rint",
    "round",
    "sin",
    "sinh",
    "sqrt",
    "tan",
    "tanh",
    "trunc",
    "isnan",
    "isinf",
    "isfinite",
];

fn method(out: &mut String, signature: &str) {
    let _ = writeln!(out, "    @always_inline");
    let _ = writeln!(out, "    {signature}");
}

/// Writes one DSP method: `signature`, then its rendered `body`, then a
/// blank line.
///
/// Mojo 1.1.0.dev2026090705 never finishes compiling a method that reads or
/// writes a field through its receiver (`dsp.x`) inside nested loops: two
/// nested `for ... in range`, or three nested `while`, on scalar or array
/// fields, at -O0 as at -O3 (19-line programs without Faust reproduce it).
/// The same accesses through a local reference compile at once. So a method
/// whose body has a loop takes the receiver as `self` and opens with
/// `ref dsp = self`: the body text, the method name and its argument types
/// stay those of the C++ backend; only the receiver's name differs, which no
/// caller sees.
fn write_method(out: &mut String, signature: &str, body: &str) {
    let has_loop = body.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("for ") || line.starts_with("while ")
    });
    if has_loop {
        let signature =
            signature
                .replacen("(mut dsp", "(mut self", 1)
                .replacen("(imm dsp", "(imm self", 1);
        debug_assert!(signature.contains(" self"));
        method(out, &signature);
        out.push_str("        ref dsp = self\n");
    } else {
        method(out, signature);
    }
    out.push_str(body);
    let _ = writeln!(out);
}

fn write_pass(out: &mut String) {
    let _ = writeln!(out, "        pass");
}

fn tab(indent: usize) -> String {
    "    ".repeat(indent)
}

fn is_empty_body(store: &FirStore, decl: &FunctionDecl) -> bool {
    decl.body.is_none_or(
        |body| matches!(match_fir(store, body), FirMatch::Block(items) if items.is_empty()),
    )
}

fn soundfile_error() -> CodegenError {
    CodegenError::new(
        CodegenErrorCode::UnsupportedNode,
        "soundfiles are not supported by the Mojo backend (as in the C++ Mojo backend)",
    )
}

/// A UI parameter as the C++ backend writes it: `FaustFloat(<real>)`.
fn faust_float(value: f64) -> String {
    format!("FaustFloat({})", MOJO_REAL.format(value))
}

fn mojo_string(input: &str) -> String {
    quoted_string(input, StringDialect::C)
}

/// The value of a numeric literal, read as a real.
fn literal_real(store: &FirStore, value: FirId) -> Option<f64> {
    match match_fir(store, value) {
        FirMatch::Float32 { value, .. } => Some(f64::from(value)),
        FirMatch::Float64 { value, .. } => Some(value),
        FirMatch::Int32 { value, .. } => Some(f64::from(value)),
        _ => None,
    }
}

/// Returns the FIR result type of one value node, when it carries one.
fn value_type(store: &FirStore, value: FirId) -> Option<FirType> {
    match match_fir(store, value) {
        FirMatch::LoadSoundfileLength { .. }
        | FirMatch::LoadSoundfileRate { .. }
        | FirMatch::LoadSoundfileChannels { .. } => Some(FirType::Int32),
        _ => store.value_type(value),
    }
}

/// C usual-arithmetic-conversion rank used for comparison promotion.
fn numeric_rank(typ: &FirType) -> u8 {
    match typ {
        FirType::Bool => 0,
        FirType::Int32 => 1,
        FirType::Int64 => 2,
        FirType::Float32 => 3,
        FirType::FaustFloat => 4,
        FirType::Float64 => 5,
        _ => 0,
    }
}

fn compile_options(options: &MojoOptions) -> String {
    options.compile_options.clone().unwrap_or_else(|| {
        match options.real_type {
            MojoRealType::Float32 => "-lang mojo -single",
            MojoRealType::Float64 => "-lang mojo -double",
        }
        .to_owned()
    })
}

/// The comment banner of C++ `MojoCodeContainer::writeFaustHeader`.
fn emit_banner(out: &mut String, options: &MojoOptions) {
    const RULE: &str =
        "# ==============================================================================";
    let _ = writeln!(out, "{RULE}");
    let _ = writeln!(
        out,
        "# Code generated with Faust {} (https://faust.grame.fr)",
        crate::VERSION
    );
    for key in ["author", "copyright", "license", "name", "version"] {
        let values: Vec<String> = if key == "name" {
            options
                .metadata_name
                .iter()
                .map(|value| mojo_string(value))
                .collect()
        } else {
            options
                .metadata_entries
                .iter()
                .filter(|(entry, _)| entry == key)
                .map(|(_, value)| mojo_string(value))
                .collect()
        };
        if !values.is_empty() {
            let _ = writeln!(out, "# {key}: {}", values.join(", "));
        }
    }
    let _ = writeln!(out, "# Compilation options: ");
    let _ = writeln!(out, "{}", wrap_compile_options(&compile_options(options)));
    let _ = writeln!(out, "{RULE}");
}

/// C++ `formatCompilerOptions(2, "# ")`: an option and its value stay on one
/// line, and lines stop before 81 columns.
fn wrap_compile_options(options: &str) -> String {
    const MAX_WIDTH: usize = 81;
    let words: Vec<&str> = options.split(' ').filter(|w| !w.is_empty()).collect();
    let mut units = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        if word.starts_with('-') && i + 1 < words.len() && !words[i + 1].starts_with('-') {
            units.push(format!("{word} {}", words[i + 1]));
            i += 2;
        } else {
            units.push(word.to_owned());
            i += 1;
        }
    }
    let prefix = "#   ";
    let mut out = String::from(prefix);
    let mut count = prefix.len();
    for unit in units {
        let needed = 1 + unit.len();
        if count + needed > MAX_WIDTH {
            out.push('\n');
            out.push_str(prefix);
            count = prefix.len();
        }
        out.push_str(&unit);
        out.push(' ');
        count += needed;
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

mod batch;
mod names;
#[cfg(test)]
mod tests;
