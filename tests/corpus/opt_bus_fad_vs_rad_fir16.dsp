// optimizers.lib: the same sixteen-tap FIR learned by the two bus loss-first
// loops, `descend_N` (forward mode, sixteen tangents) and `descend_N_rad`
// (reverse mode, one sweep). The loss has no recursion between the taps and
// the output, so both loops compute the same gradient and follow the same
// trajectory; the two residuals are the same signal up to rounding.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual_fad, residual_rad]

import("stdfaust.lib");
op = library("optimizers.lib");

N = 16;
x = no.noise;
taps = x <: par(i, N, @(i));
fir(h) = (h, taps) : ro.interleave(N, 2) : par(i, N, *) :> _;
h_star(i) = sin(0.5 * i) * exp(-0.2 * i);
y_target = fir(par(i, N, h_star(i)));

fir_loss = fir(si.bus(N)) : sq_err with { sq_err(y) = op.mse(y, y_target); };
h_fad = op.descend_N(N, fir_loss, op.sgd_g(0.02), -2.0, 2.0, 0.0, 0.0);
h_rad = op.descend_N_rad(N, fir_loss, op.sgd_g(0.02), -2.0, 2.0, 0.0, 0.0);

process = y_target - fir(h_fad), y_target - fir(h_rad);
