// DDSP example (FAD): hum cancellation with an adaptive notch.
//
// The input is a 1 kHz "hum" (a sine at 0.5) buried in a little noise. A
// constrained notch filter -- zeros on the unit circle at +-w, poles at
// radius r behind them -- removes one frequency; which one is learned by
// minimising the output power, the classic adaptive notch of Nehorai (1985)
// and Rao & Kung (1984). The learned parameter is c = cos(w), so that the
// notch stays constrained by construction; the frequency in hertz is read
// back with acos.
//
// The notch is recursive: the derivative of its output with respect to c
// depends on the whole past of the filter. `fad` carries that derivative
// along with the state (the RTRL derivative), so the sensitivity is exact
// where the classic algorithms use a simplified gradient. The loop is
// `lsq_1D` with a target of zero and the normalised engine `nlms`: the step
// `mu r j / E[j^2]` is proportional to the residual, so it settles by itself
// once the null is found -- where an Adam step, normalised to ~lr per
// sample, random-walks by lr at the optimum (a jitter of +-25 Hz here).
//
// Convergence: c -> cos(2 pi 1000 / SR) from 1400 Hz within a few thousand
// samples, then 1000.0 +- 0.1 Hz; the residual falls to the noise floor.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual, learned frequency in Hz]

import("stdfaust.lib");
op = library("optimizers.lib");

// ---- the signal to clean: a sine 28 dB above a white noise floor
f0 = 1000.0;                          // the hum, hidden from the learner
r = 0.95;                             // pole radius: the width of the null
hum = 0.5 * os.osc(f0);
x = hum + 0.02 * no.noise;

// ---- the model
// Constrained notch: H(z) = (1 - 2c z^-1 + z^-2) / (1 - 2rc z^-1 + r^2 z^-2)
// The numerator puts two zeros on the unit circle at the angles +-w, with
// c = cos(w); the denominator puts two poles at the same angles at radius r,
// so the response is flat away from w and the null gets narrower as r
// approaches 1. `fi.tf2(b0, b1, b2, a1, a2)` is the direct-form biquad,
// y = b0 x + b1 x' + b2 x'' - a1 y' - a2 y''. The one learned parameter c
// enters the numerator and the denominator together, which keeps the poles
// behind the zeros whatever step the optimiser takes: the constraint is in
// the shape of the model, not in the loop.
notch(c, sig) = sig : fi.tf2(1.0, -2.0 * c, 1.0, -2.0 * r * c, r * r);

// ---- the learning loop
// The start: the cosine of 1400 Hz, 400 Hz above the hum.
c0 = cos(2.0 * ma.PI * 1400.0 / ma.SR);
// lsq_1D(mdl, engine, lo, hi, init, reset, target, x): least squares of the
// model output against the target, here 0.0, so the loop minimises the output
// power. Each sample it has the residual r = notch(c, x) - 0 and the
// sensitivity j = d notch / dc from `fad`, exact through the recursion, and
// the NLMS engine steps c by mu r j / (eps + E[j^2]) with mu = 0.002,
// eps = 1e-6 and the power E[j^2] smoothed with a = 0.99. The bounds [-1, 1]
// keep c a cosine; the reset input is the constant 0.
c = op.lsq_1D(notch, op.nlms(0.002, 0.000001, 0.99), -1.0, 1.0, c0, 0.0, 0.0, x);
// the frequency of the null, read back from c = cos(2 pi f / SR)
f_hz = acos(c) * ma.SR / (2.0 * ma.PI);

// ---- outputs
// [0] the cleaned signal: x through the notch. Once c has converged the hum
//     is gone and what remains is the noise, 0.02 rms; the residual power is
//     the quantity the loop minimises.
// [1] the learned frequency of the null in Hz: starts at 1400, reaches
//     1000.0 +- 0.1 within a few thousand samples and stays there (the NLMS
//     step vanishes with the residual).
process = notch(c, x), f_hz;
