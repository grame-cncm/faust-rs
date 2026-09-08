// DDSP example (FAD): an amplifier model learned end to end.
//
// The model is the smallest "amp": a drive into a tanh saturation, a tone
// control (a one-pole low-pass), a gain. Given the output of a hidden amp on
// noise, the three parameters are learned by gradient descent on the
// waveform error with one Adam per parameter (`descend_3D`). The drive is
// learned in the log domain (a multiplicative parameter), the tone as the
// pole coefficient, bounded below 0.95.
//
// `fad` differentiates through the foreign `tanh` and through the one-pole
// recursion (the derivative with respect to the pole coefficient depends on
// the whole past of the filter), so the three gradients are exact.
//
// Convergence: (drive, gain, tone) -> (4, 0.7, 0.8) from (1, 1, 0.5).
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [drive, gain, tone, residual]

import("stdfaust.lib");
op = library("optimizers.lib");

// ---- the excitation and the model
x = 0.8 * no.noise;
// drive into a tanh saturation, then a one-pole "tone", then a gain. The
// drive is passed as its logarithm so that the learned parameter is
// additive, exp(ldrive) always positive. `si.smooth(tone)` is the one-pole
// y = (1 - tone) x + tone y': the higher tone, the lower its cutoff, and it
// must stay below 1 to be stable, hence the bound 0.95 below.
amp(ldrive, gain, tone, sig) = gain * ma.tanh(exp(ldrive) * sig) : si.smooth(tone);
// the hidden amplifier: drive 4, gain 0.7, tone 0.8
target = amp(log(4.0), 0.7, 0.8, x);

// ---- the learning loop
// The loss as a function of the three parameters: the squared error of the
// model against the target at the current sample (`op.mse(y, t)` is
// (y - t)^2; the engines average it over time).
loss(ld, g, t) = op.mse(amp(ld, g, t, x), target);
// one Adam engine per parameter: rate 0.002, the moments of the paper, eps 1e-8
adam = op.adam_g(0.002, 0.9, 0.999, 1e-8);
// descend_3D(loss, upd1, upd2, upd3, lo1, hi1, lo2, hi2, lo3, hi3,
//            init1, init2, init3, reset): `fad` on the loss gives the three
// gradients at once; the bounds are ldrive in [log 0.5, log 8], gain in
// [0, 2], tone in [0, 0.95]; the start is (log 1, 1, 0.5), i.e. drive 1;
// the reset input is the constant 0. The three outputs are the parameters.
learned = op.descend_3D(loss, adam, adam, adam,
                        log(0.5), log(8.0), 0.0, 2.0, 0.0, 0.95,
                        log(1.0), 1.0, 0.5, 0.0);
// Faust has no destructuring: each parameter is a projection of the bus
ld = learned : _, !, !;
g  = learned : !, _, !;
t  = learned : !, !, _;

// ---- outputs
// [0] the learned drive, linear (exp of the log parameter): 1 -> 4
// [1] the learned gain: 1 -> 0.7
// [2] the learned tone, the pole coefficient of the one-pole: 0.5 -> 0.8
// [3] the residual, target - model on the same excitation: -> 0 as the three
//     parameters converge
process = exp(ld), g, t, target - amp(ld, g, t, x);
