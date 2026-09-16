//! Compile-time budget for a `fad` whose seed carries a large expression.
//!
//! Regression for the 2026-09-16 finding: `op.lsq_1D` on a waveguide string,
//! with the initial estimate a `K`-lane autocorrelation fold, compiled in
//! 34 s at `K = 8` and 115 s at `K = 24` (about 4 s per lane in release),
//! against 0.07 s for the same program without the `fad`. Two memos were
//! missing: the exact Box-to-Signal result memo was switched off for any root
//! containing an AD node (`propagate::result_memo`), and the evaluator's arity
//! oracle (`eval::apply::infer_box_arity`) walked the shared box DAG once per
//! path. The seed `p1` is mentioned a dozen times by the interpolated delay of
//! the model, and it carries the whole estimate, so both costs multiplied by
//! the number of mentions and by `K`.
//!
//! The program below is a self-contained transcription of that shape (no
//! library import, as `AGENTS.md` §3 requires of tests): the string, a
//! four-tap Lagrange fractional delay, `smooth`, the `ema`/`nlms`/`lsq_1D`
//! idioms of `optimizers.lib`, and the `K`-lane fold. Fixed, it compiles in
//! about 60 ms at `K = 32` in release; the budget below is the same test in a
//! debug build with a wide margin, and the pre-fix compiler needs minutes at
//! this `K` in release alone.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use codegen::backends::cpp::CppOptions;
use compiler::{Compiler, SignalFirLane};

/// Wall-clock ceiling for one compilation of the `K`-lane program in a debug
/// build. The memoized compiler takes well under a second; the unmemoized one
/// scaled at about 4 s per lane in a release build.
const BUDGET: Duration = Duration::from_secs(120);

fn lsq_string_program(lanes: usize) -> String {
    format!(
        r#"
SR = fconstant(int fSamplingFreq, <math.h>);
noise = (+(12345) ~ *(1103515245)) / 2147483647.0;
smooth(s) = *(1.0 - s) : + ~ *(s);
frac(x) = x - floor(x);
delay(n, d, x) = x @ min(n, max(0, d));
fdelay4(n, d, x) =
    delay(n, id, x) * fdm1 * fdm2 * fdm3 / (0.0 - 6.0)
  + delay(n, id + 1, x) * fd * fdm2 * fdm3 / 2.0
  + delay(n, id + 2, x) * fd * fdm1 * fdm3 / (0.0 - 2.0)
  + delay(n, id + 3, x) * fd * fdm1 * fdm2 / 6.0
with {{
    o = 1.49999;
    dmo = d - o;
    id = int(dmo);
    fd = o + frac(dmo);
    fdm1 = fd - 1.0;
    fdm2 = fd - 2.0;
    fdm3 = fd - 3.0;
}};
bus(n) = par(i, n, _);
clip(lo, hi, x) = max(lo, min(hi, x));
ema(a, x) = x : smooth(a);
nlms(mu, eps, a, r, j) = mu * r * j / (eps + ema(a, j * j));
dev(reset, delta) = select2(reset != 0.0, delta, 0.0);
from_init(init, reset, delta) = init + dev(reset, delta);
lsq_1D(mdl, upd1, lo1, hi1, init1, reset, target, x) = (loop ~ _) : +(init1)
with {{
    loop(prev1) = next1
    with {{
        p1 = clip(lo1, hi1, from_init(init1, reset, prev1));
        pt = fad(mdl(p1, x), p1);
        r = (pt : _, !) - target;
        j1 = pt : !, _;
        next1 = clip(lo1 - init1, hi1 - init1, dev(reset, prev1) - upd1(r, j1));
    }};
}};
MAXD = 512;
x = 0.1 * noise;
string(d, g, s) = (+(s) : fdelay4(MAXD, d - 1.0) : *(g) : smooth(0.3)) ~ _;
target = string(SR / 220.0, 0.95, x);
mdl(d, s) = string(d, 0.95, s);
L0 = 147;
K = {lanes};
acf(k) = ema(0.999, target * (target @ (L0 + k)));
lanes = par(k, K, (acf(k), float(L0 + k)));
pick(bv, bl, v, l) = select2(v > bv, bv, v), select2(v > bv, bl, l);
best_lag = lanes : seq(i, K - 1, (pick, bus(2 * (K - 2 - i)))) : !, _;
estimate = best_lag * 0.98;
d = lsq_1D(mdl, nlms(0.02, 0.000001, 0.99), 100.0, 400.0, estimate, 0.0, target, x);
process = SR / d, target - mdl(d, x);
"#
    )
}

/// Compiles the `K`-lane program to C++ on a worker thread and returns the
/// generated text with the elapsed time, or `None` past [`BUDGET`].
fn compile_within_budget(lanes: usize) -> Option<(String, Duration)> {
    let (sender, receiver) = mpsc::channel();
    let source = lsq_string_program(lanes);
    let name = format!("lsq_string_k{lanes}");
    std::thread::Builder::new()
        .name(name.clone())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let started = Instant::now();
            let cpp = Compiler::new()
                .compile_source_to_cpp_with_lane(
                    &name,
                    &source,
                    &CppOptions::default(),
                    SignalFirLane::TransformFastLane,
                )
                .unwrap_or_else(|error| panic!("{name} C++ compilation failed: {error}"));
            let _ = sender.send((cpp, started.elapsed()));
        })
        .expect("spawn compile worker");
    receiver.recv_timeout(BUDGET).ok()
}

#[test]
fn fad_seed_with_a_large_estimate_compiles_within_budget() {
    let (cpp, elapsed) = compile_within_budget(32)
        .unwrap_or_else(|| panic!("the 32-lane lsq string did not compile within {BUDGET:?}"));
    assert!(
        cpp.contains("virtual void compute("),
        "generated C++ must contain a compute method"
    );
    eprintln!("lsq string, K = 32: compiled in {elapsed:?}");
}

/// The cost must not scale with the number of mentions of the seed: the same
/// program at four times the lanes compiles in the same order of time. The
/// tolerance is wide because a debug build's timings are noisy; the pre-fix
/// compiler scaled linearly in `K` with seconds per lane.
#[test]
fn fad_seed_cost_does_not_scale_with_the_estimate() {
    let (_, small) = compile_within_budget(8)
        .unwrap_or_else(|| panic!("the 8-lane lsq string did not compile within {BUDGET:?}"));
    let (_, large) = compile_within_budget(32)
        .unwrap_or_else(|| panic!("the 32-lane lsq string did not compile within {BUDGET:?}"));
    eprintln!("lsq string: K = 8 in {small:?}, K = 32 in {large:?}");
    assert!(
        large < Duration::from_secs(20),
        "K = 32 took {large:?}; the lane count must not multiply the fad cost"
    );
}
