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

// ---- the excitation
// Guitar-like excitation: three partials and a little band-limited noise,
// about +-1.5 V, so that the diodes conduct on the peaks.
x = 0.9 * os.osc(110.0) + 0.4 * os.osc(220.0) + 0.2 * os.osc(330.0) + 0.3 * (no.noise : fi.lowpass(2, 2000.0));

// ---- the circuit, solved implicitly at every sample
h = 1.0 / ma.SR;                          // the time step of backward Euler, one sample
vt = 0.09;                                // 2 n Vt with n = 1.75
// The backward-Euler residual of the circuit equation at the new sample:
// G(v) = v - v[n-1] - h f(v, x[n]), with f the right-hand side of the ODE,
// (x - v) / tau for the RC charge and -k sinh(v / vt) for the diode pair.
// G = 0 defines v[n] implicitly; its slope in v is what Newton needs.
G(tau, k, xn, vprev, v) = v - vprev - h * ((xn - v) / tau - k * ma.sinh(v / vt));
// One safeguarded Newton step: the slope is an inner fad with the iterate
// as seed, the iterate is kept within +-2 V.
// v <- v - G(v) / G'(v), the slope G'(v) = dG/dv from `fad(G(...), v) : !, _`
// (the primal G is dropped, the tangent kept); min/max clamp the iterate.
newton(tau, k, xn, vprev, v) = max(-2.0, min(2.0, v - G(tau, k, xn, vprev, v) / (fad(G(tau, k, xn, vprev, v), v) : !, _)));
// The iteration starts from an explicit-Euler predictor rather than from
// v[n-1] itself: seeds are matched by identity, and with v = vprev the inner
// fad would differentiate both occurrences of the same signal.
predictor(tau, k, xn, vprev) = vprev + h * ((xn - vprev) / tau - k * ma.sinh(vprev / vt));
NIT = 4;                                  // Newton iterations per sample, unrolled by seq
// The clipper: at each sample, the predictor from v[n-1], then NIT Newton
// steps; `state ~ _` feeds the solved v back as v[n-1] for the next sample.
clipper(tau, k, xn) = state ~ _
with {
    state(vprev) = predictor(tau, k, xn, vprev) : seq(i, NIT, newton(tau, k, xn, vprev));
};

// ---- the hidden circuit and the learning loop
tau_s = 0.0001;                           // R C = 2.2 kOhm * 47 nF
k_s = 0.1;                                // 2 Is / C
target = clipper(tau_s, k_s, x);

// the model learns log tau and log k: both are positive and span decades
mdl(ltau, lk, xn) = clipper(exp(ltau), exp(lk), xn);
// lm_2D(mdl, mu, lambda, a, lo1, hi1, lo2, hi2, init1, init2, reset, target, x):
// damped Gauss-Newton with gain mu = 0.01, Marquardt damping 0.1 and a
// forgetting factor 0.99 on the information matrix; log tau in
// [log 2e-5, log 1e-3], log k in [log 0.01, log 1]; start at
// (3e-4 s, 0.03), i.e. three times too slow and three times too weak; no
// reset. The two sensitivities come from the outer `fad` through the
// unrolled Newton steps and the recursion.
learned = op.lm_2D(mdl, 0.01, 0.1, 0.99,
                   log(0.00002), log(0.001), log(0.01), log(1.0),
                   log(0.0003), log(0.03), 0.0, target, x);
ltau = learned : _, !;
lk = learned : !, _;

// ---- the check of the derivative
// The derivative of the solved v with respect to k at the hidden parameters:
// through the unrolled solver, and by the implicit-function theorem through
// the recursion.
v_s = clipper(tau_s, k_s, x);
// how far the last Newton iterate is from the root, G(v[n]) with v[n-1] = v_s'
newton_residual = G(tau_s, k_s, x, v_s', v_s);
// dv/dk of the unrolled solver: the tangent of the clipper with k as seed
dv_unrolled = fad(clipper(tau_s, k_s, x), k_s) : !, _;
// the three partial derivatives of G at the solved trajectory, with respect
// to k, to v[n-1] and to v: one fad with three seeds, the primal dropped
dG = fad(G(tau_s, k_s, x, v_s', v_s), (k_s, v_s', v_s)) : !, _, _, _;
Gk = dG : _, !, !;
Gp = dG : !, _, !;
Gv = dG : !, !, _;
// implicit-function theorem on G(k, v[n-1], v[n]) = 0, differentiated in k:
// G_k + G_p s[n-1] + G_v s[n] = 0, so s[n] = -(G_k + G_p s[n-1]) / G_v,
// a recursion on the sensitivity s = dv/dk
dv_implicit = (\(sp).(0.0 - (Gk + Gp * sp) / Gv)) ~ _;

// ---- outputs
// [0] the learned tau in units of 1e-4 s (R C): 3 -> 1
// [1] the learned k (2 Is / C): 0.03 -> 0.1
// [2] the residual, target - model, in volts: -> 0 within 8 000 samples
// [3] the Newton residual |G(v[n])| of the hidden clipper: stays below 1e-8,
//     the solver's accuracy at every sample
// [4] the difference between the two derivatives dv/dk, unrolled solver minus
//     implicit-function theorem: about 1e-7, the check of the outer fad
// [5] the derivative dv/dk itself, for the scale of [4]
process = exp(ltau) * 10000.0, exp(lk), target - mdl(ltau, lk, x),
          abs(newton_residual), dv_unrolled - dv_implicit, dv_unrolled;
