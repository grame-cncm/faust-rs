//! `.fbc` serialization — read/write FBC bytecode in the interpreter text format.
//!
//! # Source provenance (C++)
//! - Write: `interpreter_dsp_factory_aux::write()` in `interpreter_dsp_aux.hh`,
//!   `FBCBasicInstruction::write()` / `FIRBlockStoreRealInstruction::write()` /
//!   `FIRBlockStoreIntInstruction::write()` / `FIRUserInterfaceInstruction::write()` /
//!   `FIRMetaInstruction::write()` in `interpreter_bytecode.hh`.
//! - Read: `interpreter_dsp_factory_aux::read()` in `interpreter_dsp.hh`,
//!   `readCodeBlock()` / `readCodeInstruction()` / `readUIBlock()` /
//!   `readMetaBlock()` in `interpreter_dsp_aux.hh`.
//!
//! # Design notes
//! - Two modes: **normal** (human-readable labels) and **small** (compact tokens).
//!   Both are text-based, line-oriented, and readable by [`read_fbc`], which
//!   auto-detects the mode from the first header token.
//! - Sub-blocks (for If/Select/Loop instructions) are written/read recursively.
//! - String quoting: labels, keys, and values are wrapped in double quotes
//!   (`quote1`/`unquote1`).
//! - Real values use round-trip precision so text `.fbc` dumps preserve the
//!   original scalar payload when read back.
//!
//! # Compatibility notes
//! - The Rust reader/writer targets the historical interpreter text format so
//!   `.fbc` artifacts remain inspectable and diffable.
//! - Stability is format-level, not byte-for-byte source-level: whitespace and
//!   quoting stay human-oriented, while semantic round-tripping is the primary
//!   compatibility goal.

use std::fmt::Write as FmtWrite;
use std::io::{self, BufRead, Write};

use super::bytecode::{
    BlockId, BlockStoreData, FbcBlock, FbcBlockArena, FbcInstruction, FbcMetaInstruction,
    FbcUiInstruction,
};
use super::factory::FbcDspFactory;
use super::opcode::{FBC_INSTRUCTION_NAMES, FbcOpcode, INTERP_FILE_VERSION};
use super::real::FbcReal;

/// Oldest append-compatible interpreter file version accepted by the reader.
///
/// Version 9 only appended the three Rust foreign-call opcodes after the
/// complete version-8 opcode range. Consequently every version-8 instruction
/// keeps the same numeric id and serialization grammar. Accepting V8 is needed
/// to execute bytecode emitted by the pinned C++ reference (`8eebea429`).
const MIN_READABLE_INTERP_FILE_VERSION: u32 = 8;

// ── Constants ──────────────────────────────────────────────────────────────

/// Faust version string written into `.fbc` headers.
///
/// C++ Faust uses a single `FAUSTVERSION` macro everywhere, `interp` included
/// (see `interpreter_dsp_aux.hh`'s `write()`); this tracks the same
/// [`codegen::VERSION`](crate::VERSION) the other text backends print rather
/// than a separate frozen string.
pub const FAUST_VERSION: &str = crate::VERSION;

// ── Error type ─────────────────────────────────────────────────────────────

/// Errors that can occur during `.fbc` deserialization.
///
/// The error set stays intentionally parse-oriented: once a factory is rebuilt
/// successfully, deeper runtime validation is handled by the interpreter
/// execution layer rather than by the serializer.
#[derive(Clone, Debug)]
pub enum FbcSerialError {
    /// Unexpected token in the input stream.
    UnexpectedToken {
        /// Grammar token or literal expected at the current position.
        expected: String,
        /// Token that was actually read.
        got: String,
    },
    /// File format version mismatch.
    VersionMismatch {
        /// FBC format version supported by this reader.
        expected: u32,
        /// Format version declared in the input header.
        got: u32,
    },
    /// REAL type mismatch (e.g., file says "double" but we're reading as f32).
    TypeMismatch {
        /// Scalar type selected by the Rust reader.
        expected: String,
        /// Scalar type declared in the FBC header.
        got: String,
    },
    /// Failed to parse an integer.
    ParseInt(String),
    /// Failed to parse a real number.
    ParseReal(String),
    /// I/O error.
    Io(String),
    /// Unexpected end of input.
    UnexpectedEof,
}

impl std::fmt::Display for FbcSerialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedToken { expected, got } => {
                write!(f, "expected token '{expected}', got '{got}'")
            }
            Self::VersionMismatch { expected, got } => {
                write!(f, "file version {got} != compiled version {expected}")
            }
            Self::TypeMismatch { expected, got } => {
                write!(f, "REAL type mismatch: expected '{expected}', got '{got}'")
            }
            Self::ParseInt(s) => write!(f, "failed to parse integer: '{s}'"),
            Self::ParseReal(s) => write!(f, "failed to parse real: '{s}'"),
            Self::Io(s) => write!(f, "I/O error: {s}"),
            Self::UnexpectedEof => write!(f, "unexpected end of input"),
        }
    }
}

impl From<io::Error> for FbcSerialError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

// ── String quoting (C++ quote1 / unquote1) ─────────────────────────────────

/// Wraps a string in double quotes.
///
/// # Source provenance (C++)
/// - `quote1()` in `interpreter_bytecode.hh`.
fn quote1(s: &str) -> String {
    format!("\"{s}\"")
}

/// Removes surrounding double quotes from a string, if present.
///
/// # Source provenance (C++)
/// - `unquote1()` in `interpreter_bytecode.hh`.
#[allow(dead_code)]
fn unquote1(s: &str) -> String {
    if s.starts_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

// ── Real precision ─────────────────────────────────────────────────────────

/// Returns "float" or "double" for the REAL type.
fn real_type_name<R: FbcReal>() -> &'static str {
    if std::mem::size_of::<R>() == 4 {
        "float"
    } else {
        "double"
    }
}

/// Formats a REAL value with round-trip precision.
fn fmt_real<R: FbcReal>(v: R) -> String {
    format!("{v}")
}

// ═══════════════════════════════════════════════════════════════════════════
// WRITE PATH
// ═══════════════════════════════════════════════════════════════════════════

/// Serializes an [`FbcDspFactory`] to `.fbc` text format.
///
/// # Arguments
/// - `factory`: the factory to serialize.
/// - `writer`: output stream.
/// - `small`: if `true`, use compact (small) format; otherwise, normal format.
///
/// Both modes encode the same semantic content and are readable by
/// [`read_fbc`], which auto-detects the mode from the first header token.
/// `small=true` keeps tokens short for compact fixtures, while normal mode
/// favors readability and parity with the traditional C++ textual dumps.
///
/// # Source provenance (C++)
/// - `interpreter_dsp_factory_aux::write()` in `interpreter_dsp_aux.hh`.
pub fn write_fbc<R: FbcReal>(
    factory: &FbcDspFactory<R>,
    writer: &mut dyn Write,
    small: bool,
) -> io::Result<()> {
    if small {
        writeln!(writer, "i {}", real_type_name::<R>())?;
        writeln!(writer, "f {INTERP_FILE_VERSION}")?;
        writeln!(writer, "v {FAUST_VERSION}")?;
        writeln!(writer, "c {}", factory.compile_options)?;
        writeln!(writer, "n {}", factory.name)?;
        writeln!(writer, "s {}", factory.sha_key)?;
        writeln!(writer, "o {}", factory.opt_level)?;
        writeln!(writer, "i {} o {}", factory.num_inputs, factory.num_outputs)?;
        writeln!(
            writer,
            "i {} r {} s {} c {} i {}",
            factory.int_heap_size,
            factory.real_heap_size,
            factory.sr_offset,
            factory.count_offset,
            factory.iota_offset
        )?;

        writeln!(writer, "m")?;
        write_meta_block(&factory.meta_block, writer, small)?;

        writeln!(writer, "u")?;
        write_ui_block(&factory.ui_block, writer, small)?;

        writeln!(writer, "s")?;
        write_code_block(&factory.arena, factory.static_init_block, writer, small)?;

        writeln!(writer, "i")?;
        write_code_block(&factory.arena, factory.init_block, writer, small)?;

        writeln!(writer, "c")?;
        write_code_block(&factory.arena, factory.reset_ui_block, writer, small)?;

        writeln!(writer, "c")?;
        write_code_block(&factory.arena, factory.clear_block, writer, small)?;

        writeln!(writer, "c")?;
        write_code_block(&factory.arena, factory.compute_block, writer, small)?;

        writeln!(writer, "d")?;
        write_code_block(&factory.arena, factory.compute_dsp_block, writer, small)?;
    } else {
        writeln!(writer, "interpreter_dsp_factory {}", real_type_name::<R>())?;
        writeln!(writer, "file_version {INTERP_FILE_VERSION}")?;
        writeln!(writer, "Faust version {FAUST_VERSION}")?;
        writeln!(writer, "compile_options {}", factory.compile_options)?;
        writeln!(writer, "name {}", factory.name)?;
        writeln!(writer, "sha_key {}", factory.sha_key)?;
        writeln!(writer, "opt_level {}", factory.opt_level)?;
        writeln!(
            writer,
            "inputs {} outputs {}",
            factory.num_inputs, factory.num_outputs
        )?;
        writeln!(
            writer,
            "int_heap_size {} real_heap_size {} sr_offset {} count_offset {} iota_offset {}",
            factory.int_heap_size,
            factory.real_heap_size,
            factory.sr_offset,
            factory.count_offset,
            factory.iota_offset
        )?;

        writeln!(writer, "meta_block")?;
        write_meta_block(&factory.meta_block, writer, small)?;

        writeln!(writer, "user_interface_block")?;
        write_ui_block(&factory.ui_block, writer, small)?;

        writeln!(writer, "static_init_block")?;
        write_code_block(&factory.arena, factory.static_init_block, writer, small)?;

        writeln!(writer, "constants_block")?;
        write_code_block(&factory.arena, factory.init_block, writer, small)?;

        writeln!(writer, "reset_ui")?;
        write_code_block(&factory.arena, factory.reset_ui_block, writer, small)?;

        writeln!(writer, "clear_block")?;
        write_code_block(&factory.arena, factory.clear_block, writer, small)?;

        writeln!(writer, "control_block")?;
        write_code_block(&factory.arena, factory.compute_block, writer, small)?;

        writeln!(writer, "dsp_block")?;
        write_code_block(&factory.arena, factory.compute_dsp_block, writer, small)?;
    }

    Ok(())
}

/// Writes a meta block (block_size + meta instructions).
fn write_meta_block(
    meta: &[FbcMetaInstruction],
    writer: &mut dyn Write,
    small: bool,
) -> io::Result<()> {
    writeln!(writer, "block_size {}", meta.len())?;
    for m in meta {
        write_meta_instruction(m, writer, small)?;
    }
    Ok(())
}

/// Writes a single meta instruction.
///
/// # Source provenance (C++)
/// - `FIRMetaInstruction::write()` in `interpreter_bytecode.hh`.
fn write_meta_instruction(
    m: &FbcMetaInstruction,
    writer: &mut dyn Write,
    small: bool,
) -> io::Result<()> {
    if small {
        writeln!(writer, "m k {} v {}", quote1(&m.key), quote1(&m.value))
    } else {
        writeln!(
            writer,
            "meta key {} value {}",
            quote1(&m.key),
            quote1(&m.value)
        )
    }
}

/// Writes a UI block (block_size + UI instructions).
fn write_ui_block<R: FbcReal>(
    ui: &[FbcUiInstruction<R>],
    writer: &mut dyn Write,
    small: bool,
) -> io::Result<()> {
    writeln!(writer, "block_size {}", ui.len())?;
    for u in ui {
        write_ui_instruction(u, writer, small)?;
    }
    Ok(())
}

/// Writes a single UI instruction.
///
/// # Source provenance (C++)
/// - `FIRUserInterfaceInstruction::write()` in `interpreter_bytecode.hh`.
fn write_ui_instruction<R: FbcReal>(
    u: &FbcUiInstruction<R>,
    writer: &mut dyn Write,
    small: bool,
) -> io::Result<()> {
    let opcode_num = u.opcode as u16;
    if small {
        writeln!(
            writer,
            "o {} k  o {} l {} k {} v {} i {} m {} m {} s {}",
            opcode_num,
            u.offset,
            quote1(&u.label),
            quote1(&u.key),
            quote1(&u.value),
            fmt_real(u.init),
            fmt_real(u.min),
            fmt_real(u.max),
            fmt_real(u.step),
        )
    } else {
        let opcode_name = FBC_INSTRUCTION_NAMES
            .get(opcode_num as usize)
            .unwrap_or(&"unknown");
        writeln!(
            writer,
            "opcode {} {} offset {} label {} key {} value {} init {} min {} max {} step {}",
            opcode_num,
            opcode_name,
            u.offset,
            quote1(&u.label),
            quote1(&u.key),
            quote1(&u.value),
            fmt_real(u.init),
            fmt_real(u.min),
            fmt_real(u.max),
            fmt_real(u.step),
        )
    }
}

/// Writes a code block (`block_size` + instructions, with recursive sub-blocks).
///
/// # Source provenance (C++)
/// - `FBCBlockInstruction::write()` in `interpreter_bytecode.hh`.
///
/// Branch targets are serialized structurally rather than by raw block id so
/// the file stays self-contained even though the in-memory model uses arena
/// indices.
fn write_code_block<R: FbcReal>(
    arena: &FbcBlockArena<R>,
    block_id: BlockId,
    writer: &mut dyn Write,
    small: bool,
) -> io::Result<()> {
    let block = arena.get(block_id);
    writeln!(writer, "block_size {}", block.instructions.len())?;
    for instr in &block.instructions {
        if let Some(data) = instr.block_store.as_ref() {
            write_block_store_instruction(instr, data, writer, small)?;
        } else {
            write_instruction(instr, writer, small)?;
        }

        // Write sub-blocks for branching instructions.
        if instr.get_branch1().is_some() {
            write_code_block(arena, instr.get_branch1().unwrap(), writer, small)?;
        }
        if instr.get_branch2().is_some() {
            write_code_block(arena, instr.get_branch2().unwrap(), writer, small)?;
        }
    }
    Ok(())
}

/// Writes a single regular instruction.
///
/// # Source provenance (C++)
/// - `FBCBasicInstruction::write()` in `interpreter_bytecode.hh`.
fn write_instruction<R: FbcReal>(
    instr: &FbcInstruction<R>,
    writer: &mut dyn Write,
    small: bool,
) -> io::Result<()> {
    let opcode_num = instr.opcode as u16;
    if small {
        let mut line = format!(
            "o {} k  i {} r {} o {} o {}",
            opcode_num,
            instr.int_value,
            fmt_real(instr.real_value),
            instr.offset1,
            instr.offset2,
        );
        if !instr.name.is_empty() {
            write!(line, " n {}", instr.name).unwrap();
        }
        writeln!(writer, "{line}")
    } else {
        let opcode_name = FBC_INSTRUCTION_NAMES
            .get(opcode_num as usize)
            .unwrap_or(&"unknown");
        let mut line = format!(
            "opcode {} {} int {} real {} offset1 {} offset2 {}",
            opcode_num,
            opcode_name,
            instr.int_value,
            fmt_real(instr.real_value),
            instr.offset1,
            instr.offset2,
        );
        if !instr.name.is_empty() {
            write!(line, " name {}", instr.name).unwrap();
        }
        writeln!(writer, "{line}")
    }
}

/// Writes a BlockStoreReal or BlockStoreInt instruction with its data line.
///
/// # Source provenance (C++)
/// - `FIRBlockStoreRealInstruction::write()` and
///   `FIRBlockStoreIntInstruction::write()` in `interpreter_bytecode.hh`.
fn write_block_store_instruction<R: FbcReal>(
    instr: &FbcInstruction<R>,
    data: &BlockStoreData<R>,
    writer: &mut dyn Write,
    small: bool,
) -> io::Result<()> {
    let opcode_num = instr.opcode as u16;
    match data {
        BlockStoreData::Real(values) => {
            if small {
                writeln!(
                    writer,
                    "o {} k  o {} o {} s {}",
                    opcode_num,
                    instr.offset1,
                    instr.offset2,
                    values.len()
                )?;
            } else {
                let opcode_name = FBC_INSTRUCTION_NAMES
                    .get(opcode_num as usize)
                    .unwrap_or(&"unknown");
                writeln!(
                    writer,
                    "opcode {} {} offset1 {} offset2 {} size {}",
                    opcode_num,
                    opcode_name,
                    instr.offset1,
                    instr.offset2,
                    values.len()
                )?;
            }
            // Write data values on a single line.
            let mut line = String::new();
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    line.push(' ');
                }
                write!(line, "{}", fmt_real(*v)).unwrap();
            }
            writeln!(writer, "{line}")?;
        }
        BlockStoreData::Int(values) => {
            if small {
                writeln!(
                    writer,
                    "o {} k  o {} o {} s {}",
                    opcode_num,
                    instr.offset1,
                    instr.offset2,
                    values.len()
                )?;
            } else {
                let opcode_name = FBC_INSTRUCTION_NAMES
                    .get(opcode_num as usize)
                    .unwrap_or(&"unknown");
                writeln!(
                    writer,
                    "opcode {} {} offset1 {} offset2 {} size {}",
                    opcode_num,
                    opcode_name,
                    instr.offset1,
                    instr.offset2,
                    values.len()
                )?;
            }
            // Write data values on a single line.
            let mut line = String::new();
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    line.push(' ');
                }
                write!(line, "{v}").unwrap();
            }
            writeln!(writer, "{line}")?;
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// READ PATH
// ═══════════════════════════════════════════════════════════════════════════

/// Deserializes an [`FbcDspFactory`] from `.fbc` text format.
///
/// The reader must be positioned at the start of the `.fbc` content
/// (the `"interpreter_dsp_factory float|double"` line, or `"i float|double"`
/// for the small/compact format — see [`write_fbc`]). The format is detected
/// from the first header token; the rest of the file body (meta/UI/code
/// blocks) uses the same block-size-prefixed, position-based grammar in both
/// formats and needs no mode-specific handling.
///
/// # Source provenance (C++)
/// - `read_real_type()` in `interpreter_dsp_aux.cpp` (header line).
/// - `interpreter_dsp_factory_aux::read()` in `interpreter_dsp.hh` (body).
pub fn read_fbc<R: FbcReal>(reader: &mut dyn BufRead) -> Result<FbcDspFactory<R>, FbcSerialError> {
    // ── Header line: "interpreter_dsp_factory float|double" (normal) or
    //    "i float|double" (small) — the leading token selects the grammar
    //    used for the remaining header fields below.
    let header_line = read_line(reader)?;
    let mut header_tokens = header_line.split_whitespace();
    let head = header_tokens.next().ok_or(FbcSerialError::UnexpectedEof)?;
    let small = match head {
        "interpreter_dsp_factory" => false,
        "i" => true,
        other => {
            return Err(FbcSerialError::UnexpectedToken {
                expected: "interpreter_dsp_factory' or 'i".to_string(),
                got: other.to_string(),
            });
        }
    };
    let real_type = header_tokens
        .next()
        .ok_or(FbcSerialError::UnexpectedEof)?
        .to_string();
    let expected_type = real_type_name::<R>();
    if real_type != expected_type {
        return Err(FbcSerialError::TypeMismatch {
            expected: expected_type.to_string(),
            got: real_type,
        });
    }

    // ── file_version ───────────────────────────────────────────────────
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    header_token(&mut tokens, "file_version", "f", small)?;
    let file_num = parse_i32(tokens.next())?;
    if !(MIN_READABLE_INTERP_FILE_VERSION..=INTERP_FILE_VERSION).contains(&(file_num as u32)) {
        return Err(FbcSerialError::VersionMismatch {
            expected: INTERP_FILE_VERSION,
            got: file_num as u32,
        });
    }

    // ── Faust version (read and discard) ───────────────────────────────
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    if small {
        header_token(&mut tokens, "", "v", true)?;
    } else {
        check_token(tokens.next(), "Faust")?;
        check_token(tokens.next(), "version")?;
    }
    // Version string is read but not stored.

    // ── compile_options ────────────────────────────────────────────────
    let line = read_line(reader)?;
    let compile_options = line
        .strip_prefix(if small { "c " } else { "compile_options " })
        .unwrap_or("")
        .to_string();

    // ── name ───────────────────────────────────────────────────────────
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    header_token(&mut tokens, "name", "n", small)?;
    let name = tokens.next().unwrap_or("").to_string();

    // ── sha_key ────────────────────────────────────────────────────────
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    header_token(&mut tokens, "sha_key", "s", small)?;
    let sha_key = tokens.next().unwrap_or("").to_string();

    // ── opt_level ──────────────────────────────────────────────────────
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    header_token(&mut tokens, "opt_level", "o", small)?;
    let opt_level = parse_i32(tokens.next())?;

    // ── inputs / outputs ───────────────────────────────────────────────
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    header_token(&mut tokens, "inputs", "i", small)?;
    let num_inputs = parse_i32(tokens.next())?;
    header_token(&mut tokens, "outputs", "o", small)?;
    let num_outputs = parse_i32(tokens.next())?;

    // ── heap sizes / offsets ───────────────────────────────────────────
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    header_token(&mut tokens, "int_heap_size", "i", small)?;
    let int_heap_size = parse_i32(tokens.next())?;
    header_token(&mut tokens, "real_heap_size", "r", small)?;
    let real_heap_size = parse_i32(tokens.next())?;
    header_token(&mut tokens, "sr_offset", "s", small)?;
    let sr_offset = parse_i32(tokens.next())?;
    header_token(&mut tokens, "count_offset", "c", small)?;
    let count_offset = parse_i32(tokens.next())?;
    header_token(&mut tokens, "iota_offset", "i", small)?;
    let iota_offset = parse_i32(tokens.next())?;

    // ── meta_block ─────────────────────────────────────────────────────
    let _label = read_line(reader)?; // "meta_block"
    let meta_block = read_meta_block(reader)?;

    // ── user_interface_block ───────────────────────────────────────────
    let _label = read_line(reader)?; // "user_interface_block"
    let ui_block = read_ui_block::<R>(reader)?;

    // ── 6 code blocks ──────────────────────────────────────────────────
    let mut arena = FbcBlockArena::<R>::new();

    let _label = read_line(reader)?; // "static_init_block"
    let static_init_block = read_code_block(reader, &mut arena)?;

    let _label = read_line(reader)?; // "constants_block"
    let init_block = read_code_block(reader, &mut arena)?;

    let _label = read_line(reader)?; // "reset_ui"
    let reset_ui_block = read_code_block(reader, &mut arena)?;

    let _label = read_line(reader)?; // "clear_block"
    let clear_block = read_code_block(reader, &mut arena)?;

    let _label = read_line(reader)?; // "control_block"
    let compute_block = read_code_block(reader, &mut arena)?;

    let _label = read_line(reader)?; // "dsp_block"
    let compute_dsp_block = read_code_block(reader, &mut arena)?;

    Ok(FbcDspFactory::new(
        name,
        sha_key,
        compile_options,
        file_num as u32,
        num_inputs,
        num_outputs,
        int_heap_size,
        real_heap_size,
        sr_offset,
        count_offset,
        iota_offset,
        opt_level,
        arena,
        meta_block,
        ui_block,
        static_init_block,
        init_block,
        reset_ui_block,
        clear_block,
        compute_block,
        compute_dsp_block,
    ))
}

// ── Read helpers ───────────────────────────────────────────────────────────

/// Reads a single line from the reader, trimming the trailing newline.
fn read_line(reader: &mut dyn BufRead) -> Result<String, FbcSerialError> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(FbcSerialError::UnexpectedEof);
    }
    // Trim trailing newline/carriage return.
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(line)
}

/// Reads a "logical line" that may contain quoted strings with embedded
/// newlines (e.g. `label "sustain\n"`).
///
/// Keeps appending physical lines (joined with `\n`) until all double-quote
/// characters are balanced — i.e. the total count is even, meaning every
/// opened `"` has a matching closing `"`.
fn read_quoted_logical_line(reader: &mut dyn BufRead) -> Result<String, FbcSerialError> {
    let mut line = read_line(reader)?;
    while line.chars().filter(|&c| c == '"').count() % 2 != 0 {
        let next = read_line(reader)?;
        line.push('\n');
        line.push_str(&next);
    }
    Ok(line)
}

/// Checks that a token matches the expected string.
fn check_token(token: Option<&str>, expected: &str) -> Result<(), FbcSerialError> {
    match token {
        Some(t) if t == expected => Ok(()),
        Some(t) => Err(FbcSerialError::UnexpectedToken {
            expected: expected.to_string(),
            got: t.to_string(),
        }),
        None => Err(FbcSerialError::UnexpectedToken {
            expected: expected.to_string(),
            got: String::new(),
        }),
    }
}

/// Checks the next header token against whichever grammar is active.
///
/// Header fields use a distinct literal keyword per format (e.g.
/// `"file_version"` in normal mode vs `"f"` in small mode) at the same
/// position, so callers only need to supply both spellings and let this
/// helper pick the one matching the format detected from the first header
/// line. `normal == ""` skips the check (used for small-only header
/// fields that have no normal-mode counterpart token, like the Faust
/// version line's single `"v"` label).
fn header_token(
    tokens: &mut std::str::SplitWhitespace<'_>,
    normal: &str,
    small_label: &str,
    small: bool,
) -> Result<(), FbcSerialError> {
    let expected = if small { small_label } else { normal };
    if expected.is_empty() {
        return Ok(());
    }
    check_token(tokens.next(), expected)
}

/// Parses an i32 from an optional token.
fn parse_i32(token: Option<&str>) -> Result<i32, FbcSerialError> {
    match token {
        Some(s) => s
            .parse::<i32>()
            .map_err(|_| FbcSerialError::ParseInt(s.to_string())),
        None => Err(FbcSerialError::UnexpectedEof),
    }
}

/// Parses a REAL value from an optional token.
fn parse_real<R: FbcReal>(token: Option<&str>) -> Result<R, FbcSerialError> {
    match token {
        Some(s) => s
            .parse::<R>()
            .map_err(|_| FbcSerialError::ParseReal(s.to_string())),
        None => Err(FbcSerialError::UnexpectedEof),
    }
}

/// Reads a meta block: `block_size N` followed by N meta instructions.
///
/// # Source provenance (C++)
/// - `readMetaBlock()` in `interpreter_dsp_aux.hh`.
fn read_meta_block(reader: &mut dyn BufRead) -> Result<Vec<FbcMetaInstruction>, FbcSerialError> {
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    let _label = tokens.next(); // "block_size"
    let size = parse_i32(tokens.next())?;

    let mut result = Vec::with_capacity(size as usize);
    for _ in 0..size {
        let line = read_quoted_logical_line(reader)?;
        result.push(read_meta_instruction(&line)?);
    }
    Ok(result)
}

/// Reads a single meta instruction from a line.
///
/// Format: `meta key "K" value "V"`
///
/// # Source provenance (C++)
/// - `readMetaInstruction()` in `interpreter_dsp_aux.hh`.
fn read_meta_instruction(line: &str) -> Result<FbcMetaInstruction, FbcSerialError> {
    // Split by quotes to extract key and value strings.
    // Format: meta key "KEY" value "VALUE"
    let rest = line.trim();

    // Find key content between first pair of quotes.
    let key = extract_nth_quoted(rest, 0).unwrap_or_default();
    let value = extract_nth_quoted(rest, 1).unwrap_or_default();

    Ok(FbcMetaInstruction::new(key, value))
}

/// Reads a UI block: `block_size N` followed by N UI instructions.
///
/// # Source provenance (C++)
/// - `readUIBlock()` in `interpreter_dsp_aux.hh`.
fn read_ui_block<R: FbcReal>(
    reader: &mut dyn BufRead,
) -> Result<Vec<FbcUiInstruction<R>>, FbcSerialError> {
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    let _label = tokens.next(); // "block_size"
    let size = parse_i32(tokens.next())?;

    let mut result = Vec::with_capacity(size as usize);
    for _ in 0..size {
        let line = read_quoted_logical_line(reader)?;
        result.push(read_ui_instruction::<R>(&line)?);
    }
    Ok(result)
}

/// Reads a single UI instruction from a line.
///
/// Format (normal):
/// `opcode NUM kName offset O label "L" key "K" value "V" init I min MN max MX step ST`
///
/// # Source provenance (C++)
/// - `readUIInstruction()` in `interpreter_dsp_aux.hh`.
fn read_ui_instruction<R: FbcReal>(line: &str) -> Result<FbcUiInstruction<R>, FbcSerialError> {
    let mut tokens = line.split_whitespace();

    let _opcode_label = tokens.next(); // "opcode"
    let opcode_num = parse_i32(tokens.next())?;
    let opcode = FbcOpcode::from_u16(opcode_num as u16)
        .ok_or_else(|| FbcSerialError::ParseInt(format!("invalid opcode {opcode_num}")))?;
    let _opcode_name = tokens.next(); // opcode string representation (unused)

    let _offset_label = tokens.next(); // "offset"
    let offset = parse_i32(tokens.next())?;

    let _label_label = tokens.next(); // "label"
    // tokens is consumed by collect; we re-parse using extract_nth_quoted below.
    // Since quoted strings can contain spaces, we need to find the tokens
    // after each quoted field.
    let label_str = extract_nth_quoted(line, 0).unwrap_or_default();
    let key_str = extract_nth_quoted(line, 1).unwrap_or_default();
    let value_str = extract_nth_quoted(line, 2).unwrap_or_default();

    // Parse numeric fields after the last quoted string.
    let after_last_quote = after_nth_quote_pair(line, 3);
    let mut num_tokens = after_last_quote.split_whitespace();

    let _init_label = num_tokens.next(); // "init"
    let init = parse_real::<R>(num_tokens.next())?;
    let _min_label = num_tokens.next(); // "min"
    let min = parse_real::<R>(num_tokens.next())?;
    let _max_label = num_tokens.next(); // "max"
    let max = parse_real::<R>(num_tokens.next())?;
    let _step_label = num_tokens.next(); // "step"
    let step = parse_real::<R>(num_tokens.next())?;

    Ok(FbcUiInstruction {
        opcode,
        offset,
        label: label_str,
        key: key_str,
        value: value_str,
        init,
        min,
        max,
        step,
    })
}

/// Reads a code block: `block_size N` followed by N instructions.
///
/// # Source provenance (C++)
/// - `readCodeBlock()` in `interpreter_dsp_aux.hh`.
fn read_code_block<R: FbcReal>(
    reader: &mut dyn BufRead,
    arena: &mut FbcBlockArena<R>,
) -> Result<BlockId, FbcSerialError> {
    let line = read_line(reader)?;
    let mut tokens = line.split_whitespace();
    let _label = tokens.next(); // "block_size"
    let size = parse_i32(tokens.next())?;

    let mut block = FbcBlock::<R>::new();

    for _ in 0..size {
        let line = read_line(reader)?;
        let instr = read_code_instruction::<R>(&line, reader, arena)?;

        // Special case for loops: CondBranch's branch1 is set to the
        // containing block (loop-back pointer). We'll fix this up after
        // the block is allocated.
        let is_cond_branch = instr.opcode == FbcOpcode::CondBranch;

        block.push(instr);

        // For CondBranch, branch1 will be set to this block's ID after allocation.
        if is_cond_branch {
            // We'll fix this up below.
        }
    }

    let block_id = arena.alloc(block);

    // Fix up CondBranch loop-back pointers.
    let block_ref = arena.get_mut(block_id);
    for instr in &mut block_ref.instructions {
        if instr.opcode == FbcOpcode::CondBranch {
            instr.branch1 = Some(block_id);
        }
    }

    Ok(block_id)
}

/// Reads a single code instruction from a line, potentially consuming
/// additional lines for sub-blocks or block-store data.
///
/// # Source provenance (C++)
/// - `readCodeInstruction()` in `interpreter_dsp_aux.hh`.
fn read_code_instruction<R: FbcReal>(
    line: &str,
    reader: &mut dyn BufRead,
    arena: &mut FbcBlockArena<R>,
) -> Result<FbcInstruction<R>, FbcSerialError> {
    let mut tokens = line.split_whitespace();

    let _opcode_label = tokens.next(); // "opcode"
    let opcode_num = parse_i32(tokens.next())?;
    let opcode = FbcOpcode::from_u16(opcode_num as u16)
        .ok_or_else(|| FbcSerialError::ParseInt(format!("invalid opcode {opcode_num}")))?;
    let _opcode_name = tokens.next(); // opcode string representation (unused)

    if opcode == FbcOpcode::BlockStoreReal {
        // Format: opcode NUM kBlockStoreReal offset1 O1 offset2 O2 size S
        let _offset1_label = tokens.next(); // "offset1"
        let offset1 = parse_i32(tokens.next())?;
        let _offset2_label = tokens.next(); // "offset2"
        let offset2 = parse_i32(tokens.next())?;
        let _size_label = tokens.next(); // "size"
        let block_size = parse_i32(tokens.next())?;

        // Read data values from next line.
        let data_line = read_line(reader)?;
        let mut values = Vec::with_capacity(block_size as usize);
        for token in data_line.split_whitespace() {
            let v = token
                .parse::<R>()
                .map_err(|_| FbcSerialError::ParseReal(token.to_string()))?;
            values.push(v);
        }

        let mut instr =
            FbcInstruction::with_values_and_offsets(opcode, 0, R::default(), offset1, offset2);
        instr.block_store = Some(BlockStoreData::Real(values));
        Ok(instr)
    } else if opcode == FbcOpcode::BlockStoreInt {
        // Format: opcode NUM kBlockStoreInt offset1 O1 offset2 O2 size S
        let _offset1_label = tokens.next(); // "offset1"
        let offset1 = parse_i32(tokens.next())?;
        let _offset2_label = tokens.next(); // "offset2"
        let offset2 = parse_i32(tokens.next())?;
        let _size_label = tokens.next(); // "size"
        let block_size = parse_i32(tokens.next())?;

        // Read data values from next line.
        let data_line = read_line(reader)?;
        let mut values = Vec::with_capacity(block_size as usize);
        for token in data_line.split_whitespace() {
            let v = token
                .parse::<i32>()
                .map_err(|_| FbcSerialError::ParseInt(token.to_string()))?;
            values.push(v);
        }

        let mut instr =
            FbcInstruction::with_values_and_offsets(opcode, 0, R::default(), offset1, offset2);
        instr.block_store = Some(BlockStoreData::Int(values));
        Ok(instr)
    } else {
        // General instruction format:
        // opcode NUM kName int V real R offset1 O1 offset2 O2 [name N]
        let _int_label = tokens.next(); // "int"
        let int_value = parse_i32(tokens.next())?;
        let _real_label = tokens.next(); // "real"
        let real_value = parse_real::<R>(tokens.next())?;
        let _offset1_label = tokens.next(); // "offset1"
        let offset1 = parse_i32(tokens.next())?;
        let _offset2_label = tokens.next(); // "offset2"
        let offset2 = parse_i32(tokens.next())?;

        // Optional "name"/"n" field: present or absent, but its label token is
        // discarded positionally like the others above (its literal spelling
        // differs between normal ("name") and small ("n") mode).
        let name = if tokens.next().is_some() {
            tokens.next().unwrap_or("").to_string()
        } else {
            String::new()
        };

        // Possibly read sub-blocks for branching instructions.
        let mut branch1 = None;
        let mut branch2 = None;

        if opcode.is_choice() || opcode == FbcOpcode::Loop {
            branch1 = Some(read_code_block(reader, arena)?);
            branch2 = Some(read_code_block(reader, arena)?);
        }

        let instr = FbcInstruction::full(
            opcode, name, int_value, real_value, offset1, offset2, branch1, branch2,
        );
        Ok(instr)
    }
}

// ── String extraction helpers ──────────────────────────────────────────────

/// Extracts the content of the Nth double-quoted string in the input.
///
/// Returns `None` if there are fewer than `n+1` quoted strings.
fn extract_nth_quoted(s: &str, n: usize) -> Option<String> {
    let mut count = 0;
    let mut chars = s.chars().enumerate();
    while let Some((i, c)) = chars.next() {
        if c == '"' {
            // Find the closing quote.
            let start = i + 1;
            for (j, c2) in chars.by_ref() {
                if c2 == '"' {
                    if count == n {
                        return Some(s[start..j].to_string());
                    }
                    count += 1;
                    break;
                }
            }
        }
    }
    None
}

/// Returns the portion of the string after the Nth pair of double quotes.
///
/// Used to find numeric fields that follow quoted string fields.
fn after_nth_quote_pair(s: &str, n: usize) -> &str {
    let mut count = 0;
    let mut in_quote = false;
    for (i, c) in s.char_indices() {
        if c == '"' {
            if in_quote {
                count += 1;
                if count == n {
                    return &s[i + 1..];
                }
                in_quote = false;
            } else {
                in_quote = true;
            }
        }
    }
    ""
}

// ═══════════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests;
