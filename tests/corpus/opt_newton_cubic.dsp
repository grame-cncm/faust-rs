// optimizers.lib: the implicit equation y^3 + y = x solved every sample with
// six unrolled Newton steps, `F` and `F'` both coming from one `fad` call.
//
// The residual F(y) = y^3 + y - x of the solution is zero to numerical
// precision on every frame: with x in [-1, 1], six steps from 0 are more than
// the quadratic convergence needs.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual_L, residual_R]

op = library("optimizers.lib");

// Inline LCG white noise: the right-hand side x in [-1, 1].
noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

x = noise;
F(y) = y * y * y + y - x;
y = op.newton(6, F, 0.0);

process = F(y) <: _, _;
