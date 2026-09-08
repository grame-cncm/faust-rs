// DDSP example (FAD, gated): a feedback delay network that calibrates itself
// to a target decay, then stops paying for its learning.
//
// The four-line FDN of `ddsp_fad_fdn_reverb_lm.dsp` (Jot 1991: prime delays
// of 24 to 41 ms, a Hadamard matrix, a per-line gain set by a reverberation
// time T60, a per-line one-pole damping) learns its T60 and its damping from
// the response of a hidden FDN to an impulse train, with `lm_2D` as before.
// Two things are new. The whole learning (the model carrying the tangents,
// the Gauss-Newton loop, the residual) lives in `op.gated(learn)`, an
// `ondemand` domain whose clock the block's own flag switches off: the flag
// is `op.stop_below`, raised at the end of the first period whose residual
// energy is under 1e-7 (a residual of 2.5e-6 rms, an exact match for an
// effect), and from that sample on nothing of the learning is computed and
// the parameters hold. The criterion is a threshold rather than
// `op.stop_relative` because the target is exact: the residual has no floor
// and keeps falling geometrically, so its relative change never settles;
// on a measured target, with a noise floor, `stop_relative` is the one. And
// the reverberator that renders the output does not compute the four gains
// `10^(-3 len_i / (T60 SR))` at every sample: `op.on_change` recomputes them
// only when T60 changes, once per learning step, never after the stop.
//
// Convergence: (T60, damping) -> (0.600 s, 0.300) within three periods of
// 16 384 samples; the flag rises at a period boundary a few periods later,
// and the parameters are constant afterwards.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [T60 in seconds, damping, done, residual of the rendered reverb]

import("stdfaust.lib");
op = library("optimizers.lib");

// ---- the network
N = 4;                                    // delay lines
PERIOD = 16384;                           // samples between two impulses: 0.37 s, past the decay
// prime delay lengths in samples, 24 to 41 ms at 44.1 kHz, mutually prime
// so that the echoes of the four lines never coincide
len(i) = ba.take(i + 1, (1051, 1327, 1597, 1801));
// Jot's per-line gain: a signal going round line i loses 60 dB in T60
// seconds, i.e. 10^(-3 len_i / (T60 SR)) per pass
gain(t60, i) = pow(10.0, 0.0 - 3.0 * len(i) / (t60 * ma.SR));
imp = (ba.time % PERIOD) == 0;            // an impulse at the start of every period
clock = (ba.time % PERIOD) == (PERIOD - 1); // the last sample of every period: where stop_below decides

// the network with its gains computed from T60 (the learning instance) ...
fdn(t60, d, x) = fdn_g(gain(t60, 0), gain(t60, 1), gain(t60, 2), gain(t60, 3), d, x);
// ... and with its gains given (the rendering instance, gains held by on_change)
// Per line: the input is added to the feedback, delayed by len(i), damped by
// the one-pole si.smooth(d) (the higher d, the duller the tail), scaled by the
// line's gain; the four lines are mixed by the Hadamard butterfly, scaled by
// 0.5 = 1 / sqrt(N) to make it orthogonal, and fed back by `~ si.bus(N)`.
// The output is the sum of the lines, scaled by 0.25.
fdn_g(g0, g1, g2, g3, d, x) = (par(i, N, +(x))
                  : par(i, N, @(len(i)))
                  : par(i, N, si.smooth(d))
                  : (*(g0), *(g1), *(g2), *(g3))
                  : ro.hadamard(N) : par(i, N, *(0.5))) ~ si.bus(N) :> _ * 0.25;

target = fdn(0.6, 0.3, imp);              // the hidden network: T60 0.6 s, damping 0.3
mdl(lt60, d, x) = fdn(exp(lt60), d, x);   // the model learns log T60, positive by construction

// the learning block: Gauss-Newton on (log T60, damping) at audio rate, and the
// stop flag from the residual energy of each period
// Its inputs are the block's inputs, target t and excitation x; its outputs
// end with the flag, as `gated` requires.
learn(t, x) = lt60, d, flag
with {
    // lm_2D(mdl, mu, lambda, a, lo1, hi1, lo2, hi2, init1, init2, reset, target, x):
    // gain 0.01, damping 0.1, forgetting 0.999 (the information matrix keeps
    // the last decay between impulses); log T60 in [log 0.1, log 3], damping
    // in [0, 0.9]; start at (0.3 s, 0); no reset
    learned = op.lm_2D(mdl, 0.01, 0.1, 0.999, log(0.1), log(3.0), 0.0, 0.9, log(0.3), 0.0, 0.0, t, x);
    lt60 = learned : _, !;
    d = learned : !, _;
    r = t - mdl(lt60, d, x);              // the residual of the learning instance
    // stop_below(clock, threshold): sums r^2 over each period and, on the
    // clock, raises the flag for good once the sum is under 1e-7
    flag = r * r : op.stop_below(clock, 1e-7);
};

// gated(C) runs C in an ondemand block clocked by `not done`, the last output
// of C being `done`: once the flag rises the block never fires again and its
// outputs hold. The block's inputs are the target and the excitation.
params = (target, imp) : op.gated(learn);     // [log T60, damping, done], held once done
lt60 = params : _, !, !;
d = params : !, _, !;
done = params : !, !, _;

// the rendered reverb: gains recomputed only when T60 changes
// on_change(C) fires C only on the samples where its input changed: the four
// pow() run once per learning step and never after the stop.
gains = exp(lt60) : op.on_change(\(t).(par(i, N, gain(t, i))));
wet = (gains, d, imp) : fdn_g;

// ---- outputs
// [0] the learned T60 in seconds: 0.3 -> 0.600 within three periods, then
//     constant once the block has stopped
// [1] the learned damping: 0 -> 0.300, likewise
// [2] done: 0 while learning, 1 from the last sample of the first period
//     whose residual energy is under 1e-7, and 1 for ever after
// [3] the residual of the rendered reverb, target - wet: -> 2.5e-6 rms,
//     computed by the rendering instance whose gains on_change holds
process = exp(lt60), d, done, target - wet;
