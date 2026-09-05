// optimizers.lib: one gain learned in the graph with the loss-first loop
// `descend_1D` and the bias-corrected Adam engine `adam_g`.
//
// Target system: y_target[n] = g_star * x[n]
// Learned model: y_pred[n]   = g[n]   * x[n]
//
// The loss `mse(g * x, y_target)` is closed over the excitation and the
// target; `descend_1D` differentiates it with `fad` and steps `g` every
// sample. With the bias correction the first steps are `lr` in size, not
// `sqrt(1 - b2) / (1 - b1) * lr`.
//
// Convergence: g[n] -> g_star; residual -> 0 (below 1e-5 after ~2000 frames).
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual_L, residual_R]

op = library("optimizers.lib");

g_star = 0.7;

// Inline LCG white noise excitation.
noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

x = noise;
y_target = g_star * x;

loss(g) = op.mse(g * x, y_target);
g_learned = op.descend_1D(loss, op.adam_g(0.002, 0.9, 0.999, 0.00000001), -4.0, 4.0, 0.0, 0.0);

process = (y_target - g_learned * x) <: _, _;
