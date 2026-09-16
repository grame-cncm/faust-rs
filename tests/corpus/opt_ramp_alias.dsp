// optimizers.lib: `lr_exp` is `ramp_exp` under its learning-rate name, and
// `ramp_lin` reaches its end value at sample T.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [lr_exp(0.01, 0.0001, 4800), ramp_exp(0.01, 0.0001, 4800),
//           ramp_lin(0.01, 0.0001, 4800)]
// Lanes 0 and 1 are bit-identical; lane 2 is 0.01 at frame 0, 0.00505 at
// frame 2400, 0.0001 from frame 4800 on.

op = library("optimizers.lib");

process = op.lr_exp(0.01, 0.0001, 4800.0), op.ramp_exp(0.01, 0.0001, 4800.0), op.ramp_lin(0.01, 0.0001, 4800.0);
