//! W2/W3 scheduling of `compute` (`fir::schedule`) on compiled programs.
//!
//! Design: `porting/wasm-w2-w3-sinking-forwarding-design-2026-10-17-en.md`.
//! The pass must keep every output bit-identical: the interpreter runs each
//! program with and without it (`InterpOptions::keep_fir_order`), and the FIR
//! verifier checks the scheduled module. In debug builds the pass also asserts
//! that its independent checker accepted every rewrite.

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};
use fir::{AccessType, FirId, FirMatch, FirStore, match_fir};

/// The shape that motivated W2/W3: coupled-form resonators, each state read at
/// the top of the sample and copied at the end.
const BANK: &str = r#"
step(c, s, re, im, x) = c*re - s*im + x, s*re + c*im;
mode(i) = step(c, s) ~ (_, _) : !, _
with {
    f = 100.0 + 37.0 * i;
    c = cos(2.0 * 3.14159 * f / 44100.0) * 0.999;
    s = sin(2.0 * 3.14159 * f / 44100.0) * 0.999;
};
process = _ <: par(i, 12, mode(i)) :> _;
"#;

/// Delays, a table, a selector, an `ondemand` block, two inputs and outputs.
const MIXED: &str = r#"
tbl = rdtable(64, (+(1) ~ _) * 0.01, int(abs(_) * 63.0) & 63);
lp(x) = x : + ~ *(0.9);
gate(x) = (x > 0.5, x) : ondemand(\(v).(v * 2.0 + v'));
process(a, b) = lp(a) + tbl(b), select2(a > 0.0, a@3, b@5 + gate(b));
"#;

/// A recursion read, written and read again in one sample.
const FEEDBACK: &str = r#"
f(x) = (x + _ * 0.5 : sin) ~ _;
g = (+ : *(0.5)) ~ (_ <: _, mem : -);
process = _ <: f, g;
"#;

fn interp_render(name: &str, source: &str, keep_fir_order: bool) -> Vec<Vec<f32>> {
    let options = InterpOptions {
        keep_fir_order,
        ..InterpOptions::default()
    };
    let fbc = Compiler::new()
        .compile_source_to_interp_with_lane(
            name,
            source,
            &options,
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{name}: interp compilation failed: {e}"));
    let mut reader = std::io::Cursor::new(fbc);
    let mut factory =
        read_fbc::<f32>(&mut reader).unwrap_or_else(|e| panic!("{name}: fbc parse failed: {e}"));
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(44_100);
    let frames = 4_096;
    let (ins, outs) = (
        instance.get_num_inputs() as usize,
        instance.get_num_outputs() as usize,
    );
    // A deterministic noise on every input.
    let mut seed = 0x1234_5678_u32;
    let inputs: Vec<Vec<f32>> = (0..ins)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
                })
                .collect()
        })
        .collect();
    let input_refs: Vec<&[f32]> = inputs.iter().map(Vec::as_slice).collect();
    let mut output_data = vec![vec![0.0_f32; frames]; outs];
    let mut output_refs: Vec<&mut [f32]> = output_data.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frames as i32, &input_refs, &mut output_refs)
        .unwrap_or_else(|e| panic!("{name}: interp execution failed: {e}"));
    output_data
}

/// Number of struct-table loads in `id`'s subtree.
fn struct_loads(store: &FirStore, id: FirId) -> usize {
    let node = match_fir(store, id);
    let own = usize::from(matches!(
        node,
        FirMatch::LoadTable {
            access: AccessType::Struct,
            ..
        }
    ));
    own + fir_children(store, id)
        .into_iter()
        .map(|c| struct_loads(store, c))
        .sum::<usize>()
}

fn fir_children(store: &FirStore, id: FirId) -> Vec<FirId> {
    fir::fir_match_children(store, id)
}

fn compute_body(store: &FirStore, module: FirId) -> FirId {
    let FirMatch::Module { functions, .. } = match_fir(store, module) else {
        panic!("not a module");
    };
    let FirMatch::Block(items) = match_fir(store, functions) else {
        panic!("functions is not a block");
    };
    items
        .into_iter()
        .find_map(|id| match match_fir(store, id) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } if name == "compute" => Some(body),
            _ => None,
        })
        .expect("compute")
}

#[test]
fn scheduling_keeps_interpreter_outputs_bit_identical() {
    for (name, source) in [("bank", BANK), ("mixed", MIXED), ("feedback", FEEDBACK)] {
        let fir_order = interp_render(name, source, true);
        let scheduled = interp_render(name, source, false);
        assert_eq!(fir_order.len(), scheduled.len(), "{name}: output count");
        for (ch, (a, b)) in fir_order.iter().zip(&scheduled).enumerate() {
            let first = a
                .iter()
                .zip(b)
                .position(|(x, y)| x.to_bits() != y.to_bits());
            assert_eq!(
                first, None,
                "{name}: output {ch} differs from frame {first:?}"
            );
        }
    }
}

#[test]
fn the_scheduled_bank_is_verified_and_reads_less_memory() {
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane("bank", BANK, SignalFirLane::TransformFastLane)
        .unwrap_or_else(|e| panic!("bank: FIR compilation failed: {e}"));
    let (store, module) = fir::schedule::schedule_compute(&fir.store, fir.module)
        .expect("the bank's compute is rescheduled");
    let report = fir::checker::verify_fir_module(&store, module);
    assert!(
        !report.has_errors(),
        "the scheduled module verifies: {:?}",
        report.diagnostics
    );
    let before = struct_loads(&fir.store, compute_body(&fir.store, fir.module));
    let after = struct_loads(&store, compute_body(&store, module));
    // Each mode stores two states and reloads both, for the copies and the
    // output sum: W3 reads the stored locals instead.
    assert!(
        after < before,
        "struct loads: {before} before, {after} after"
    );
}

#[test]
fn scheduling_is_idempotent() {
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane("bank", BANK, SignalFirLane::TransformFastLane)
        .unwrap_or_else(|e| panic!("bank: FIR compilation failed: {e}"));
    let (store, module) = fir::schedule::schedule_compute(&fir.store, fir.module)
        .expect("the bank's compute is rescheduled");
    assert!(
        fir::schedule::schedule_compute(&store, module).is_none(),
        "a scheduled compute has nothing left to move or forward"
    );
}
