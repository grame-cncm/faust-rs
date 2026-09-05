// optimizers.lib: three FIR taps learned with the least-squares loop `lsq_3D`
// and the normalized LMS engine `nlms`, on an excitation ten times louder
// than the unit level a plain LMS step size would be tuned for.
//
// Target system: y_target[n] = 0.5 x[n] + 0.3 x[n-1] - 0.2 x[n-2]
// Learned model: y_pred[n]   = h0 x[n] + h1 x[n-1] + h2 x[n-2]
//
// `nlms` divides each step by the smoothed power of the tap's sensitivity
// (here the delayed input itself), so the convergence speed is independent of
// the input level: `lms(0.02)` diverges at this level, `nlms(0.02, ...)`
// converges in a few hundred frames.
//
// Convergence: (h0, h1, h2) -> (0.5, 0.3, -0.2); residual -> 0.
//
// Requires -I libraries (project-local optimizers.lib; no stdfaust.lib).
//
// Outputs: [residual_L, residual_R]

op = library("optimizers.lib");

h0_star = 0.5;
h1_star = 0.3;
h2_star = -0.2;
level = 10.0;

// Inline LCG white noise excitation.
noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

x = level * noise;
fir(h0, h1, h2, sig) = h0 * sig + h1 * sig' + h2 * sig'';
y_target = fir(h0_star, h1_star, h2_star, x);

upd = op.nlms(0.02, 0.000001, 0.99);
taps = op.lsq_3D(fir, upd, upd, upd,
                 -4.0, 4.0, -4.0, 4.0, -4.0, 4.0,
                 0.0, 0.0, 0.0, 0.0, y_target, x);
h0 = taps : _, !, !;
h1 = taps : !, _, !;
h2 = taps : !, !, _;

process = (y_target - fir(h0, h1, h2, x)) <: _, _;
