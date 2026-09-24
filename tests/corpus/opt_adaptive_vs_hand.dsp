// optimizers.lib: `adaptive_fad` and `adaptive_rad` (version 0.11.0) on a
// three-control model `e(x) = x * gain + bias + slope/c * x'`, written with
// sliders and never rewritten, against `descend_N_fad_clocked` on the same
// model written by hand as a function of its three parameters, with the
// bounds and starts copied from the sliders in interface order (bias, gain,
// slope/c). `cinputs`, `cinput` and the wildcard modulation `"*"` supply what
// the hand-written form spells out, so the two follow the same trajectory to
// the bit; `adaptive_rad` follows it too (no recursion between the controls
// and the output), to the rounding of a reverse sweep.
//
// Outputs: [residual of adaptive_fad, residual of the hand-written loop,
// residual of adaptive_rad, adaptive_fad - hand-written (0 on every sample),
// the three parameters learned by adaptive_fad].
import("stdfaust.lib");
op = library("optimizers.lib");

e(x) = x * hslider("gain", 1, 0, 2, 0.01) + hslider("bias", 0, -1, 1, 0.01)
     + hgroup("slope", hslider("c", 0, -5, 5, 0.01)) * x';
x = no.noise;
target = 0.7 * x + 0.3 - 2.0 * x';
clock = ((+(1) : %(64)) ~ _) == 0;
adam(lr) = op.adam_g(lr, 0.9, 0.999, 1e-8);

auto_f = op.adaptive_fad(e, op.mse, adam(0.02), clock, 0, x, target);
auto_r = op.adaptive_rad(e, op.mse, adam(0.02), clock, 0, x, target) : _, !, !, !;
loss(b, g, c) = op.mse(x * g + b + c * x', target);
hand = op.descend_N_fad_clocked(3, clock, loss, adam(0.02), (-1, 0, -5), (1, 2, 5), (0, 1, 0), 0)
     : \(b, g, c).(x * g + b + c * x');
y = auto_f : _, !, !, !;
process = y - target, hand - target, auto_r - target, y - hand, (auto_f : !, _, _, _);
