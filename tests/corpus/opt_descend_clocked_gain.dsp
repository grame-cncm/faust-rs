// optimizers.lib: a gain learned with `descend_1D_clocked`: the gradient is
// computed on every sample, averaged over a 64-sample frame with `frame_mean`,
// and the step is taken once per frame inside an `ondemand` block.
//
// Target system: y_target[n] = g_star * x[n]
// Learned model: y_pred[n]   = g[n]   * x[n], g held between frames
//
// The frame-mean gradient is the mini-batch gradient, so a plain SGD step of
// 0.5 per frame converges in a few frames; the parameter is `init` (0) until
// the first firing, so the model never sees the block's unfired output.
//
// Convergence: g -> g_star; residual -> 0.
//
// Requires -I libraries (project-local optimizers.lib; no stdfaust.lib).
//
// Outputs: [residual_L, residual_R]

op = library("optimizers.lib");

g_star = 0.7;
N = 64;

// Inline LCG white noise excitation and an inline frame clock.
noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };
clock = ((+(1) : %(N)) ~ _) == 0;

x = noise;
y_target = g_star * x;

loss(g) = op.mse(g * x, y_target);
g_learned = op.descend_1D_clocked(clock, loss, op.sgd_g(0.5), -4.0, 4.0, 0.0, 0.0);

process = (y_target - g_learned * x) <: _, _;
