// optimizers.lib: `langevin_g`, the SGD step plus an annealed noise, leaves
// a shallow well where `sgd_g` stays.
//
// The loss `(p^2 - 1)^2 + 0.3 p` has two wells: a shallow one at p = 0.96
// (loss 0.29) and a deep one at p = -1.04 (loss -0.31), with a barrier of
// 1.01 at p = 0.04. Both descents start at p = 1, in the shallow well. Plain
// SGD settles there. Langevin with a temperature annealed from 0.5 to 0
// (time constant 20 000 samples) crosses the barrier while the temperature
// is high and descends into the deep well as it cools. The loss has no
// data, so the outcome is deterministic given the noise generator's seed.
// At temperature 0 the Langevin step is the SGD step bit for bit.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [p with sgd_g, p with langevin_g, p with langevin_g at temp 0]

import("stdfaust.lib");
op = library("optimizers.lib");

loss(p) = (p * p - 1.0) * (p * p - 1.0) + 0.3 * p;
noise = no.noise * sqrt(3.0);                 // unit variance
temp = op.ramp_exp(0.5, 0.0, 20000.0);
lr = 0.01;

p_sgd = op.descend_1D(loss, op.sgd_g(lr), -3.0, 3.0, 1.0, 0.0);
p_langevin = op.descend_1D(loss, op.langevin_g(lr, temp, noise), -3.0, 3.0, 1.0, 0.0);
p_cold = op.descend_1D(loss, op.langevin_g(lr, 0.0, noise), -3.0, 3.0, 1.0, 0.0);

process = p_sgd, p_langevin, p_cold;
