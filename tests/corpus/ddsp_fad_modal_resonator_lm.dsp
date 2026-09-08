// DDSP example (FAD): calibrating one mode of a modal model by Gauss-Newton.
//
// A mode of a modal synthesiser is a resonant band-pass with a frequency and
// a quality factor (a decay). Given the response of a hidden mode to noise,
// the loop identifies both parameters of a model mode with `lm_2D`, the
// damped Gauss-Newton (Levenberg-Marquardt) loop: each sample, `fad` gives
// the two sensitivities of the model output -- exact through the resonator's
// recursion -- and the loop solves the 2x2 normal equations built from
// them, so a parameter in hertz and one without unit take steps of the
// right scale without any tuning. First-order engines need a log domain or
// per-parameter rates for that (tutorial, section 5).
//
// Convergence: (f, q) -> (800, 25) from (600, 10) in a few thousand samples.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [f, q, residual]

import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise;                             // the excitation
// one mode: `fi.resonbp(f, q, gain)`, the constant-peak-gain resonant
// band-pass of filters.lib, centre f in Hz, quality factor q (f / bandwidth),
// peak gain 1; its recursion is what `fad` differentiates
mode(f, q, sig) = sig : fi.resonbp(f, q, 1.0);
target = mode(800.0, 25.0, x);            // the hidden mode: 800 Hz, q 25

// lm_2D(mdl, mu, lambda, a, lo1, hi1, lo2, hi2, init1, init2, reset, target, x):
// damped Gauss-Newton, gain 0.01, Marquardt damping 0.1, forgetting 0.99;
// f in [100, 5000] Hz, q in [2, 60]; start at (600 Hz, 10); no reset. The
// normal equations built from the two sensitivities give a parameter in
// hertz and one without unit each its own step size.
learned = op.lm_2D(mode, 0.01, 0.1, 0.99, 100.0, 5000.0, 2.0, 60.0, 600.0, 10.0, 0.0, target, x);
f = learned : _, !;
q = learned : !, _;

// ---- outputs
// [0] the learned centre frequency in Hz: 600 -> 800
// [1] the learned quality factor: 10 -> 25
// [2] the residual, target - model on the same noise: -> 0
process = f, q, target - mode(f, q, x);
