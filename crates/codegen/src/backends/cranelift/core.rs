//! Core Cranelift backend data types and shared helpers.
//!
//! This module defines the public configuration, error, JIT module, and layout
//! structures used by the Cranelift facade. Implementation modules depend on
//! these types but do not expand the public backend API directly.

use super::*;

/// Stable backend identifier used by tooling and future CLI wiring.
pub const BACKEND_NAME: &str = "cranelift";

#[must_use]
/// Returns the stable backend identifier (`"cranelift"`).
pub fn backend_id() -> &'static str {
    BACKEND_NAME
}

/// Cranelift optimization level (backend-local configuration surface).
///
/// API mapping status: `adapted` (no direct C++ Cranelift backend exists).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CraneliftOptLevel {
    /// Fastest compile time.
    None,
    /// Balanced mode (default planned mode for validation bring-up).
    #[default]
    Speed,
    /// Highest optimization effort.
    SpeedAndSize,
}

/// Options controlling Cranelift backend compilation (scaffold).
///
/// This mirrors the shape planned in `porting/cranelift-backend-plan-en.md`,
/// but no codegen semantics are implemented yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CraneliftOptions {
    /// Custom host-managed state layout.
    ///
    /// API mapping status: `adapted`; Faust C++ has no Cranelift backend.
    /// The value is retained in the compiled module/factory path and changes
    /// JIT layout only when it is [`MemoryManagerMode::Mem0`].
    pub memory_manager_mode: MemoryManagerMode,
    /// Optimization level requested for Cranelift.
    pub opt_level: CraneliftOptLevel,
    /// Optional explicit target triple (string form for portability at the
    /// facade boundary; parsed later by the backend implementation).
    pub target_triple: Option<String>,
    /// Enable deterministic NaN canonicalization when supported.
    pub enable_nan_canonicalization: bool,
    /// Emit backend debug IR dumps once implemented.
    pub debug_ir_dump: bool,
    /// Fails compilation when the `compute` body falls outside the current
    /// lowering subset instead of emitting the no-op fallback stub.
    ///
    /// This is intended for strict validation/CI workflows to prevent silent
    /// runtime acceptance with reduced behavior.
    pub fail_on_subset_gap: bool,
    /// Optional host addresses for FIR `AccessType::Global` scalar symbols
    /// (for example `fvariable(float extvar, ...)`).
    ///
    /// The backend imports these as external data symbols and loads them
    /// directly from JIT-resolved storage during `compute` lowering.
    pub extern_data_symbols: HashMap<String, *const c_void>,
    /// Optional host addresses for foreign function symbols referenced by FIR
    /// `FunCall` nodes that are not covered by the built-in math registry.
    ///
    /// This provides the Cranelift equivalent of LLVM's
    /// `registerForeignFunction`, but uses an explicit name -> pointer map
    /// rather than process-global symbol lookup.
    pub extern_function_symbols: HashMap<String, *const c_void>,
    /// Use 64-bit (`double`) precision for `FAUSTFLOAT` I/O, UI zones and state.
    ///
    /// When `true`, the abstract [`FirType::FaustFloat`] maps to Cranelift `F64`
    /// (and an 8-byte/8-align struct field) instead of `F32`. This must match the
    /// precision the FIR was produced with (`RealType::Float64`) so the internal
    /// real type (`Float64`) and the I/O type agree.
    pub double_precision: bool,
}

/// Stable error codes for the Cranelift backend scaffold and future lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CraneliftBackendErrorCode {
    /// Backend is scaffolded but not implemented yet.
    NotImplemented,
    /// FIR root is not a module or has an unexpected shape.
    UnsupportedModuleShape,
    /// FIR module does not contain a supported `compute` declaration.
    MissingCompute,
    /// Failed to initialize or use the Cranelift JIT toolchain.
    JitFailure,
    /// Custom-memory layout analysis or lowering failed.
    MemoryLayout,
}

impl CraneliftBackendErrorCode {
    /// Returns the stable machine-readable error code string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotImplemented => "FRS-CGEN-CLIF-0001",
            Self::UnsupportedModuleShape => "FRS-CGEN-CLIF-0002",
            Self::MissingCompute => "FRS-CGEN-CLIF-0003",
            Self::JitFailure => "FRS-CGEN-CLIF-0004",
            Self::MemoryLayout => "FRS-CGEN-CLIF-0005",
        }
    }
}

/// Typed Cranelift backend error used by the Cranelift codegen entry points.
///
/// This is the stable Rust-facing error container returned by
/// [`generate_cranelift_module`] and related helpers. It carries:
/// - a stable machine-readable code ([`CraneliftBackendErrorCode`]),
/// - a human-readable message suitable for diagnostics/logging.
///
/// # Stability notes
/// - `code` is intended to remain stable for tooling/tests.
/// - `message` may evolve as lowering coverage and diagnostics improve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraneliftBackendError {
    /// Machine-readable stable backend error code.
    pub code: CraneliftBackendErrorCode,
    /// Human-readable message.
    pub message: String,
}

impl CraneliftBackendError {
    /// Builds an `UnsupportedModuleShape` backend error.
    pub(crate) fn unsupported_module_shape(message: impl Into<String>) -> Self {
        Self {
            code: CraneliftBackendErrorCode::UnsupportedModuleShape,
            message: message.into(),
        }
    }

    /// Builds a `MissingCompute` backend error.
    pub(crate) fn missing_compute(message: impl Into<String>) -> Self {
        Self {
            code: CraneliftBackendErrorCode::MissingCompute,
            message: message.into(),
        }
    }

    /// Builds a `JitFailure` backend error.
    pub(crate) fn jit_failure(message: impl Into<String>) -> Self {
        Self {
            code: CraneliftBackendErrorCode::JitFailure,
            message: message.into(),
        }
    }

    /// Builds a `MemoryLayout` backend error.
    pub(crate) fn memory_layout(message: impl Into<String>) -> Self {
        Self {
            code: CraneliftBackendErrorCode::MemoryLayout,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CraneliftBackendError {
    /// Formats the typed error as `[CODE] message`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CraneliftBackendError {}

/// Compiled JIT module handle for the Cranelift backend bring-up path.
///
/// Current contents:
/// - owned Cranelift JIT module (keeps finalized code alive),
/// - finalized `compute` symbol address (opaque integer),
/// - module/function names for debug/test assertions.
/// - `compute` subset-lowering status,
/// - backend-derived `dsp*` memory layout contract.
///
/// # Ownership / lifetime
/// The finalized machine code remains valid only while this value is alive,
/// because the underlying [`JITModule`] owns the emitted code memory and import
/// bookkeeping. Dropping [`JitDspModule`] invalidates `compute_entry_addr()`.
///
/// # Safety note
/// This type intentionally exposes the finalized entry address as `usize`
/// instead of a typed function pointer because invoking the code would require
/// `unsafe`, which is deferred to the future runtime/FFI layers.
///
/// API mapping status: `adapted`.
pub struct JitDspModule {
    pub(crate) module_name: String,
    pub(crate) compute_symbol_name: String,
    pub(crate) compute_entry_addr: usize,
    pub(crate) static_init_entry_addr: usize,
    pub(crate) instance_constants_entry_addr: usize,
    pub(crate) instance_clear_entry_addr: usize,
    pub(crate) compute_body_lowered: bool,
    pub(crate) generated_functions_clif: Vec<(String, String)>,
    pub(crate) struct_layout: StructLayoutPlan,
    pub(crate) mem0_analysis: Option<Mem0Analysis>,
    pub(crate) static_memory_slots: Vec<StaticMemorySlot>,
    pub(crate) jit_module: JITModule,
}

impl std::fmt::Debug for JitDspModule {
    /// Renders a compact debug view that avoids dumping the owned `JITModule`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JitDspModule")
            .field("module_name", &self.module_name)
            .field("compute_symbol_name", &self.compute_symbol_name)
            .field("compute_entry_addr", &self.compute_entry_addr)
            .field(
                "instance_constants_entry_addr",
                &self.instance_constants_entry_addr,
            )
            .field("instance_clear_entry_addr", &self.instance_clear_entry_addr)
            .field(
                "generated_functions_clif_count",
                &self.generated_functions_clif.len(),
            )
            .finish()
    }
}

impl JitDspModule {
    /// Returns the FIR module name captured from the FIR `Module` node.
    ///
    /// This is useful for logging/debugging and for predictable symbol naming
    /// assertions in tests.
    #[must_use]
    pub fn module_name(&self) -> &str {
        &self.module_name
    }

    /// Returns the finalized Cranelift symbol name used for `compute`.
    ///
    /// The name is exported inside the Cranelift JIT module and is currently
    /// derived as `"{module_name}::compute"`.
    #[must_use]
    pub fn compute_symbol_name(&self) -> &str {
        &self.compute_symbol_name
    }

    /// Returns the finalized `compute` entry address as an opaque integer.
    ///
    /// The early bring-up path intentionally avoids exposing/calling a typed
    /// function pointer here because workspace lints forbid `unsafe`.
    #[must_use]
    pub fn compute_entry_addr(&self) -> usize {
        self.compute_entry_addr
    }

    /// Returns the finalized `instanceConstants` entry address when emitted.
    #[must_use]
    pub fn instance_constants_entry_addr(&self) -> usize {
        self.instance_constants_entry_addr
    }

    /// Returns the finalized `staticInit` entry address when emitted.
    ///
    /// Non-zero only when the module has a `staticInit` function, which is the
    /// case whenever a generated table needs filling.
    #[must_use]
    pub fn static_init_entry_addr(&self) -> usize {
        self.static_init_entry_addr
    }

    /// Returns the finalized `instanceClear` entry address when emitted.
    ///
    /// `instanceClear` resets state buffers; some DSPs (e.g. `prefix`) fill them
    /// with non-zero initial values here, so the runtime must run it at init.
    #[must_use]
    pub fn instance_clear_entry_addr(&self) -> usize {
        self.instance_clear_entry_addr
    }

    /// Returns `true` when a finalized non-null `compute` symbol address exists.
    ///
    /// This is a cheap postcondition check for callers that only need to know
    /// whether JIT finalization produced an address, without inspecting the
    /// lowering mode (`real subset lowering` vs `stub fallback`).
    #[must_use]
    pub fn has_compute_entry(&self) -> bool {
        self.compute_entry_addr != 0
    }

    /// Returns true when the current subset lowering handled the FIR `compute`
    /// body. False means the backend emitted the no-op stub fallback.
    #[must_use]
    pub fn compute_body_lowered(&self) -> bool {
        self.compute_body_lowered
    }

    /// Returns textual CLIF IR generated for all backend-defined DSP functions.
    ///
    /// Each entry is `(function_symbol_name, clif_text)`.
    /// In the current backend state this contains at least `compute`, and it is
    /// designed to grow as more DSP lifecycle functions are lowered.
    #[must_use]
    pub fn generated_functions_clif(&self) -> &[(String, String)] {
        &self.generated_functions_clif
    }

    /// Returns the backend `dsp*` struct layout contract derived from FIR
    /// `globals` declarations.
    ///
    /// The returned layout is deterministic for a given FIR module and current
    /// backend contract (including the current `FAUSTFLOAT -> f32` decision).
    /// It is used by lowering of `AccessType::Struct` loads/stores and is also
    /// intended to be reused by future runtime allocation paths.
    #[must_use]
    pub fn struct_layout(&self) -> &StructLayoutPlan {
        &self.struct_layout
    }

    /// Returns the canonical custom-memory snapshot retained by a `-mem0` JIT.
    ///
    /// The FFI runtime consumes this exact snapshot for manager description and
    /// allocation; it never reclassifies FIR after machine code generation.
    #[must_use]
    pub fn mem0_analysis(&self) -> Option<&Mem0Analysis> {
        self.mem0_analysis.as_ref()
    }

    /// Returns manager-backed static-table pointer slots owned by this JIT.
    #[must_use]
    pub fn static_memory_slots(&self) -> &[StaticMemorySlot] {
        &self.static_memory_slots
    }

    /// Internal guard used by tests to ensure the JIT module stays owned/alive.
    ///
    /// This method intentionally touches the private `jit_module` field so test
    /// code can assert the ownership path without exposing the Cranelift type
    /// itself in the public API.
    #[must_use]
    pub fn jit_module_is_alive(&self) -> bool {
        let _ = &self.jit_module;
        true
    }
}

/// Deterministic layout of the backend `dsp*` instance state for Cranelift.
///
/// Contract (current V1-oriented backend contract):
/// - Derived from FIR module `globals` block in declaration order.
/// - Includes `DeclareVar` with `AccessType::Struct`.
/// - `FAUSTFLOAT` is laid out as `f32` during the current bring-up.
/// - Natural alignment is applied per field (backend-target pointer size aware).
/// - Offsets are byte offsets relative to the `dsp*` base pointer.
///
/// This contract is backend-internal for now, but is designed to be stable and
/// reusable by the future `cranelift_dsp` instance allocation/runtime path.
#[derive(Clone, Debug, PartialEq)]
pub struct StructLayoutPlan {
    fields: Vec<StructFieldLayout>,
    size_bytes: u32,
    align_bytes: u32,
}

impl StructLayoutPlan {
    /// Returns all fields in declaration/layout order.
    ///
    /// Order is significant: offsets are assigned by iterating FIR `globals`
    /// in order and applying alignment. Callers should not assume name sorting.
    #[must_use]
    pub fn fields(&self) -> &[StructFieldLayout] {
        &self.fields
    }

    /// Returns the total struct size in bytes, including final padding.
    ///
    /// The size is rounded up to `align_bytes()`.
    #[must_use]
    pub fn size_bytes(&self) -> u32 {
        self.size_bytes
    }

    /// Returns the required alignment of the full `dsp*` state struct in bytes.
    ///
    /// This is currently the maximum alignment of all included fields.
    #[must_use]
    pub fn align_bytes(&self) -> u32 {
        self.align_bytes
    }

    /// Looks up a field by FIR/global name.
    ///
    /// Returns `None` when the name is not part of the backend layout (for
    /// example helper prototypes in `globals`, which are intentionally ignored).
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&StructFieldLayout> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// One field in the Cranelift backend `dsp*` struct layout.
#[derive(Clone, Debug, PartialEq)]
pub struct StructFieldLayout {
    /// FIR/global variable name used as the layout key.
    pub name: String,
    /// Storage shape in the backend contract (scalar or inline table).
    pub kind: StructFieldKind,
    /// Byte offset relative to the `dsp*` base pointer.
    pub offset_bytes: u32,
    /// Field storage size in bytes (table payload size for tables).
    pub size_bytes: u32,
    /// Field alignment in bytes.
    pub align_bytes: u32,
}

/// Backend `dsp*` field storage kind in the Cranelift layout contract.
#[derive(Clone, Debug, PartialEq)]
pub enum StructFieldKind {
    /// Single scalar value stored inline.
    Scalar(FirType),
    /// Inline array/table payload stored inside the `dsp*` allocation.
    ///
    /// `len` is the number of elements (not bytes).
    Table { elem_type: FirType, len: u32 },
    /// Pointer slot whose payload is allocated by the host memory manager.
    ExternalTable {
        /// Scalar representation stored in the manager-owned payload.
        elem_type: FirType,
        /// Logical payload length in elements.
        len: u32,
        /// Stable identity in the retained canonical memory layout.
        zone_id: MemoryZoneId,
    },
}

/// Finalized JIT data slot used to publish one manager-owned static table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticMemorySlot {
    /// FIR/JIT data symbol name.
    pub name: String,
    /// Stable identity of the matching class allocation.
    pub zone_id: MemoryZoneId,
    /// Finalized address of the JIT-owned pointer slot.
    pub address: usize,
}

impl StructFieldLayout {
    /// Returns the scalar FIR type when this field is [`StructFieldKind::Scalar`].
    ///
    /// This is a convenience helper for callers that only care about scalar
    /// state and want to skip manual enum matching.
    #[must_use]
    pub fn scalar_type(&self) -> Option<&FirType> {
        match &self.kind {
            StructFieldKind::Scalar(t) => Some(t),
            StructFieldKind::Table { .. } | StructFieldKind::ExternalTable { .. } => None,
        }
    }
}

/// Concrete size/alignment pair for one scalar storage type in the layout plan.
///
/// This is internal because only the finalized [`StructLayoutPlan`] is part of
/// the backend contract exposed to callers.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LayoutScalar {
    /// Size of the scalar in bytes (e.g. 4 for `f32`/`i32`, 8 for `f64`/`i64`).
    size: u32,
    /// Required alignment of the scalar in bytes.  Used by [`align_up`] when
    /// packing consecutive fields into the `dsp*` struct layout.
    align: u32,
}

/// Rounds `value` up to the next multiple of `align`.
///
/// `align <= 1` is treated as already aligned.
pub(crate) fn align_up(value: u32, align: u32) -> u32 {
    if align <= 1 {
        return value;
    }
    let rem = value % align;
    if rem == 0 {
        value
    } else {
        value + (align - rem)
    }
}

/// Finds the FIR module name and the concrete `compute` declaration to lower.
///
/// # Expected FIR shape
/// - root node: `Module`
/// - `functions`: `Block([...])`
/// - a `DeclareFun { name: "compute", body: Some(..) }` entry exists
///
/// Prototype-only `compute` declarations (`body: None`) are ignored because the
/// Cranelift backend currently requires an executable body.
pub(crate) fn find_module_and_function(
    store: &FirStore,
    module: FirId,
    function_name: &str,
) -> Result<(String, FirId), CraneliftBackendError> {
    let (module_name, _globals, functions) = match match_fir(store, module) {
        FirMatch::Module {
            name,
            globals,
            functions,
            ..
        } => (name, globals, functions),
        other => {
            return Err(CraneliftBackendError::unsupported_module_shape(format!(
                "expected FIR Module root, got {other:?} at {}",
                module.as_u32()
            )));
        }
    };

    let function_nodes = match match_fir(store, functions) {
        FirMatch::Block(items) => items,
        other => {
            return Err(CraneliftBackendError::unsupported_module_shape(format!(
                "module functions must be FIR Block, got {other:?} at {}",
                functions.as_u32()
            )));
        }
    };

    let compute_id = function_nodes
        .into_iter()
        .find(|id| {
            matches!(
                match_fir(store, *id),
                FirMatch::DeclareFun {
                    ref name,
                    body: Some(_),
                    ..
                } if name == function_name
            )
        })
        .ok_or_else(|| {
            CraneliftBackendError::missing_compute(format!(
                "FIR module `{module_name}` has no supported `{function_name}` definition"
            ))
        })?;

    Ok((module_name, compute_id))
}

/// Shorthand for [`find_module_and_function`] looking up `compute` specifically.
///
/// This is the lookup used by the main [`generate_cranelift_module`] entry
/// point, since every backend compilation revolves around lowering `compute`.
pub(crate) fn find_module_and_compute(
    store: &FirStore,
    module: FirId,
) -> Result<(String, FirId), CraneliftBackendError> {
    find_module_and_function(store, module, "compute")
}

/// Maps a FIR scalar/storage type to the backend `dsp*` layout scalar size/alignment.
///
/// This helper is used only while deriving the backend `StructLayoutPlan` from
/// FIR `globals`. It intentionally reflects the current bring-up contract
/// (notably `FAUSTFLOAT -> f32`).
///
/// Unsupported FIR types here are rejected as module-shape issues because they
/// make the current backend state layout contract undefined.
pub(crate) fn fir_type_layout_scalar(
    ptr_size: u32,
    typ: &FirType,
    double: bool,
) -> Result<LayoutScalar, CraneliftBackendError> {
    let s = match typ {
        FirType::Bool => LayoutScalar { size: 1, align: 1 },
        FirType::Int32 => LayoutScalar { size: 4, align: 4 },
        FirType::Float32 => LayoutScalar { size: 4, align: 4 },
        FirType::FaustFloat if double => LayoutScalar { size: 8, align: 8 },
        FirType::FaustFloat => LayoutScalar { size: 4, align: 4 },
        FirType::Int64 | FirType::Float64 => LayoutScalar { size: 8, align: 8 },
        FirType::Ptr(_) | FirType::Obj | FirType::UI | FirType::Meta | FirType::Sound => {
            LayoutScalar {
                size: ptr_size,
                align: ptr_size,
            }
        }
        other => {
            return Err(CraneliftBackendError::unsupported_module_shape(format!(
                "unsupported Cranelift dsp* layout field type in V1 bring-up contract: {other:?}"
            )));
        }
    };
    Ok(s)
}

/// Maps a FIR scalar/storage type to the Cranelift IR (CLIF) register type
/// used to hold it during `compute` lowering.
///
/// This is distinct from [`fir_type_layout_scalar`]: that helper computes
/// byte size/alignment for the `dsp*` memory layout, while this one picks
/// the Cranelift SSA value type (`I32`, `F32`, `F64`, pointer type, ...) used
/// once a value is loaded into a register during lowering.
pub(crate) fn fir_type_to_clif_type(
    ptr_ty: Type,
    typ: &FirType,
    double: bool,
) -> Result<Type, String> {
    match typ {
        FirType::Int32 => Ok(types::I32),
        FirType::Int64 => Ok(types::I64),
        FirType::Float32 => Ok(types::F32),
        FirType::Float64 => Ok(types::F64),
        FirType::FaustFloat if double => Ok(types::F64),
        FirType::FaustFloat => Ok(types::F32),
        FirType::Bool => Ok(types::I8),
        FirType::Ptr(_) | FirType::Obj | FirType::UI | FirType::Meta | FirType::Sound => Ok(ptr_ty),
        other => Err(format!(
            "unsupported FIR type in Cranelift subset lowering: {other:?}"
        )),
    }
}

/// Builds the deterministic backend `dsp*` state layout from FIR module globals.
///
/// # Inclusion rules (current contract)
/// - includes `DeclareVar { access: Struct, .. }` as scalar fields
/// - includes `DeclareVar { access: Struct, typ: Array(..), .. }` as inline
///   array payload fields
/// - includes `DeclareTable { access: Struct, .. }` as inline table fields
/// - ignores helper function prototypes (`DeclareFun { body: None, .. }`)
/// - ignores helper declarations/prototypes that are not `dsp*` state
///
/// # Rejection policy
/// Any other global entry shape (for example unsupported access classes or
/// unsupported FIR types) is rejected with `UnsupportedModuleShape`, because
/// lowering `AccessType::Struct` depends on a total, unambiguous layout.
pub(crate) fn build_struct_layout_for_module(
    store: &FirStore,
    module: FirId,
    ptr_size: u32,
    double: bool,
    mem0: Option<&Mem0Analysis>,
) -> Result<StructLayoutPlan, CraneliftBackendError> {
    let (dsp_struct, globals) = match match_fir(store, module) {
        FirMatch::Module {
            dsp_struct,
            globals,
            ..
        } => (dsp_struct, globals),
        other => {
            return Err(CraneliftBackendError::unsupported_module_shape(format!(
                "expected FIR Module root for struct layout, got {other:?} at {}",
                module.as_u32()
            )));
        }
    };
    let dsp_struct_items = match match_fir(store, dsp_struct) {
        FirMatch::Block(items) => items,
        other => {
            return Err(CraneliftBackendError::unsupported_module_shape(format!(
                "module dsp_struct must be FIR Block, got {other:?} at {}",
                dsp_struct.as_u32()
            )));
        }
    };
    let global_items = match match_fir(store, globals) {
        FirMatch::Block(items) => items,
        other => {
            return Err(CraneliftBackendError::unsupported_module_shape(format!(
                "module globals must be FIR Block, got {other:?} at {}",
                globals.as_u32()
            )));
        }
    };

    let external_zones: HashMap<_, _> = mem0
        .into_iter()
        .flat_map(|analysis| analysis.memory_layout.zones.iter())
        .filter(|zone| {
            zone.scope == MemoryScope::Instance && zone.role == MemoryRole::InstanceBuffer
        })
        .map(|zone| (zone.name.as_str(), zone.id))
        .collect();
    let mut fields = Vec::new();
    let mut offset = 0u32;
    let mut struct_align = if mem0.is_some() { ptr_size } else { 1 };
    for item in dsp_struct_items.into_iter().chain(global_items) {
        match match_fir(store, item) {
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Struct,
                ..
            } => match typ {
                FirType::Array(inner, len) => {
                    let elem_type = *inner;
                    let scalar = fir_type_layout_scalar(ptr_size, &elem_type, double)?;
                    let len = u32::try_from(len).map_err(|_| {
                        CraneliftBackendError::unsupported_module_shape(
                            "Cranelift dsp* array field length does not fit in u32",
                        )
                    })?;
                    let payload_size = scalar.size.checked_mul(len).ok_or_else(|| {
                        CraneliftBackendError::unsupported_module_shape(
                            "Cranelift dsp* array field size overflow",
                        )
                    })?;
                    let external_zone = external_zones.get(name.as_str()).copied();
                    let storage = if external_zone.is_some() {
                        LayoutScalar {
                            size: ptr_size,
                            align: ptr_size,
                        }
                    } else {
                        LayoutScalar {
                            size: payload_size,
                            align: scalar.align,
                        }
                    };
                    offset = align_up(offset, storage.align);
                    fields.push(StructFieldLayout {
                        name,
                        kind: match external_zone {
                            Some(zone_id) => StructFieldKind::ExternalTable {
                                elem_type,
                                len,
                                zone_id,
                            },
                            None => StructFieldKind::Table { elem_type, len },
                        },
                        offset_bytes: offset,
                        size_bytes: storage.size,
                        align_bytes: storage.align,
                    });
                    offset = offset.checked_add(storage.size).ok_or_else(|| {
                        CraneliftBackendError::unsupported_module_shape(
                            "Cranelift dsp* layout size overflow",
                        )
                    })?;
                    struct_align = struct_align.max(storage.align);
                }
                scalar_ty => {
                    let scalar = fir_type_layout_scalar(ptr_size, &scalar_ty, double)?;
                    offset = align_up(offset, scalar.align);
                    fields.push(StructFieldLayout {
                        name,
                        kind: StructFieldKind::Scalar(scalar_ty),
                        offset_bytes: offset,
                        size_bytes: scalar.size,
                        align_bytes: scalar.align,
                    });
                    offset = offset.checked_add(scalar.size).ok_or_else(|| {
                        CraneliftBackendError::unsupported_module_shape(
                            "Cranelift dsp* layout size overflow",
                        )
                    })?;
                    struct_align = struct_align.max(scalar.align);
                }
            },
            FirMatch::DeclareVar {
                access: AccessType::Global,
                ..
            } => {
                // File-scope extern/global scalar symbols are not part of the
                // per-instance `dsp*` layout. They are resolved through
                // `CraneliftOptions::extern_data_symbols`.
            }
            FirMatch::DeclareVar { access, name, .. } => {
                return Err(CraneliftBackendError::unsupported_module_shape(format!(
                    "unsupported global variable access class for Cranelift dsp* layout: {name} ({access:?})"
                )));
            }
            FirMatch::DeclareTable {
                name,
                access: AccessType::Struct,
                elem_type,
                values,
            } => {
                let scalar = fir_type_layout_scalar(ptr_size, &elem_type, double)?;
                let len = u32::try_from(values.len()).map_err(|_| {
                    CraneliftBackendError::unsupported_module_shape(
                        "Cranelift dsp* table length does not fit in u32",
                    )
                })?;
                let payload_size = scalar.size.checked_mul(len).ok_or_else(|| {
                    CraneliftBackendError::unsupported_module_shape(
                        "Cranelift dsp* table size overflow",
                    )
                })?;
                let external_zone = external_zones.get(name.as_str()).copied();
                let storage = if external_zone.is_some() {
                    LayoutScalar {
                        size: ptr_size,
                        align: ptr_size,
                    }
                } else {
                    LayoutScalar {
                        size: payload_size,
                        align: scalar.align,
                    }
                };
                offset = align_up(offset, storage.align);
                fields.push(StructFieldLayout {
                    name,
                    kind: match external_zone {
                        Some(zone_id) => StructFieldKind::ExternalTable {
                            elem_type,
                            len,
                            zone_id,
                        },
                        None => StructFieldKind::Table { elem_type, len },
                    },
                    offset_bytes: offset,
                    size_bytes: storage.size,
                    align_bytes: storage.align,
                });
                offset = offset.checked_add(storage.size).ok_or_else(|| {
                    CraneliftBackendError::unsupported_module_shape(
                        "Cranelift dsp* layout size overflow",
                    )
                })?;
                struct_align = struct_align.max(storage.align);
            }
            FirMatch::DeclareTable {
                access: AccessType::Static | AccessType::Global,
                ..
            } => {
                // File-scope constant tables — not part of the per-instance dsp* struct.
                // They are handled separately by `define_static_tables_in_jit`.
            }
            FirMatch::DeclareTable { access, name, .. } => {
                return Err(CraneliftBackendError::unsupported_module_shape(format!(
                    "unsupported global table access class for Cranelift dsp* layout: {name} ({access:?})"
                )));
            }
            // Fast-lane FIR may place helper math prototypes (`fmin`, `pow`, ...)
            // in module globals. They are declarations, not `dsp*` state fields.
            FirMatch::DeclareFun { body: None, .. } => {}
            other => {
                return Err(CraneliftBackendError::unsupported_module_shape(format!(
                    "unsupported globals entry for Cranelift dsp* layout: {other:?}"
                )));
            }
        }
    }
    let size_bytes = if mem0.is_some() {
        align_up(offset, struct_align).max(1)
    } else {
        align_up(offset, struct_align)
    };
    Ok(StructLayoutPlan {
        fields,
        size_bytes,
        align_bytes: struct_align,
    })
}

/// Creates and configures a Cranelift JIT builder for the host machine.
///
/// # Current policy
/// - Uses the host ISA via `cranelift_native`.
/// - Applies backend options such as optimization level.
/// - Sets `is_pic=false` because the JIT allocates executable memory at
///   absolute addresses; position-independent code is unnecessary and adds
///   GOT-indirection overhead for a short-lived in-process JIT.
/// - Sets `use_colocated_libcalls=false` to use the default ABI-compatible
///   libcall symbols (`__udivti3`, etc.) rather than colocated stubs, which
///   simplifies early cross-platform bring-up.
/// - Registers default libcall names (Cranelift helper convention).
///
/// Host math symbols used by FIR math lowering are registered later by
/// [`register_host_symbols`].
pub(crate) fn make_jit_builder(
    options: &CraneliftOptions,
) -> Result<JITBuilder, CraneliftBackendError> {
    let mut builder = settings::builder();
    let opt_level = match options.opt_level {
        CraneliftOptLevel::None => "none",
        CraneliftOptLevel::Speed => "speed",
        CraneliftOptLevel::SpeedAndSize => "speed_and_size",
    };
    builder.set("opt_level", opt_level).map_err(|e| {
        CraneliftBackendError::jit_failure(format!("invalid Cranelift opt_level: {e}"))
    })?;
    builder.set("is_pic", "false").map_err(|e| {
        CraneliftBackendError::jit_failure(format!("invalid Cranelift is_pic flag: {e}"))
    })?;
    builder
        .set("use_colocated_libcalls", "false")
        .map_err(|e| {
            CraneliftBackendError::jit_failure(format!(
                "invalid Cranelift use_colocated_libcalls flag: {e}"
            ))
        })?;
    if options.enable_nan_canonicalization {
        let _ = builder.set("enable_nan_canonicalization", "true");
    }
    let isa_builder = cranelift_native::builder().map_err(|msg| {
        CraneliftBackendError::jit_failure(format!("host machine is not supported: {msg}"))
    })?;
    let isa = isa_builder
        .finish(settings::Flags::new(builder))
        .map_err(|e| CraneliftBackendError::jit_failure(format!("native ISA init failed: {e}")))?;
    Ok(JITBuilder::with_isa(isa, default_libcall_names()))
}
