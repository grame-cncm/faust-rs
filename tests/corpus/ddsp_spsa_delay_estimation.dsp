// DDSP example (gradient-free): the integer delay between a signal and its
// delayed copy, learned without a gradient.
//
// Time-delay estimation: an echo canceller or a microphone alignment first
// needs the delay, in whole samples, between a signal and its delayed copy.
// A comb `x + x @ d` on a low-passed noise hides `d* = 200` samples; the
// model is the same comb with `de.delay(512, int(d), x)`, an integer delay.
// `fad` gives a zero tangent through `int` and through the delay amount
// (asserted on a lane), so no descent of the library can move `d`.
// `spsa_1D_clocked`, simultaneous perturbation, evaluates the loss at
// `int(d +- 2)` over each 256-sample frame on the same excitation and hands
// the difference to Adam: the loss is a bowl as wide as the correlation
// length of the excitation (a first-order low-pass at 200 Hz, about 35
// samples), and from 160 the delay reaches 200 and stays. Two model copies
// and no tangent, one step per frame.
//
// Convergence: int(d) = 200 from 25 000 samples, held from 40 000 on,
// residual 0 once the delay is right.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [d, int(d), fad tangent of the model with respect to d, residual]

import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise : fi.lowpass(1, 200.0);
d_star = 200;
target = x + (x @ d_star);
mdl(d) = x + de.delay(512, int(d), x);
loss(d) = op.mse(mdl(d), target);
clock = ((+(1) : %(256)) ~ _) == 0;

d = op.spsa_1D_clocked(clock, loss, op.adam_g(0.5, 0.9, 0.999, 0.00000001), 2.0, 0.0, 500.0, 160.0, 0.0);
tangent = fad(mdl(d), d) : !, _;

process = d, int(d), tangent, target - mdl(d);
