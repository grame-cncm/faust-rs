// DDSP example (FAD): a waveguide string that tunes itself, a detector then
// the gradient.
//
// The plucked-string model of ddsp_fad_waveguide_string_pitch.dsp, whose
// waveform loss is a well +-1 Hz wide around the hidden pitch on a flat
// plateau, captured from above only: a start chosen by hand decides
// whether the gradient finds it. Here no start is chosen. For T = 8 192
// samples the loop is held at `init` by `init_reset` while `init` follows an
// estimate of the target's pitch: the lag of the peak of its smoothed
// autocorrelation over a grid of integer lags from 161 to 279 Hz (the
// standard zero-crossing tracker `an.pitchTracker` reads hundreds of hertz
// or single digits on this noise-driven string). At T the estimate is
// frozen by `init_latch`, shortened by 2 % so that the start lands on the
// side the well captures from, and the loop is released: normalised least
// squares on the waveform then locks the pitch on 220 Hz. This is the
// detector-then-gradient scheme of the DDSP systems, written in Faust, the
// detector costing thirty smoothed products and no model copy.
//
// Convergence: init frozen at 222.77 Hz; pitch 219.998 Hz at 24 000
// samples, 220.000000 from 48 000 on, residual under 1e-6.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
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
