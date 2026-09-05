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

f0 = 1000.0;                          // the hum, hidden from the learner
r = 0.95;                             // pole radius: the width of the null
hum = 0.5 * os.osc(f0);
x = hum + 0.02 * no.noise;

// Constrained notch: H(z) = (1 - 2c z^-1 + z^-2) / (1 - 2rc z^-1 + r^2 z^-2)
notch(c, sig) = sig : fi.tf2(1.0, -2.0 * c, 1.0, -2.0 * r * c, r * r);

c0 = cos(2.0 * ma.PI * 1400.0 / ma.SR);
c = op.lsq_1D(notch, op.nlms(0.002, 0.000001, 0.99), -1.0, 1.0, c0, 0.0, 0.0, x);
f_hz = acos(c) * ma.SR / (2.0 * ma.PI);

process = notch(c, x), f_hz;
