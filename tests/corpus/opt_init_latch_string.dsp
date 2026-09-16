// optimizers.lib: a loop started from an outside estimate with
// `init_latch(T, estimate)` and `init_reset(T)`.
//
// The waveguide string of ddsp_fad_waveguide_string_pitch.dsp: the waveform
// loss between two strings is a well +-1 Hz wide around the target on a flat
// plateau, captured from above only (228 Hz locks on 220, 200 and 176 Hz
// drift away), so the plain loop needs a start chosen by hand. Here no start
// is chosen: for T = 8 192 samples the loop is held by `init_reset` at
// `init`, which follows an estimate of the target's pitch as it is observed
// -- the lag of the peak of the target's smoothed autocorrelation over a grid
// of integer lags from 161 to 279 Hz (the standard zero-crossing tracker
// `an.pitchTracker` reads hundreds of hertz or single digits on this
// noise-driven string, so the estimate is computed here). At T the estimate
// is frozen, shortened by 2 % to land on the side of the well the loop
// captures from, and the loop is released: from that init the pitch locks on
// 220 Hz. The estimate reaches the loop through `init`, which the 1D loops
// take as an input wire of their recursion since 0.9.0. (Closed over inside
// the body, an `init` of this size once multiplied the compile time by a
// hundred: three unmemoized walks of the compiler, fixed the same day, see
// porting/journal/2026-09-16.md; the input wire stays as the cleaner form.)
//
// Measured (faustprobe --double, 80 000 frames): the frozen init is 222.77 Hz
// (lag 202 x 0.98); the pitch is 219.74 Hz at 16 000 samples, 219.998 at
// 24 000, 220.000000 from 48 000 on; the residual falls under 1e-6.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [pitch in Hz, residual, latched init in Hz]

import("stdfaust.lib");
op = library("optimizers.lib");

MAXD = 512;
x = 0.1 * no.noise;
string(d, g, s) = (+(s) : de.fdelay4(MAXD, d - 1.0) : *(g) : si.smooth(0.3)) ~ _;

f_star = 220.0;
target = string(ma.SR / f_star, 0.95, x);
mdl(d, s) = string(d, 0.95, s);

// ---- the estimate: argmax over a grid of integer lags of the smoothed
// autocorrelation, 30 lags 4 samples apart from 158 to 274 samples (279 to
// 161 Hz), a 2 % grid at 220 Hz: the peak lag is within 1 % of the period,
// so with the 2 % shortening the start lands 0.8 to 2.8 % above the target.
L0 = 158;
STEP = 4;
K = 30;
acf(k) = op.ema(0.999, target * (target @ (L0 + STEP * k)));
lanes = par(k, K, (acf(k), float(L0 + STEP * k)));
pick(bv, bl, v, l) = select2(v > bv, bv, v), select2(v > bv, bl, l);
best_lag = lanes : seq(i, K - 1, (pick, si.bus(2 * (K - 2 - i)))) : !, _;
estimate = best_lag * 0.98;                // 2 % shorter: the pitch 2 % above

T = 8192.0;
init = op.init_latch(T, estimate);
d = op.lsq_1D(mdl, op.nlms(0.02, 0.000001, 0.99), 100.0, 400.0, init, op.init_reset(T), target, x);

process = ma.SR / d, target - mdl(d, x), ma.SR / init;
