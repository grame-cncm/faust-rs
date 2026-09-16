// optimizers.lib: `multistart_1D` on the two-well loss of
// opt_langevin_two_wells.dsp, `(p^2 - 1)^2 + 0.3 p`.
//
// Four SGD descents from the cell centres of [-3, 3]: -2.25, -0.75, 0.75 and
// 2.25. The first two end in the deep well at -1.036 (loss -0.31), the last
// two in the shallow well at 0.960 (loss 0.29). The loop follows the lowest
// smoothed loss: the first of the two deep descents, index 0.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [p of the best start, its index]

op = library("optimizers.lib");

loss(p) = (p * p - 1.0) * (p * p - 1.0) + 0.3 * p;
process = op.multistart_1D(4, op.grid_init(4, -3.0, 3.0), loss, op.sgd_g(0.01), -3.0, 3.0, 0.999, 0.0);
