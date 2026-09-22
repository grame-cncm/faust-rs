// optimizers.lib: `descend_N_clocked` with a list for each of `upd`, `lo`,
// `hi` and `init` (version 0.10.0): three parameters of different scales,
// a gain in [0, 2], an offset in [-100, 100] and a slope in [-5, 5], each
// with its own Adam rate, its own bounds and its own start, learned from a
// target `0.7 x + 30 + (-2) x'` on white noise. With one scalar rate the
// offset, thirty times larger than the others, would take thirty times
// longer; with a rate per parameter all three settle together.
//
// Convergence: residual -> 0.
import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise;
target = 0.7 * x + 30.0 - 2.0 * x';
clock = ((+(1) : %(64)) ~ _) == 0;
adam(lr) = op.adam_g(lr, 0.9, 0.999, 1e-8);
loss(g, b, c) = op.mse(g * x + b + c * x', target);
params = op.descend_N_clocked(3, clock, loss, (adam(0.02), adam(1.0), adam(0.05)),
                              (0.0, -100.0, -5.0), (2.0, 100.0, 5.0), (1.0, 0.0, 0.0), 0);
g = params : _, !, !;
b = params : !, _, !;
c = params : !, !, _;
process = (g * x + b + c * x' - target) <: _, _;
