//! Custom memory-manager mode and canonical allocation analysis shared by the
//! C, C++, and Cranelift backends.
//!
//! # Source provenance
//!
//! The option mapping follows Faust C++ `compiler/global.cpp`, where `-mem`,
//! `-mem0`, `--memory-manager`, and `--memory-manager0` all select
//! `gMemoryManager = 0`. Unlike that process-global integer, faust-rs passes a
//! typed value explicitly through each compilation request and backend option.
//!
//! The layout model ports the intent of C++
//! `CodeContainer::createMemoryLayout`, `StructInstVisitor`, and
//! `ArrayToPointer`. It is an `adapted` representation: field identity, type,
//! allocation phase, and access counts stay co-located in [`MemoryZone`], and
//! all byte arithmetic is checked. This removes the reference implementation's
//! index side tables, `int` accumulation, object-size heuristic, and
//! static-table `size == 0` sentinel.
//!
//! Only mode zero is in the approved Rust port; future C++ modes are
//! deliberately absent so they cannot be accepted and silently lowered as
//! [`MemoryManagerMode::Mem0`].

use std::collections::{BTreeMap, BTreeSet};

use fir::{AccessType, FirId, FirMatch, FirStore, FirType, fir_match_children, match_fir};

use crate::compute_cost::{ComputeCost, ComputeCostError, analyze_compute_cost};

/// Native backend custom-memory allocation strategy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryManagerMode {
    /// Preserve the backend's ordinary embedded/owned state layout.
    #[default]
    None,
    /// Externalize eligible DSP arrays and runtime-generated tables through
    /// the host memory-manager contract.
    Mem0,
}

impl MemoryManagerMode {
    /// Canonical Faust option spelling recorded in generated metadata and
    /// factory identities.
    #[must_use]
    pub const fn option_spelling(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Mem0 => Some("-mem0"),
        }
    }

    /// Whether custom allocation analysis and emission are enabled.
    #[must_use]
    pub const fn is_mem0(self) -> bool {
        matches!(self, Self::Mem0)
    }
}

/// Version of the explicit, non-sentinel memory-layout schema.
pub const MEMORY_LAYOUT_VERSION: u32 = 2;

/// Static access-count interpretation used by all three native backends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessMetric {
    /// Syntactic accesses in one occurrence of the scalar sample-loop body;
    /// loop trip counts and vector lanes are not multiplied in.
    StaticAccessesPerScalarFrame,
}

impl AccessMetric {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StaticAccessesPerScalarFrame => "static_accesses_per_scalar_frame",
        }
    }
}

/// Provenance of a target-layout number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutValueSource {
    /// Derived exactly from an explicit target ABI model.
    Computed,
    /// The generated language uses `sizeof`/`alignof`; the compiler-side JSON
    /// number is a non-authoritative companion estimate.
    CompilerExpression,
    /// Best available estimate, explicitly not exact.
    Estimated,
}

/// Size and alignment of one target value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeLayout {
    pub size: u64,
    pub alignment: u64,
}

impl TypeLayout {
    #[must_use]
    pub const fn new(size: u64, alignment: u64) -> Self {
        Self { size, alignment }
    }
}

/// Explicit ABI facts used instead of assuming the Rust compiler host layout.
///
/// Cranelift callers must populate this from the effective native ISA. The
/// generated C/C++ paths may use [`Self::native`] for JSON estimates while
/// emitting `sizeof`/`alignof` expressions as the runtime authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetAbi {
    pub target: String,
    pub pointer: TypeLayout,
    pub int32: TypeLayout,
    pub int64: TypeLayout,
    pub float32: TypeLayout,
    pub float64: TypeLayout,
    pub bool_: TypeLayout,
    pub quad: Option<TypeLayout>,
    pub fixed_point: Option<TypeLayout>,
    pub maximum_allocation_alignment: u64,
    pub source: LayoutValueSource,
}

impl TargetAbi {
    /// Builds the effective native ABI used by source backends by default.
    /// Every primitive fact comes from Rust's current compilation target; a
    /// cross-target caller must construct an explicit value instead.
    #[must_use]
    pub fn native() -> Self {
        let pointer = TypeLayout::new(
            std::mem::size_of::<*const ()>() as u64,
            std::mem::align_of::<*const ()>() as u64,
        );
        Self {
            target: "native".to_owned(),
            pointer,
            int32: TypeLayout::new(
                std::mem::size_of::<i32>() as u64,
                std::mem::align_of::<i32>() as u64,
            ),
            int64: TypeLayout::new(
                std::mem::size_of::<i64>() as u64,
                std::mem::align_of::<i64>() as u64,
            ),
            float32: TypeLayout::new(
                std::mem::size_of::<f32>() as u64,
                std::mem::align_of::<f32>() as u64,
            ),
            float64: TypeLayout::new(
                std::mem::size_of::<f64>() as u64,
                std::mem::align_of::<f64>() as u64,
            ),
            bool_: TypeLayout::new(
                std::mem::size_of::<bool>() as u64,
                std::mem::align_of::<bool>() as u64,
            ),
            quad: None,
            fixed_point: None,
            maximum_allocation_alignment: pointer.alignment.max(16),
            source: LayoutValueSource::Computed,
        }
    }

    fn validate(&self) -> Result<(), MemoryLayoutError> {
        for (name, layout) in [
            ("pointer", self.pointer),
            ("int32", self.int32),
            ("int64", self.int64),
            ("float32", self.float32),
            ("float64", self.float64),
            ("bool", self.bool_),
        ] {
            if layout.size == 0 || !layout.alignment.is_power_of_two() {
                return Err(MemoryLayoutError::InvalidTargetAbi(format!(
                    "{name} has size {} and alignment {}",
                    layout.size, layout.alignment
                )));
            }
            if layout.alignment > self.maximum_allocation_alignment {
                return Err(MemoryLayoutError::UnsupportedAlignment {
                    requested: layout.alignment,
                    maximum: self.maximum_allocation_alignment,
                });
            }
        }
        Ok(())
    }
}

/// Effective sample representation for `FirType::FaustFloat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleType {
    Float32,
    Float64,
}

/// Backend object model needed to calculate the manager-visible main block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryLayoutFlavor {
    C,
    Cpp,
    Cranelift,
}

/// Inputs that make a memory analysis specific to one effective backend FIR
/// snapshot without introducing mutable global compiler state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mem0AnalysisOptions {
    pub target_abi: TargetAbi,
    pub sample_type: SampleType,
    pub flavor: MemoryLayoutFlavor,
}

impl Mem0AnalysisOptions {
    #[must_use]
    pub fn native(flavor: MemoryLayoutFlavor, double_precision: bool) -> Self {
        let mut target_abi = TargetAbi::native();
        if flavor == MemoryLayoutFlavor::C {
            // The current C emitter spells FirType::Bool as `int`, while C++
            // and Cranelift use a one-byte boolean representation.
            target_abi.bool_ = target_abi.int32;
        }
        Self {
            target_abi,
            sample_type: if double_precision {
                SampleType::Float64
            } else {
                SampleType::Float32
            },
            flavor,
        }
    }
}

/// Complete, deterministic memory description for one effective backend FIR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryLayout {
    pub version: u32,
    pub mode: MemoryManagerMode,
    pub target_abi: TargetAbi,
    pub access_metric: AccessMetric,
    pub zones: Vec<MemoryZone>,
}

/// Stable identifier of a zone inside [`MemoryLayout::zones`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryZoneId(pub u32);

/// Manager vocabulary. Legacy Faust C++ spellings retain their semantic
/// ordering; `Int64` and `Bool` are the append-only D5 extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryType {
    Int32,
    Int32Ptr,
    Float32,
    Float32Ptr,
    Float64,
    Float64Ptr,
    Quad,
    QuadPtr,
    FixedPoint,
    FixedPointPtr,
    Object,
    ObjectPtr,
    Sound,
    SoundPtr,
    Int64,
    Int64Ptr,
    Bool,
    BoolPtr,
}

impl MemoryType {
    #[must_use]
    pub const fn legacy_name(self) -> &'static str {
        match self {
            Self::Int32 => "kInt32",
            Self::Int32Ptr => "kInt32_ptr",
            Self::Float32 => "kFloat",
            Self::Float32Ptr => "kFloat_ptr",
            Self::Float64 => "kDouble",
            Self::Float64Ptr => "kDouble_ptr",
            Self::Quad => "kQuad",
            Self::QuadPtr => "kQuad_ptr",
            Self::FixedPoint => "kFixedPoint",
            Self::FixedPointPtr => "kFixedPoint_ptr",
            Self::Object => "kObj",
            Self::ObjectPtr => "kObj_ptr",
            Self::Sound => "kSound",
            Self::SoundPtr => "kSound_ptr",
            Self::Int64 => "kInt64",
            Self::Int64Ptr => "kInt64_ptr",
            Self::Bool => "kBool",
            Self::BoolPtr => "kBool_ptr",
        }
    }

    /// Enumerator in the versioned plain-C `faust_memory_manager` ABI.
    ///
    /// This is deliberately separate from [`Self::legacy_name`]: the C++
    /// architecture contract retains its historical spellings, while C and
    /// Cranelift share the guarded header from `ffi-common`.
    #[must_use]
    pub const fn c_abi_name(self) -> &'static str {
        match self {
            Self::Int32 => "kMemInt32",
            Self::Int32Ptr => "kMemInt32Ptr",
            Self::Float32 => "kMemFloat32",
            Self::Float32Ptr => "kMemFloat32Ptr",
            Self::Float64 => "kMemFloat64",
            Self::Float64Ptr => "kMemFloat64Ptr",
            Self::Quad => "kMemQuad",
            Self::QuadPtr => "kMemQuadPtr",
            Self::FixedPoint => "kMemFixedPoint",
            Self::FixedPointPtr => "kMemFixedPointPtr",
            Self::Object => "kMemObject",
            Self::ObjectPtr => "kMemObjectPtr",
            Self::Sound => "kMemSound",
            Self::SoundPtr => "kMemSoundPtr",
            Self::Int64 => "kMemInt64",
            Self::Int64Ptr => "kMemInt64Ptr",
            Self::Bool => "kMemBool",
            Self::BoolPtr => "kMemBoolPtr",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryScope {
    Temporary,
    Class,
    Instance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryRole {
    Subcontainer,
    StaticTable,
    DspObject,
    InstanceBuffer,
    EmbeddedScalar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocationPhase {
    DescribeOnly,
    ClassCreate,
    ClassInit,
    CreateObject,
    InstanceCreate,
}

/// One described field/allocation. The allocation identity is co-located with
/// the FIR name and physical properties so emitters cannot silently rebuild a
/// divergent side table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryZone {
    pub id: MemoryZoneId,
    pub name: String,
    pub memory_type: MemoryType,
    pub scope: MemoryScope,
    pub role: MemoryRole,
    pub element_count: u64,
    pub element_size: u64,
    pub size_bytes: u64,
    pub alignment: u64,
    pub size_exact: bool,
    pub size_source: LayoutValueSource,
    pub runtime_allocated: bool,
    pub allocation_phase: AllocationPhase,
    pub allocation_order: u32,
    pub reads: u64,
    pub writes: u64,
}

/// Immutable analysis snapshot consumed by layout lowering, source emission,
/// description callbacks, and JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mem0Analysis {
    pub memory_layout: MemoryLayout,
    pub compute_cost: ComputeCost,
}

/// Typed failure for an unsafe or unrepresentable `mem0` analysis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryLayoutError {
    InvalidModuleRoot { node: u32, kind: String },
    InvalidSection { name: &'static str, node: u32 },
    InvalidTargetAbi(String),
    UnsupportedType(String),
    UnsupportedFirNode { node: u32, kind: String },
    UnsupportedAlignment { requested: u64, maximum: u64 },
    Overflow(&'static str),
    TooManyZones,
    EffectiveFir(String),
    ComputeCost(ComputeCostError),
}

impl std::fmt::Display for MemoryLayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidModuleRoot { node, kind } => {
                write!(f, "mem0 expected a FIR module at node {node}, got {kind}")
            }
            Self::InvalidSection { name, node } => {
                write!(f, "mem0 FIR section '{name}' at node {node} is not a block")
            }
            Self::InvalidTargetAbi(reason) => write!(f, "invalid target ABI: {reason}"),
            Self::UnsupportedType(typ) => write!(f, "unsupported mem0 FIR type: {typ}"),
            Self::UnsupportedFirNode { node, kind } => {
                write!(f, "unsupported FIR node in mem0 analysis {node}: {kind}")
            }
            Self::UnsupportedAlignment { requested, maximum } => write!(
                f,
                "mem0 requests alignment {requested}, target manager limit is {maximum}"
            ),
            Self::Overflow(field) => write!(f, "mem0 checked arithmetic overflow: {field}"),
            Self::TooManyZones => f.write_str("mem0 zone count exceeds u32 allocation identity"),
            Self::EffectiveFir(reason) => {
                write!(
                    f,
                    "cannot build the effective backend FIR for mem0: {reason}"
                )
            }
            Self::ComputeCost(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for MemoryLayoutError {}

impl From<ComputeCostError> for MemoryLayoutError {
    fn from(value: ComputeCostError) -> Self {
        Self::ComputeCost(value)
    }
}

/// Selects the effective backend FIR snapshot, then builds [`Mem0Analysis`].
///
/// C and C++ consume their nested-container module directly. Cranelift first
/// applies the same `MergedStructFields` submodule flattening used by
/// `generate_cranelift_module`; this boundary prevents JSON/layout analysis
/// from describing a different program than the JIT. The temporary flattened
/// arena is owned for the duration of analysis and no [`FirId`] escapes it.
pub fn analyze_effective_mem0(
    store: &FirStore,
    module: FirId,
    options: &Mem0AnalysisOptions,
) -> Result<Mem0Analysis, MemoryLayoutError> {
    if options.flavor == MemoryLayoutFlavor::Cranelift
        && fir::subcontainer::has_sub_modules(store, module)
    {
        let (effective_store, effective_module) = fir::subcontainer::flatten_sub_modules_owned(
            store,
            module,
            fir::subcontainer::SubModuleStatePolicy::MergedStructFields,
        )
        .map_err(|error| MemoryLayoutError::EffectiveFir(error.to_string()))?;
        analyze_mem0(&effective_store, effective_module, options)
    } else {
        analyze_mem0(store, module, options)
    }
}

/// Builds the one canonical `mem0` snapshot for a selected effective FIR
/// module. Backends retain this value instead of repeating classification or
/// access analysis.
pub fn analyze_mem0(
    store: &FirStore,
    module: FirId,
    options: &Mem0AnalysisOptions,
) -> Result<Mem0Analysis, MemoryLayoutError> {
    options.target_abi.validate()?;
    let FirMatch::Module {
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
        ..
    } = match_fir(store, module)
    else {
        return Err(MemoryLayoutError::InvalidModuleRoot {
            node: module.as_u32(),
            kind: format!("{:?}", match_fir(store, module)),
        });
    };

    let compute_cost = analyze_compute_cost(store, functions)?;
    let compute_body = find_compute_body(store, functions)?;
    let accesses = analyze_field_accesses(store, compute_body)?;
    let mut builder = LayoutBuilder::new(options, accesses);
    builder.collect_module(store, &name, dsp_struct, globals, static_decls, sub_modules)?;
    Ok(Mem0Analysis {
        memory_layout: MemoryLayout {
            version: MEMORY_LAYOUT_VERSION,
            mode: MemoryManagerMode::Mem0,
            target_abi: options.target_abi.clone(),
            access_metric: AccessMetric::StaticAccessesPerScalarFrame,
            zones: builder.zones,
        },
        compute_cost,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AccessCount {
    reads: u64,
    writes: u64,
}

fn find_compute_body(store: &FirStore, functions: FirId) -> Result<FirId, MemoryLayoutError> {
    let items = block_items(store, functions, "functions")?;
    items
        .into_iter()
        .find_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } if name == "compute" => Some(body),
            _ => None,
        })
        .ok_or(MemoryLayoutError::ComputeCost(
            ComputeCostError::MissingCompute,
        ))
}

fn analyze_field_accesses(
    store: &FirStore,
    root: FirId,
) -> Result<BTreeMap<String, AccessCount>, MemoryLayoutError> {
    fn visit(
        store: &FirStore,
        id: FirId,
        counts: &mut BTreeMap<String, AccessCount>,
    ) -> Result<(), MemoryLayoutError> {
        match match_fir(store, id) {
            FirMatch::Unknown => {
                return Err(MemoryLayoutError::UnsupportedFirNode {
                    node: id.as_u32(),
                    kind: "Unknown".to_owned(),
                });
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct | AccessType::Static,
                ..
            }
            | FirMatch::LoadTable {
                name,
                access: AccessType::Struct | AccessType::Static,
                ..
            } => {
                let entry = counts.entry(name).or_default();
                entry.reads = entry
                    .reads
                    .checked_add(1)
                    .ok_or(MemoryLayoutError::Overflow("field reads"))?;
            }
            FirMatch::StoreVar {
                name,
                access: AccessType::Struct | AccessType::Static,
                ..
            }
            | FirMatch::StoreTable {
                name,
                access: AccessType::Struct | AccessType::Static,
                ..
            }
            | FirMatch::TeeVar {
                name,
                access: AccessType::Struct | AccessType::Static,
                ..
            } => {
                let entry = counts.entry(name).or_default();
                entry.writes = entry
                    .writes
                    .checked_add(1)
                    .ok_or(MemoryLayoutError::Overflow("field writes"))?;
            }
            _ => {}
        }
        for child in fir_match_children(store, id) {
            visit(store, child, counts)?;
        }
        Ok(())
    }

    let mut counts = BTreeMap::new();
    visit(store, root, &mut counts)?;
    Ok(counts)
}

struct LayoutBuilder<'a> {
    options: &'a Mem0AnalysisOptions,
    accesses: BTreeMap<String, AccessCount>,
    zones: Vec<MemoryZone>,
}

impl<'a> LayoutBuilder<'a> {
    fn new(options: &'a Mem0AnalysisOptions, accesses: BTreeMap<String, AccessCount>) -> Self {
        Self {
            options,
            accesses,
            zones: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_module(
        &mut self,
        store: &FirStore,
        module_name: &str,
        dsp_struct: FirId,
        globals: FirId,
        static_decls: FirId,
        sub_modules: FirId,
    ) -> Result<(), MemoryLayoutError> {
        let state_items = [
            block_items(store, dsp_struct, "dsp_struct")?,
            block_items(store, globals, "globals")?,
        ]
        .concat();
        let static_items = block_items(store, static_decls, "static_decls")?;
        let sub_items = block_items(store, sub_modules, "sub_modules")?;

        let runtime_static_names: BTreeSet<String> = static_items
            .iter()
            .filter_map(|id| match match_fir(store, *id) {
                FirMatch::DeclareVar {
                    name,
                    typ: FirType::Array(_, _),
                    access: AccessType::Static,
                    init: None,
                } => Some(name),
                _ => None,
            })
            .collect();

        let mut class_submodules = Vec::new();
        let mut instance_submodules = Vec::new();
        for sub in sub_items {
            let FirMatch::SubModule { ref name, .. } = match_fir(store, sub) else {
                return Err(MemoryLayoutError::UnsupportedFirNode {
                    node: sub.as_u32(),
                    kind: format!("{:?}", match_fir(store, sub)),
                });
            };
            if runtime_static_names
                .iter()
                .any(|table| table.ends_with(name))
            {
                class_submodules.push(sub);
            } else {
                instance_submodules.push(sub);
            }
        }

        // Preserve the reference's semantic phase order: class table helpers
        // and tables, main object/state, then instance-init helpers.
        for sub in &class_submodules {
            self.push_submodule(store, *sub, MemoryScope::Class)?;
        }
        for item in static_items {
            self.push_static_declaration(store, item)?;
        }

        let (object_size, object_alignment, object_exact, object_source) =
            self.object_layout(store, &state_items)?;
        let scalar_accesses = state_items.iter().try_fold(
            AccessCount::default(),
            |mut total, item| -> Result<_, MemoryLayoutError> {
                let (name, is_scalar) = match match_fir(store, *item) {
                    FirMatch::DeclareVar { name, typ, .. } => (
                        name,
                        !matches!(typ, FirType::Array(_, _) | FirType::Vector(_, _)),
                    ),
                    FirMatch::DeclareTable { name, .. } => (name, false),
                    _ => return Ok(total),
                };
                if is_scalar {
                    let count = self.accesses.get(&name).copied().unwrap_or_default();
                    total.reads = total
                        .reads
                        .checked_add(count.reads)
                        .ok_or(MemoryLayoutError::Overflow("DSP object reads"))?;
                    total.writes = total
                        .writes
                        .checked_add(count.writes)
                        .ok_or(MemoryLayoutError::Overflow("DSP object writes"))?;
                }
                Ok(total)
            },
        )?;
        self.push_zone(ZoneSpec {
            name: module_name.to_owned(),
            memory_type: MemoryType::ObjectPtr,
            scope: MemoryScope::Instance,
            role: MemoryRole::DspObject,
            element_count: 1,
            element_size: object_size,
            size_bytes: object_size.max(1),
            alignment: object_alignment,
            size_exact: object_exact,
            size_source: object_source,
            runtime_allocated: true,
            allocation_phase: AllocationPhase::CreateObject,
            reads: scalar_accesses.reads,
            writes: scalar_accesses.writes,
        })?;

        for item in state_items {
            self.push_state_declaration(store, item)?;
        }
        for sub in instance_submodules {
            self.push_submodule(store, sub, MemoryScope::Temporary)?;
        }
        Ok(())
    }

    fn push_submodule(
        &mut self,
        store: &FirStore,
        sub: FirId,
        scope: MemoryScope,
    ) -> Result<(), MemoryLayoutError> {
        let FirMatch::SubModule {
            name,
            dsp_struct,
            globals,
            sub_modules,
            ..
        } = match_fir(store, sub)
        else {
            return Err(MemoryLayoutError::UnsupportedFirNode {
                node: sub.as_u32(),
                kind: format!("{:?}", match_fir(store, sub)),
            });
        };
        for nested in block_items(store, sub_modules, "sub_modules")? {
            self.push_submodule(store, nested, scope)?;
        }
        let state_items = [
            block_items(store, dsp_struct, "submodule dsp_struct")?,
            block_items(store, globals, "submodule globals")?,
        ]
        .concat();
        let (size, alignment, exact, source) = self.object_layout(store, &state_items)?;
        self.push_zone(ZoneSpec {
            name,
            memory_type: MemoryType::ObjectPtr,
            scope,
            role: MemoryRole::Subcontainer,
            element_count: 1,
            element_size: size,
            size_bytes: size.max(1),
            alignment,
            size_exact: exact,
            size_source: source,
            runtime_allocated: true,
            allocation_phase: if scope == MemoryScope::Class {
                self.class_allocation_phase()
            } else {
                AllocationPhase::InstanceCreate
            },
            reads: 0,
            writes: 0,
        })
    }

    fn push_static_declaration(
        &mut self,
        store: &FirStore,
        item: FirId,
    ) -> Result<(), MemoryLayoutError> {
        match match_fir(store, item) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(elem, count),
                access: AccessType::Static,
                init,
            } => {
                let runtime = init.is_none() && count > 0;
                self.push_array_zone(
                    name,
                    &elem,
                    count,
                    MemoryScope::Class,
                    MemoryRole::StaticTable,
                    runtime,
                    if runtime {
                        self.class_allocation_phase()
                    } else {
                        AllocationPhase::DescribeOnly
                    },
                )
            }
            FirMatch::DeclareTable {
                name,
                access: AccessType::Static,
                elem_type,
                values,
            } => self.push_array_zone(
                name,
                &elem_type,
                values.len(),
                MemoryScope::Class,
                MemoryRole::StaticTable,
                false,
                AllocationPhase::DescribeOnly,
            ),
            FirMatch::NullStatement => Ok(()),
            other => Err(MemoryLayoutError::UnsupportedFirNode {
                node: item.as_u32(),
                kind: format!("{other:?}"),
            }),
        }
    }

    fn push_state_declaration(
        &mut self,
        store: &FirStore,
        item: FirId,
    ) -> Result<(), MemoryLayoutError> {
        match match_fir(store, item) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(elem, count),
                access: AccessType::Struct,
                ..
            } => self.push_array_zone(
                name,
                &elem,
                count,
                MemoryScope::Instance,
                MemoryRole::InstanceBuffer,
                count > 0,
                if count > 0 {
                    AllocationPhase::InstanceCreate
                } else {
                    AllocationPhase::DescribeOnly
                },
            ),
            FirMatch::DeclareTable {
                name,
                access: AccessType::Struct,
                elem_type,
                values,
            } => self.push_array_zone(
                name,
                &elem_type,
                values.len(),
                MemoryScope::Instance,
                MemoryRole::InstanceBuffer,
                !values.is_empty(),
                if values.is_empty() {
                    AllocationPhase::DescribeOnly
                } else {
                    AllocationPhase::InstanceCreate
                },
            ),
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Struct,
                ..
            } => {
                let layout = self.type_layout(&typ)?;
                let access = self.accesses.get(&name).copied().unwrap_or_default();
                self.push_zone(ZoneSpec {
                    name,
                    memory_type: self.memory_type(&typ, false)?,
                    scope: MemoryScope::Instance,
                    role: MemoryRole::EmbeddedScalar,
                    element_count: 1,
                    element_size: layout.size,
                    size_bytes: layout.size,
                    alignment: layout.alignment,
                    size_exact: true,
                    size_source: self.options.target_abi.source,
                    runtime_allocated: false,
                    allocation_phase: AllocationPhase::DescribeOnly,
                    reads: access.reads,
                    writes: access.writes,
                })
            }
            // Global prototypes and file-scope declarations are not DSP state.
            FirMatch::DeclareFun { .. }
            | FirMatch::DeclareStructType { .. }
            | FirMatch::NullStatement => Ok(()),
            other => Err(MemoryLayoutError::UnsupportedFirNode {
                node: item.as_u32(),
                kind: format!("{other:?}"),
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_array_zone(
        &mut self,
        name: String,
        elem: &FirType,
        count: usize,
        scope: MemoryScope,
        role: MemoryRole,
        runtime_allocated: bool,
        phase: AllocationPhase,
    ) -> Result<(), MemoryLayoutError> {
        let elem_layout = self.type_layout(elem)?;
        let count = u64::try_from(count).map_err(|_| MemoryLayoutError::Overflow("array count"))?;
        let bytes = elem_layout
            .size
            .checked_mul(count)
            .ok_or(MemoryLayoutError::Overflow("array size_bytes"))?;
        let access = self.accesses.get(&name).copied().unwrap_or_default();
        self.push_zone(ZoneSpec {
            name,
            memory_type: self.memory_type(elem, true)?,
            scope,
            role,
            element_count: count,
            element_size: elem_layout.size,
            size_bytes: bytes,
            alignment: elem_layout.alignment,
            size_exact: true,
            size_source: self.options.target_abi.source,
            runtime_allocated,
            allocation_phase: phase,
            reads: access.reads,
            writes: access.writes,
        })
    }

    fn object_layout(
        &self,
        store: &FirStore,
        declarations: &[FirId],
    ) -> Result<(u64, u64, bool, LayoutValueSource), MemoryLayoutError> {
        let pointer = self.options.target_abi.pointer;
        let (mut offset, mut alignment, exact, source) = match self.options.flavor {
            // Captured manager pointer is embedded in generated C state.
            MemoryLayoutFlavor::C => (
                pointer.size,
                pointer.alignment,
                true,
                self.options.target_abi.source,
            ),
            // Virtual dsp base/vptr plus captured manager. C++ object-model
            // padding is target-compiler authoritative, so the numeric JSON
            // companion is explicitly non-exact.
            MemoryLayoutFlavor::Cpp => (
                pointer
                    .size
                    .checked_mul(2)
                    .ok_or(MemoryLayoutError::Overflow("C++ object prefix"))?,
                pointer.alignment,
                false,
                LayoutValueSource::CompilerExpression,
            ),
            MemoryLayoutFlavor::Cranelift => {
                (0, pointer.alignment, true, self.options.target_abi.source)
            }
        };
        for declaration in declarations {
            let layout = match match_fir(store, *declaration) {
                FirMatch::DeclareVar {
                    typ: FirType::Array(_, _) | FirType::Vector(_, _),
                    access: AccessType::Struct,
                    ..
                }
                | FirMatch::DeclareTable {
                    access: AccessType::Struct,
                    ..
                } => pointer,
                FirMatch::DeclareVar {
                    typ,
                    access: AccessType::Struct,
                    ..
                } => self.type_layout(&typ)?,
                FirMatch::DeclareFun { .. }
                | FirMatch::DeclareStructType { .. }
                | FirMatch::NullStatement => continue,
                other => {
                    return Err(MemoryLayoutError::UnsupportedFirNode {
                        node: declaration.as_u32(),
                        kind: format!("{other:?}"),
                    });
                }
            };
            if layout.alignment > self.options.target_abi.maximum_allocation_alignment {
                return Err(MemoryLayoutError::UnsupportedAlignment {
                    requested: layout.alignment,
                    maximum: self.options.target_abi.maximum_allocation_alignment,
                });
            }
            offset = align_up(offset, layout.alignment)?;
            offset = offset
                .checked_add(layout.size)
                .ok_or(MemoryLayoutError::Overflow("object field offset"))?;
            alignment = alignment.max(layout.alignment);
        }
        let size = align_up(offset, alignment)?.max(1);
        Ok((size, alignment.max(1), exact, source))
    }

    fn type_layout(&self, typ: &FirType) -> Result<TypeLayout, MemoryLayoutError> {
        let abi = &self.options.target_abi;
        let layout = match typ {
            FirType::Int32 => abi.int32,
            FirType::Int64 => abi.int64,
            FirType::Float32 => abi.float32,
            FirType::Float64 => abi.float64,
            FirType::FaustFloat => match self.options.sample_type {
                SampleType::Float32 => abi.float32,
                SampleType::Float64 => abi.float64,
            },
            FirType::Bool => abi.bool_,
            FirType::Quad => abi
                .quad
                .ok_or_else(|| MemoryLayoutError::UnsupportedType("Quad".to_owned()))?,
            FirType::FixedPoint => abi
                .fixed_point
                .ok_or_else(|| MemoryLayoutError::UnsupportedType("FixedPoint".to_owned()))?,
            FirType::Obj
            | FirType::Sound
            | FirType::UI
            | FirType::Meta
            | FirType::Ptr(_)
            | FirType::Fun { .. } => abi.pointer,
            FirType::Array(elem, count) | FirType::Vector(elem, count) => {
                let elem = self.type_layout(elem)?;
                let count = u64::try_from(*count)
                    .map_err(|_| MemoryLayoutError::Overflow("aggregate count"))?;
                TypeLayout::new(
                    elem.size
                        .checked_mul(count)
                        .ok_or(MemoryLayoutError::Overflow("aggregate size"))?,
                    elem.alignment,
                )
            }
            FirType::Struct(_, fields) => {
                let mut size = 0;
                let mut alignment = 1;
                for field in fields {
                    let field = self.type_layout(field)?;
                    size = align_up(size, field.alignment)?;
                    size = size
                        .checked_add(field.size)
                        .ok_or(MemoryLayoutError::Overflow("struct size"))?;
                    alignment = alignment.max(field.alignment);
                }
                TypeLayout::new(align_up(size, alignment)?.max(1), alignment)
            }
            FirType::Void => {
                return Err(MemoryLayoutError::UnsupportedType("Void".to_owned()));
            }
        };
        if layout.alignment > abi.maximum_allocation_alignment {
            return Err(MemoryLayoutError::UnsupportedAlignment {
                requested: layout.alignment,
                maximum: abi.maximum_allocation_alignment,
            });
        }
        Ok(layout)
    }

    fn memory_type(&self, typ: &FirType, pointer: bool) -> Result<MemoryType, MemoryLayoutError> {
        let scalar = match typ {
            FirType::Int32 => (MemoryType::Int32, MemoryType::Int32Ptr),
            FirType::Int64 => (MemoryType::Int64, MemoryType::Int64Ptr),
            FirType::Float32 => (MemoryType::Float32, MemoryType::Float32Ptr),
            FirType::Float64 => (MemoryType::Float64, MemoryType::Float64Ptr),
            FirType::FaustFloat => match self.options.sample_type {
                SampleType::Float32 => (MemoryType::Float32, MemoryType::Float32Ptr),
                SampleType::Float64 => (MemoryType::Float64, MemoryType::Float64Ptr),
            },
            FirType::Quad => (MemoryType::Quad, MemoryType::QuadPtr),
            FirType::FixedPoint => (MemoryType::FixedPoint, MemoryType::FixedPointPtr),
            FirType::Bool => (MemoryType::Bool, MemoryType::BoolPtr),
            FirType::Sound => (MemoryType::Sound, MemoryType::SoundPtr),
            FirType::Obj
            | FirType::UI
            | FirType::Meta
            | FirType::Ptr(_)
            | FirType::Struct(_, _) => (MemoryType::Object, MemoryType::ObjectPtr),
            other => return Err(MemoryLayoutError::UnsupportedType(format!("{other:?}"))),
        };
        Ok(if pointer { scalar.1 } else { scalar.0 })
    }

    fn class_allocation_phase(&self) -> AllocationPhase {
        match self.options.flavor {
            MemoryLayoutFlavor::Cranelift => AllocationPhase::ClassCreate,
            MemoryLayoutFlavor::C | MemoryLayoutFlavor::Cpp => AllocationPhase::ClassInit,
        }
    }

    fn push_zone(&mut self, spec: ZoneSpec) -> Result<(), MemoryLayoutError> {
        if spec.runtime_allocated && spec.size_bytes == 0 {
            return Err(MemoryLayoutError::InvalidTargetAbi(format!(
                "runtime zone '{}' has zero allocation bytes",
                spec.name
            )));
        }
        let order = u32::try_from(self.zones.len()).map_err(|_| MemoryLayoutError::TooManyZones)?;
        self.zones.push(MemoryZone {
            id: MemoryZoneId(order),
            name: spec.name,
            memory_type: spec.memory_type,
            scope: spec.scope,
            role: spec.role,
            element_count: spec.element_count,
            element_size: spec.element_size,
            size_bytes: spec.size_bytes,
            alignment: spec.alignment,
            size_exact: spec.size_exact,
            size_source: spec.size_source,
            runtime_allocated: spec.runtime_allocated,
            allocation_phase: spec.allocation_phase,
            allocation_order: order,
            reads: spec.reads,
            writes: spec.writes,
        });
        Ok(())
    }
}

struct ZoneSpec {
    name: String,
    memory_type: MemoryType,
    scope: MemoryScope,
    role: MemoryRole,
    element_count: u64,
    element_size: u64,
    size_bytes: u64,
    alignment: u64,
    size_exact: bool,
    size_source: LayoutValueSource,
    runtime_allocated: bool,
    allocation_phase: AllocationPhase,
    reads: u64,
    writes: u64,
}

fn block_items(
    store: &FirStore,
    block: FirId,
    name: &'static str,
) -> Result<Vec<FirId>, MemoryLayoutError> {
    match match_fir(store, block) {
        FirMatch::Block(items) => Ok(items),
        _ => Err(MemoryLayoutError::InvalidSection {
            name,
            node: block.as_u32(),
        }),
    }
}

fn align_up(value: u64, alignment: u64) -> Result<u64, MemoryLayoutError> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(MemoryLayoutError::InvalidTargetAbi(format!(
            "invalid alignment {alignment}"
        )));
    }
    let mask = alignment - 1;
    value
        .checked_add(mask)
        .map(|value| value & !mask)
        .ok_or(MemoryLayoutError::Overflow("alignment rounding"))
}

#[cfg(test)]
mod tests {
    use fir::{AccessType, FirBuilder, FirType, NamedType};

    use super::*;

    #[test]
    fn default_is_ordinary_embedded_memory() {
        assert_eq!(MemoryManagerMode::default(), MemoryManagerMode::None);
        assert_eq!(MemoryManagerMode::None.option_spelling(), None);
        assert!(!MemoryManagerMode::None.is_mem0());
    }

    #[test]
    fn mem0_has_one_canonical_spelling() {
        assert_eq!(MemoryManagerMode::Mem0.option_spelling(), Some("-mem0"));
        assert!(MemoryManagerMode::Mem0.is_mem0());
    }

    fn build_layout_fixture() -> (FirStore, FirId) {
        let mut store = FirStore::new();
        let module = {
            let mut b = FirBuilder::new(&mut store);
            let zero = b.int32(0);
            let scalar = b.declare_var("fScalar", FirType::Int32, AccessType::Struct, Some(zero));
            let delay = b.declare_var(
                "fDelay",
                FirType::Array(Box::new(FirType::Float32), 8),
                AccessType::Struct,
                None,
            );
            let empty = b.declare_var(
                "fEmpty",
                FirType::Array(Box::new(FirType::Int64), 0),
                AccessType::Struct,
                None,
            );
            let flags = b.declare_var(
                "fFlags",
                FirType::Array(Box::new(FirType::Bool), 4),
                AccessType::Struct,
                None,
            );
            let dsp_struct = b.block(&[scalar, delay, empty, flags]);
            let globals = b.block(&[]);
            let static_decls = b.block(&[]);

            let index = b.int32(0);
            let scalar_load = b.load_var("fScalar", AccessType::Struct, FirType::Int32);
            let delay_load = b.load_table("fDelay", AccessType::Struct, index, FirType::Float32);
            let drop_scalar = b.drop_(scalar_load);
            let store_delay = b.store_table("fDelay", AccessType::Struct, index, delay_load);
            let body = b.block(&[drop_scalar, store_delay]);
            let compute_type = FirType::Fun {
                args: vec![
                    FirType::Int32,
                    FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                    FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                ],
                ret: Box::new(FirType::Void),
            };
            let args = [
                NamedType {
                    name: "count".to_owned(),
                    typ: FirType::Int32,
                },
                NamedType {
                    name: "inputs".to_owned(),
                    typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                },
                NamedType {
                    name: "outputs".to_owned(),
                    typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                },
            ];
            let compute = b.declare_fun("compute", compute_type, &args, Some(body), false);
            let functions = b.block(&[compute]);
            b.module(
                0,
                0,
                "mydsp",
                dsp_struct,
                globals,
                functions,
                static_decls,
                &[],
            )
        };
        (store, module)
    }

    #[test]
    fn layout_externalizes_each_array_once_and_co_locates_zone_identity() {
        let (store, module) = build_layout_fixture();
        let analysis = analyze_mem0(
            &store,
            module,
            &Mem0AnalysisOptions::native(MemoryLayoutFlavor::Cranelift, false),
        )
        .unwrap();
        let zones = &analysis.memory_layout.zones;
        assert_eq!(zones.len(), 5);
        assert!(zones.iter().enumerate().all(|(index, zone)| {
            zone.id == MemoryZoneId(index as u32) && zone.allocation_order == index as u32
        }));

        let object = zones
            .iter()
            .find(|zone| zone.role == MemoryRole::DspObject)
            .unwrap();
        assert_eq!((object.reads, object.writes), (1, 0));
        assert!(object.runtime_allocated);

        let delay = zones.iter().find(|zone| zone.name == "fDelay").unwrap();
        assert_eq!(delay.memory_type, MemoryType::Float32Ptr);
        assert_eq!((delay.element_count, delay.size_bytes), (8, 32));
        assert_eq!((delay.reads, delay.writes), (1, 1));

        let empty = zones.iter().find(|zone| zone.name == "fEmpty").unwrap();
        assert_eq!(empty.memory_type, MemoryType::Int64Ptr);
        assert_eq!((empty.element_count, empty.size_bytes), (0, 0));
        assert!(!empty.runtime_allocated);

        let flags = zones.iter().find(|zone| zone.name == "fFlags").unwrap();
        assert_eq!(flags.memory_type, MemoryType::BoolPtr);
    }

    #[test]
    fn empty_cranelift_object_requests_one_byte_at_pointer_alignment() {
        let mut store = FirStore::new();
        let module = {
            let mut b = FirBuilder::new(&mut store);
            let empty = b.block(&[]);
            let body = b.block(&[]);
            let compute = b.declare_fun(
                "compute",
                FirType::Fun {
                    args: Vec::new(),
                    ret: Box::new(FirType::Void),
                },
                &[],
                Some(body),
                false,
            );
            let functions = b.block(&[compute]);
            b.module(0, 0, "empty", empty, empty, functions, empty, &[])
        };
        let options = Mem0AnalysisOptions::native(MemoryLayoutFlavor::Cranelift, false);
        let pointer_alignment = options.target_abi.pointer.alignment;
        let analysis = analyze_mem0(&store, module, &options).unwrap();
        let object = &analysis.memory_layout.zones[0];
        assert_eq!(object.size_bytes, 1);
        assert_eq!(object.alignment, pointer_alignment);
    }

    #[test]
    fn cpp_object_estimate_is_explicitly_non_exact() {
        let (store, module) = build_layout_fixture();
        let analysis = analyze_mem0(
            &store,
            module,
            &Mem0AnalysisOptions::native(MemoryLayoutFlavor::Cpp, false),
        )
        .unwrap();
        let object = &analysis.memory_layout.zones[0];
        assert!(!object.size_exact);
        assert_eq!(object.size_source, LayoutValueSource::CompilerExpression);
    }

    #[test]
    fn rejects_target_over_alignment_before_emission() {
        let (store, module) = build_layout_fixture();
        let mut options = Mem0AnalysisOptions::native(MemoryLayoutFlavor::C, false);
        options.target_abi.maximum_allocation_alignment = 2;
        let error = analyze_mem0(&store, module, &options).unwrap_err();
        assert!(matches!(
            error,
            MemoryLayoutError::UnsupportedAlignment { .. }
        ));
    }

    #[test]
    fn canonical_codegen_fixture_is_fully_analyzable() {
        let (store, module) = crate::fixtures::build_table_state_delay_test_module();
        let analysis = analyze_mem0(
            &store,
            module,
            &Mem0AnalysisOptions::native(MemoryLayoutFlavor::C, false),
        )
        .unwrap();
        assert!(analysis.compute_cost.loops > 0);
        assert!(
            analysis
                .memory_layout
                .zones
                .iter()
                .any(|zone| zone.name == "fDelay" && zone.runtime_allocated)
        );
    }

    #[test]
    fn scalar_codegen_fixture_family_is_fully_analyzable() {
        let fixtures = [
            crate::fixtures::build_sine_phasor_test_module,
            crate::fixtures::build_passthrough_test_module,
            crate::fixtures::build_gain_bias_ui_meta_test_module,
            crate::fixtures::build_control_flow_test_module,
            crate::fixtures::build_math_intrinsics_test_module,
            crate::fixtures::build_heavy_bench_test_module,
            crate::fixtures::build_ir_coverage_test_module,
        ];
        for fixture in fixtures {
            let (store, module) = fixture();
            analyze_mem0(
                &store,
                module,
                &Mem0AnalysisOptions::native(MemoryLayoutFlavor::Cpp, false),
            )
            .unwrap();
        }
    }

    #[test]
    fn generated_static_table_and_subcontainer_have_explicit_class_roles() {
        let mut store = FirStore::new();
        let module = {
            let mut b = FirBuilder::new(&mut store);
            let empty = b.block(&[]);
            let table = b.declare_var(
                "ftbl0mydspSIG0",
                FirType::Array(Box::new(FirType::Float32), 16),
                AccessType::Static,
                None,
            );
            let static_decls = b.block(&[table]);
            let sub = b.sub_module(
                "mydspSIG0",
                FirType::Float32,
                empty,
                empty,
                empty,
                empty,
                &[],
            );
            let index = b.int32(0);
            let load = b.load_table(
                "ftbl0mydspSIG0",
                AccessType::Static,
                index,
                FirType::Float32,
            );
            let drop_load = b.drop_(load);
            let body = b.block(&[drop_load]);
            let compute = b.declare_fun(
                "compute",
                FirType::Fun {
                    args: Vec::new(),
                    ret: Box::new(FirType::Void),
                },
                &[],
                Some(body),
                false,
            );
            let functions = b.block(&[compute]);
            b.module(0, 0, "mydsp", empty, empty, functions, static_decls, &[sub])
        };

        let analysis = analyze_mem0(
            &store,
            module,
            &Mem0AnalysisOptions::native(MemoryLayoutFlavor::Cranelift, false),
        )
        .unwrap();
        let helper = &analysis.memory_layout.zones[0];
        let table = &analysis.memory_layout.zones[1];
        assert_eq!(helper.role, MemoryRole::Subcontainer);
        assert_eq!(helper.scope, MemoryScope::Class);
        assert_eq!(helper.allocation_phase, AllocationPhase::ClassCreate);
        assert_eq!(table.role, MemoryRole::StaticTable);
        assert_eq!(table.element_count, 16);
        assert_eq!((table.reads, table.writes), (1, 0));
        assert!(table.runtime_allocated);
    }
}
