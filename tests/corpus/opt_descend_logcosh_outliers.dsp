// optimizers.lib: a gain learned through the robust `logcosh` loss while the
// target carries impulsive outliers.
//
// Target: y_target[n] = g_star * x[n] + spike[n], with spikes of +/-20 every
//         97 samples on a signal of amplitude 0.7.
// Model:  y_pred[n]   = g * x[n]
//
// The gradient of `logcosh` is `tanh(err)`, bounded by 1, so a spike moves
// `g` by at most `lr` while `mse` would move it by `2 * 20 * lr * x`.
//
// Convergence: g -> g_star within ~1e-2 (mse would keep a jitter of ~0.2).
//
// Requires -I libraries (project-local optimizers.lib; no stdfaust.lib).
//
// Outputs: [g - g_star, g - g_star]

op = library("optimizers.lib");

g_star = 0.7;

// Inline LCG white noise excitation.
noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

x = noise;
time = (+(1) ~ _) - 1;
spike = float((time % 97) == 0) * 20.0 * op.sgn(noise');
y_target = g_star * x + spike;

loss(g) = op.logcosh(g * x, y_target);
g_learned = op.descend_1D(loss, op.sgd_g(0.01), -4.0, 4.0, 0.0, 0.0);

process = (g_learned - g_star) <: _, _;
