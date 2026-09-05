// optimizers.lib: a one-pole filter's pole and output gain identified with
// the damped recursive Gauss-Newton loop `lm_2D`.
//
// Target system: y_target[n] = g_star * (x[n] + p_star * y1[n-1])
// Learned model: y_pred[n]   = g      * (x[n] + p      * y1[n-1])
//
// The two parameters have different sensitivities and are correlated through
// the filter gain; `lm_2D` solves the 2x2 damped normal equations every
// sample, so a single gain `mu = 1 - a` serves both, with no per-parameter
// learning rate. The gain starts at 0.1 rather than 0 so the pole is
// observable from the first sample.
//
// Convergence: (p, g) -> (0.6, 0.5); residual -> 0.
//
// Requires -I libraries (project-local optimizers.lib; no stdfaust.lib).
//
// Outputs: [residual_L, residual_R]

op = library("optimizers.lib");

p_star = 0.6;
g_star = 0.5;

// Inline LCG white noise excitation.
noise = lcg * 4.656612873077333e-10
with { lcg = +(12345) ~ *(1103515245); };

x = noise;
mdl(p, g, sig) = g * (sig : + ~ *(p));
y_target = mdl(p_star, g_star, x);

learned = op.lm_2D(mdl, 0.01, 0.1, 0.99,
                   -0.99, 0.99, -4.0, 4.0,
                   0.0, 0.1, 0.0, y_target, x);
p = learned : _, !;
g = learned : !, _;

process = (y_target - mdl(p, g, x)) <: _, _;
