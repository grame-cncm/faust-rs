// Differentiable STFT spectral loss (milestone S4) on the frame-rate FFT.
//
// A learnable gain `g` scales the window INSIDE the ondemand block; the block
// computes a magnitude spectral loss `(Σ|X| − target)²` and `fad` differentiates
// it w.r.t. `g`. The block therefore emits [loss, dloss/dg] at frame rate — the
// gradient an in-graph optimizer would descend. No mainstream real-time
// environment offers gradient-through-FFT in the same graph.
//
// This is the "fad inside the block" form: the seed path crosses the block's
// TempVar window inputs, handled by the FAD Phase B (roadmap P5) boundary
// wrapper rules `(snap u)' = snap(u')`. Gradient validated vs central
// differences (crates/compiler/tests/ondemand_pipeline.rs). Magnitude carries an
// epsilon so the derivative is defined at a zero bin.
//
// Requires -I <faustlibraries> (analyzers, maths, signals) and -I . (interleave.lib).

il = library("interleave.lib");
an = library("analyzers.lib");
ma = library("maths.lib");
si = library("signals.lib");

N = 8;
g = hslider("gain[scale:log]", 1.0, 0.01, 10.0, 0.001);   // learnable parameter
target = 4.0;                                              // reference spectral energy

cmag(re, im) = sqrt(re*re + im*im + 0.000000001);          // |bin| with epsilon
magsum = par(m, N, cmag) :> _;
sq = _ <: _*_;

// Frame operator: scale window by g, FFT, magnitude sum, squared error to target.
lossFX(NN) = par(i, NN, *(g) : (_, 0)) : an.fft(NN) : magsum : -(target) : sq;

// [loss, dloss/dg] held at frame rate.
process = il.serialize_in(N) : (il.frame_clock(N), si.bus(N)) : ondemand(fad(lossFX(N), g));
