//! Vector mode (`-vec`) bit-exactness oracle — roadmap P6, vector doc V6.
//!
//! Vector mode only changes *storage/loop structure*, not the per-sample
//! arithmetic, so its output must be **bit-identical** to scalar. These tests
//! compile the same DSP scalar and with `ComputeMode::Vector`, run both through
//! the interpreter over a block larger than the vector size (so state crosses a
//! chunk boundary), and assert the outputs are exactly equal.

use std::io::Cursor;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{
    Compiler, ComputeMode, SchedulingStrategy, SignalFirLane, VectorEffectiveMode,
    VectorFallbackReason, VectorPipelineStatus,
};

// A read-only `rdtable` whose index is block-invariant (a UI slider rather than
// an input sample). The invariant index splits the program into two loops, so
// the generator and the read can be fissioned into opposite orders; a
// per-sample index keeps everything in one unfissionable loop and hides the
// case. The table is filled before `compute` and never stored to, so the
// generator carries no compute-time write to reverse against the read.
const READONLY_TABLE_INVARIANT_INDEX_SOURCE: &str = concat!(
    "w(x) = waveform{10,20,30,40,50}, int(x) : rdtable;\n",
    "process = *(sin(w(4*hslider(\"value\",0,0,1,0.01)))),",
    " w(4*hslider(\"value\",0,0,1,0.01));"
);
// One read-only table, one float `rwtable`, and one int `rwtable` with a
// recursive generator, sharing a recursive index: the `table1`/`table2`
// corpus shapes. Admission rejected every live-port table outright before
// mutable lowering landed, so certification alone discriminates.
const MUTABLE_TABLE_SOURCE: &str = concat!(
    "wf = waveform{0,0.5,1,0.5,0,-0.5,-1,-0.5};\n",
    "size = wf : _,!;\n",
    "idx = (+(1)~_) % size;\n",
    "tro = wf, idx : rdtable;\n",
    "trw = wf, idx, (waveform{10,10.5,11,10.5,10,-10.5,-11,-10.5} : !,_), idx : rwtable;\n",
    "iinteg = +(1)~_;\n",
    "irw = rwtable(6, iinteg, (iinteg+1)%6, 2*iinteg, (iinteg+2)%6);\n",
    "process = tro, trw, irw;"
);
// The `sound.dsp` shape - a recursive index into a multi-part soundfile -
// with a live length read so `LoadSoundfileLength` sits on the certified
// path. The pipeline rejected every soundfile program outright before E2, so
// certification alone discriminates. Numeric coverage is the native C++
// matrix: the interpreter lane does not run soundfiles.
const SOUNDFILE_READ_SOURCE: &str = concat!(
    "sf = soundfile(\"son[url:{'sound1';'sound2'}]\", 2);\n",
    "process = 0, _~+(1) : sf : _,!,_,_;"
);
const PULSE_COUNTUP_LOOP_SOURCE: &str =
    "ba = library(\"basics.lib\"); process = ba.pulse_countup_loop(4, 1) + 0.001;";
const PULSE_COUNTDOWN_LOOP_SOURCE: &str =
    "ba = library(\"basics.lib\"); process = ba.pulse_countdown_loop(4, 1) + 0.001;";
const INDIRECT_RECURSIVE_DELAY_SOURCE: &str = r#"
    import("music.lib");
    indirect(freq, reset, replacement) =
        (select2(prefix(1, clock), +(increment), replacement) : decimal) ~ _
    with {
        clock = reset > 0;
        increment = freq / SR;
    };
    process = indirect(750, reset, replacement), reset, replacement
    with {
        reset = waveform {0, 0, 1, 0, 0} : !, _;
        replacement = waveform {0.125, 0.75, 0.5} : !, _;
    };
"#;
const LOCKSTEP_PAIR_SOURCE: &str = include_str!("../../../tests/corpus/vector_lockstep_pair.dsp");
const LOCKSTEP_QUAD_SOURCE: &str = include_str!("../../../tests/corpus/vector_lockstep_quad.dsp");
const LOCKSTEP_SIMD_QUAD_SOURCE: &str =
    include_str!("../../../tests/corpus/vector_lockstep_simd_quad.dsp");
const LOCKSTEP_MIXED_REDUCE_SOURCE: &str =
    include_str!("../../../tests/corpus/vector_lockstep_mixed_reduce.dsp");
const LOCKSTEP_MIXED_BRANCH_SOURCE: &str =
    include_str!("../../../tests/corpus/vector_lockstep_mixed_branch.dsp");
const LOCKSTEP_NEAR_ISOMORPHIC_SOURCE: &str =
    include_str!("../../../tests/corpus/vector_lockstep_near_isomorphic.dsp");
const SMOOTHDELAY_SOURCE: &str = include_str!("../../../tests/impulse-tests/dsp/smoothdelay.dsp");

/// Compiles `source` to interpreter bytecode with the given compute and
/// scheduling modes, then runs one block with the provided input channels.
fn run_channels_with_strategy(
    source: &str,
    mode: ComputeMode,
    inputs: &[Vec<f32>],
    strategy: SchedulingStrategy,
) -> Vec<Vec<f32>> {
    let path = std::env::temp_dir().join(format!(
        "faust-rs-vecmode-{}-{:?}.dsp",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, source).expect("write temp dsp");
    let fbc = Compiler::new()
        .with_compute_mode(mode)
        .with_scheduling_strategy(strategy)
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("compile failed ({mode:?}): {e}"));
    let _ = std::fs::remove_file(&path);

    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader).expect("fbc parse");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("outputs");
    let frames = inputs.first().map_or(0, Vec::len);
    assert!(inputs.iter().all(|input| input.len() == frames));
    let mut outputs = vec![vec![0.0_f32; frames]; num_outputs];
    let mut slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    let input_slices = inputs.iter().map(Vec::as_slice).collect::<Vec<_>>();
    instance
        .try_compute(frames as i32, &input_slices, &mut slices)
        .expect("compute");
    outputs
}

fn assert_channels_bit_exact(name: &str, source: &str, inputs: &[Vec<f32>], vec_size: u32) {
    for strategy in scheduling_strategies() {
        let scalar = run_channels_with_strategy(source, ComputeMode::Scalar, inputs, strategy);
        for loop_variant in [0_u8, 1] {
            let vector = run_channels_with_strategy(
                source,
                ComputeMode::Vector {
                    vec_size,
                    loop_variant,
                },
                inputs,
                strategy,
            );
            assert_eq!(
                scalar, vector,
                "{name}: scalar differs from -lv {loop_variant} under {strategy:?}"
            );
        }
    }
}

fn scheduling_strategies() -> [SchedulingStrategy; 4] {
    [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ]
}

#[test]
fn scalar_scheduling_strategies_are_bit_exact() {
    let source = "process = _,_ <: (_ * 2.0 + 1.0), (_ * 3.0 + 4.0) :> _;";
    let inputs = [ramp(67), ramp(67).into_iter().rev().collect()];
    let expected = run_channels_with_strategy(
        source,
        ComputeMode::Scalar,
        &inputs,
        SchedulingStrategy::DepthFirst,
    );
    for strategy in scheduling_strategies().into_iter().skip(1) {
        assert_eq!(
            run_channels_with_strategy(source, ComputeMode::Scalar, &inputs, strategy),
            expected,
            "scalar execution changed under {strategy:?}"
        );
    }
}

#[test]
fn scalar_mode_does_not_run_vector_certification() {
    let path = std::env::temp_dir().join(format!(
        "faust-rs-scalar-vector-boundary-{}-{:?}.dsp",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, "process = _ * 0.5;").expect("write temp dsp");

    for strategy in scheduling_strategies() {
        let result = Compiler::new()
            .with_compute_mode(ComputeMode::Scalar)
            .with_scheduling_strategy(strategy)
            .compile_file_default_to_fir_with_lane(&path, SignalFirLane::TransformFastLane);
        let output =
            result.unwrap_or_else(|error| panic!("scalar FIR under {strategy:?}: {error}"));
        assert_eq!(
            output.vector_pipeline_status,
            VectorPipelineStatus::NotRequested,
            "scalar mode must not invoke checked vector certification under {strategy:?}"
        );
        assert_eq!(
            output.vector_effective_mode,
            VectorEffectiveMode::Scalar,
            "scalar mode must retain scalar FIR under {strategy:?}"
        );
        assert!(
            output.vector_pipeline_detail.is_none(),
            "scalar mode must not produce a vector fallback diagnostic under {strategy:?}"
        );
    }

    let _ = std::fs::remove_file(path);
}

/// A `frames`-sample deterministic ramp with a non-integer step.
fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|k| 0.13 * k as f32 - 1.0).collect()
}

fn assert_scalar_vector_bit_exact(name: &str, source: &str, vec_size: u32) {
    // 64-sample block: with vec_size 32 → two full chunks + a boundary crossing;
    // with a non-dividing vec_size → a short remainder / tail chunk. Both loop
    // variants (`-lv 0` fastest, `-lv 1` simple) must match scalar bit-for-bit.
    let frames = 64;
    let input = ramp(frames);
    for strategy in scheduling_strategies() {
        let scalar = run_channels_with_strategy(
            source,
            ComputeMode::Scalar,
            std::slice::from_ref(&input),
            strategy,
        );
        for loop_variant in [0_u8, 1] {
            let vector = run_channels_with_strategy(
                source,
                ComputeMode::Vector {
                    vec_size,
                    loop_variant,
                },
                std::slice::from_ref(&input),
                strategy,
            );
            assert_eq!(
                scalar.len(),
                vector.len(),
                "{name}: output channel count differs (-lv {loop_variant}, {strategy:?})"
            );
            for (ch, (s, v)) in scalar.iter().zip(vector.iter()).enumerate() {
                assert_eq!(
                    s, v,
                    "{name}: channel {ch} differs, scalar vs -vec (-vs {vec_size} -lv {loop_variant} {strategy:?})"
                );
            }
        }
    }
}

fn assert_vector_pipeline_certified(name: &str, source: &str, vec_size: u32) {
    for strategy in scheduling_strategies() {
        for loop_variant in [0_u8, 1] {
            let path = std::env::temp_dir().join(format!(
                "faust-rs-vecmode-status-{}-{:?}.dsp",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::write(&path, source).expect("write temp dsp");
            let result = Compiler::new()
                .with_compute_mode(ComputeMode::Vector {
                    vec_size,
                    loop_variant,
                })
                .with_scheduling_strategy(strategy)
                .compile_file_default_to_fir_with_lane(&path, SignalFirLane::TransformFastLane);
            let _ = std::fs::remove_file(&path);
            let output = result.unwrap_or_else(|error| {
                panic!("{name} vector FIR (-lv {loop_variant}, {strategy:?}): {error}")
            });
            assert_eq!(
                output.vector_pipeline_status,
                VectorPipelineStatus::Certified,
                "{name} must use the checked vector path under -lv {loop_variant} and {strategy:?}: {:?}",
                output.vector_pipeline_detail
            );
        }
    }
}

#[test]
fn stateless_gain_is_bit_exact() {
    assert_scalar_vector_bit_exact("gain", "process = _ * 0.5;", 32);
}

#[test]
fn ui_slider_and_bargraph_are_certified_and_bit_exact() {
    let source =
        "process = _ * hslider(\"gain\", 0.5, 0.0, 1.0, 0.01) : hbargraph(\"level\", -10.0, 10.0);";
    assert_vector_pipeline_certified("ui_slider_bargraph", source, 24);
    assert_scalar_vector_bit_exact("ui_slider_bargraph", source, 24);
}

#[test]
fn sampling_frequency_constant_is_certified_and_bit_exact() {
    let source = r#"
        sr = fconstant(int fSamplingFreq, <math.h>);
        process = _ * float(sr) / 48000.0;
    "#;
    assert_vector_pipeline_certified("sampling_frequency", source, 24);
    assert_scalar_vector_bit_exact("sampling_frequency", source, 24);
}

#[test]
fn block_count_foreign_variable_is_certified_and_bit_exact() {
    let source = r#"
        block_size = fvariable(int count, <math.h>);
        process = _ + float(block_size);
    "#;
    assert_vector_pipeline_certified("block_count", source, 24);
    assert_scalar_vector_bit_exact("block_count", source, 24);
}

#[test]
fn prefix_state_is_certified_and_bit_exact() {
    let source = "process = prefix(0.5);";
    assert_vector_pipeline_certified("prefix", source, 24);
    assert_scalar_vector_bit_exact("prefix", source, 24);
}

#[test]
fn direct_waveform_is_certified_and_bit_exact() {
    let source = "process = waveform {0.1, 0.25, -0.5, 0.75, 1.0};";
    assert_vector_pipeline_certified("direct_waveform", source, 24);
    assert_scalar_vector_bit_exact("direct_waveform", source, 24);
}

#[test]
fn readonly_generated_table_is_certified_and_bit_exact() {
    let source = r#"
        process = waveform {10, 20, 30, 40, 50, 60, 70},
            ((%(7) ~ +(3)) : max(0) : min(6)) : rdtable;
    "#;
    assert_vector_pipeline_certified("readonly_generated_table", source, 24);
    assert_scalar_vector_bit_exact("readonly_generated_table", source, 24);
}

#[test]
fn recursion_and_delay_cross_chunk_boundary_bit_exact() {
    // Integrator (loop-carried state) plus a 5-sample delay: both the recursive
    // carrier and the delay line must survive the chunk boundary unchanged.
    assert_scalar_vector_bit_exact(
        "rec_delay",
        "process = _ <: (+ ~ _), (@(5) * 0.5) : + ;",
        32,
    );
}

#[test]
fn recursive_short_delay_transport_reads_after_copy_in() {
    assert_scalar_vector_bit_exact("pulse_countup_loop", PULSE_COUNTUP_LOOP_SOURCE, 32);
    assert_scalar_vector_bit_exact("pulse_countdown_loop", PULSE_COUNTDOWN_LOOP_SOURCE, 32);
    // 64 samples with vec_size 24 exercises two full chunks and a 16-sample tail.
    assert_scalar_vector_bit_exact("pulse_countup_loop_tail", PULSE_COUNTUP_LOOP_SOURCE, 24);
    // vec_size exceeds the 64-sample block, covering count < vec_size.
    assert_scalar_vector_bit_exact("pulse_countup_loop_short", PULSE_COUNTUP_LOOP_SOURCE, 96);
}

#[test]
fn recursive_short_delay_transport_uses_certified_vector_pipeline() {
    assert_vector_pipeline_certified("pulse_countup_loop", PULSE_COUNTUP_LOOP_SOURCE, 32);
}

#[test]
fn indirect_cross_loop_delayed_read_is_fused_and_bit_exact() {
    assert_vector_pipeline_certified(
        "indirect_cross_loop_delayed_read",
        INDIRECT_RECURSIVE_DELAY_SOURCE,
        24,
    );
    assert_scalar_vector_bit_exact(
        "indirect_cross_loop_delayed_read",
        INDIRECT_RECURSIVE_DELAY_SOURCE,
        24,
    );
}

#[test]
fn recursive_short_delay_cpp_has_one_fused_read_compute_write_loop() {
    let path = std::env::temp_dir().join(format!(
        "faust-rs-vec-fused-cpp-{}-{:?}.dsp",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, PULSE_COUNTUP_LOOP_SOURCE).expect("write fused vector DSP");
    let cpp = Compiler::new()
        .with_compute_mode(ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 1,
        })
        .with_scheduling_strategy(SchedulingStrategy::ReverseBreadthFirst)
        .compile_file_default_to_cpp_with_lane(
            &path,
            &codegen::backends::cpp::CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .expect("compile fused vector C++");
    let _ = std::fs::remove_file(path);

    assert!(cpp.contains("float transport_s23_l2_l1;"));
    assert!(!cpp.contains("float transport_s23_l2_l1[32];"));
    let first_loop = cpp
        .find("for (int i0 = vindex;")
        .expect("fused serial sample loop");
    assert!(
        !cpp[first_loop + 1..].contains("for (int i0 = vindex;"),
        "the safe pure tail must stay fused inside the single sample loop"
    );
    let fused = &cpp[first_loop..];
    let read = fused
        .find("transport_s23_l2_l1 = vstate_s22_tmp")
        .expect("recursive delayed read in fused loop");
    let write = fused
        .find("vstate_s22_tmp[(4 + (i0 - vindex))] =")
        .expect("recursive state write in fused loop");
    assert!(read < write, "delayed read must precede the state write");
    let tail = fused
        .find("output0[i0] =")
        .expect("safe pure tail in fused loop");
    assert!(write < tail, "the state write must precede the pure tail");
}

#[test]
fn soundfile_reads_are_certified() {
    assert_vector_pipeline_certified("soundfile_read", SOUNDFILE_READ_SOURCE, 32);
}

#[test]
fn mutable_rwtable_is_certified_and_bit_exact() {
    assert_vector_pipeline_certified("mutable_rwtable", MUTABLE_TABLE_SOURCE, 32);
    assert_scalar_vector_bit_exact("mutable_rwtable", MUTABLE_TABLE_SOURCE, 32);
    assert_scalar_vector_bit_exact("mutable_rwtable_tail", MUTABLE_TABLE_SOURCE, 24);
}

#[test]
fn readonly_table_with_block_invariant_index_is_certified() {
    assert_vector_pipeline_certified(
        "readonly_table_invariant_index",
        READONLY_TABLE_INVARIANT_INDEX_SOURCE,
        32,
    );
    assert_scalar_vector_bit_exact(
        "readonly_table_invariant_index",
        READONLY_TABLE_INVARIANT_INDEX_SOURCE,
        32,
    );
    assert_scalar_vector_bit_exact(
        "readonly_table_invariant_index_tail",
        READONLY_TABLE_INVARIANT_INDEX_SOURCE,
        24,
    );
}

#[test]
fn vec_size_not_dividing_the_block_bit_exact() {
    // vec_size = 24 does not divide 64 → a short tail chunk (16) exercises the
    // `min(vindex + vs, count)` clamp.
    assert_scalar_vector_bit_exact("tail_chunk", "process = (_ : + ~ _) * 0.25;", 24);
}

#[test]
fn recursive_split_pure_tail_is_bit_exact() {
    // `(_ : + ~ _) * 0.5` is the S-D split case: the integrator is the serial
    // core, the `* 0.5` output scaling is hoisted into a second vectorizable inner
    // loop fed by a chunk buffer. The split must stay bit-exact vs scalar.
    assert_scalar_vector_bit_exact("split_tail", "process = (_ : + ~ _) * 0.5;", 32);
    // Tail chunk (vec_size ∤ block) through the split path too.
    assert_scalar_vector_bit_exact("split_tail_odd", "process = (_ : + ~ _) * 0.5;", 24);
}

#[test]
fn two_pole_filter_is_bit_exact() {
    // A second-order recurrence — deeper loop-carried state across the boundary.
    assert_scalar_vector_bit_exact(
        "biquad_like",
        "process = _ : + ~ (_ <: 0.5 * _' , -0.2 * _'' :> _);",
        32,
    );
}

#[test]
fn lockstep_recursive_instances_are_certified_and_bit_exact() {
    let inputs = [ramp(67), ramp(67).into_iter().rev().collect()];
    assert_vector_pipeline_certified("lockstep_pair", LOCKSTEP_PAIR_SOURCE, 24);
    assert_channels_bit_exact("lockstep_pair", LOCKSTEP_PAIR_SOURCE, &inputs, 24);
}

#[test]
fn complex_and_partial_lockstep_corpus_is_bit_exact() {
    let four_inputs = vec![
        ramp(67),
        ramp(67).into_iter().rev().collect(),
        (0..67).map(|index| (index % 7) as f32 * 0.125).collect(),
        (0..67).map(|index| (index % 5) as f32 * -0.25).collect(),
    ];
    assert_channels_bit_exact(
        "lockstep_simd_quad",
        LOCKSTEP_SIMD_QUAD_SOURCE,
        &four_inputs,
        24,
    );
    assert_channels_bit_exact(
        "lockstep_mixed_reduce",
        LOCKSTEP_MIXED_REDUCE_SOURCE,
        &four_inputs,
        24,
    );

    let mut five_inputs = four_inputs;
    five_inputs.push((0..67).map(|index| (index % 3) as f32 - 1.0).collect());
    assert_channels_bit_exact(
        "lockstep_mixed_branch",
        LOCKSTEP_MIXED_BRANCH_SOURCE,
        &five_inputs,
        24,
    );
}

#[test]
fn complex_and_partial_lockstep_corpus_is_certified_at_default_vec_size() {
    for (name, source) in [
        ("lockstep_simd_quad", LOCKSTEP_SIMD_QUAD_SOURCE),
        ("lockstep_mixed_reduce", LOCKSTEP_MIXED_REDUCE_SOURCE),
        ("lockstep_mixed_branch", LOCKSTEP_MIXED_BRANCH_SOURCE),
    ] {
        assert_vector_pipeline_certified(name, source, ComputeMode::DEFAULT_VEC_SIZE);
    }
}

#[test]
fn smoothdelay_uses_general_compact_events_at_default_vec_size() {
    assert_vector_pipeline_certified(
        "smoothdelay",
        SMOOTHDELAY_SOURCE,
        ComputeMode::DEFAULT_VEC_SIZE,
    );
}

#[test]
fn lockstep_corpus_cpp_has_expected_physical_sample_loops() {
    for (name, source, expected_loops, register_carried) in [
        ("pair", LOCKSTEP_PAIR_SOURCE, 1, true),
        ("quad", LOCKSTEP_QUAD_SOURCE, 1, true),
        ("simd_quad", LOCKSTEP_SIMD_QUAD_SOURCE, 1, true),
        ("mixed_reduce", LOCKSTEP_MIXED_REDUCE_SOURCE, 2, true),
        ("mixed_branch", LOCKSTEP_MIXED_BRANCH_SOURCE, 2, true),
        ("near_isomorphic", LOCKSTEP_NEAR_ISOMORPHIC_SOURCE, 2, false),
    ] {
        let path = std::env::temp_dir().join(format!(
            "faust-rs-lockstep-{name}-{}-{:?}.dsp",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, source).expect("write lockstep DSP");
        let cpp = Compiler::new()
            .with_compute_mode(ComputeMode::Vector {
                vec_size: 24,
                loop_variant: 1,
            })
            .compile_file_default_to_cpp_with_lane(
                &path,
                &codegen::backends::cpp::CppOptions::default(),
                SignalFirLane::TransformFastLane,
            )
            .unwrap_or_else(|error| panic!("compile {name} vector C++: {error}"));
        let _ = std::fs::remove_file(path);

        assert_eq!(
            cpp.match_indices("for (int i0 = vindex;").count(),
            expected_loops,
            "{name}: unexpected physical sample-loop count"
        );
        if register_carried {
            assert!(
                cpp.contains("vlock_b0_l0_") && cpp.contains("_state"),
                "{name}: missing register-carried lockstep state"
            );
            assert!(
                !cpp.contains("vstate_s") || !cpp.contains("_tmp["),
                "{name}: lockstep delay-one state remained chunk-array-backed"
            );
        }
    }
}

#[test]
fn clock_island_and_expanded_fad_are_bit_exact() {
    let input = (0..64)
        .map(|index| if index % 3 == 0 { 1.0 } else { 0.0 })
        .collect::<Vec<_>>();
    let clock_inputs = vec![input, ramp(64)];
    assert_channels_bit_exact(
        "clock",
        "process = ((_ != 0), _) : ondemand(*(2));",
        &clock_inputs,
        24,
    );
    assert_channels_bit_exact(
        "clock_recursion",
        "process = ((_ != 0), _) : ondemand(+ ~ _);",
        &clock_inputs,
        24,
    );
    assert_channels_bit_exact(
        "clock_delay",
        "process = ((_ != 0), _) : ondemand(_ <: _, @(3) :> +);",
        &clock_inputs,
        24,
    );

    let inputs = vec![
        ramp(64),
        (0..64).map(|index| 0.25 + index as f32 * 0.01).collect(),
        (0..64).map(|index| 1.0 - index as f32 * 0.02).collect(),
    ];
    assert_channels_bit_exact("fad", "process = fad(*, (_,_,_));", &inputs, 24);
}

#[test]
fn bounded_variable_delay_is_bit_exact() {
    let amount = (0..64)
        .map(|index| (index as f32 * 0.31).sin() * 0.9)
        .collect::<Vec<_>>();
    assert_channels_bit_exact(
        "variable_delay",
        "process(carrier, amount) = carrier @ int(amount + 10);",
        &[ramp(64), amount],
        24,
    );
}

#[test]
fn production_fir_reports_certified_and_named_fallback_paths() {
    let compiler = Compiler::new().with_compute_mode(ComputeMode::Vector {
        vec_size: 8,
        loop_variant: 0,
    });
    let pure = compiler
        .compile_source_to_fir_with_lane(
            "pure.dsp",
            "process = _ * 0.5;",
            SignalFirLane::TransformFastLane,
        )
        .expect("pure vector FIR");
    assert_eq!(pure.vector_pipeline_status, VectorPipelineStatus::Certified);
    assert_eq!(
        pure.vector_effective_mode,
        VectorEffectiveMode::CertifiedVector
    );
    assert_eq!(pure.vector_pipeline_detail, None);

    let stateful = compiler
        .compile_source_to_fir_with_lane(
            "stateful.dsp",
            "process = _ : mem;",
            SignalFirLane::TransformFastLane,
        )
        .expect("stateful transitional vector FIR");
    assert_eq!(
        stateful.vector_pipeline_status,
        VectorPipelineStatus::Certified
    );

    for strategy in scheduling_strategies() {
        let compiler = Compiler::new()
            .with_compute_mode(ComputeMode::Vector {
                vec_size: 8,
                loop_variant: 0,
            })
            .with_scheduling_strategy(strategy);
        for (name, source) in [
            ("recursive", "process = (_ : + ~ _) * 0.5;"),
            ("fad", "process = fad(*, (_,_,_));"),
            ("clock", "process = ((_ != 0), _) : ondemand(*(2));"),
            ("clock-state", "process = ((_ != 0), _) : ondemand(+ ~ _);"),
            ("variable-delay", "process = @(int(_ + 10));"),
        ] {
            let output = compiler
                .compile_source_to_fir_with_lane(
                    &format!("{name}.dsp"),
                    source,
                    SignalFirLane::TransformFastLane,
                )
                .unwrap_or_else(|error| panic!("{name} vector FIR: {error}"));
            assert_eq!(
                output.vector_pipeline_status,
                VectorPipelineStatus::Certified,
                "{name} must use the checked vector path under {strategy:?}: {:?}",
                output.vector_pipeline_detail
            );
        }
    }

    let ui = compiler
        .compile_source_to_fir_with_lane(
            "ui.dsp",
            "process = _ * hslider(\"gain\", 0.5, 0.0, 1.0, 0.01);",
            SignalFirLane::TransformFastLane,
        )
        .expect("UI transitional vector FIR");
    assert_eq!(ui.vector_pipeline_status, VectorPipelineStatus::Certified);
    assert_eq!(
        ui.vector_effective_mode,
        VectorEffectiveMode::CertifiedVector
    );
    assert_eq!(ui.vector_pipeline_detail, None);

    let rad = compiler
        .compile_source_to_fir_with_lane(
            "rad.dsp",
            "process = rad(_', _);",
            SignalFirLane::TransformFastLane,
        )
        .expect("RAD transitional vector FIR");
    assert_eq!(
        rad.vector_pipeline_status,
        VectorPipelineStatus::Fallback(VectorFallbackReason::ReverseAd)
    );
    assert_eq!(rad.vector_effective_mode, VectorEffectiveMode::Scalar);
    assert!(rad.vector_pipeline_detail.is_some());
    assert_eq!(VectorFallbackReason::ReverseAd.code(), "FRS-VEC-RAD-SCALAR");
}

#[test]
fn phase2_plan_accepts_multi_projection_recursion_and_table_value_transports() {
    std::thread::Builder::new()
        .name("phase2-vector-plan-corpus".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let compiler = Compiler::new().with_compute_mode(ComputeMode::Vector {
                vec_size: 32,
                loop_variant: 0,
            });
            let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/impulse-tests/dsp");
            for name in ["APF.dsp", "pow.dsp"] {
                let path = corpus.join(name);
                let output = compiler
                    .compile_file_default_to_fir_with_lane(&path, SignalFirLane::TransformFastLane)
                    .unwrap_or_else(|error| panic!("{name} vector FIR: {error}"));
                assert_ne!(
                    output.vector_pipeline_status,
                    VectorPipelineStatus::Fallback(VectorFallbackReason::VectorPlan),
                    "{name} must pass the checked phase-2 vector plan"
                );
            }
        })
        .expect("spawn large-stack phase-2 test")
        .join()
        .expect("phase-2 test thread");
}

#[test]
fn phase3_state_plan_accepts_temporal_slow_values_and_special_state_cells() {
    std::thread::Builder::new()
        .name("phase3-vector-state-corpus".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let compiler = Compiler::new().with_compute_mode(ComputeMode::Vector {
                vec_size: 32,
                loop_variant: 0,
            });
            let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/impulse-tests/dsp");
            for name in ["echo_bug.dsp", "norm3.dsp", "prefix.dsp", "waveform1.dsp"] {
                let path = corpus.join(name);
                let output = compiler
                    .compile_file_default_to_fir_with_lane(&path, SignalFirLane::TransformFastLane)
                    .unwrap_or_else(|error| panic!("{name} vector FIR: {error}"));
                assert_ne!(
                    output.vector_pipeline_status,
                    VectorPipelineStatus::Fallback(VectorFallbackReason::StatePlan),
                    "{name} must pass the checked phase-3 state plan"
                );
            }
        })
        .expect("spawn large-stack phase-3 test")
        .join()
        .expect("phase-3 test thread");
}

#[test]
fn vector_copy_delay_loops_reach_c_family_backends() {
    let source = include_str!("../../../tests/impulse-tests/dsp/noiseabs.dsp");
    for loop_variant in [0_u8, 1] {
        let compiler = Compiler::new()
            .with_compute_mode(ComputeMode::Vector {
                vec_size: 32,
                loop_variant,
            })
            .with_scheduling_strategy(SchedulingStrategy::DepthFirst);
        let cpp = compiler
            .compile_source_to_cpp_with_lane(
                "noiseabs.dsp",
                source,
                &codegen::backends::cpp::CppOptions::default(),
                SignalFirLane::TransformFastLane,
            )
            .unwrap_or_else(|error| {
                panic!("noiseabs C++ -vec -lv {loop_variant} lowering failed: {error}")
            });
        assert!(cpp.contains("vstate_s21_perm"));

        let c = compiler
            .compile_source_to_c_with_lane(
                "noiseabs.dsp",
                source,
                &codegen::backends::c::COptions::default(),
                SignalFirLane::TransformFastLane,
            )
            .unwrap_or_else(|error| {
                panic!("noiseabs C -vec -lv {loop_variant} lowering failed: {error}")
            });
        assert!(c.contains("vstate_s21_perm"));
    }
}
