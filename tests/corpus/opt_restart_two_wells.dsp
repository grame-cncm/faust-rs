// optimizers.lib: `descend_1D_restart` on the two-well loss of
// opt_langevin_two_wells.dsp, `(p^2 - 1)^2 + 0.3 p`.
//
// The first start, p = 1, is in the shallow well: SGD settles at 0.960 with
// a loss of 0.29, above eps_l = 0.1, and once settled the loss makes no
// progress; after 2 W = 8 000 samples the loop zeroes the deviation and
// takes the second start, p = -1, in the deep well, where the loss is -0.31,
// below eps_l: the search is finished and no further restart fires.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [p, index of the current start]

op = library("optimizers.lib");

loss(p) = (p * p - 1.0) * (p * p - 1.0) + 0.3 * p;
init(k) = select2(k, 1.0, -1.0);
learned = op.descend_1D_restart(2, init, loss, op.sgd_g(0.01), -3.0, 3.0, 4000, 0.05, 0.1, 0.0);

process = learned;
