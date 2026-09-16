// optimizers.lib: the SPSA gradient estimate against the fad gradient on a
// loss quadratic in the parameter.
//
// `spsa_1D_clocked` and `descend_1D_clocked` learn the same gain with the
// same engine (SGD 0.5 per 64-sample frame) on the same excitation. The
// symmetric difference `(L(p + c) - L(p - c)) / 2c` of a quadratic loss is
// its exact derivative, so the two trajectories coincide to rounding: SPSA
// sees the frame gradient without ever differentiating.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [gain by spsa, gain by fad, their difference]

import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise;
target = 0.7 * x;
loss(g) = op.mse(g * x, target);
clock = ((+(1) : %(64)) ~ _) == 0;

g_spsa = op.spsa_1D_clocked(clock, loss, op.sgd_g(0.5), 0.05, -4.0, 4.0, 0.0, 0.0);
g_fad = op.descend_1D_clocked(clock, loss, op.sgd_g(0.5), -4.0, 4.0, 0.0, 0.0);

process = g_spsa, g_fad, g_spsa - g_fad;
