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

H = 16;
N = 256;
f0 = 440.0;
w(h) = 2.0 * ma.PI * h * f0 / ma.SR;
a_star(h) = 1.0 / (h + 1.0);
a0 = 0.1;                                          // initial amplitude of every harmonic

// The target, at audio rate, and its frame analysis: windowed correlations
// at the harmonics, summed over the frame.
target_audio = par(h, H, a_star(h) * os.osc((h + 1) * f0)) :> _;
clk = il.frame_clock(N);
j = float(ba.time % N);
win(j) = 0.5 - 0.5 * cos(2.0 * ma.PI * j / N);
target_corr = par(h, H, (op.frame_sum(clk, target_audio * win(j) * cos(w(h + 1) * j)),
                         op.frame_sum(clk, target_audio * win(j) * sin(w(h + 1) * j))));

// Inside the block: the synthesized frame for log-amplitudes (p_1..p_H) and
// frame start t, its magnitudes, the loss against the target's.
sample(jj) = (si.bus(H), (_ <: si.bus(H))) : ro.interleave(H, 2)
           : par(h, H, (_, _ : \(p, t).(exp(p) * sin(w(h + 1) * (t + jj))))) :> _;
synth_frame = (si.bus(H), _) <: par(jj, N, sample(jj));
magnitude(c, s) = sqrt(c * c + s * s + 1e-9);
mag(h) = (par(jj, N, *(win(jj) * cos(w(h + 1) * jj))) :> _),
         (par(jj, N, *(win(jj) * sin(w(h + 1) * jj))) :> _) : magnitude;
spectrum = si.bus(N) <: par(h, H, mag(h));
target_mags = par(h, H, magnitude);
loss_block = (((si.bus(H), _) : synth_frame : spectrum), (si.bus(2 * H) : target_mags))
           : ro.interleave(H, 2) : par(h, H, (_, _ : \(m, mt).((m - mt) * (m - mt)))) :> _;

// The loop inside the block: the recursion keeps the deviation of the
// log-amplitudes from log(a0); the block inputs t and the 2H correlations
// are the loss's free inputs.
upd(g) = op.adam_g(0.02, 0.9, 0.999, 1e-8, g);
step = (par(h, H, op.clip(-8.0, 1.0, log(a0) + _)), _, si.bus(2 * H))
     : ((si.bus(H) <: (si.bus(H), si.bus(H))), _, si.bus(2 * H))
     : (si.bus(H), (rad(loss_block, si.bus(H)) : !, si.bus(H)))
     : ro.interleave(H, 2) : par(h, H, (_, _ : \(p, g).(op.clip(-8.0, 1.0, p - upd(g)) - log(a0))));
train = step ~ si.bus(H);
amps = (clk, ba.time - N, target_corr) : ondemand(train) : par(h, H, +(log(a0)) : exp);

resynthesis = (amps, par(h, H, os.osc((h + 1) * f0))) : ro.interleave(H, 2) : par(h, H, *) :> _;

process = amps, target_audio - resynthesis;
