// DDSP example (RAD, state of the art): a harmonic synthesizer fitted to a
// target through a per-frame spectral loss, inside an `ondemand` block.
//
// The model is the harmonic oscillator bank of DDSP (Engel et al. 2020):
// sixteen harmonics of 440 Hz whose amplitudes are learned, positive by
// construction (a_h = exp(p_h)). The loss is spectral, computed once per
// 256-sample frame: the windowed magnitudes of the synthesized frame at the
// sixteen harmonic frequencies against those of the target frame. The
// target is any audio signal: it is analysed at audio rate (windowed
// correlations at the harmonics summed over the frame with `frame_sum`)
// and enters the block as inputs; here it is a hidden harmonic tone with
// amplitudes 1/h.
//
// Inside the block, fired once per frame, the synthesizer's frame is
// computed from the amplitudes and the frame start, its magnitudes are
// compared with the target's, and `rad` gives the sixteen gradients of
// that frame loss from one reverse sweep -- the reverse sweep runs in the
// block's own domain, at frame rate, on a feed-forward loss; one Adam step
// per frame updates the amplitudes, held between frames. The magnitude
// loss is blind to the sign of an amplitude (a harmonic converges to -a as
// readily as to a), which is why the amplitudes are exponentials, as in
// DDSP.
//
// Convergence: the sixteen amplitudes within 1e-3 of 1/h in 100 frames
// (0.6 s); the resynthesized tone matches the target.
//
// Requires -I libraries (project-local optimizers.lib and interleave.lib)
// and the directory of the Faust standard libraries on the import path
// (-I <faustlibraries>).
//
// Outputs: [16 amplitudes, target - resynthesis]

import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");

H = 16;                                            // harmonics
N = 256;                                           // samples per frame
f0 = 440.0;                                        // the fundamental, known
w(h) = 2.0 * ma.PI * h * f0 / ma.SR;               // angular frequency of harmonic h, per sample
a_star(h) = 1.0 / (h + 1.0);                       // the hidden amplitudes: 1/h for harmonic h = 1..H (h is 0-based here)
a0 = 0.1;                                          // initial amplitude of every harmonic

// ---- the target and its analysis, at audio rate
// The target, at audio rate, and its frame analysis: windowed correlations
// at the harmonics, summed over the frame.
target_audio = par(h, H, a_star(h) * os.osc((h + 1) * f0)) :> _;
clk = il.frame_clock(N);                           // non-zero on the last sample of every frame
j = float(ba.time % N);                            // position in the current frame, 0 .. N-1
win(j) = 0.5 - 0.5 * cos(2.0 * ma.PI * j / N);     // Hann window over the frame
// for each harmonic, the frame's correlation with the windowed cosine and
// sine at w(h): 2H sums, valid on the clock sample (frame_sum resets after it)
target_corr = par(h, H, (op.frame_sum(clk, target_audio * win(j) * cos(w(h + 1) * j)),
                         op.frame_sum(clk, target_audio * win(j) * sin(w(h + 1) * j))));

// ---- the loss, a frame operator: all its inputs are block inputs
// Inside the block: the synthesized frame for log-amplitudes (p_1..p_H) and
// frame start t, its magnitudes, the loss against the target's.
// sample(jj): inputs (p_1 .. p_H, t), output the synthesized sample at frame
// position jj, sum over h of exp(p_h) sin(w_h (t + jj)); `_ <: si.bus(H)`
// copies t to every harmonic, interleave pairs each p_h with it
sample(jj) = (si.bus(H), (_ <: si.bus(H))) : ro.interleave(H, 2)
           : par(h, H, (_, _ : \(p, t).(exp(p) * sin(w(h + 1) * (t + jj))))) :> _;
// the whole frame: (p bus, t) fanned out to the N sample positions
synth_frame = (si.bus(H), _) <: par(jj, N, sample(jj));
// |c + i s| with an epsilon under the root, so the derivative exists at 0
magnitude(c, s) = sqrt(c * c + s * s + 1e-9);
// the windowed correlations of an N-sample frame at harmonic h, then its magnitude
mag(h) = (par(jj, N, *(win(jj) * cos(w(h + 1) * jj))) :> _),
         (par(jj, N, *(win(jj) * sin(w(h + 1) * jj))) :> _) : magnitude;
spectrum = si.bus(N) <: par(h, H, mag(h));         // a frame to its H magnitudes
target_mags = par(h, H, magnitude);                // the target's 2H correlations to its H magnitudes
// inputs (p_1 .. p_H, t, the 2H target correlations); output the sum over h
// of (|X_h| - |T_h|)^2, the frame's spectral loss
loss_block = (((si.bus(H), _) : synth_frame : spectrum), (si.bus(2 * H) : target_mags))
           : ro.interleave(H, 2) : par(h, H, (_, _ : \(m, mt).((m - mt) * (m - mt)))) :> _;

// ---- the loop, in the block's own time: one iteration per frame
// The loop inside the block: the recursion keeps the deviation of the
// log-amplitudes from log(a0); the block inputs t and the 2H correlations
// are the loss's free inputs.
upd(g) = op.adam_g(0.02, 0.9, 0.999, 1e-8, g);     // the Adam step for one gradient
// step: inputs (H deviations q_h, t, 2H correlations); outputs the H new
// deviations. Line by line: the log-amplitudes p_h = clip(log a0 + q_h) in
// [-8, 1]; the p bus duplicated, one copy kept, one fed to the loss; `rad` on
// the loss block with the H p inputs as seeds, its loss output dropped, its
// H gradients kept; each (p_h, g_h) pair stepped by Adam, clipped, and turned
// back into a deviation.
step = (par(h, H, op.clip(-8.0, 1.0, log(a0) + _)), _, si.bus(2 * H))
     : ((si.bus(H) <: (si.bus(H), si.bus(H))), _, si.bus(2 * H))
     : (si.bus(H), (rad(loss_block, si.bus(H)) : !, si.bus(H)))
     : ro.interleave(H, 2) : par(h, H, (_, _ : \(p, g).(op.clip(-8.0, 1.0, p - upd(g)) - log(a0))));
train = step ~ si.bus(H);                          // the deviations are the recursion state, advanced per firing
// the block: fired by clk, its inputs the frame's time origin (which advances
// by N per frame; the loss compares magnitudes, so its phase against the
// target is immaterial) and the 2H correlations; its outputs, held between
// frames, are turned back into amplitudes
amps = (clk, ba.time - N, target_corr) : ondemand(train) : par(h, H, +(log(a0)) : exp);

// the resynthesis at audio rate with the learned amplitudes
resynthesis = (amps, par(h, H, os.osc((h + 1) * f0))) : ro.interleave(H, 2) : par(h, H, *) :> _;

// ---- outputs
// [0..15] the learned amplitudes of harmonics 1 to 16, held between frames:
//         all start at 0.1 and reach 1/h within 1e-3 in 100 frames
// [16]    target - resynthesis at audio rate: -> 0 as the amplitudes converge
//         (the oscillators of the target and of the resynthesis share their
//         phase, so the difference is the amplitude error only)
process = amps, target_audio - resynthesis;
