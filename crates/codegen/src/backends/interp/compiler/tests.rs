use super::*;
use crate::backends::interp::{
    clear_foreign_functions, register_foreign_function, unregister_foreign_function,
};
use fir::{FirBinOp, FirBuilder};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn foreign_registry_test_guard() -> MutexGuard<'static, ()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("interp foreign-registry test mutex")
}

/// Helper: compile a single FIR node and finalize.
fn compile_one<R: FbcReal>(store: &FirStore, id: FirId) -> FbcCompileResult<R> {
    let mut compiler = FirToFbcCompiler::<R>::new();
    compiler.compile_node(store, id).unwrap();
    compiler.finalize().unwrap()
}

/// Helper: get the instruction opcodes from a block (excluding final Return).
fn opcodes<R: FbcReal>(result: &FbcCompileResult<R>, block_id: BlockId) -> Vec<FbcOpcode> {
    result
        .arena
        .get(block_id)
        .instructions
        .iter()
        .map(|i| i.opcode)
        .collect()
}

// --- Phase A: Literal values ---

#[test]
fn test_compile_int32() {
    let mut store = FirStore::new();
    let id = FirBuilder::new(&mut store).int32(42);
    let result = compile_one::<f32>(&store, id);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(ops, vec![FbcOpcode::Int32Value, FbcOpcode::Return]);
    assert_eq!(
        result.arena.get(result.entry_block).instructions[0].int_value,
        42
    );
}

#[test]
fn test_compile_float32() {
    let mut store = FirStore::new();
    let id = FirBuilder::new(&mut store).float32(3.125);
    let result = compile_one::<f32>(&store, id);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(ops, vec![FbcOpcode::RealValue, FbcOpcode::Return]);
    let rv = result.arena.get(result.entry_block).instructions[0].real_value;
    assert!((rv - 3.125).abs() < 1e-6);
}

#[test]
fn test_compile_float64() {
    let mut store = FirStore::new();
    let id = FirBuilder::new(&mut store).float64(2.5);
    let result = compile_one::<f64>(&store, id);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(ops, vec![FbcOpcode::RealValue, FbcOpcode::Return]);
    let rv = result.arena.get(result.entry_block).instructions[0].real_value;
    assert!((rv - 2.5).abs() < 1e-10);
}

#[test]
fn test_compile_bool() {
    let mut store = FirStore::new();
    let id = FirBuilder::new(&mut store).bool_(true);
    let result = compile_one::<f32>(&store, id);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(ops, vec![FbcOpcode::Int32Value, FbcOpcode::Return]);
    assert_eq!(
        result.arena.get(result.entry_block).instructions[0].int_value,
        1
    );
}

// --- Phase B: Binops ---

#[test]
fn test_compile_binop_add_int() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let three = b.int32(3);
    let four = b.int32(4);
    let add = b.binop(FirBinOp::Add, three, four, FirType::Int32);
    let result = compile_one::<f32>(&store, add);
    let ops = opcodes(&result, result.entry_block);
    // rhs (4) compiled first, then lhs (3), then AddInt
    assert_eq!(
        ops,
        vec![
            FbcOpcode::Int32Value, // 4 (rhs)
            FbcOpcode::Int32Value, // 3 (lhs)
            FbcOpcode::AddInt,
            FbcOpcode::Return
        ]
    );
    assert_eq!(
        result.arena.get(result.entry_block).instructions[0].int_value,
        4
    );
    assert_eq!(
        result.arena.get(result.entry_block).instructions[1].int_value,
        3
    );
}

#[test]
fn test_compile_binop_add_real() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let one = b.float32(1.0);
    let two = b.float32(2.0);
    let add = b.binop(FirBinOp::Add, one, two, FirType::Float32);
    let result = compile_one::<f32>(&store, add);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![
            FbcOpcode::RealValue, // 2.0 (rhs)
            FbcOpcode::RealValue, // 1.0 (lhs)
            FbcOpcode::AddReal,
            FbcOpcode::Return
        ]
    );
}

// --- Phase C: Declare + Load + Store ---

#[test]
fn test_compile_declare_and_load_int() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let init_val = b.int32(42);
    let decl = b.declare_var("x", FirType::Int32, AccessType::Struct, Some(init_val));
    let load = b.load_var("x", AccessType::Struct, FirType::Int32);

    let mut compiler = FirToFbcCompiler::<f32>::new();
    compiler.compile_node(&store, decl).unwrap();
    compiler.compile_node(&store, load).unwrap();
    let result = compiler.finalize().unwrap();

    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![
            FbcOpcode::Int32Value, // init value 42
            FbcOpcode::StoreInt,   // store to heap[0]
            FbcOpcode::LoadInt,    // load from heap[0]
            FbcOpcode::Return,
        ]
    );
    // Verify offset = 0 on store and load.
    assert_eq!(
        result.arena.get(result.entry_block).instructions[1].offset1,
        0
    );
    assert_eq!(
        result.arena.get(result.entry_block).instructions[2].offset1,
        0
    );
    assert_eq!(result.int_heap_size, 1);
}

#[test]
fn test_compile_declare_real() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let init_val = b.float32(1.5);
    let decl = b.declare_var("y", FirType::Float32, AccessType::Struct, Some(init_val));

    let mut compiler = FirToFbcCompiler::<f32>::new();
    compiler.compile_node(&store, decl).unwrap();
    let result = compiler.finalize().unwrap();

    assert_eq!(result.real_heap_size, 1);
    assert_eq!(result.int_heap_size, 0);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![
            FbcOpcode::RealValue,
            FbcOpcode::StoreReal,
            FbcOpcode::Return
        ]
    );
}

// --- Phase D: Cast ---

#[test]
fn test_compile_cast_int_to_real() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let val = b.int32(7);
    let cast = b.cast(FirType::Float32, val);
    let result = compile_one::<f32>(&store, cast);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![
            FbcOpcode::Int32Value,
            FbcOpcode::CastReal,
            FbcOpcode::Return
        ]
    );
}

#[test]
fn test_compile_cast_real_to_int() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let val = b.float32(7.5);
    let cast = b.cast(FirType::Int32, val);
    let result = compile_one::<f32>(&store, cast);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![FbcOpcode::RealValue, FbcOpcode::CastInt, FbcOpcode::Return]
    );
}

#[test]
fn test_compile_cast_same_type_no_op() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let val = b.int32(5);
    let cast = b.cast(FirType::Int32, val);
    let result = compile_one::<f32>(&store, cast);
    let ops = opcodes(&result, result.entry_block);
    // No CastInt emitted because operand is already int.
    assert_eq!(ops, vec![FbcOpcode::Int32Value, FbcOpcode::Return]);
}

// --- Phase E: Select2 ---

#[test]
fn test_compile_select2() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let cond = b.int32(1);
    let then_v = b.int32(10);
    let else_v = b.int32(20);
    let sel = b.select2(cond, then_v, else_v, FirType::Int32);
    let result = compile_one::<f32>(&store, sel);

    // Entry block: cond + kSelectInt.
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![
            FbcOpcode::Int32Value,
            FbcOpcode::SelectInt,
            FbcOpcode::Return
        ]
    );

    // Check sub-blocks exist.
    let select_instr = &result.arena.get(result.entry_block).instructions[1];
    let then_id = select_instr.branch1.unwrap();
    let else_id = select_instr.branch2.unwrap();
    let then_ops = opcodes(&result, then_id);
    let else_ops = opcodes(&result, else_id);
    assert_eq!(then_ops, vec![FbcOpcode::Int32Value, FbcOpcode::Return]);
    assert_eq!(else_ops, vec![FbcOpcode::Int32Value, FbcOpcode::Return]);
}

// --- Phase F: ForLoop ---

#[test]
fn test_compile_for_loop_structure() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    // for (i = 0; i < 10; i++) { /* empty body */ }: `end` is the exclusive
    // bound, as the lowerings build it, not a comparison.
    let init_val = b.int32(0);
    let init_decl = b.declare_var("i", FirType::Int32, AccessType::Loop, Some(init_val));
    let ten = b.int32(10);
    let load_i2 = b.load_var("i", AccessType::Loop, FirType::Int32);
    let one = b.int32(1);
    let incr_val = b.binop(FirBinOp::Add, load_i2, one, FirType::Int32);
    let step = b.store_var("i", AccessType::Loop, incr_val);
    let body = b.block(&[]);
    let loop_node = b.for_loop("i", init_decl, ten, step, body, false);

    let result = compile_one::<f32>(&store, loop_node);

    // Entry block: the guard `init < end` (bound, init, LTInt), then kIf whose
    // `then` block holds kLoop; a zero bound runs the body zero times.
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![
            FbcOpcode::Int32Value,
            FbcOpcode::Int32Value,
            FbcOpcode::LTInt,
            FbcOpcode::If,
            FbcOpcode::Return
        ]
    );
    let if_instr = &result.arena.get(result.entry_block).instructions[3];
    let then_id = if_instr.branch1.unwrap();
    assert_eq!(
        opcodes(&result, then_id),
        vec![FbcOpcode::Loop, FbcOpcode::Return]
    );

    // kLoop instruction should reference init and loop-body blocks.
    let loop_instr = &result.arena.get(then_id).instructions[0];
    assert!(loop_instr.branch1.is_some());
    assert!(loop_instr.branch2.is_some());

    // Init block should contain init declaration + Return.
    let init_id = loop_instr.branch1.unwrap();
    let init_ops = opcodes(&result, init_id);
    assert!(init_ops.contains(&FbcOpcode::StoreInt));
    assert_eq!(*init_ops.last().unwrap(), FbcOpcode::Return);

    // Loop body block should end with CondBranch + Return.
    let body_id = loop_instr.branch2.unwrap();
    let body_ops = opcodes(&result, body_id);
    assert!(body_ops.contains(&FbcOpcode::CondBranch));
    assert_eq!(*body_ops.last().unwrap(), FbcOpcode::Return);
}

#[test]
fn test_compile_reverse_simple_for_loop_structure() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let upper = b.int32(4);
    let body = b.block(&[]);
    let loop_node = b.simple_for_loop("i", upper, body, true);

    let result = compile_one::<f32>(&store, loop_node);
    // The guard `upper > 0`, then kIf whose `then` block holds kLoop.
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![
            FbcOpcode::Int32Value,
            FbcOpcode::Int32Value,
            FbcOpcode::GTInt,
            FbcOpcode::If,
            FbcOpcode::Return
        ]
    );
    let then_id = result.arena.get(result.entry_block).instructions[3]
        .branch1
        .unwrap();
    assert_eq!(
        opcodes(&result, then_id),
        vec![FbcOpcode::Loop, FbcOpcode::Return]
    );

    let loop_instr = &result.arena.get(then_id).instructions[0];
    let init_id = loop_instr.branch1.unwrap();
    let body_id = loop_instr.branch2.unwrap();
    let init_ops = opcodes(&result, init_id);
    assert!(
        init_ops.contains(&FbcOpcode::SubInt),
        "reverse simple loop init should compute upper - 1"
    );
    let body_ops = opcodes(&result, body_id);
    assert!(
        body_ops.contains(&FbcOpcode::SubInt),
        "reverse simple loop body should decrement the loop variable"
    );
    assert!(
        body_ops.contains(&FbcOpcode::GEInt),
        "reverse simple loop condition should keep iterating while i >= 0"
    );
}

// --- Phase G: Function calls ---

#[test]
fn test_compile_fun_call_sin() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let arg = b.float32(0.5);
    let call = b.fun_call("sinf", &[arg], FirType::Float32);
    let result = compile_one::<f32>(&store, call);
    let ops = opcodes(&result, result.entry_block);
    assert_eq!(
        ops,
        vec![FbcOpcode::RealValue, FbcOpcode::Sinf, FbcOpcode::Return]
    );
}

#[test]
fn test_unknown_function_error() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let arg = b.float32(1.0);
    let call = b.fun_call("bogus_fn", &[arg], FirType::Float32);
    let mut compiler = FirToFbcCompiler::<f32>::new();
    let err = compiler.compile_node(&store, call).unwrap_err();
    match err {
        CompileError::UnknownMathFunction { name } => {
            assert_eq!(name, "bogus_fn");
        }
        _ => panic!("expected UnknownMathFunction, got: {err:?}"),
    }
}

extern "C" fn interp_test_foreign_gain(x: f32) -> f32 {
    x * 3.0
}

extern "C" fn interp_test_foreign_inc(x: i32) -> i32 {
    x + 1
}

extern "C" fn interp_test_foreign_probe(x: f32) {
    let _ = x;
}

#[test]
fn test_compile_registered_foreign_function_opcodes() {
    let _guard = foreign_registry_test_guard();
    clear_foreign_functions();
    register_foreign_function(
        "interp_test_foreign_gain",
        (interp_test_foreign_gain as *const ()).cast_mut().cast(),
    );
    register_foreign_function(
        "interp_test_foreign_inc",
        (interp_test_foreign_inc as *const ()).cast_mut().cast(),
    );
    register_foreign_function(
        "interp_test_foreign_probe",
        (interp_test_foreign_probe as *const ()).cast_mut().cast(),
    );

    let mut store = FirStore::new();
    let gain_call = {
        let mut b = FirBuilder::new(&mut store);
        let gain_arg = b.float32(0.5);
        b.fun_call("interp_test_foreign_gain", &[gain_arg], FirType::Float32)
    };
    let gain_result = compile_one::<f32>(&store, gain_call);
    let gain_block = gain_result.arena.get(gain_result.entry_block);
    assert_eq!(gain_block.instructions[0].opcode, FbcOpcode::RealValue);
    assert_eq!(
        gain_block.instructions[1].opcode,
        FbcOpcode::ForeignCallReal
    );
    assert_eq!(
        gain_block.instructions[1].name,
        "interp_test_foreign_gain|f|f"
    );

    let bool_call = {
        let mut b = FirBuilder::new(&mut store);
        let int_arg = b.int32(41);
        b.fun_call("interp_test_foreign_inc", &[int_arg], FirType::Int32)
    };
    let bool_result = compile_one::<f32>(&store, bool_call);
    let bool_block = bool_result.arena.get(bool_result.entry_block);
    assert_eq!(bool_block.instructions[1].opcode, FbcOpcode::ForeignCallInt);
    assert_eq!(
        bool_block.instructions[1].name,
        "interp_test_foreign_inc|i|i"
    );

    let void_call = {
        let mut b = FirBuilder::new(&mut store);
        let void_arg = b.float32(0.5);
        b.fun_call("interp_test_foreign_probe", &[void_arg], FirType::Void)
    };
    let void_result = compile_one::<f32>(&store, void_call);
    let void_block = void_result.arena.get(void_result.entry_block);
    assert_eq!(
        void_block.instructions[1].opcode,
        FbcOpcode::ForeignCallVoid
    );
    assert_eq!(
        void_block.instructions[1].name,
        "interp_test_foreign_probe|v|f"
    );

    unregister_foreign_function("interp_test_foreign_gain");
    unregister_foreign_function("interp_test_foreign_inc");
    unregister_foreign_function("interp_test_foreign_probe");
    clear_foreign_functions();
}

#[test]
fn test_roundtrip_registered_foreign_function() {
    use super::super::executor::FbcExecutor;

    let _guard = foreign_registry_test_guard();
    clear_foreign_functions();
    register_foreign_function(
        "interp_test_foreign_gain",
        (interp_test_foreign_gain as *const ()).cast_mut().cast(),
    );

    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let init_val = b.float32(0.0);
    let decl = b.declare_var("x", FirType::Float32, AccessType::Struct, Some(init_val));
    let arg = b.float32(0.5);
    let call = b.fun_call("interp_test_foreign_gain", &[arg], FirType::Float32);
    let st = b.store_var("x", AccessType::Struct, call);

    let mut compiler = FirToFbcCompiler::<f32>::new();
    compiler.compile_node(&store, decl).unwrap();
    compiler.compile_node(&store, st).unwrap();
    let result = compiler.finalize().unwrap();

    let mut exec = FbcExecutor::<f32>::new(
        result.int_heap_size as usize,
        result.real_heap_size as usize,
    );
    exec.execute_block(&result.arena, result.entry_block);
    assert_eq!(exec.real_heap[0], 1.5);

    unregister_foreign_function("interp_test_foreign_gain");
    clear_foreign_functions();
}

// --- Phase I: UI ---

#[test]
fn test_ui_slider() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    // Declare the zone variable first.
    let init_val = b.float32(0.5);
    let decl = b.declare_var(
        "fGain",
        FirType::Float32,
        AccessType::Struct,
        Some(init_val),
    );
    let slider = b.add_slider(
        SliderType::Horizontal,
        "Gain",
        "fGain",
        fir::SliderRange {
            init: 0.5,
            lo: 0.0,
            hi: 1.0,
            step: 0.01,
        },
    );

    let mut compiler = FirToFbcCompiler::<f32>::new();
    compiler.compile_node(&store, decl).unwrap();
    compiler.compile_node(&store, slider).unwrap();
    let result = compiler.finalize().unwrap();

    assert_eq!(result.ui_instructions.len(), 1);
    assert_eq!(
        result.ui_instructions[0].opcode,
        FbcOpcode::AddHorizontalSlider
    );
    assert_eq!(result.ui_instructions[0].label, "Gain");
    assert_eq!(result.ui_instructions[0].offset, 0);
}

// --- Phase J: Integration tests (compile + execute roundtrip) ---

#[test]
fn test_roundtrip_int32() {
    use super::super::executor::FbcExecutor;

    let mut store = FirStore::new();
    let id = FirBuilder::new(&mut store).int32(42);
    let result = compile_one::<f32>(&store, id);

    let mut exec = FbcExecutor::<f32>::new(16, 16);
    exec.execute_block(&result.arena, result.entry_block);
    // After execution, 42 should be on the int stack.
    // Since execute_block returns normally after kReturn,
    // we verify by compiling a store+check.
}

#[test]
fn test_roundtrip_add() {
    use super::super::executor::FbcExecutor;

    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    // Declare x, compute 3 + 4, store in x.
    let init_val = b.int32(0);
    let decl = b.declare_var("x", FirType::Int32, AccessType::Struct, Some(init_val));
    let three = b.int32(3);
    let four = b.int32(4);
    let add = b.binop(FirBinOp::Add, three, four, FirType::Int32);
    let st = b.store_var("x", AccessType::Struct, add);

    let mut compiler = FirToFbcCompiler::<f32>::new();
    compiler.compile_node(&store, decl).unwrap();
    compiler.compile_node(&store, st).unwrap();
    let result = compiler.finalize().unwrap();

    let mut exec = FbcExecutor::<f32>::new(
        result.int_heap_size as usize,
        result.real_heap_size as usize,
    );
    exec.execute_block(&result.arena, result.entry_block);
    assert_eq!(exec.int_heap[0], 7);
}

#[test]
fn test_roundtrip_store_load() {
    use super::super::executor::FbcExecutor;

    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    // Declare x = 42, then store 99 into x.
    let init_val = b.int32(42);
    let decl = b.declare_var("x", FirType::Int32, AccessType::Struct, Some(init_val));
    let ninety_nine = b.int32(99);
    let st = b.store_var("x", AccessType::Struct, ninety_nine);

    let mut compiler = FirToFbcCompiler::<f32>::new();
    compiler.compile_node(&store, decl).unwrap();
    compiler.compile_node(&store, st).unwrap();
    let result = compiler.finalize().unwrap();

    let mut exec = FbcExecutor::<f32>::new(
        result.int_heap_size as usize,
        result.real_heap_size as usize,
    );
    exec.execute_block(&result.arena, result.entry_block);
    assert_eq!(exec.int_heap[0], 99);
}

#[test]
fn test_roundtrip_for_loop() {
    use super::super::executor::FbcExecutor;

    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    // Declare x = 0 (at struct level, offset 0 in int heap).
    let x_init = b.int32(0);
    let x_decl = b.declare_var("x", FirType::Int32, AccessType::Struct, Some(x_init));

    // for (i = 0; i < 10; i += 1) { x = x + 1; }
    //
    // Canonical `ForLoop` contract (matches the signal->FIR transform and the
    // Cranelift backend): `init` is a `DeclareVar(kLoop)` seeding the variable,
    // `end` is the (exclusive) bound value, `step` is the signed increment value,
    // and the direction comes from `is_reverse`.
    let i_init = b.int32(0);
    let i_decl = b.declare_var("i", FirType::Int32, AccessType::Loop, Some(i_init));
    let end = b.int32(10);
    let step = b.int32(1);

    // Body: x = x + 1
    let load_x = b.load_var("x", AccessType::Struct, FirType::Int32);
    let one2 = b.int32(1);
    let add_x = b.binop(FirBinOp::Add, load_x, one2, FirType::Int32);
    let store_x = b.store_var("x", AccessType::Struct, add_x);
    let body = b.block(&[store_x]);

    let loop_node = b.for_loop("i", i_decl, end, step, body, false);

    let mut compiler = FirToFbcCompiler::<f32>::new();
    compiler.compile_node(&store, x_decl).unwrap();
    compiler.compile_node(&store, loop_node).unwrap();
    let result = compiler.finalize().unwrap();

    let mut exec = FbcExecutor::<f32>::new(
        result.int_heap_size as usize,
        result.real_heap_size as usize,
    );
    exec.execute_block(&result.arena, result.entry_block);

    // x should be 10 after 10 iterations.
    let x_offset = result.field_table["x"].offset as usize;
    assert_eq!(exec.int_heap[x_offset], 10);
}

// --- Lookup table tests ---

#[test]
fn test_math_lib_lookup() {
    assert_eq!(math_lib_lookup("sinf"), Some(FbcOpcode::Sinf));
    assert_eq!(math_lib_lookup("sin"), Some(FbcOpcode::Sinf));
    assert_eq!(math_lib_lookup("abs"), Some(FbcOpcode::Abs));
    assert_eq!(math_lib_lookup("fminf"), Some(FbcOpcode::Minf));
    assert_eq!(math_lib_lookup("fmin"), Some(FbcOpcode::Minf));
    assert_eq!(math_lib_lookup("fmaxf"), Some(FbcOpcode::Maxf));
    assert_eq!(math_lib_lookup("fmax"), Some(FbcOpcode::Maxf));
    assert_eq!(math_lib_lookup("unknown"), None);
}

#[test]
fn test_binop_to_fbc() {
    let (int_op, real_op) = binop_to_fbc(fir::FirBinOp::Add);
    assert_eq!(int_op, FbcOpcode::AddInt);
    assert_eq!(real_op, FbcOpcode::AddReal);
}

#[test]
fn test_parse_io_channel() {
    assert_eq!(parse_io_channel("input0", "input"), Some(0));
    assert_eq!(parse_io_channel("input3", "input"), Some(3));
    assert_eq!(parse_io_channel("output1", "output"), Some(1));
    assert_eq!(parse_io_channel("fGain", "input"), None);
}
