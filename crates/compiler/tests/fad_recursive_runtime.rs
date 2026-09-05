//! Runtime regressions for forward-mode AD across recursive structures.
//!
//! These tests complement the structural `signal_pipeline` coverage with
//! numeric checks against either closed-form recurrences or central finite
//! differences executed through the interpreter fast lane.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

static NEXT_TEMP_DSP_ID: AtomicUsize = AtomicUsize::new(0);

struct CentralDifferenceCase<BuildFad, BuildPrimal> {
    stem: &'static str,
    primal_outputs: usize,
    frame_count: usize,
    base_param: f32,
    epsilon: f32,
    abs_tol: f32,
    build_fad_source: BuildFad,
    build_primal_source: BuildPrimal,
}

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn run_interp_file(path: &std::path::Path, frame_count: usize) -> Vec<Vec<f32>> {
    let compiler = Compiler::new();
    let fbc = compiler
        .compile_file_default_to_interp_with_lane(
            path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{} interp compilation failed: {e}", path.display()));

    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader)
        .unwrap_or_else(|e| panic!("{} interp bytecode parse failed: {e}", path.display()));
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);

    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("non-negative outputs");
    let mut outputs = vec![vec![0.0_f32; frame_count]; num_outputs];
    let mut output_slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frame_count as i32, &[], &mut output_slices)
        .unwrap_or_else(|e| panic!("{} interp execution failed: {e}", path.display()));
    outputs
}

fn run_interp_temp_source(stem: &str, source: &str, frame_count: usize) -> Vec<Vec<f32>> {
    let unique_id = NEXT_TEMP_DSP_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "faust-rs-{stem}-{}-{unique_id}.dsp",
        std::process::id()
    ));
    fs::write(&path, source)
        .unwrap_or_else(|e| panic!("failed to write temporary DSP {}: {e}", path.display()));
    let result = run_interp_file(&path, frame_count);
    let _ = fs::remove_file(&path);
    result
}

fn assert_close(actual: f32, expected: f32, abs_tol: f32, label: &str) {
    let diff = (actual - expected).abs();
    let rel_tol = 1.0e-5_f32 * actual.abs().max(expected.abs());
    let allowed = abs_tol.max(rel_tol);
    assert!(
        diff <= allowed,
        "{label}: expected {expected}, got {actual}, abs diff {diff}, allowed {allowed}"
    );
}

fn assert_single_seed_fad_matches_central_difference(
    case: CentralDifferenceCase<impl Fn(f32) -> String, impl Fn(f32) -> String>,
) {
    let fad_outputs = run_interp_temp_source(
        &format!("{}-fad", case.stem),
        &(case.build_fad_source)(case.base_param),
        case.frame_count,
    );
    let primal_outputs_base = run_interp_temp_source(
        &format!("{}-primal", case.stem),
        &(case.build_primal_source)(case.base_param),
        case.frame_count,
    );
    let primal_outputs_plus = run_interp_temp_source(
        &format!("{}-plus", case.stem),
        &(case.build_primal_source)(case.base_param + case.epsilon),
        case.frame_count,
    );
    let primal_outputs_minus = run_interp_temp_source(
        &format!("{}-minus", case.stem),
        &(case.build_primal_source)(case.base_param - case.epsilon),
        case.frame_count,
    );

    assert_eq!(
        fad_outputs.len(),
        case.primal_outputs * 2,
        "{}: one-seed FAD layout must be [p0, t0, p1, t1, ...]",
        case.stem
    );
    assert_eq!(primal_outputs_base.len(), case.primal_outputs);
    assert_eq!(primal_outputs_plus.len(), case.primal_outputs);
    assert_eq!(primal_outputs_minus.len(), case.primal_outputs);

    for primal_index in 0..case.primal_outputs {
        for frame in 0..case.frame_count {
            let actual_primal = fad_outputs[primal_index * 2][frame];
            let expected_primal = primal_outputs_base[primal_index][frame];
            assert_close(
                actual_primal,
                expected_primal,
                case.abs_tol,
                &format!("{} primal[{primal_index}] frame {frame}", case.stem),
            );

            let expected_tangent = (primal_outputs_plus[primal_index][frame]
                - primal_outputs_minus[primal_index][frame])
                / (2.0 * case.epsilon);
            let actual_tangent = fad_outputs[primal_index * 2 + 1][frame];
            assert_close(
                actual_tangent,
                expected_tangent,
                case.abs_tol,
                &format!("{} tangent[{primal_index}] frame {frame}", case.stem),
            );
        }
    }
}

/// Generic harness for multi-seed FAD: compares each tangent lane against its
/// own central finite difference, by perturbing one seed at a time.
///
/// `build_fad_source(seeds)` must yield a `fad(expr, (s_0, …, s_{N-1}))`
/// program with exactly `primal_outputs · (1 + N)` outputs in the standard
/// interleaved order. `build_primal_source(seeds)` builds the same `expr`
/// without `fad(...)` for ground-truth evaluation. `base_seeds`,
/// `epsilons`, and `abs_tol` parameterise the finite-difference comparison.
#[allow(clippy::too_many_arguments)]
fn assert_multi_seed_fad_matches_central_difference<BuildFad, BuildPrimal>(
    stem: &str,
    primal_outputs: usize,
    frame_count: usize,
    base_seeds: &[f32],
    epsilons: &[f32],
    abs_tol: f32,
    build_fad_source: BuildFad,
    build_primal_source: BuildPrimal,
) where
    BuildFad: Fn(&[f32]) -> String,
    BuildPrimal: Fn(&[f32]) -> String,
{
    assert_eq!(
        base_seeds.len(),
        epsilons.len(),
        "{stem}: base_seeds and epsilons must agree on N"
    );
    let n = base_seeds.len();

    let fad_outputs = run_interp_temp_source(
        &format!("{stem}-fad"),
        &build_fad_source(base_seeds),
        frame_count,
    );
    let primal_outputs_base = run_interp_temp_source(
        &format!("{stem}-primal"),
        &build_primal_source(base_seeds),
        frame_count,
    );

    assert_eq!(
        fad_outputs.len(),
        primal_outputs * (1 + n),
        "{stem}: multi-seed FAD layout must be [p_i, t_i_s0, …, t_i_s{{N-1}}]*"
    );
    assert_eq!(primal_outputs_base.len(), primal_outputs);

    let mut primal_plus = Vec::with_capacity(n);
    let mut primal_minus = Vec::with_capacity(n);
    for j in 0..n {
        let mut up = base_seeds.to_vec();
        up[j] += epsilons[j];
        let mut dn = base_seeds.to_vec();
        dn[j] -= epsilons[j];
        primal_plus.push(run_interp_temp_source(
            &format!("{stem}-plus-{j}"),
            &build_primal_source(&up),
            frame_count,
        ));
        primal_minus.push(run_interp_temp_source(
            &format!("{stem}-minus-{j}"),
            &build_primal_source(&dn),
            frame_count,
        ));
    }

    for primal_index in 0..primal_outputs {
        for frame in 0..frame_count {
            let actual_primal = fad_outputs[primal_index * (1 + n)][frame];
            let expected_primal = primal_outputs_base[primal_index][frame];
            assert_close(
                actual_primal,
                expected_primal,
                abs_tol,
                &format!("{stem} primal[{primal_index}] frame {frame}"),
            );

            for j in 0..n {
                let expected_tangent = (primal_plus[j][primal_index][frame]
                    - primal_minus[j][primal_index][frame])
                    / (2.0 * epsilons[j]);
                let actual_tangent = fad_outputs[primal_index * (1 + n) + 1 + j][frame];
                assert_close(
                    actual_tangent,
                    expected_tangent,
                    abs_tol,
                    &format!("{stem} tangent[{primal_index}/seed {j}] frame {frame}"),
                );
            }
        }
    }
}

#[test]
fn fastlane_interp_self_recursive_fad_matches_closed_form_recurrence() {
    let outputs = run_interp_file(&corpus_path("fad_recursive_parametric_self.dsp"), 6);
    assert_eq!(outputs.len(), 2);

    let p = 0.2_f32;
    let mut primal_prev = 0.0_f32;
    let mut tangent_prev = 0.0_f32;
    for (frame, (&actual_primal, &actual_tangent)) in
        outputs[0].iter().zip(outputs[1].iter()).enumerate().take(6)
    {
        let primal = p * primal_prev + 2.0;
        let tangent = primal_prev + p * tangent_prev;

        assert_close(
            actual_primal,
            primal,
            1.0e-6,
            &format!("fad_recursive_parametric_self primal frame {frame}"),
        );
        assert_close(
            actual_tangent,
            tangent,
            1.0e-6,
            &format!("fad_recursive_parametric_self tangent frame {frame}"),
        );

        primal_prev = primal;
        tangent_prev = tangent;
    }
}

#[test]
fn fastlane_interp_waveform_lookup_fad_matches_central_difference() {
    fn fad_source(k: f32) -> String {
        format!(
            r#"
k = hslider("k", {k}, 1, 6, 1);
process = fad(rdtable(waveform{{0, 1, 4, 9, 16, 25, 36, 49}}, k), k);
"#
        )
    }

    fn primal_source(k: f32) -> String {
        format!(
            r#"
k = hslider("k", {k}, 1, 6, 1);
process = rdtable(waveform{{0, 1, 4, 9, 16, 25, 36, 49}}, k);
"#
        )
    }

    assert_single_seed_fad_matches_central_difference(CentralDifferenceCase {
        stem: "fad-waveform-lookup",
        primal_outputs: 1,
        frame_count: 4,
        base_param: 3.0,
        epsilon: 1.0,
        abs_tol: 1.0e-6,
        build_fad_source: fad_source,
        build_primal_source: primal_source,
    });
}

#[test]
fn fastlane_interp_readonly_generator_lookup_fad_matches_central_difference() {
    fn fad_source(k: f32) -> String {
        format!(
            r#"
k = hslider("k", {k}, 1, 6, 1);
process = fad(rdtable(8, 1 : + ~ _, k), k);
"#
        )
    }

    fn primal_source(k: f32) -> String {
        format!(
            r#"
k = hslider("k", {k}, 1, 6, 1);
process = rdtable(8, 1 : + ~ _, k);
"#
        )
    }

    assert_single_seed_fad_matches_central_difference(CentralDifferenceCase {
        stem: "fad-readonly-generator-lookup",
        primal_outputs: 1,
        frame_count: 4,
        base_param: 3.0,
        epsilon: 1.0,
        abs_tol: 1.0e-6,
        build_fad_source: fad_source,
        build_primal_source: primal_source,
    });
}

#[test]
fn fastlane_interp_recursive_table_index_fad_matches_central_difference() {
    fn fad_source(step: f32) -> String {
        format!(
            r#"
step = hslider("step", {step}, 0, 2, 1);
phase = step : + ~ _;
process = fad(rdtable(32, 1 : + ~ _, phase), step);
"#
        )
    }

    fn primal_source(step: f32) -> String {
        format!(
            r#"
step = hslider("step", {step}, 0, 2, 1);
phase = step : + ~ _;
process = rdtable(32, 1 : + ~ _, phase);
"#
        )
    }

    assert_single_seed_fad_matches_central_difference(CentralDifferenceCase {
        stem: "fad-recursive-table-index",
        primal_outputs: 1,
        frame_count: 4,
        base_param: 1.0,
        epsilon: 1.0,
        abs_tol: 1.0e-6,
        build_fad_source: fad_source,
        build_primal_source: primal_source,
    });
}

#[test]
fn fastlane_interp_nested_recursive_fad_matches_central_difference() {
    fn fad_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
inner = 2 : + ~ *(p);
outer = 1 : + ~ *(inner);
process = fad(outer, p);
"#
        )
    }

    fn primal_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
inner = 2 : + ~ *(p);
outer = 1 : + ~ *(inner);
process = outer;
"#
        )
    }

    assert_single_seed_fad_matches_central_difference(CentralDifferenceCase {
        stem: "fad-nested-recursive",
        primal_outputs: 1,
        frame_count: 8,
        base_param: 0.2,
        epsilon: 1.0e-3,
        abs_tol: 5.0e-3,
        build_fad_source: fad_source,
        build_primal_source: primal_source,
    });
}

#[test]
fn fastlane_interp_triple_nested_recursive_fad_matches_central_difference() {
    fn fad_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
inner = 2 : + ~ *(p);
middle = 1 : + ~ *(inner);
outer = 0.5 : + ~ *(middle);
process = fad(outer, p);
"#
        )
    }

    fn primal_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
inner = 2 : + ~ *(p);
middle = 1 : + ~ *(inner);
outer = 0.5 : + ~ *(middle);
process = outer;
"#
        )
    }

    assert_single_seed_fad_matches_central_difference(CentralDifferenceCase {
        stem: "fad-triple-nested-recursive",
        primal_outputs: 1,
        frame_count: 5,
        base_param: 0.2,
        epsilon: 1.0e-3,
        abs_tol: 1.0e1,
        build_fad_source: fad_source,
        build_primal_source: primal_source,
    });
}

#[test]
fn fastlane_interp_multi_output_recursive_fad_matches_central_difference() {
    fn fad_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
bus2 = _,_;
process = fad(bus2 ~ (*(p), *(0.25)), p);
"#
        )
    }

    fn primal_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
bus2 = _,_;
process = bus2 ~ (*(p), *(0.25));
"#
        )
    }

    assert_single_seed_fad_matches_central_difference(CentralDifferenceCase {
        stem: "fad-multi-output-recursive",
        primal_outputs: 2,
        frame_count: 8,
        base_param: 0.2,
        epsilon: 1.0e-3,
        abs_tol: 2.0e-3,
        build_fad_source: fad_source,
        build_primal_source: primal_source,
    });
}

#[test]
fn fastlane_interp_mutual_recursive_fad_matches_central_difference() {
    fn fad_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
bus2 = _,_;
cross2 = _,_ <: !,_,_,!;
process = fad(bus2 ~ ((*(p), *(0.25)) : cross2), p);
"#
        )
    }

    fn primal_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
bus2 = _,_;
cross2 = _,_ <: !,_,_,!;
process = bus2 ~ ((*(p), *(0.25)) : cross2);
"#
        )
    }

    assert_single_seed_fad_matches_central_difference(CentralDifferenceCase {
        stem: "fad-mutual-recursive",
        primal_outputs: 2,
        frame_count: 8,
        base_param: 0.2,
        epsilon: 1.0e-3,
        abs_tol: 2.0e-3,
        build_fad_source: fad_source,
        build_primal_source: primal_source,
    });
}

#[test]
fn fastlane_interp_multi_seed_recursive_fad_matches_central_difference_per_seed() {
    fn fad_source(a: f32, b: f32) -> String {
        format!(
            r#"
a = hslider("a", {a}, -0.9, 0.9, 0.001);
b = hslider("b", {b}, -2.0, 2.0, 0.001);
process = fad((b : + ~ *(a)), (a, b));
"#
        )
    }

    fn primal_source(a: f32, b: f32) -> String {
        format!(
            r#"
a = hslider("a", {a}, -0.9, 0.9, 0.001);
b = hslider("b", {b}, -2.0, 2.0, 0.001);
process = (b : + ~ *(a));
"#
        )
    }

    let base_a = 0.2_f32;
    let base_b = 1.0_f32;
    let epsilon = 1.0e-3_f32;
    let frame_count = 8;

    let fad_outputs = run_interp_temp_source(
        "fad-multi-seed-recursive-fad",
        &fad_source(base_a, base_b),
        frame_count,
    );
    let primal_outputs = run_interp_temp_source(
        "fad-multi-seed-recursive-primal",
        &primal_source(base_a, base_b),
        frame_count,
    );
    let primal_plus_a = run_interp_temp_source(
        "fad-multi-seed-recursive-plus-a",
        &primal_source(base_a + epsilon, base_b),
        frame_count,
    );
    let primal_minus_a = run_interp_temp_source(
        "fad-multi-seed-recursive-minus-a",
        &primal_source(base_a - epsilon, base_b),
        frame_count,
    );
    let primal_plus_b = run_interp_temp_source(
        "fad-multi-seed-recursive-plus-b",
        &primal_source(base_a, base_b + epsilon),
        frame_count,
    );
    let primal_minus_b = run_interp_temp_source(
        "fad-multi-seed-recursive-minus-b",
        &primal_source(base_a, base_b - epsilon),
        frame_count,
    );

    assert_eq!(fad_outputs.len(), 3);
    assert_eq!(primal_outputs.len(), 1);

    for frame in 0..frame_count {
        assert_close(
            fad_outputs[0][frame],
            primal_outputs[0][frame],
            2.0e-3,
            &format!("fad-multi-seed-recursive primal frame {frame}"),
        );

        let expected_da = (primal_plus_a[0][frame] - primal_minus_a[0][frame]) / (2.0 * epsilon);
        assert_close(
            fad_outputs[1][frame],
            expected_da,
            2.0e-3,
            &format!("fad-multi-seed-recursive da frame {frame}"),
        );

        let expected_db = (primal_plus_b[0][frame] - primal_minus_b[0][frame]) / (2.0 * epsilon);
        assert_close(
            fad_outputs[2][frame],
            expected_db,
            2.0e-3,
            &format!("fad-multi-seed-recursive db frame {frame}"),
        );
    }
}

#[test]
fn fastlane_interp_mutual_multi_seed_recursive_fad_matches_central_difference_per_seed() {
    fn fad_source(p: f32, q: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
q = hslider("q", {q}, -0.9, 0.9, 0.001);
bus2 = _,_;
cross2 = _,_ <: !,_,_,!;
process = fad(bus2 ~ ((*(p), *(q)) : cross2), (p, q));
"#
        )
    }

    fn primal_source(p: f32, q: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
q = hslider("q", {q}, -0.9, 0.9, 0.001);
bus2 = _,_;
cross2 = _,_ <: !,_,_,!;
process = bus2 ~ ((*(p), *(q)) : cross2);
"#
        )
    }

    let base_p = 0.2_f32;
    let base_q = 0.35_f32;
    let epsilon = 1.0e-3_f32;
    let frame_count = 8;

    let fad_outputs = run_interp_temp_source(
        "fad-mutual-multi-seed-recursive-fad",
        &fad_source(base_p, base_q),
        frame_count,
    );
    let primal_outputs = run_interp_temp_source(
        "fad-mutual-multi-seed-recursive-primal",
        &primal_source(base_p, base_q),
        frame_count,
    );
    let primal_plus_p = run_interp_temp_source(
        "fad-mutual-multi-seed-recursive-plus-p",
        &primal_source(base_p + epsilon, base_q),
        frame_count,
    );
    let primal_minus_p = run_interp_temp_source(
        "fad-mutual-multi-seed-recursive-minus-p",
        &primal_source(base_p - epsilon, base_q),
        frame_count,
    );
    let primal_plus_q = run_interp_temp_source(
        "fad-mutual-multi-seed-recursive-plus-q",
        &primal_source(base_p, base_q + epsilon),
        frame_count,
    );
    let primal_minus_q = run_interp_temp_source(
        "fad-mutual-multi-seed-recursive-minus-q",
        &primal_source(base_p, base_q - epsilon),
        frame_count,
    );

    assert_eq!(fad_outputs.len(), 6);
    assert_eq!(primal_outputs.len(), 2);

    for primal_index in 0..2 {
        for frame in 0..frame_count {
            assert_close(
                fad_outputs[primal_index * 3][frame],
                primal_outputs[primal_index][frame],
                3.0e-3,
                &format!("fad-mutual-multi-seed-recursive primal[{primal_index}] frame {frame}"),
            );

            let expected_dp = (primal_plus_p[primal_index][frame]
                - primal_minus_p[primal_index][frame])
                / (2.0 * epsilon);
            assert_close(
                fad_outputs[primal_index * 3 + 1][frame],
                expected_dp,
                3.0e-3,
                &format!("fad-mutual-multi-seed-recursive dp[{primal_index}] frame {frame}"),
            );

            let expected_dq = (primal_plus_q[primal_index][frame]
                - primal_minus_q[primal_index][frame])
                / (2.0 * epsilon);
            assert_close(
                fad_outputs[primal_index * 3 + 2][frame],
                expected_dq,
                3.0e-3,
                &format!("fad-mutual-multi-seed-recursive dq[{primal_index}] frame {frame}"),
            );
        }
    }
}

#[test]
fn fastlane_interp_nested_mutual_recursive_fad_matches_central_difference() {
    fn fad_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
bus2 = _,_;
cross2 = _,_ <: !,_,_,!;
core = bus2 ~ ((*(p), *(0.25)) : cross2);
mix = 1 : + ~ *(core : +);
process = fad(mix, p);
"#
        )
    }

    fn primal_source(p: f32) -> String {
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
bus2 = _,_;
cross2 = _,_ <: !,_,_,!;
core = bus2 ~ ((*(p), *(0.25)) : cross2);
mix = 1 : + ~ *(core : +);
process = mix;
"#
        )
    }

    assert_single_seed_fad_matches_central_difference(CentralDifferenceCase {
        stem: "fad-nested-mutual-recursive",
        primal_outputs: 1,
        frame_count: 8,
        base_param: 0.2,
        epsilon: 1.0e-3,
        abs_tol: 2.0e-2,
        build_fad_source: fad_source,
        build_primal_source: primal_source,
    });
}

#[test]
fn fastlane_interp_multi_seed_nested_recursive_fad_matches_central_difference_per_seed() {
    fn fad_source(seeds: &[f32]) -> String {
        let (a, b) = (seeds[0], seeds[1]);
        format!(
            r#"
a = hslider("a", {a}, -0.9, 0.9, 0.001);
b = hslider("b", {b}, -0.9, 0.9, 0.001);
inner = 2 : + ~ *(a);
outer = 1 : + ~ *(b * inner);
process = fad(outer, (a, b));
"#
        )
    }

    fn primal_source(seeds: &[f32]) -> String {
        let (a, b) = (seeds[0], seeds[1]);
        format!(
            r#"
a = hslider("a", {a}, -0.9, 0.9, 0.001);
b = hslider("b", {b}, -0.9, 0.9, 0.001);
inner = 2 : + ~ *(a);
outer = 1 : + ~ *(b * inner);
process = outer;
"#
        )
    }

    assert_multi_seed_fad_matches_central_difference(
        "fad-multi-seed-nested-recursive",
        1,
        6,
        &[0.2, 0.15],
        &[1.0e-3, 1.0e-3],
        5.0e-2,
        fad_source,
        primal_source,
    );
}

#[test]
fn fastlane_interp_multi_seed_nested_mutual_recursive_fad_matches_central_difference_per_seed() {
    fn fad_source(seeds: &[f32]) -> String {
        let (p, q) = (seeds[0], seeds[1]);
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
q = hslider("q", {q}, -0.9, 0.9, 0.001);
bus2 = _,_;
cross2 = _,_ <: !,_,_,!;
core = bus2 ~ ((*(p), *(q)) : cross2);
mix = 1 : + ~ *(core : +);
process = fad(mix, (p, q));
"#
        )
    }

    fn primal_source(seeds: &[f32]) -> String {
        let (p, q) = (seeds[0], seeds[1]);
        format!(
            r#"
p = hslider("p", {p}, -0.9, 0.9, 0.001);
q = hslider("q", {q}, -0.9, 0.9, 0.001);
bus2 = _,_;
cross2 = _,_ <: !,_,_,!;
core = bus2 ~ ((*(p), *(q)) : cross2);
mix = 1 : + ~ *(core : +);
process = mix;
"#
        )
    }

    assert_multi_seed_fad_matches_central_difference(
        "fad-multi-seed-nested-mutual-recursive",
        1,
        6,
        &[0.2, 0.35],
        &[1.0e-3, 1.0e-3],
        5.0e-2,
        fad_source,
        primal_source,
    );
}

#[test]
fn recursive_projection_seed_stays_a_leaf_inside_an_unrelated_recursion() {
    // The seed is the recursive input `g` itself, i.e. `delay1(proj(0, ref(1)))`
    // in de Bruijn form. The loss also reads `x`, an LCG whose own back-edge
    // is spelled exactly the same way *inside its own body*. The outer-scope
    // seed must not leak into that body: `x` does not depend on `g`, so the
    // `fad` gradient has to equal the hand-written partial derivative
    // `2 (g x - t) x` on every frame. Before the cache was scoped per
    // recursion, the LCG state picked up a tangent of 1 and `x` a spurious
    // tangent of ~0.5.
    let source = r#"
x = (+(12345) ~ *(1103515245)) * 4.656612873077393e-10;
target = 0.7 * x;
lr = 0.01;
loss(g) = (g * x - target) * (g * x - target);
process = (loop ~ _) : !, _, _
with {
    loop(g) = g - lr * gf, gf, gm
    with {
        gf = fad(loss(g), g) : !, _;
        gm = 2.0 * (g * x - target) * x;
    };
};
"#;
    let outputs = run_interp_temp_source("recursive-seed-unrelated-recursion", source, 256);
    assert_eq!(outputs.len(), 2);
    for (frame, (&fad_grad, &hand_grad)) in outputs[0].iter().zip(&outputs[1]).enumerate() {
        assert_close(
            fad_grad,
            hand_grad,
            1.0e-6,
            &format!("recursive-seed gradient vs hand-written gradient at frame {frame}"),
        );
    }
}
