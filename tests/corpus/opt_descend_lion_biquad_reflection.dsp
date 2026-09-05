// optimizers.lib: five biquad coefficients learned with `descend_5D`, the
// Lion engine on an exponentially decaying learning rate, and the poles
// parameterized by reflection coefficients.
//
// Target system: y = (0.1 x + 0.2 x' + 0.1 x'') / (1 - 1.0 z^-1 + 0.4 z^-2)
// Learned model: b0, b1, b2 direct; a1 = k1 (1 + k2), a2 = k2 with
//                k1, k2 in (-1, 1), which is exactly the stability triangle.
//
// One learning rate serves the five parameters: Lion steps by +/-lr in the
// direction of the sign of its momentum, whatever the gradient scale. Every
// intermediate filter is stable by construction, which rectangular bounds on
// (a1, a2) cannot guarantee.
//
// Convergence: (b0, b1, b2, a1, a2) -> (0.1, 0.2, 0.1, -1.0, 0.4); residual -> 0.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual_L, residual_R]

op = library("optimizers.lib");

b0_star = 0.1;
b1_star = 0.2;
b2_star = 0.1;
a1_star = -1.0;
a2_star = 0.4;

// Inline LCG white noise excitation.
noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

x = noise;

// Direct-form biquad: y = num - a1 y[n-1] - a2 y[n-2].
biquad(b0, b1, b2, a1, a2, sig) = loop ~ _
with {
    num = b0 * sig + b1 * sig' + b2 * sig'';
    loop(yp) = num - a1 * yp - a2 * yp';
};
y_target = biquad(b0_star, b1_star, b2_star, a1_star, a2_star, x);

mdl(b0, b1, b2, k1, k2) = biquad(b0, b1, b2, a1, a2, x)
with {
    a1 = op.poles_from_reflection(k1, k2) : _, !;
    a2 = op.poles_from_reflection(k1, k2) : !, _;
};
loss(b0, b1, b2, k1, k2) = op.mse(mdl(b0, b1, b2, k1, k2), y_target);

lion = op.lion_g(op.lr_exp(0.0005, 0.00001, 20000.0), 0.9, 0.99);
coefs = op.descend_5D(loss, lion, lion, lion, lion, lion,
                      -2.0, 2.0, -2.0, 2.0, -2.0, 2.0, -0.999, 0.999, -0.999, 0.999,
                      0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
c0 = coefs : _, !, !, !, !;
c1 = coefs : !, _, !, !, !;
c2 = coefs : !, !, _, !, !;
k1 = coefs : !, !, !, _, !;
k2 = coefs : !, !, !, !, _;

process = (y_target - mdl(c0, c1, c2, k1, k2)) <: _, _;
