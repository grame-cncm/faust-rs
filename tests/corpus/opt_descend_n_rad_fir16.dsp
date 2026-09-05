// optimizers.lib: sixteen FIR taps learned with the bus loss-first loop
// `descend_N_rad` and plain gradient descent (`sgd_g`, the LMS step), on white
// noise from the standard library. One reverse sweep per sample gives the
// sixteen gradients of the loss; `descend_N` would need sixteen tangents.
//
// Target system: y_target[n] = sum_i h_i* x[n-i], h_i* = sin(0.5 i) exp(-0.2 i)
// Learned model: y_pred[n]   = sum_i h_i x[n-i]
//
// Convergence: h -> h*; residual -> 0.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual_L, residual_R]

import("stdfaust.lib");
op = library("optimizers.lib");

N = 16;
x = no.noise;
taps = x <: par(i, N, @(i));
fir(h) = (h, taps) : ro.interleave(N, 2) : par(i, N, *) :> _;
h_star(i) = sin(0.5 * i) * exp(-0.2 * i);
y_target = fir(par(i, N, h_star(i)));

// The loss as a block of the N parameters, closed over the data. (`op.mse(_, t)`
// would be a two-input block: a free `_` is duplicated wherever it is used.)
fir_loss = fir(si.bus(N)) : sq_err with { sq_err(y) = op.mse(y, y_target); };
h = op.descend_N_rad(N, fir_loss, op.sgd_g(0.02), -2.0, 2.0, 0.0, 0.0);

process = (y_target - fir(h)) <: _, _;
