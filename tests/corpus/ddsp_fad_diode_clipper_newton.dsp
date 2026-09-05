// DDSP example (FAD, state of the art): a diode clipper solved implicitly,
// whose circuit components are learned online through the solver.
//
// The circuit is the RC + diode-pair clipper of every overdrive pedal (Yeh,
// Abel & Smith 2007): dv/dt = (x - v) / (R C) - (2 Is / C) sinh(v / (2 n Vt)).
// Discretised by backward Euler it is an implicit equation in v[n],
// G(v) = v - v[n-1] - h f(v, x[n]) = 0, solved at every sample by a few
// safeguarded Newton iterations whose slope G'(v) comes from an inner `fad`
// -- a zero-delay-feedback virtual-analog model in the usual sense.
//
// Two component values, tau = R C and k = 2 Is / C, are then learned from
// the output of a hidden clipper by `lm_2D` (damped Gauss-Newton), in the
// log domain. The outer `fad` differentiates through the unrolled Newton
// iterations *and* through the state recursion: `fad` inside `fad` inside a
// recursion, expanded at compile time. Frameworks differentiate implicit
// solvers by the implicit-function theorem or by unrolling in a tensor
// graph; here the derivative of the unrolled solver is checked against the
// implicit-function-theorem derivative propagated through the recursion,
// s[n] = -(G_k + G_vprev s[n-1]) / G_v, at the hidden parameters: they agree
// to 1e-7, and the Newton residual stays below 1e-8.
//
// Convergence: (tau, k) -> (1e-4 s, 0.1) from (3e-4, 0.03) within 8 000
// samples; the residual against the hidden clipper vanishes.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [tau * 1e4, k, residual, Newton residual |G|,
//           unrolled derivative - implicit derivative, unrolled derivative]

import("stdfaust.lib");
op = library("optimizers.lib");

// Guitar-like excitation: three partials and a little band-limited noise,
// about +-1.5 V, so that the diodes conduct on the peaks.
x = 0.9 * os.osc(110.0) + 0.4 * os.osc(220.0) + 0.2 * os.osc(330.0) + 0.3 * (no.noise : fi.lowpass(2, 2000.0));

h = 1.0 / ma.SR;
vt = 0.09;                                // 2 n Vt with n = 1.75
G(tau, k, xn, vprev, v) = v - vprev - h * ((xn - v) / tau - k * ma.sinh(v / vt));
// One safeguarded Newton step: the slope is an inner fad with the iterate
// as seed, the iterate is kept within +-2 V.
newton(tau, k, xn, vprev, v) = max(-2.0, min(2.0, v - G(tau, k, xn, vprev, v) / (fad(G(tau, k, xn, vprev, v), v) : !, _)));
// The iteration starts from an explicit-Euler predictor rather than from
// v[n-1] itself: seeds are matched by identity, and with v = vprev the inner
// fad would differentiate both occurrences of the same signal.
predictor(tau, k, xn, vprev) = vprev + h * ((xn - vprev) / tau - k * ma.sinh(vprev / vt));
NIT = 4;
clipper(tau, k, xn) = state ~ _
with {
    state(vprev) = predictor(tau, k, xn, vprev) : seq(i, NIT, newton(tau, k, xn, vprev));
};

tau_s = 0.0001;                           // R C = 2.2 kOhm * 47 nF
k_s = 0.1;                                // 2 Is / C
target = clipper(tau_s, k_s, x);

mdl(ltau, lk, xn) = clipper(exp(ltau), exp(lk), xn);
learned = op.lm_2D(mdl, 0.01, 0.1, 0.99,
                   log(0.00002), log(0.001), log(0.01), log(1.0),
                   log(0.0003), log(0.03), 0.0, target, x);
ltau = learned : _, !;
lk = learned : !, _;

// The derivative of the solved v with respect to k at the hidden parameters:
// through the unrolled solver, and by the implicit-function theorem through
// the recursion.
v_s = clipper(tau_s, k_s, x);
newton_residual = G(tau_s, k_s, x, v_s', v_s);
dv_unrolled = fad(clipper(tau_s, k_s, x), k_s) : !, _;
dG = fad(G(tau_s, k_s, x, v_s', v_s), (k_s, v_s', v_s)) : !, _, _, _;
Gk = dG : _, !, !;
Gp = dG : !, _, !;
Gv = dG : !, !, _;
dv_implicit = (\(sp).(0.0 - (Gk + Gp * sp) / Gv)) ~ _;

process = exp(ltau) * 10000.0, exp(lk), target - mdl(ltau, lk, x),
          abs(newton_residual), dv_unrolled - dv_implicit, dv_unrolled;
