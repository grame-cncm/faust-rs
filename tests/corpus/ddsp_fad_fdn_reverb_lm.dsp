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

N = 4;
len(i) = ba.take(i + 1, (1051, 1327, 1597, 1801));
gain(t60, i) = pow(10.0, 0.0 - 3.0 * len(i) / (t60 * ma.SR));
imp = (ba.time % 16384) == 0;

fdn(t60, d, x) = (par(i, N, +(x))
                  : par(i, N, @(len(i)))
                  : par(i, N, si.smooth(d))
                  : par(i, N, *(gain(t60, i)))
                  : ro.hadamard(N) : par(i, N, *(0.5))) ~ si.bus(N) :> _ * 0.25;

target = fdn(0.6, 0.3, imp);
mdl(lt60, d, x) = fdn(exp(lt60), d, x);
learned = op.lm_2D(mdl, 0.01, 0.1, 0.999, log(0.1), log(3.0), 0.0, 0.9, log(0.3), 0.0, 0.0, target, imp);
lt60 = learned : _, !;
d = learned : !, _;

process = exp(lt60), d, target - fdn(exp(lt60), d, imp);
