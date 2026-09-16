// optimizers.lib: `grid_then_descend_1D` on the two-well loss of
// opt_langevin_two_wells.dsp, `(p^2 - 1)^2 + 0.3 p`.
//
// Eight candidates at the cell centres of [-3, 3] are scored for 2 000
// samples with no tangent; the lowest loss is the cell at -1.125 (index 2,
// loss -0.30, next to the deep well's bottom at -1.036). At sample 2 000 it
// becomes the start of one SGD descent, held until then, which settles at
// -1.036.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [p, index of the chosen candidate]

op = library("optimizers.lib");

loss(p) = (p * p - 1.0) * (p * p - 1.0) + 0.3 * p;
process = op.grid_then_descend_1D(8, 2000, op.grid_init(8, -3.0, 3.0), loss, op.sgd_g(0.01), -3.0, 3.0, 0.0);
