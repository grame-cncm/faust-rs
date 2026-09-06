// A parameter near 1000 learned in single precision with steps below the
// precision of its value. `init` = 1000, the target 1000.001; the SGD steps
// (at most 2e-5) are smaller than half an ulp of the value (6e-5 in f32).
// The loops apply the update to the deviation from `init`, where the steps
// are kept, so the residual falls to the rounding floor; applied to the value
// itself every step would be lost and the residual would not move.
// Outputs the residual as a stereo pair.
import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise;
target = 1000.001 * x;
model(p) = p * x;
loss(p) = op.mse(model(p), target);
p = op.descend_1D(loss, op.sgd_g(0.01), 900.0, 1100.0, 1000.0, 0);

process = target - model(p) <: _, _;
