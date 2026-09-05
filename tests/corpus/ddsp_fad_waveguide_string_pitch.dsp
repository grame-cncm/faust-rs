// DDSP example (FAD): learning the pitch of a waveguide string through its
// fractional delay.
//
// A plucked-string model (Karplus-Strong / waveguide): a loop with a
// fractional delay of d samples (4th-order Lagrange interpolation), a loss
// gain and a one-pole damping. Given a hidden string of the same shape at
// 220 Hz driven by noise, the loop learns the delay length -- the pitch --
// by normalised least squares on the waveform: `fad` differentiates the
// loop output with respect to the delay length, through the interpolation
// (the derivative of an interpolated read with respect to the read
// position) and through the feedback, sample by sample. Tensor frameworks
// have no derivative with respect to a delay length.
//
// What the landscape allows. The waveform error between two strings is a
// deep well +-1 Hz wide around 220 Hz and a flat plateau elsewhere, and on
// the plateau the error's own gradient is not zero: a loop filter's group
// delay shifts the string's autocorrelation peak off the delay length, so
// the model's output power depends on d and the normalised step drifts
// toward *lower* pitch whatever the target -- a correlation loss
// (-model * target) removes that bias but has no pull on the plateau either.
// Fine tuning works: from 228 Hz (+3.6 %) the pitch locks to 220.000000 Hz;
// with the model's damping annealed from 0.70 to 0.95 (broad resonances
// first) it locks from 264 Hz (+20 %) too, while from below (200, 176 Hz)
// it drifts away. That is why DDSP systems estimate the pitch with a
// detector and let the gradient refine it.
//
// Convergence: d -> SR / 220 from SR / 228 within 60 000 samples, residual
// under 1e-6.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [pitch in Hz, residual]

import("stdfaust.lib");
op = library("optimizers.lib");

MAXD = 512;
x = 0.1 * no.noise;
// loop: input, fractional delay of d samples (one is the loop's own), gain, damping
string(d, g, s) = (+(s) : de.fdelay4(MAXD, d - 1.0) : *(g) : si.smooth(0.3)) ~ _;

f_star = 220.0;
target = string(ma.SR / f_star, 0.95, x);
mdl(d, s) = string(d, 0.95, s);
d = op.lsq_1D(mdl, op.nlms(0.02, 0.000001, 0.99), 100.0, 400.0, ma.SR / 228.0, 0.0, target, x);

process = ma.SR / d, target - mdl(d, x);
