// optimizers.lib: eight FIR taps learned with the bus least-squares loop
// `lsq_N_rad` and the normalized LMS engine `nlms`, on white noise from the
// standard library at level 10. The eight sensitivities of the model come from
// one reverse sweep per sample; `nlms` makes the convergence speed independent
// of the input level.
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

N = 8;
level = 10.0;
x = level * no.noise;

// The model as a block: N taps, then the input.
fir = si.bus(N), (_ <: par(i, N, @(i))) : ro.interleave(N, 2) : par(i, N, *) :> _;
h_star(i) = sin(0.5 * i) * exp(-0.2 * i);
y_target = (par(i, N, h_star(i)), x) : fir;

h = op.lsq_N_rad(N, fir, op.nlms(0.02, 0.000001, 0.99), -4.0, 4.0, 0.0, 0.0, y_target, x);

process = (y_target - ((h, x) : fir)) <: _, _;
