// DDSP example (FAD, state of the art): a feedback delay network calibrated
// online to a target decay.
//
// A four-line FDN (Jot 1991): prime-length delays of 24 to 41 ms, an
// orthogonal Hadamard mixing matrix, a per-line gain set by a reverberation
// time T60 (gain_i = 10^(-3 len_i / (T60 SR))) and a per-line one-pole
// damping that shortens the decay of high frequencies. Given the response
// of a hidden FDN to an impulse train, the program learns its T60 and its
// damping coefficient: differentiable artificial reverberation (Lee, Choi &
// Lee 2022), whose long recursions tensor frameworks approximate or unroll,
// and which `fad` differentiates exactly by carrying a tangent through the
// four delay lines and the feedback matrix, sample by sample.
//
// The loop is `lm_2D`, damped Gauss-Newton on the two parameters (T60 in the
// log domain): a parameter in seconds and one without unit take steps of
// the right scale, and the identification is exact. The same program with
// Adam wanders once the impulse response has decayed (the gradient carries
// no information between impulses); Gauss-Newton with a forgetting factor
// keeps the last decay in its information matrix.
//
// Convergence: (T60, damping) -> (0.600 s, 0.300) from (0.3 s, 0) within
// 60 000 samples (four impulses).
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [T60 in seconds, damping, residual]

import("stdfaust.lib");
op = library("optimizers.lib");

// ---- the network
N = 4;                                    // delay lines
// prime delay lengths in samples, 24 to 41 ms at 44.1 kHz, mutually prime
// so that the echoes of the four lines never coincide
len(i) = ba.take(i + 1, (1051, 1327, 1597, 1801));
// Jot's per-line gain: a signal going round line i loses 60 dB in T60
// seconds, i.e. 10^(-3 len_i / (T60 SR)) per pass
gain(t60, i) = pow(10.0, 0.0 - 3.0 * len(i) / (t60 * ma.SR));
imp = (ba.time % 16384) == 0;             // an impulse every 0.37 s, past the decay

// Per line: the input is added to the feedback, delayed by len(i), damped by
// the one-pole si.smooth(d) (the higher d, the duller the tail), scaled by the
// line's gain; the four lines are mixed by the Hadamard butterfly, scaled by
// 0.5 = 1 / sqrt(N) to make it orthogonal, and fed back by `~ si.bus(N)`.
// The output is the sum of the lines, scaled by 0.25.
fdn(t60, d, x) = (par(i, N, +(x))
                  : par(i, N, @(len(i)))
                  : par(i, N, si.smooth(d))
                  : par(i, N, *(gain(t60, i)))
                  : ro.hadamard(N) : par(i, N, *(0.5))) ~ si.bus(N) :> _ * 0.25;

// ---- the hidden network and the learning loop
target = fdn(0.6, 0.3, imp);              // T60 0.6 s, damping 0.3
mdl(lt60, d, x) = fdn(exp(lt60), d, x);   // the model learns log T60, positive by construction
// lm_2D(mdl, mu, lambda, a, lo1, hi1, lo2, hi2, init1, init2, reset, target, x):
// gain 0.01, Marquardt damping 0.1, forgetting factor 0.999 so that the
// information matrix keeps the last decay while the gradient carries nothing
// between impulses; log T60 in [log 0.1, log 3], damping in [0, 0.9]; start at
// (0.3 s, 0); no reset; the two sensitivities from `fad` through the lines.
learned = op.lm_2D(mdl, 0.01, 0.1, 0.999, log(0.1), log(3.0), 0.0, 0.9, log(0.3), 0.0, 0.0, target, imp);
lt60 = learned : _, !;
d = learned : !, _;

// ---- outputs
// [0] the learned T60 in seconds: 0.3 -> 0.600 within four impulses
// [1] the learned damping: 0 -> 0.300
// [2] the residual, target - model on the same impulse train: -> 0
process = exp(lt60), d, target - fdn(exp(lt60), d, imp);
