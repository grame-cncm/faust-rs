//! FBC interpreter execution engine.
//!
//! # Source provenance (C++)
//! - `compiler/generator/interpreter/fbc_interpreter.hh`
//!   (`FBCInterpreter<REAL, TRACE>::executeBlock`)
//!
//! # Design notes
//! - Replaces C++ computed-goto dispatch (`goto* fDispatchTable[opcode]`) with a
//!   tight `loop { match }` pattern. The `#[repr(u16)]` opcode enum enables LLVM
//!   to generate a jump table, achieving comparable dispatch efficiency.
//! - No `unsafe`: all stack/heap accesses are bounds-checked. The performance
//!   overhead is negligible versus the compute-bound math operations.
//! - Equivalent to C++ `TRACE=0` mode (no tracing / overflow checks).
//!
//! # Control flow model
//! The interpreter uses three stacks, emptied at the start of each
//! `execute_block_io` call and kept by the executor from one call to the next
//! so that executing a block allocates nothing once they have grown (the C++
//! interpreter keeps them in local arrays):
//! - `real_stack[512]` — computation stack for REAL values
//! - `int_stack[512]` — computation stack for integers
//! - `addr_stack[64]` — return addresses as `(BlockId, pc)` pairs
//!
//! When a branch instruction is encountered (If, Select, Loop), the current
//! execution position is saved on `addr_stack`, and control jumps to the target
//! block. When `Return` is reached and the address stack is non-empty, execution
//! resumes at the saved position.
//!
//! # API mapping status
//! - `FBCInterpreter<REAL, TRACE>::executeBlock` (C++) →
//!   [`FbcExecutor::execute_block_io`] (Rust): adapted (computed goto → match,
//!   raw pointers → `BlockId`, C arrays → `Vec`).

use super::bytecode::{BlockId, BlockStoreData, FbcBlockArena};
use super::foreign::{ForeignScalarType, ForeignSignature, lookup_foreign_function};
use super::opcode::FbcOpcode;
use super::real::FbcReal;
use super::soundfile::Soundfile;
use foreign_call::{ScalarType as HostScalarType, Value as HostValue};

/// Stack sizes matching the C++ interpreter constants.
const REAL_STACK_CAPACITY: usize = 512;
const INT_STACK_CAPACITY: usize = 512;
const ADDR_STACK_CAPACITY: usize = 64;

/// The evaluation stacks of an executor, kept across block executions.
struct ExecStacks<R> {
    real: Vec<R>,
    int: Vec<i32>,
    addr: Vec<(BlockId, usize)>,
}

impl<R> Default for ExecStacks<R> {
    /// Empty stacks, with no allocation.
    fn default() -> Self {
        Self {
            real: Vec::new(),
            int: Vec::new(),
            addr: Vec::new(),
        }
    }
}

impl<R> ExecStacks<R> {
    /// Empties the stacks and makes sure they hold the C++ capacities, which
    /// allocates only the first time (or after a panic dropped them).
    fn prepare(&mut self) {
        self.real.clear();
        self.int.clear();
        self.addr.clear();
        self.real.reserve(REAL_STACK_CAPACITY);
        self.int.reserve(INT_STACK_CAPACITY);
        self.addr.reserve(ADDR_STACK_CAPACITY);
    }
}

/// Execution stack kind used in structured runtime errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FbcStackKind {
    /// Integer value stack (`int_stack`).
    Int,
    /// Real value stack (`real_stack`).
    Real,
}

/// Structured runtime execution error for bytecode interpreter failures.
///
/// Errors are intentionally normalized into a small stable taxonomy so tests and
/// higher-level runtime wrappers can distinguish:
/// - bytecode bugs (`stack_underflow`, `missing_branch_target`),
/// - malformed factory/block references (`invalid_block_id`, `invalid_block_pc`),
/// - memory/runtime access failures (`heap_oob`, `io_oob`),
/// - and residual trapped panics (`panic_trapped`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FbcExecError {
    /// Human-readable stable error category.
    pub kind: &'static str,
    /// Opcode being executed when the error occurred.
    pub opcode: FbcOpcode,
    /// Block id of the executing bytecode block.
    pub block_id: BlockId,
    /// Program counter within the block.
    pub pc: usize,
    /// Optional stack underflow detail.
    pub stack: Option<FbcStackKind>,
    /// Optional I/O channel index related to the failure.
    pub channel: Option<usize>,
    /// Optional sample index related to the failure.
    pub sample: Option<usize>,
}

impl FbcExecError {
    /// Creates a `stack_underflow` runtime error.
    fn stack_underflow(
        opcode: FbcOpcode,
        block_id: BlockId,
        pc: usize,
        stack: FbcStackKind,
    ) -> Self {
        Self {
            kind: "stack_underflow",
            opcode,
            block_id,
            pc,
            stack: Some(stack),
            channel: None,
            sample: None,
        }
    }

    /// Creates a `missing_branch_target` runtime error.
    fn missing_branch_target(opcode: FbcOpcode, block_id: BlockId, pc: usize) -> Self {
        Self {
            kind: "missing_branch_target",
            opcode,
            block_id,
            pc,
            stack: None,
            channel: None,
            sample: None,
        }
    }

    /// Creates an `unsupported_runtime_feature` runtime error.
    #[allow(dead_code)]
    fn unsupported_runtime_feature(opcode: FbcOpcode, block_id: BlockId, pc: usize) -> Self {
        Self {
            kind: "unsupported_runtime_feature",
            opcode,
            block_id,
            pc,
            stack: None,
            channel: None,
            sample: None,
        }
    }

    /// Creates an `invalid_block_id` runtime error.
    fn invalid_block_id(opcode: FbcOpcode, block_id: BlockId, pc: usize) -> Self {
        Self {
            kind: "invalid_block_id",
            opcode,
            block_id,
            pc,
            stack: None,
            channel: None,
            sample: None,
        }
    }

    /// Creates an `invalid_block_pc` runtime error.
    fn invalid_block_pc(opcode: FbcOpcode, block_id: BlockId, pc: usize) -> Self {
        Self {
            kind: "invalid_block_pc",
            opcode,
            block_id,
            pc,
            stack: None,
            channel: None,
            sample: None,
        }
    }

    /// Creates a `heap_oob` runtime error.
    fn heap_oob(opcode: FbcOpcode, block_id: BlockId, pc: usize) -> Self {
        Self {
            kind: "heap_oob",
            opcode,
            block_id,
            pc,
            stack: None,
            channel: None,
            sample: None,
        }
    }

    /// Creates a `panic_trapped` runtime error.
    fn panic_trapped(opcode: FbcOpcode, block_id: BlockId, pc: usize) -> Self {
        Self {
            kind: "panic_trapped",
            opcode,
            block_id,
            pc,
            stack: None,
            channel: None,
            sample: None,
        }
    }

    /// Creates an `io_oob` runtime error.
    fn io_oob(
        opcode: FbcOpcode,
        block_id: BlockId,
        pc: usize,
        channel: usize,
        sample: usize,
    ) -> Self {
        Self {
            kind: "io_oob",
            opcode,
            block_id,
            pc,
            stack: None,
            channel: Some(channel),
            sample: Some(sample),
        }
    }
}

impl std::fmt::Display for FbcExecError {
    /// Formats the execution error as a compact runtime diagnostic line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.stack, self.channel, self.sample) {
            (Some(stack), _, _) => write!(
                f,
                "FBC runtime error [{}] opcode={:?} block={:?} pc={} stack={:?}",
                self.kind, self.opcode, self.block_id, self.pc, stack
            ),
            (None, Some(ch), Some(smp)) => write!(
                f,
                "FBC runtime error [{}] opcode={:?} block={:?} pc={} channel={} sample={}",
                self.kind, self.opcode, self.block_id, self.pc, ch, smp
            ),
            _ => write!(
                f,
                "FBC runtime error [{}] opcode={:?} block={:?} pc={}",
                self.kind, self.opcode, self.block_id, self.pc
            ),
        }
    }
}

impl std::error::Error for FbcExecError {}

/// Pops one REAL value from the execution stack with structured underflow
/// reporting.
fn pop_real_stack<R: FbcReal>(
    real_stack: &mut Vec<R>,
    opcode: FbcOpcode,
    block_id: BlockId,
    pc: usize,
) -> Result<R, FbcExecError> {
    real_stack
        .pop()
        .ok_or_else(|| FbcExecError::stack_underflow(opcode, block_id, pc, FbcStackKind::Real))
}

/// Pops one integer value from the execution stack with structured underflow
/// reporting.
fn pop_int_stack(
    int_stack: &mut Vec<i32>,
    opcode: FbcOpcode,
    block_id: BlockId,
    pc: usize,
) -> Result<i32, FbcExecError> {
    int_stack
        .pop()
        .ok_or_else(|| FbcExecError::stack_underflow(opcode, block_id, pc, FbcStackKind::Int))
}

/// Extracts a mandatory branch target from an instruction field.
///
/// The bytecode compiler is expected to provide all control-flow targets, so a
/// `None` here indicates malformed/generated-invalid bytecode rather than a
/// recoverable user error.
fn require_branch_target(
    target: Option<BlockId>,
    opcode: FbcOpcode,
    block_id: BlockId,
    pc: usize,
) -> Result<BlockId, FbcExecError> {
    target.ok_or_else(|| FbcExecError::missing_branch_target(opcode, block_id, pc))
}

/// Extracts a string message from a caught panic payload when possible.
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> Option<&str> {
    if let Some(msg) = payload.downcast_ref::<&'static str>() {
        Some(msg)
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        Some(msg.as_str())
    } else {
        None
    }
}

/// Maps a trapped Rust panic back into the closest structured runtime category.
///
/// This keeps the public executor API deterministic even when an internal
/// bounds check or slice panic escaped an unchecked helper.
fn classify_trapped_panic(payload: &(dyn std::any::Any + Send), site: ExecSite) -> FbcExecError {
    if let Some(msg) = panic_payload_message(payload)
        && (msg.contains("index out of bounds")
            || msg.contains("range end index")
            || msg.contains("range start index")
            || msg.contains("slice index"))
    {
        return FbcExecError::heap_oob(site.opcode, site.block_id, site.pc);
    }
    FbcExecError::panic_trapped(site.opcode, site.block_id, site.pc)
}

fn foreign_error(
    kind: &'static str,
    opcode: FbcOpcode,
    block_id: BlockId,
    pc: usize,
) -> FbcExecError {
    FbcExecError {
        kind,
        opcode,
        block_id,
        pc,
        stack: None,
        channel: None,
        sample: None,
    }
}

fn pop_foreign_arg<R: FbcReal>(
    typ: ForeignScalarType,
    real_stack: &mut Vec<R>,
    int_stack: &mut Vec<i32>,
    opcode: FbcOpcode,
    block_id: BlockId,
    pc: usize,
) -> Result<ForeignArg<R>, FbcExecError> {
    Ok(match typ {
        ForeignScalarType::Int32 => {
            ForeignArg::Int32(pop_int_stack(int_stack, opcode, block_id, pc)?)
        }
        ForeignScalarType::Float32 => {
            ForeignArg::Float32(pop_real_stack(real_stack, opcode, block_id, pc)?.to_f64() as f32)
        }
        ForeignScalarType::Float64 => {
            ForeignArg::Float64(pop_real_stack(real_stack, opcode, block_id, pc)?.to_f64())
        }
        ForeignScalarType::FaustFloat => {
            ForeignArg::FaustFloat(pop_real_stack(real_stack, opcode, block_id, pc)?)
        }
        ForeignScalarType::Bool => {
            ForeignArg::Bool(pop_int_stack(int_stack, opcode, block_id, pc)? != 0)
        }
        ForeignScalarType::Void => {
            return Err(foreign_error(
                "invalid_foreign_argument_type",
                opcode,
                block_id,
                pc,
            ));
        }
    })
}

enum ForeignArg<R: FbcReal> {
    Int32(i32),
    Float32(f32),
    Float64(f64),
    FaustFloat(R),
    Bool(bool),
}

fn invoke_foreign_call<R: FbcReal>(
    signature: &ForeignSignature,
    addr: usize,
    real_stack: &mut Vec<R>,
    int_stack: &mut Vec<i32>,
    opcode: FbcOpcode,
    block_id: BlockId,
    pc: usize,
) -> Result<(), FbcExecError> {
    let args: Vec<_> = signature
        .args
        .iter()
        .map(|&typ| pop_foreign_arg(typ, real_stack, int_stack, opcode, block_id, pc))
        .collect::<Result<_, _>>()?;
    let ret = match signature.ret {
        ForeignScalarType::Int32 => HostScalarType::Int32,
        ForeignScalarType::Float32 => HostScalarType::Float32,
        ForeignScalarType::Float64 => HostScalarType::Float64,
        ForeignScalarType::FaustFloat => {
            if R::TYPE_NAME == "f32" {
                HostScalarType::Float32
            } else {
                HostScalarType::Float64
            }
        }
        ForeignScalarType::Bool => HostScalarType::Bool,
        ForeignScalarType::Void => HostScalarType::Void,
    };
    let host_args: Vec<_> = args
        .into_iter()
        .map(|arg: ForeignArg<R>| match arg {
            ForeignArg::Int32(v) => HostValue::Int32(v),
            ForeignArg::Float32(v) => HostValue::Float32(v),
            ForeignArg::Float64(v) => HostValue::Float64(v),
            ForeignArg::FaustFloat(v) => {
                if R::TYPE_NAME == "f32" {
                    HostValue::Float32(v.to_f64() as f32)
                } else {
                    HostValue::Float64(v.to_f64())
                }
            }
            ForeignArg::Bool(v) => HostValue::Bool(v),
        })
        .collect();
    match foreign_call::invoke(addr, ret, &host_args) {
        Some(HostValue::Void) => {}
        Some(HostValue::Int32(v)) => int_stack.push(v),
        Some(HostValue::Float32(v)) => real_stack.push(R::from_f64(v as f64)),
        Some(HostValue::Float64(v)) => real_stack.push(R::from_f64(v)),
        Some(HostValue::Bool(v)) => int_stack.push(i32::from(v)),
        None => {
            return Err(foreign_error(
                "unsupported_foreign_signature",
                opcode,
                block_id,
                pc,
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
/// Last interpreter execution site used to classify trapped panics.
struct ExecSite {
    opcode: FbcOpcode,
    block_id: BlockId,
    pc: usize,
}

/// FBC bytecode execution engine.
///
/// Holds mutable state (heaps) and executes bytecode blocks from an
/// [`FbcBlockArena`]. This is the core dispatch loop, ported from C++
/// `FBCInterpreter<REAL, TRACE>::executeBlock`.
///
/// # Source provenance (C++)
/// - `FBCInterpreter<REAL, TRACE>` in `fbc_interpreter.hh`
///
/// # Memory model
/// - **Int heap** (`int_heap`): counters, indices, loop variables.
/// - **Real heap** (`real_heap`): state variables, filter memory, UI zones.
/// - **Execution stacks**: emptied at the start of each `execute_block_io`
///   call, kept across calls.
///
/// Heaps and stacks are instance-owned and reused across block executions, so
/// executing a block allocates nothing once the stacks have grown.
pub struct FbcExecutor<R: FbcReal> {
    /// Integer heap (counters, indices, loop variables).
    pub int_heap: Vec<i32>,
    /// Real heap (state variables, filters, UI zones).
    pub real_heap: Vec<R>,
    /// Soundfile slots, indexed by the slot number assigned at compile time.
    pub soundfiles: Vec<Box<Soundfile>>,
    /// Evaluation stacks, emptied per call, kept across calls.
    stacks: ExecStacks<R>,
}

impl<R: FbcReal> FbcExecutor<R> {
    /// Creates a new executor with zeroed heaps of the given sizes.
    #[must_use]
    pub fn new(int_heap_size: usize, real_heap_size: usize) -> Self {
        Self {
            int_heap: vec![0; int_heap_size],
            real_heap: vec![R::default(); real_heap_size],
            soundfiles: Vec::new(),
            stacks: ExecStacks::default(),
        }
    }

    /// Executes a block without audio I/O (for init, clear, control blocks).
    ///
    /// This convenience wrapper preserves the historical panic-on-bug behavior
    /// of the C++ interpreter.
    pub fn execute_block(&mut self, arena: &FbcBlockArena<R>, block_id: BlockId) {
        self.execute_block_io(arena, block_id, &[], &mut []);
    }

    /// Executes a block without audio I/O (for init, clear, control blocks),
    /// returning a structured runtime error instead of panicking.
    pub fn try_execute_block(
        &mut self,
        arena: &FbcBlockArena<R>,
        block_id: BlockId,
    ) -> Result<(), FbcExecError> {
        self.try_execute_block_io(arena, block_id, &[], &mut [])
    }

    /// Executes a block with audio I/O (for compute blocks).
    ///
    /// # Arguments
    /// - `arena`: the block arena containing all compiled blocks.
    /// - `block_id`: the block to execute.
    /// - `inputs`: audio input buffers (`inputs[channel][sample]`).
    /// - `outputs`: audio output buffers (`outputs[channel][sample]`).
    ///
    /// # Panics
    /// Panics on stack underflow, out-of-bounds heap access, or missing branch
    /// targets. These indicate bugs in bytecode generation, not user errors.
    #[allow(clippy::too_many_lines)]
    pub fn execute_block_io(
        &mut self,
        arena: &FbcBlockArena<R>,
        block_id: BlockId,
        inputs: &[&[R]],
        outputs: &mut [&mut [R]],
    ) {
        let mut last_site = ExecSite {
            opcode: FbcOpcode::Nop,
            block_id,
            pc: 0,
        };
        self.try_execute_block_io_inner(arena, block_id, inputs, outputs, &mut last_site)
            .unwrap_or_else(|e| panic!("{e}"));
    }

    /// Executes a block with audio I/O and returns a structured runtime error
    /// instead of panicking.
    ///
    /// This is the preferred surface for tests and differential harnesses that
    /// want to classify interpreter failures without aborting the process.
    pub fn try_execute_block_io(
        &mut self,
        arena: &FbcBlockArena<R>,
        block_id: BlockId,
        inputs: &[&[R]],
        outputs: &mut [&mut [R]],
    ) -> Result<(), FbcExecError> {
        let mut last_site = ExecSite {
            opcode: FbcOpcode::Nop,
            block_id,
            pc: 0,
        };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.try_execute_block_io_inner(arena, block_id, inputs, outputs, &mut last_site)
        })) {
            Ok(result) => result,
            Err(payload) => Err(classify_trapped_panic(&*payload, last_site)),
        }
    }

    /// Runs one block on the executor's stacks, emptied first. The stacks
    /// leave `self` for the run, since the dispatch loop borrows both; a
    /// panic drops them, and the next run allocates them again.
    fn try_execute_block_io_inner(
        &mut self,
        arena: &FbcBlockArena<R>,
        block_id: BlockId,
        inputs: &[&[R]],
        outputs: &mut [&mut [R]],
        last_site: &mut ExecSite,
    ) -> Result<(), FbcExecError> {
        let mut stacks = std::mem::take(&mut self.stacks);
        stacks.prepare();
        let result = self.run_block(arena, block_id, inputs, outputs, last_site, &mut stacks);
        self.stacks = stacks;
        result
    }

    #[allow(clippy::too_many_lines)]
    /// Core dispatch loop shared by panic-on-error and structured-error entry
    /// points, on empty `stacks`.
    fn run_block(
        &mut self,
        arena: &FbcBlockArena<R>,
        block_id: BlockId,
        inputs: &[&[R]],
        outputs: &mut [&mut [R]],
        last_site: &mut ExecSite,
        stacks: &mut ExecStacks<R>,
    ) -> Result<(), FbcExecError> {
        use FbcOpcode::*;

        let ExecStacks {
            real: real_stack,
            int: int_stack,
            addr: addr_stack,
        } = stacks;

        // Current execution position.
        let mut cur_block = block_id;
        let mut pc: usize = 0;

        loop {
            let block = arena
                .try_get(cur_block)
                .ok_or_else(|| FbcExecError::invalid_block_id(last_site.opcode, cur_block, pc))?;
            let instr = block
                .instructions
                .get(pc)
                .ok_or_else(|| FbcExecError::invalid_block_pc(last_site.opcode, cur_block, pc))?;
            *last_site = ExecSite {
                opcode: instr.opcode,
                block_id: cur_block,
                pc,
            };

            // Pre-extract commonly used instruction fields.
            let o1 = instr.offset1 as usize;
            let o2 = instr.offset2 as usize;
            let iv = instr.int_value;
            let rv = instr.real_value;

            match instr.opcode {
                // ── Numbers ─────────────────────────────────────────────
                RealValue => {
                    real_stack.push(rv);
                    pc += 1;
                }
                Int32Value => {
                    int_stack.push(iv);
                    pc += 1;
                }

                // ── Memory: load/store ──────────────────────────────────
                LoadReal => {
                    real_stack.push(self.real_heap[o1]);
                    pc += 1;
                }
                LoadInt => {
                    int_stack.push(self.int_heap[o1]);
                    pc += 1;
                }
                LoadSoundFieldInt => {
                    // offset1 = soundfile slot index; int_value = field selector
                    // (0 = fLength, 1 = fSR); pops part from int stack.
                    let part = pop_int_stack(int_stack, instr.opcode, cur_block, pc)? as usize;
                    let sf = self
                        .soundfiles
                        .get(o1)
                        .ok_or_else(|| FbcExecError::heap_oob(instr.opcode, cur_block, pc))?;
                    let val = match iv {
                        0 => sf.lengths.get(part).copied().unwrap_or(0),
                        1 => sf.sample_rates.get(part).copied().unwrap_or(44100),
                        _ => 0,
                    };
                    int_stack.push(val);
                    pc += 1;
                }
                LoadSoundFieldReal => {
                    // offset1 = soundfile slot index.
                    // Pops idx, part, chan from int stack (LIFO: idx on top).
                    let idx = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let part = pop_int_stack(int_stack, instr.opcode, cur_block, pc)? as usize;
                    let chan = pop_int_stack(int_stack, instr.opcode, cur_block, pc)? as usize;
                    let sf = self
                        .soundfiles
                        .get(o1)
                        .ok_or_else(|| FbcExecError::heap_oob(instr.opcode, cur_block, pc))?;
                    let sample = sf.read_sample(chan, part, idx);
                    real_stack.push(R::from_f64(sample));
                    pc += 1;
                }
                StoreReal => {
                    self.real_heap[o1] = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    pc += 1;
                }
                StoreInt => {
                    self.int_heap[o1] = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    pc += 1;
                }
                StoreRealValue => {
                    self.real_heap[o1] = rv;
                    pc += 1;
                }
                StoreIntValue => {
                    self.int_heap[o1] = iv;
                    pc += 1;
                }
                LoadIndexedReal => {
                    let offset = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1.wrapping_add(offset as usize)]);
                    pc += 1;
                }
                LoadIndexedInt => {
                    let offset = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1.wrapping_add(offset as usize)]);
                    pc += 1;
                }
                StoreIndexedReal => {
                    let offset = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let val = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    self.real_heap[o1.wrapping_add(offset as usize)] = val;
                    pc += 1;
                }
                StoreIndexedInt => {
                    let offset = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let val = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    self.int_heap[o1.wrapping_add(offset as usize)] = val;
                    pc += 1;
                }
                BlockStoreReal => {
                    if let Some(BlockStoreData::Real(table)) = instr.block_store.as_ref() {
                        let count = o2;
                        self.real_heap[o1..(count + o1)].copy_from_slice(&table[..count]);
                    }
                    pc += 1;
                }
                BlockStoreInt => {
                    if let Some(BlockStoreData::Int(table)) = instr.block_store.as_ref() {
                        let count = o2;
                        self.int_heap[o1..(count + o1)].copy_from_slice(&table[..count]);
                    }
                    pc += 1;
                }
                MoveReal => {
                    self.real_heap[o1] = self.real_heap[o2];
                    pc += 1;
                }
                MoveInt => {
                    self.int_heap[o1] = self.int_heap[o2];
                    pc += 1;
                }
                PairMoveReal => {
                    self.real_heap[o1] = self.real_heap[o1 - 1];
                    self.real_heap[o2] = self.real_heap[o2 - 1];
                    pc += 1;
                }
                PairMoveInt => {
                    self.int_heap[o1] = self.int_heap[o1 - 1];
                    self.int_heap[o2] = self.int_heap[o2 - 1];
                    pc += 1;
                }
                BlockPairMoveReal => {
                    let mut i = o1;
                    while i < o2 {
                        self.real_heap[i + 1] = self.real_heap[i];
                        i += 2;
                    }
                    pc += 1;
                }
                BlockPairMoveInt => {
                    let mut i = o1;
                    while i < o2 {
                        self.int_heap[i + 1] = self.int_heap[i];
                        i += 2;
                    }
                    pc += 1;
                }
                BlockShiftReal => {
                    let mut i = o1;
                    while i > o2 {
                        self.real_heap[i] = self.real_heap[i - 1];
                        i -= 1;
                    }
                    pc += 1;
                }
                BlockShiftInt => {
                    let mut i = o1;
                    while i > o2 {
                        self.int_heap[i] = self.int_heap[i - 1];
                        i -= 1;
                    }
                    pc += 1;
                }

                // ── I/O ─────────────────────────────────────────────────
                LoadInput => {
                    let sample_idx =
                        pop_int_stack(int_stack, instr.opcode, cur_block, pc)? as usize;
                    let channel = inputs.get(o1).ok_or_else(|| {
                        FbcExecError::io_oob(instr.opcode, cur_block, pc, o1, sample_idx)
                    })?;
                    let sample = channel.get(sample_idx).ok_or_else(|| {
                        FbcExecError::io_oob(instr.opcode, cur_block, pc, o1, sample_idx)
                    })?;
                    real_stack.push(*sample);
                    pc += 1;
                }
                LoadOutput => {
                    let sample_idx =
                        pop_int_stack(int_stack, instr.opcode, cur_block, pc)? as usize;
                    let channel = outputs.get(o1).ok_or_else(|| {
                        FbcExecError::io_oob(instr.opcode, cur_block, pc, o1, sample_idx)
                    })?;
                    let sample = channel.get(sample_idx).ok_or_else(|| {
                        FbcExecError::io_oob(instr.opcode, cur_block, pc, o1, sample_idx)
                    })?;
                    real_stack.push(*sample);
                    pc += 1;
                }
                StoreOutput => {
                    let sample_idx =
                        pop_int_stack(int_stack, instr.opcode, cur_block, pc)? as usize;
                    let val = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let channel = outputs.get_mut(o1).ok_or_else(|| {
                        FbcExecError::io_oob(instr.opcode, cur_block, pc, o1, sample_idx)
                    })?;
                    let slot = channel.get_mut(sample_idx).ok_or_else(|| {
                        FbcExecError::io_oob(instr.opcode, cur_block, pc, o1, sample_idx)
                    })?;
                    *slot = val;
                    pc += 1;
                }

                // ── Cast / Bitcast ──────────────────────────────────────
                CastReal => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(R::from_i32(v));
                    pc += 1;
                }
                CastInt => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v.to_i32());
                    pc += 1;
                }
                CastRealHeap => {
                    real_stack.push(R::from_i32(self.int_heap[o1]));
                    pc += 1;
                }
                CastIntHeap => {
                    int_stack.push(self.real_heap[o1].to_i32());
                    pc += 1;
                }
                BitcastInt => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v.to_bits_i32());
                    pc += 1;
                }
                BitcastReal => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(R::from_bits_i32(v));
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Standard math: stack OP stack
                // ═══════════════════════════════════════════════════════

                // ── Real arithmetic ─────────────────────────────────────
                AddReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1 + v2);
                    pc += 1;
                }
                SubReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1 - v2);
                    pc += 1;
                }
                MultReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1 * v2);
                    pc += 1;
                }
                DivReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1 / v2);
                    pc += 1;
                }
                RemReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1.fbc_remainder(v2));
                    pc += 1;
                }

                // ── Int arithmetic ──────────────────────────────────────
                AddInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1.wrapping_add(v2));
                    pc += 1;
                }
                SubInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1.wrapping_sub(v2));
                    pc += 1;
                }
                MultInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1.wrapping_mul(v2));
                    pc += 1;
                }
                DivInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(if v2 != 0 { v1.wrapping_div(v2) } else { 0 });
                    pc += 1;
                }
                RemInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(if v2 != 0 { v1.wrapping_rem(v2) } else { 0 });
                    pc += 1;
                }

                // ── Int shifts ──────────────────────────────────────────
                LshInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1.wrapping_shl(v2 as u32));
                    pc += 1;
                }
                ARshInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1.wrapping_shr(v2 as u32));
                    pc += 1;
                }
                LRshInt => {
                    // Logical right shift: cast to unsigned, shift, cast back.
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 as u32).wrapping_shr(v2 as u32) as i32);
                    pc += 1;
                }

                // ── Int comparisons ─────────────────────────────────────
                GTInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 > v2) as i32);
                    pc += 1;
                }
                LTInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 < v2) as i32);
                    pc += 1;
                }
                GEInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 >= v2) as i32);
                    pc += 1;
                }
                LEInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 <= v2) as i32);
                    pc += 1;
                }
                EQInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 == v2) as i32);
                    pc += 1;
                }
                NEInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 != v2) as i32);
                    pc += 1;
                }

                // ── Real comparisons → int ──────────────────────────────
                GTReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 > v2) as i32);
                    pc += 1;
                }
                LTReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 < v2) as i32);
                    pc += 1;
                }
                GEReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 >= v2) as i32);
                    pc += 1;
                }
                LEReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 <= v2) as i32);
                    pc += 1;
                }
                EQReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 == v2) as i32);
                    pc += 1;
                }
                NEReal => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((v1 != v2) as i32);
                    pc += 1;
                }

                // ── Int logical ─────────────────────────────────────────
                ANDInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1 & v2);
                    pc += 1;
                }
                ORInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1 | v2);
                    pc += 1;
                }
                XORInt => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1 ^ v2);
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Standard math: heap OP heap
                // ═══════════════════════════════════════════════════════
                AddRealHeap => {
                    real_stack.push(self.real_heap[o1] + self.real_heap[o2]);
                    pc += 1;
                }
                SubRealHeap => {
                    real_stack.push(self.real_heap[o1] - self.real_heap[o2]);
                    pc += 1;
                }
                MultRealHeap => {
                    real_stack.push(self.real_heap[o1] * self.real_heap[o2]);
                    pc += 1;
                }
                DivRealHeap => {
                    real_stack.push(self.real_heap[o1] / self.real_heap[o2]);
                    pc += 1;
                }
                RemRealHeap => {
                    real_stack.push(self.real_heap[o1].fbc_remainder(self.real_heap[o2]));
                    pc += 1;
                }

                AddIntHeap => {
                    int_stack.push(self.int_heap[o1].wrapping_add(self.int_heap[o2]));
                    pc += 1;
                }
                SubIntHeap => {
                    int_stack.push(self.int_heap[o1].wrapping_sub(self.int_heap[o2]));
                    pc += 1;
                }
                MultIntHeap => {
                    int_stack.push(self.int_heap[o1].wrapping_mul(self.int_heap[o2]));
                    pc += 1;
                }
                DivIntHeap => {
                    let d = self.int_heap[o2];
                    int_stack.push(if d != 0 {
                        self.int_heap[o1].wrapping_div(d)
                    } else {
                        0
                    });
                    pc += 1;
                }
                RemIntHeap => {
                    let d = self.int_heap[o2];
                    int_stack.push(if d != 0 {
                        self.int_heap[o1].wrapping_rem(d)
                    } else {
                        0
                    });
                    pc += 1;
                }

                LshIntHeap => {
                    int_stack.push(self.int_heap[o1].wrapping_shl(self.int_heap[o2] as u32));
                    pc += 1;
                }
                ARshIntHeap => {
                    int_stack.push(self.int_heap[o1].wrapping_shr(self.int_heap[o2] as u32));
                    pc += 1;
                }
                LRshIntHeap => {
                    int_stack.push(
                        (self.int_heap[o1] as u32).wrapping_shr(self.int_heap[o2] as u32) as i32,
                    );
                    pc += 1;
                }

                GTIntHeap => {
                    int_stack.push((self.int_heap[o1] > self.int_heap[o2]) as i32);
                    pc += 1;
                }
                LTIntHeap => {
                    int_stack.push((self.int_heap[o1] < self.int_heap[o2]) as i32);
                    pc += 1;
                }
                GEIntHeap => {
                    int_stack.push((self.int_heap[o1] >= self.int_heap[o2]) as i32);
                    pc += 1;
                }
                LEIntHeap => {
                    int_stack.push((self.int_heap[o1] <= self.int_heap[o2]) as i32);
                    pc += 1;
                }
                EQIntHeap => {
                    int_stack.push((self.int_heap[o1] == self.int_heap[o2]) as i32);
                    pc += 1;
                }
                NEIntHeap => {
                    int_stack.push((self.int_heap[o1] != self.int_heap[o2]) as i32);
                    pc += 1;
                }

                GTRealHeap => {
                    int_stack.push((self.real_heap[o1] > self.real_heap[o2]) as i32);
                    pc += 1;
                }
                LTRealHeap => {
                    int_stack.push((self.real_heap[o1] < self.real_heap[o2]) as i32);
                    pc += 1;
                }
                GERealHeap => {
                    int_stack.push((self.real_heap[o1] >= self.real_heap[o2]) as i32);
                    pc += 1;
                }
                LERealHeap => {
                    int_stack.push((self.real_heap[o1] <= self.real_heap[o2]) as i32);
                    pc += 1;
                }
                EQRealHeap => {
                    int_stack.push((self.real_heap[o1] == self.real_heap[o2]) as i32);
                    pc += 1;
                }
                NERealHeap => {
                    int_stack.push((self.real_heap[o1] != self.real_heap[o2]) as i32);
                    pc += 1;
                }

                ANDIntHeap => {
                    int_stack.push(self.int_heap[o1] & self.int_heap[o2]);
                    pc += 1;
                }
                ORIntHeap => {
                    int_stack.push(self.int_heap[o1] | self.int_heap[o2]);
                    pc += 1;
                }
                XORIntHeap => {
                    int_stack.push(self.int_heap[o1] ^ self.int_heap[o2]);
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Standard math: heap OP stack
                // ═══════════════════════════════════════════════════════
                AddRealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1] + v);
                    pc += 1;
                }
                SubRealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1] - v);
                    pc += 1;
                }
                MultRealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1] * v);
                    pc += 1;
                }
                DivRealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1] / v);
                    pc += 1;
                }
                RemRealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1].fbc_remainder(v));
                    pc += 1;
                }

                AddIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1].wrapping_add(v));
                    pc += 1;
                }
                SubIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1].wrapping_sub(v));
                    pc += 1;
                }
                MultIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1].wrapping_mul(v));
                    pc += 1;
                }
                DivIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(if v != 0 {
                        self.int_heap[o1].wrapping_div(v)
                    } else {
                        0
                    });
                    pc += 1;
                }
                RemIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(if v != 0 {
                        self.int_heap[o1].wrapping_rem(v)
                    } else {
                        0
                    });
                    pc += 1;
                }

                LshIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1].wrapping_shl(v as u32));
                    pc += 1;
                }
                ARshIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1].wrapping_shr(v as u32));
                    pc += 1;
                }
                LRshIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.int_heap[o1] as u32).wrapping_shr(v as u32) as i32);
                    pc += 1;
                }

                GTIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.int_heap[o1] > v) as i32);
                    pc += 1;
                }
                LTIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.int_heap[o1] < v) as i32);
                    pc += 1;
                }
                GEIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.int_heap[o1] >= v) as i32);
                    pc += 1;
                }
                LEIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.int_heap[o1] <= v) as i32);
                    pc += 1;
                }
                EQIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.int_heap[o1] == v) as i32);
                    pc += 1;
                }
                NEIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.int_heap[o1] != v) as i32);
                    pc += 1;
                }

                GTRealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.real_heap[o1] > v) as i32);
                    pc += 1;
                }
                LTRealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.real_heap[o1] < v) as i32);
                    pc += 1;
                }
                GERealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.real_heap[o1] >= v) as i32);
                    pc += 1;
                }
                LERealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.real_heap[o1] <= v) as i32);
                    pc += 1;
                }
                EQRealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.real_heap[o1] == v) as i32);
                    pc += 1;
                }
                NERealStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((self.real_heap[o1] != v) as i32);
                    pc += 1;
                }

                ANDIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1] & v);
                    pc += 1;
                }
                ORIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1] | v);
                    pc += 1;
                }
                XORIntStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1] ^ v);
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Standard math: value OP stack
                // ═══════════════════════════════════════════════════════
                AddRealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(rv + v);
                    pc += 1;
                }
                SubRealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(rv - v);
                    pc += 1;
                }
                MultRealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(rv * v);
                    pc += 1;
                }
                DivRealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(rv / v);
                    pc += 1;
                }
                RemRealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(rv.fbc_remainder(v));
                    pc += 1;
                }

                AddIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv.wrapping_add(v));
                    pc += 1;
                }
                SubIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv.wrapping_sub(v));
                    pc += 1;
                }
                MultIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv.wrapping_mul(v));
                    pc += 1;
                }
                DivIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(if v != 0 { iv.wrapping_div(v) } else { 0 });
                    pc += 1;
                }
                RemIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(if v != 0 { iv.wrapping_rem(v) } else { 0 });
                    pc += 1;
                }

                LshIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv.wrapping_shl(v as u32));
                    pc += 1;
                }
                ARshIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv.wrapping_shr(v as u32));
                    pc += 1;
                }
                LRshIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((iv as u32).wrapping_shr(v as u32) as i32);
                    pc += 1;
                }

                GTIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((iv > v) as i32);
                    pc += 1;
                }
                LTIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((iv < v) as i32);
                    pc += 1;
                }
                GEIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((iv >= v) as i32);
                    pc += 1;
                }
                LEIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((iv <= v) as i32);
                    pc += 1;
                }
                EQIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((iv == v) as i32);
                    pc += 1;
                }
                NEIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((iv != v) as i32);
                    pc += 1;
                }

                GTRealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((rv > v) as i32);
                    pc += 1;
                }
                LTRealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((rv < v) as i32);
                    pc += 1;
                }
                GERealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((rv >= v) as i32);
                    pc += 1;
                }
                LERealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((rv <= v) as i32);
                    pc += 1;
                }
                EQRealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((rv == v) as i32);
                    pc += 1;
                }
                NERealStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push((rv != v) as i32);
                    pc += 1;
                }

                ANDIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv & v);
                    pc += 1;
                }
                ORIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv | v);
                    pc += 1;
                }
                XORIntStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv ^ v);
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Standard math: value OP heap
                // ═══════════════════════════════════════════════════════
                AddRealValue => {
                    real_stack.push(rv + self.real_heap[o1]);
                    pc += 1;
                }
                SubRealValue => {
                    real_stack.push(rv - self.real_heap[o1]);
                    pc += 1;
                }
                MultRealValue => {
                    real_stack.push(rv * self.real_heap[o1]);
                    pc += 1;
                }
                DivRealValue => {
                    real_stack.push(rv / self.real_heap[o1]);
                    pc += 1;
                }
                RemRealValue => {
                    real_stack.push(rv.fbc_remainder(self.real_heap[o1]));
                    pc += 1;
                }

                AddIntValue => {
                    int_stack.push(iv.wrapping_add(self.int_heap[o1]));
                    pc += 1;
                }
                SubIntValue => {
                    int_stack.push(iv.wrapping_sub(self.int_heap[o1]));
                    pc += 1;
                }
                MultIntValue => {
                    int_stack.push(iv.wrapping_mul(self.int_heap[o1]));
                    pc += 1;
                }
                DivIntValue => {
                    let d = self.int_heap[o1];
                    int_stack.push(if d != 0 { iv.wrapping_div(d) } else { 0 });
                    pc += 1;
                }
                RemIntValue => {
                    let d = self.int_heap[o1];
                    int_stack.push(if d != 0 { iv.wrapping_rem(d) } else { 0 });
                    pc += 1;
                }

                LshIntValue => {
                    int_stack.push(iv.wrapping_shl(self.int_heap[o1] as u32));
                    pc += 1;
                }
                ARshIntValue => {
                    int_stack.push(iv.wrapping_shr(self.int_heap[o1] as u32));
                    pc += 1;
                }
                LRshIntValue => {
                    int_stack.push((iv as u32).wrapping_shr(self.int_heap[o1] as u32) as i32);
                    pc += 1;
                }

                GTIntValue => {
                    int_stack.push((iv > self.int_heap[o1]) as i32);
                    pc += 1;
                }
                LTIntValue => {
                    int_stack.push((iv < self.int_heap[o1]) as i32);
                    pc += 1;
                }
                GEIntValue => {
                    int_stack.push((iv >= self.int_heap[o1]) as i32);
                    pc += 1;
                }
                LEIntValue => {
                    int_stack.push((iv <= self.int_heap[o1]) as i32);
                    pc += 1;
                }
                EQIntValue => {
                    int_stack.push((iv == self.int_heap[o1]) as i32);
                    pc += 1;
                }
                NEIntValue => {
                    int_stack.push((iv != self.int_heap[o1]) as i32);
                    pc += 1;
                }

                GTRealValue => {
                    int_stack.push((rv > self.real_heap[o1]) as i32);
                    pc += 1;
                }
                LTRealValue => {
                    int_stack.push((rv < self.real_heap[o1]) as i32);
                    pc += 1;
                }
                GERealValue => {
                    int_stack.push((rv >= self.real_heap[o1]) as i32);
                    pc += 1;
                }
                LERealValue => {
                    int_stack.push((rv <= self.real_heap[o1]) as i32);
                    pc += 1;
                }
                EQRealValue => {
                    int_stack.push((rv == self.real_heap[o1]) as i32);
                    pc += 1;
                }
                NERealValue => {
                    int_stack.push((rv != self.real_heap[o1]) as i32);
                    pc += 1;
                }

                ANDIntValue => {
                    int_stack.push(iv & self.int_heap[o1]);
                    pc += 1;
                }
                ORIntValue => {
                    int_stack.push(iv | self.int_heap[o1]);
                    pc += 1;
                }
                XORIntValue => {
                    int_stack.push(iv ^ self.int_heap[o1]);
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Standard math: value OP heap inverted (non-commutative)
                // ═══════════════════════════════════════════════════════
                SubRealValueInvert => {
                    real_stack.push(self.real_heap[o1] - rv);
                    pc += 1;
                }
                SubIntValueInvert => {
                    int_stack.push(self.int_heap[o1].wrapping_sub(iv));
                    pc += 1;
                }
                DivRealValueInvert => {
                    real_stack.push(self.real_heap[o1] / rv);
                    pc += 1;
                }
                DivIntValueInvert => {
                    int_stack.push(if iv != 0 {
                        self.int_heap[o1].wrapping_div(iv)
                    } else {
                        0
                    });
                    pc += 1;
                }
                RemRealValueInvert => {
                    real_stack.push(self.real_heap[o1].fbc_remainder(rv));
                    pc += 1;
                }
                RemIntValueInvert => {
                    int_stack.push(if iv != 0 {
                        self.int_heap[o1].wrapping_rem(iv)
                    } else {
                        0
                    });
                    pc += 1;
                }

                LshIntValueInvert => {
                    int_stack.push(self.int_heap[o1].wrapping_shl(iv as u32));
                    pc += 1;
                }
                ARshIntValueInvert => {
                    int_stack.push(self.int_heap[o1].wrapping_shr(iv as u32));
                    pc += 1;
                }
                LRshIntValueInvert => {
                    int_stack.push((self.int_heap[o1] as u32).wrapping_shr(iv as u32) as i32);
                    pc += 1;
                }

                GTIntValueInvert => {
                    int_stack.push((self.int_heap[o1] > iv) as i32);
                    pc += 1;
                }
                LTIntValueInvert => {
                    int_stack.push((self.int_heap[o1] < iv) as i32);
                    pc += 1;
                }
                GEIntValueInvert => {
                    int_stack.push((self.int_heap[o1] >= iv) as i32);
                    pc += 1;
                }
                LEIntValueInvert => {
                    int_stack.push((self.int_heap[o1] <= iv) as i32);
                    pc += 1;
                }

                GTRealValueInvert => {
                    int_stack.push((self.real_heap[o1] > rv) as i32);
                    pc += 1;
                }
                LTRealValueInvert => {
                    int_stack.push((self.real_heap[o1] < rv) as i32);
                    pc += 1;
                }
                GERealValueInvert => {
                    int_stack.push((self.real_heap[o1] >= rv) as i32);
                    pc += 1;
                }
                LERealValueInvert => {
                    int_stack.push((self.real_heap[o1] <= rv) as i32);
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Extended unary math (stack)
                // ═══════════════════════════════════════════════════════
                Abs => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v.wrapping_abs());
                    pc += 1;
                }
                Absf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_absf());
                    pc += 1;
                }
                Acosf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_acos());
                    pc += 1;
                }
                Acoshf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_acosh());
                    pc += 1;
                }
                Asinf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_asin());
                    pc += 1;
                }
                Asinhf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_asinh());
                    pc += 1;
                }
                Atanf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_atan());
                    pc += 1;
                }
                Atanhf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_atanh());
                    pc += 1;
                }
                Ceilf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_ceil());
                    pc += 1;
                }
                Cosf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_cos());
                    pc += 1;
                }
                Coshf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_cosh());
                    pc += 1;
                }
                Expf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_exp());
                    pc += 1;
                }
                Floorf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_floor());
                    pc += 1;
                }
                Logf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_log());
                    pc += 1;
                }
                Log10f => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_log10());
                    pc += 1;
                }
                Rintf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_rint());
                    pc += 1;
                }
                Roundf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_round());
                    pc += 1;
                }
                Sinf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_sin());
                    pc += 1;
                }
                Sinhf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_sinh());
                    pc += 1;
                }
                Sqrtf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_sqrt());
                    pc += 1;
                }
                Tanf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_tan());
                    pc += 1;
                }
                Tanhf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v.fbc_tanh());
                    pc += 1;
                }
                Isnanf => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v.fbc_is_nan() as i32);
                    pc += 1;
                }
                Isinff => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v.fbc_is_infinite() as i32);
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Extended unary math (heap)
                // ═══════════════════════════════════════════════════════
                AbsHeap => {
                    int_stack.push(self.int_heap[o1].wrapping_abs());
                    pc += 1;
                }
                AbsfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_absf());
                    pc += 1;
                }
                AcosfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_acos());
                    pc += 1;
                }
                AcoshfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_acosh());
                    pc += 1;
                }
                AsinfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_asin());
                    pc += 1;
                }
                AsinhfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_asinh());
                    pc += 1;
                }
                AtanfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_atan());
                    pc += 1;
                }
                AtanhfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_atanh());
                    pc += 1;
                }
                CeilfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_ceil());
                    pc += 1;
                }
                CosfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_cos());
                    pc += 1;
                }
                CoshfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_cosh());
                    pc += 1;
                }
                ExpfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_exp());
                    pc += 1;
                }
                FloorfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_floor());
                    pc += 1;
                }
                LogfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_log());
                    pc += 1;
                }
                Log10fHeap => {
                    real_stack.push(self.real_heap[o1].fbc_log10());
                    pc += 1;
                }
                RintfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_rint());
                    pc += 1;
                }
                RoundfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_round());
                    pc += 1;
                }
                SinfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_sin());
                    pc += 1;
                }
                SinhfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_sinh());
                    pc += 1;
                }
                SqrtfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_sqrt());
                    pc += 1;
                }
                TanfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_tan());
                    pc += 1;
                }
                TanhfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_tanh());
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Extended binary math (stack OP stack)
                // ═══════════════════════════════════════════════════════
                Atan2f => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1.fbc_atan2(v2));
                    pc += 1;
                }
                Fmodf => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1.fbc_fmod(v2));
                    pc += 1;
                }
                Powf => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1.fbc_pow(v2));
                    pc += 1;
                }
                Max => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1.max(v2));
                    pc += 1;
                }
                Maxf => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    // Match C++ std::max semantics: (a < b) ? b : a
                    real_stack.push(if v1 < v2 { v2 } else { v1 });
                    pc += 1;
                }
                Min => {
                    let v1 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(v1.min(v2));
                    pc += 1;
                }
                Minf => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    // Match C++ std::min semantics: (b < a) ? b : a
                    real_stack.push(if v2 < v1 { v2 } else { v1 });
                    pc += 1;
                }
                Copysignf => {
                    let v1 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let v2 = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(v1.fbc_copysign(v2));
                    pc += 1;
                }
                ForeignCallReal | ForeignCallInt | ForeignCallVoid => {
                    let signature = ForeignSignature::decode(&instr.name).ok_or_else(|| {
                        foreign_error("invalid_foreign_signature", instr.opcode, cur_block, pc)
                    })?;
                    let addr = lookup_foreign_function(&signature.name).ok_or_else(|| {
                        foreign_error(
                            "missing_foreign_function_binding",
                            instr.opcode,
                            cur_block,
                            pc,
                        )
                    })?;
                    invoke_foreign_call(
                        &signature,
                        addr,
                        real_stack,
                        int_stack,
                        instr.opcode,
                        cur_block,
                        pc,
                    )?;
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Extended binary math (heap OP heap)
                // ═══════════════════════════════════════════════════════
                Atan2fHeap => {
                    real_stack.push(self.real_heap[o1].fbc_atan2(self.real_heap[o2]));
                    pc += 1;
                }
                FmodfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_fmod(self.real_heap[o2]));
                    pc += 1;
                }
                PowfHeap => {
                    real_stack.push(self.real_heap[o1].fbc_pow(self.real_heap[o2]));
                    pc += 1;
                }
                MaxHeap => {
                    int_stack.push(self.int_heap[o1].max(self.int_heap[o2]));
                    pc += 1;
                }
                MaxfHeap => {
                    let (a, b) = (self.real_heap[o1], self.real_heap[o2]);
                    real_stack.push(if a < b { b } else { a });
                    pc += 1;
                }
                MinHeap => {
                    int_stack.push(self.int_heap[o1].min(self.int_heap[o2]));
                    pc += 1;
                }
                MinfHeap => {
                    let (a, b) = (self.real_heap[o1], self.real_heap[o2]);
                    real_stack.push(if b < a { b } else { a });
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Extended binary math (heap OP stack)
                // ═══════════════════════════════════════════════════════
                Atan2fStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1].fbc_atan2(v));
                    pc += 1;
                }
                FmodfStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1].fbc_fmod(v));
                    pc += 1;
                }
                PowfStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(self.real_heap[o1].fbc_pow(v));
                    pc += 1;
                }
                MaxStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1].max(v));
                    pc += 1;
                }
                MaxfStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let a = self.real_heap[o1];
                    real_stack.push(if a < v { v } else { a });
                    pc += 1;
                }
                MinStack => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(self.int_heap[o1].min(v));
                    pc += 1;
                }
                MinfStack => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    let a = self.real_heap[o1];
                    real_stack.push(if v < a { v } else { a });
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Extended binary math (value OP stack)
                // ═══════════════════════════════════════════════════════
                Atan2fStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(rv.fbc_atan2(v));
                    pc += 1;
                }
                FmodfStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(rv.fbc_fmod(v));
                    pc += 1;
                }
                PowfStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(rv.fbc_pow(v));
                    pc += 1;
                }
                MaxStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv.max(v));
                    pc += 1;
                }
                MaxfStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(if rv < v { v } else { rv });
                    pc += 1;
                }
                MinStackValue => {
                    let v = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    int_stack.push(iv.min(v));
                    pc += 1;
                }
                MinfStackValue => {
                    let v = pop_real_stack(real_stack, instr.opcode, cur_block, pc)?;
                    real_stack.push(if v < rv { v } else { rv });
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Extended binary math (value OP heap)
                // ═══════════════════════════════════════════════════════
                Atan2fValue => {
                    real_stack.push(rv.fbc_atan2(self.real_heap[o1]));
                    pc += 1;
                }
                FmodfValue => {
                    real_stack.push(rv.fbc_fmod(self.real_heap[o1]));
                    pc += 1;
                }
                PowfValue => {
                    real_stack.push(rv.fbc_pow(self.real_heap[o1]));
                    pc += 1;
                }
                MaxValue => {
                    int_stack.push(iv.max(self.int_heap[o1]));
                    pc += 1;
                }
                MaxfValue => {
                    let h = self.real_heap[o1];
                    real_stack.push(if rv < h { h } else { rv });
                    pc += 1;
                }
                MinValue => {
                    int_stack.push(iv.min(self.int_heap[o1]));
                    pc += 1;
                }
                MinfValue => {
                    let h = self.real_heap[o1];
                    real_stack.push(if h < rv { h } else { rv });
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Extended binary math (value OP heap inverted)
                // ═══════════════════════════════════════════════════════
                Atan2fValueInvert => {
                    real_stack.push(self.real_heap[o1].fbc_atan2(rv));
                    pc += 1;
                }
                FmodfValueInvert => {
                    real_stack.push(self.real_heap[o1].fbc_fmod(rv));
                    pc += 1;
                }
                PowfValueInvert => {
                    real_stack.push(self.real_heap[o1].fbc_pow(rv));
                    pc += 1;
                }

                // ═══════════════════════════════════════════════════════
                // Control flow
                // ═══════════════════════════════════════════════════════
                Return => {
                    if let Some((saved_block, saved_pc)) = addr_stack.pop() {
                        cur_block = saved_block;
                        pc = saved_pc;
                    } else {
                        // Empty address stack = end of execution.
                        return Ok(());
                    }
                }

                If => {
                    // Save return address (instruction after If).
                    addr_stack.push((cur_block, pc + 1));
                    let cond = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    if cond != 0 {
                        cur_block =
                            require_branch_target(instr.branch1, instr.opcode, cur_block, pc)?;
                    } else {
                        cur_block =
                            require_branch_target(instr.branch2, instr.opcode, cur_block, pc)?;
                    }
                    pc = 0;
                }

                SelectReal | SelectInt => {
                    // Save return address.
                    addr_stack.push((cur_block, pc + 1));
                    let cond = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    if cond != 0 {
                        cur_block =
                            require_branch_target(instr.branch1, instr.opcode, cur_block, pc)?;
                    } else {
                        cur_block =
                            require_branch_target(instr.branch2, instr.opcode, cur_block, pc)?;
                    }
                    pc = 0;
                }

                CondBranch => {
                    let cond = pop_int_stack(int_stack, instr.opcode, cur_block, pc)?;
                    if cond != 0 {
                        // Loop back: jump to branch1 (loop body start).
                        cur_block =
                            require_branch_target(instr.branch1, instr.opcode, cur_block, pc)?;
                        pc = 0;
                    } else {
                        // Exit loop: advance to next instruction (typically Return).
                        pc += 1;
                    }
                }

                Loop => {
                    // Save return address (instruction after Loop).
                    addr_stack.push((cur_block, pc + 1));
                    // Push loop body (branch2) onto address stack.
                    let body = require_branch_target(instr.branch2, instr.opcode, cur_block, pc)?;
                    addr_stack.push((body, 0));
                    // Jump to init block (branch1).
                    cur_block = require_branch_target(instr.branch1, instr.opcode, cur_block, pc)?;
                    pc = 0;
                }

                // ═══════════════════════════════════════════════════════
                // UI instructions — never appear in execution blocks.
                // Handled separately by executeBuildUserInterface.
                // ═══════════════════════════════════════════════════════
                OpenVerticalBox
                | OpenHorizontalBox
                | OpenTabBox
                | CloseBox
                | AddButton
                | AddCheckButton
                | AddHorizontalSlider
                | AddVerticalSlider
                | AddNumEntry
                | AddSoundfile
                | AddHorizontalBargraph
                | AddVerticalBargraph
                | Declare => {
                    pc += 1;
                }

                Nop => {
                    pc += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
