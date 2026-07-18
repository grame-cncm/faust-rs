//! WASM linear-memory layout descriptors.
//!
//! This module computes the linear-memory contract shared by:
//! - generated WASM code,
//! - companion JSON UI metadata,
//! - host runtimes calling `getParamValue` / `setParamValue`.
//!
//! The key invariant is that exported JSON offsets are not abstract widget IDs:
//! they are raw byte offsets into one contiguous runtime prefix. That prefix can
//! contain, in order:
//! - static tables,
//! - `Soundfile*` struct fields first among mutable DSP fields,
//! - mutable DSP/global fields,
//! - the I/O zone / audio heap start,
//! - the embedded JSON data segment after the runtime area.
//!
//! This matches the observable C++ WASM contract closely enough for web-facing
//! runtimes such as `faustwasm`, which read widget `index` values from JSON and
//! pass them back to exported parameter accessors.

use std::collections::BTreeMap;

use fir::{AccessType, FirId, FirMatch, FirStore, FirType, match_fir};

use super::{WasmBackendError, WasmBackendErrorCode, WasmOptions};

/// WebAssembly linear-memory page size in bytes.
const WASM_PAGE_BYTES: u32 = 65_536;
/// Scratch capacity reserved per I/O channel before the embedded JSON segment.
///
/// This mirrors the current C++-parity sizing rule used by the Rust backend so
/// host wrappers can place pointer tables and sample buffers after `json.size`.
const IO_BUFFER_SCRATCH_SAMPLES: u32 = 8_192;
/// Pointer width used by the MVP 32-bit WASM memory model.
const WASM_PTR_BYTES: u32 = 4;

/// Scalar value kinds used by the public WASM layout descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WasmValType {
    I32,
    F32,
    F64,
}

/// One field offset/size entry inside the linear-memory layout.
///
/// `offset` is always expressed relative to the start of the runtime prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldLayout {
    pub offset: u32,
    pub typ: WasmValType,
    pub size: u32,
}

/// Maps FIR struct globals to WASM linear-memory offsets.
///
/// Mapping status: `adapted`. The engine follows the current C++ WASM
/// alignment rule that every field slot is at least one audio-sample slot wide,
/// while still widening `f64` storage in single-precision mode so the Rust FIR
/// state remains representable without truncation.
///
/// C++ parity note:
/// - the public WASM ABI uses one contiguous runtime zone where static tables
///   can precede mutable DSP fields;
/// - exported UI `index` values and `get/setParamValue(dsp, index, ...)`
///   therefore refer to absolute byte offsets inside that runtime prefix, not
///   to a compact "mutable-struct-only" sub-layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmMemoryLayout {
    /// Byte offset for each struct/static-table field, keyed by FIR variable name.
    pub field_offsets: BTreeMap<String, FieldLayout>,
    /// Byte offset for stack-local array buffers reserved inside each DSP instance.
    pub local_offsets: BTreeMap<String, FieldLayout>,
    /// Total runtime prefix size in bytes before the I/O zone.
    pub struct_size: u32,
    /// Offset where static tables begin.
    pub tables_offset: u32,
    /// Offset where I/O zone begins.
    pub io_zone_offset: u32,
    /// Offset where the embedded JSON data segment is placed.
    pub json_offset: u32,
    /// Total memory required in bytes before page rounding.
    pub total_bytes: u32,
    /// WASM pages required by the C++-parity sizing rule.
    pub pages: u32,
}

impl WasmMemoryLayout {
    /// Creates the placeholder layout used by the earliest scaffold path.
    #[must_use]
    pub fn scaffold(pages: u32, total_bytes: u32) -> Self {
        Self {
            field_offsets: BTreeMap::new(),
            local_offsets: BTreeMap::new(),
            struct_size: 0,
            tables_offset: 0,
            io_zone_offset: 0,
            json_offset: 0,
            total_bytes,
            pages,
        }
    }

    /// Derives the current WASM linear-memory layout from a FIR module.
    ///
    /// Layout policy:
    /// - static tables are allocated first,
    /// - mutable struct/global fields follow,
    /// - `struct_size` therefore means "runtime prefix size" rather than
    ///   "mutable struct size only",
    /// - `io_zone_offset` is the first byte available to host-managed audio
    ///   buffers,
    /// - `json_offset` is placed after the runtime area so the embedded JSON
    ///   data segment cannot alias DSP state.
    pub fn from_module(
        store: &FirStore,
        module: FirId,
        options: &WasmOptions,
        json_len: usize,
    ) -> Result<Self, WasmBackendError> {
        let audio_slot = if options.double_precision { 8 } else { 4 };
        let FirMatch::Module {
            dsp_struct,
            globals,
            static_decls,
            functions,
            num_inputs,
            num_outputs,
            ..
        } = match_fir(store, module)
        else {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedModuleShape,
                "WASM layout expects a FIR Module root",
            ));
        };

        let dsp_struct_items = expect_block(store, dsp_struct, "module dsp_struct")?;
        let global_items = expect_block(store, globals, "module globals")?;
        let static_items = expect_block(store, static_decls, "module static_decls")?;

        let mut field_offsets = BTreeMap::new();
        let tables_offset = 0u32;
        let mut runtime_offset = tables_offset;

        for item in global_items
            .iter()
            .copied()
            .chain(static_items.iter().copied())
        {
            match match_fir(store, item) {
                FirMatch::DeclareTable {
                    name,
                    access: AccessType::Static,
                    elem_type,
                    values,
                } => {
                    let (val_type, elem_size) = fir_type_storage(elem_type, audio_slot)?;
                    let len = u32::try_from(values.len()).map_err(|_| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM static table length does not fit in u32",
                        )
                    })?;
                    let size = elem_size.checked_mul(len).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM static table byte size overflow",
                        )
                    })?;
                    let offset = align_up(runtime_offset, elem_size);
                    field_offsets.insert(
                        name,
                        FieldLayout {
                            offset,
                            typ: val_type,
                            size,
                        },
                    );
                    runtime_offset = offset.checked_add(size).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM runtime layout size overflow while placing static table",
                        )
                    })?;
                }
                FirMatch::DeclareVar { access, .. }
                    if access != AccessType::Struct && access != AccessType::Global =>
                {
                    return Err(WasmBackendError::new(
                        WasmBackendErrorCode::UnsupportedModuleShape,
                        format!("unsupported variable access class in WASM layout: {access:?}"),
                    ));
                }
                FirMatch::DeclareTable { access, .. }
                    if access != AccessType::Struct && access != AccessType::Static =>
                {
                    return Err(WasmBackendError::new(
                        WasmBackendErrorCode::UnsupportedModuleShape,
                        format!("unsupported table access class in WASM layout: {access:?}"),
                    ));
                }
                FirMatch::DeclareFun { body: None, .. } => {}
                FirMatch::DeclareVar { .. } | FirMatch::DeclareTable { .. } => {}
                other => {
                    return Err(WasmBackendError::new(
                        WasmBackendErrorCode::UnsupportedModuleShape,
                        format!("unsupported globals entry in WASM layout: {other:?}"),
                    ));
                }
            }
        }

        let mut runtime_fields = dsp_struct_items.to_vec();
        runtime_fields.extend(global_items.iter().copied());
        runtime_fields.sort_by_key(|item| !is_sound_struct_field(store, *item));

        for item in runtime_fields.iter().copied() {
            match match_fir(store, item) {
                FirMatch::DeclareVar {
                    name,
                    typ: FirType::Array(inner, len),
                    access: AccessType::Struct,
                    ..
                } => {
                    let (val_type, elem_size) = fir_type_storage(*inner, audio_slot)?;
                    let len = u32::try_from(len).map_err(|_| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM struct array length does not fit in u32",
                        )
                    })?;
                    let size = elem_size.checked_mul(len).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM struct array byte size overflow",
                        )
                    })?;
                    let offset = align_up(runtime_offset, elem_size);
                    field_offsets.insert(
                        name,
                        FieldLayout {
                            offset,
                            typ: val_type,
                            size,
                        },
                    );
                    runtime_offset = offset.checked_add(size).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM runtime layout size overflow while placing array field",
                        )
                    })?;
                }
                FirMatch::DeclareVar {
                    name,
                    typ,
                    access: AccessType::Struct,
                    ..
                } => {
                    let (val_type, slot_size) = fir_type_storage(typ, audio_slot)?;
                    let offset = align_up(runtime_offset, slot_size);
                    field_offsets.insert(
                        name,
                        FieldLayout {
                            offset,
                            typ: val_type,
                            size: slot_size,
                        },
                    );
                    runtime_offset = offset.checked_add(slot_size).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM runtime layout size overflow while placing scalar field",
                        )
                    })?;
                }
                FirMatch::DeclareTable {
                    name,
                    access: AccessType::Struct,
                    elem_type,
                    values,
                } => {
                    let (val_type, elem_size) = fir_type_storage(elem_type, audio_slot)?;
                    let len = u32::try_from(values.len()).map_err(|_| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM struct table length does not fit in u32",
                        )
                    })?;
                    let size = elem_size.checked_mul(len).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM struct table byte size overflow",
                        )
                    })?;
                    let offset = align_up(runtime_offset, elem_size);
                    field_offsets.insert(
                        name,
                        FieldLayout {
                            offset,
                            typ: val_type,
                            size,
                        },
                    );
                    runtime_offset = offset.checked_add(size).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM runtime layout size overflow while placing struct table",
                        )
                    })?;
                }
                FirMatch::DeclareFun { body: None, .. } => {}
                FirMatch::DeclareVar { .. } | FirMatch::DeclareTable { .. } => {}
                other => {
                    return Err(WasmBackendError::new(
                        WasmBackendErrorCode::UnsupportedModuleShape,
                        format!("unsupported static declarations entry in WASM layout: {other:?}"),
                    ));
                }
            }
        }

        let function_items = expect_block(store, functions, "module functions")?;
        let mut local_offsets = BTreeMap::new();
        collect_local_stack_arrays(store, &function_items, &mut local_offsets, audio_slot)?;
        for field in local_offsets.values_mut() {
            let align = match field.typ {
                WasmValType::I32 | WasmValType::F32 => 4,
                WasmValType::F64 => 8,
            };
            let offset = align_up(runtime_offset, align);
            field.offset = offset;
            runtime_offset = offset.checked_add(field.size).ok_or_else(|| {
                WasmBackendError::new(
                    WasmBackendErrorCode::MemoryLayoutOverflow,
                    "WASM runtime layout size overflow while placing local array buffer",
                )
            })?;
        }

        let struct_size = align_up(runtime_offset, audio_slot);
        let io_zone_offset = struct_size;
        let channels = u32::try_from(num_inputs + num_outputs).map_err(|_| {
            WasmBackendError::new(
                WasmBackendErrorCode::MemoryLayoutOverflow,
                "WASM channel count does not fit in u32",
            )
        })?;
        let io_zone_bytes = channels
            .checked_mul(
                audio_slot
                    .checked_mul(IO_BUFFER_SCRATCH_SAMPLES + 1)
                    .ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::MemoryLayoutOverflow,
                            "WASM per-channel I/O zone size overflow",
                        )
                    })?,
            )
            .ok_or_else(|| {
                WasmBackendError::new(
                    WasmBackendErrorCode::MemoryLayoutOverflow,
                    "WASM total I/O zone size overflow",
                )
            })?;
        let raw_required = io_zone_offset.checked_add(io_zone_bytes).ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::MemoryLayoutOverflow,
                "WASM total memory requirement overflow",
            )
        })?;
        let json_offset = align_up(raw_required, audio_slot);
        let json_len = u32::try_from(json_len).map_err(|_| {
            WasmBackendError::new(
                WasmBackendErrorCode::MemoryLayoutOverflow,
                "WASM JSON metadata length does not fit in u32",
            )
        })?;
        let total_bytes = json_offset.checked_add(json_len).ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::MemoryLayoutOverflow,
                "WASM total memory requirement overflow while placing JSON segment",
            )
        })?;
        let pages = wasm_pages_required(total_bytes)?;

        Ok(Self {
            field_offsets,
            local_offsets,
            struct_size,
            tables_offset,
            io_zone_offset,
            json_offset,
            total_bytes,
            pages,
        })
    }
}

/// C++ parity: WASM moves `Soundfile*` fields first in the DSP struct so the
/// host runtime can install soundfile pointers at stable low offsets before any
/// other mutable DSP state.
fn is_sound_struct_field(store: &FirStore, id: FirId) -> bool {
    matches!(
        match_fir(store, id),
        FirMatch::DeclareVar {
            typ: FirType::Sound,
            access: AccessType::Struct,
            ..
        }
    )
}

/// Resolves one FIR node that must be a `Block` in module-layout positions.
///
/// The layout pass expects `dsp_struct`, `globals`, and `static_decls` to be
/// normalized as FIR blocks before codegen begins.
fn expect_block(store: &FirStore, id: FirId, label: &str) -> Result<Vec<FirId>, WasmBackendError> {
    match match_fir(store, id) {
        FirMatch::Block(items) => Ok(items),
        other => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedModuleShape,
            format!("WASM layout expects {label} to be a FIR Block, got {other:?}"),
        )),
    }
}

fn collect_local_stack_arrays(
    store: &FirStore,
    function_items: &[FirId],
    out: &mut BTreeMap<String, FieldLayout>,
    audio_slot: u32,
) -> Result<(), WasmBackendError> {
    for item in function_items {
        if let FirMatch::DeclareFun {
            body: Some(body), ..
        } = match_fir(store, *item)
        {
            collect_local_stack_arrays_in_stmt(store, body, out, audio_slot)?;
        }
    }
    Ok(())
}

fn collect_local_stack_arrays_in_stmt(
    store: &FirStore,
    id: FirId,
    out: &mut BTreeMap<String, FieldLayout>,
    audio_slot: u32,
) -> Result<(), WasmBackendError> {
    match match_fir(store, id) {
        FirMatch::Block(items) => {
            for item in items {
                collect_local_stack_arrays_in_stmt(store, item, out, audio_slot)?;
            }
        }
        FirMatch::DeclareVar {
            name,
            typ: FirType::Array(inner, len),
            access: AccessType::Stack,
            ..
        } if len > 0 => {
            if let std::collections::btree_map::Entry::Vacant(entry) = out.entry(name) {
                let (val_type, elem_size) = fir_type_storage(*inner, audio_slot)?;
                let len = u32::try_from(len).map_err(|_| {
                    WasmBackendError::new(
                        WasmBackendErrorCode::MemoryLayoutOverflow,
                        "WASM local array length does not fit in u32",
                    )
                })?;
                let size = elem_size.checked_mul(len).ok_or_else(|| {
                    WasmBackendError::new(
                        WasmBackendErrorCode::MemoryLayoutOverflow,
                        "WASM local array byte size overflow",
                    )
                })?;
                entry.insert(FieldLayout {
                    offset: 0,
                    typ: val_type,
                    size,
                });
            }
        }
        FirMatch::SimpleForLoop { body, .. }
        | FirMatch::ForLoop { body, .. }
        | FirMatch::WhileLoop { body, .. }
        | FirMatch::Control { stmt: body, .. } => {
            collect_local_stack_arrays_in_stmt(store, body, out, audio_slot)?;
        }
        FirMatch::If {
            then_block,
            else_block,
            ..
        } => {
            collect_local_stack_arrays_in_stmt(store, then_block, out, audio_slot)?;
            if let Some(else_block) = else_block {
                collect_local_stack_arrays_in_stmt(store, else_block, out, audio_slot)?;
            }
        }
        FirMatch::Switch { cases, default, .. } => {
            for (_, case_stmt) in cases {
                collect_local_stack_arrays_in_stmt(store, case_stmt, out, audio_slot)?;
            }
            if let Some(default_stmt) = default {
                collect_local_stack_arrays_in_stmt(store, default_stmt, out, audio_slot)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Returns the storage kind and aligned slot width used for one FIR field.
///
/// The returned byte width is widened to at least one audio slot so runtime
/// offsets match the current Faust/C++ WASM alignment rule.
fn fir_type_storage(typ: FirType, audio_slot: u32) -> Result<(WasmValType, u32), WasmBackendError> {
    let (val_type, native_size) = match typ {
        FirType::Bool | FirType::Int32 => (WasmValType::I32, 4),
        FirType::Float32 => (WasmValType::F32, 4),
        FirType::FaustFloat => {
            if audio_slot >= 8 {
                (WasmValType::F64, 8)
            } else {
                (WasmValType::F32, 4)
            }
        }
        FirType::Int64 | FirType::Float64 => (WasmValType::F64, 8),
        FirType::Ptr(_) | FirType::Obj | FirType::UI | FirType::Meta | FirType::Sound => {
            (WasmValType::I32, WASM_PTR_BYTES)
        }
        other => {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedModuleShape,
                format!("unsupported FIR type in WASM layout: {other:?}"),
            ));
        }
    };
    Ok((val_type, native_size.max(audio_slot)))
}

/// Aligns `value` upward to the next multiple of `align`.
fn align_up(value: u32, align: u32) -> u32 {
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

/// Converts a raw byte requirement into the page count used by the backend.
///
/// The Rust backend currently follows the same coarse sizing rule as the C++
/// path: round up to the next power of two, then convert to 64 KiB pages.
fn wasm_pages_required(total_bytes: u32) -> Result<u32, WasmBackendError> {
    let rounded = wasm_pow2limit(total_bytes.max(WASM_PAGE_BYTES))?;
    Ok(rounded / WASM_PAGE_BYTES)
}

/// Rounds a byte size up to the next power-of-two allocation bucket.
fn wasm_pow2limit(value: u32) -> Result<u32, WasmBackendError> {
    if value <= 1 {
        return Ok(WASM_PAGE_BYTES);
    }
    let rounded = value.checked_next_power_of_two().ok_or_else(|| {
        WasmBackendError::new(
            WasmBackendErrorCode::MemoryLayoutOverflow,
            "WASM memory size exceeds the supported next-power-of-two range",
        )
    })?;
    Ok(rounded.max(WASM_PAGE_BYTES))
}
