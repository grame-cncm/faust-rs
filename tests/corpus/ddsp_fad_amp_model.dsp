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

x = 0.8 * no.noise;
amp(ldrive, gain, tone, sig) = gain * ma.tanh(exp(ldrive) * sig) : si.smooth(tone);
target = amp(log(4.0), 0.7, 0.8, x);

loss(ld, g, t) = op.mse(amp(ld, g, t, x), target);
adam = op.adam_g(0.002, 0.9, 0.999, 1e-8);
learned = op.descend_3D(loss, adam, adam, adam,
                        log(0.5), log(8.0), 0.0, 2.0, 0.0, 0.95,
                        log(1.0), 1.0, 0.5, 0.0);
ld = learned : _, !, !;
g  = learned : !, _, !;
t  = learned : !, !, _;

process = exp(ld), g, t, target - amp(ld, g, t, x);
