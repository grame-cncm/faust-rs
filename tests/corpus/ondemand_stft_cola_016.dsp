// Overlap-add STFT (phase-vocoder structure) via the `interleave.lib` overlap
// primitive `il.interleave_hop(N, hop, FX)`: consecutive frames overlap by
// N-hop samples and are summed on resynthesis (serialize_out zero-stuffs at the
// hop rate and OLA-sums the delayed lanes). FX runs once per hop.
//
// COLA: with a periodic Hann analysis window wa(n) = 0.5 - 0.5 cos(2*pi*n/N) and
// hop = N/2, the shifted windows sum to 1 (wa[n] + wa[n+N/2] == 1), so an
// identity spectral stage reconstructs the input exactly in steady state
// (latency N-1). Rectangular window at the same hop double-counts the overlap
// (gain 2) — the window is what makes OLA reconstruct. FX applies the analysis
// window on the N real taps before complexify/FFT.
//
// Requires -I <faustlibraries> (analyzers.lib, maths.lib) and -I . (interleave.lib).

il = library("interleave.lib");
an = library("analyzers.lib");
ma = library("maths.lib");
N = 16; hop = 16/2;                            // 50% overlap

wa(n) = 0.5 - 0.5*cos(2.0*ma.PI*n/N);            // periodic Hann, COLA at hop N/2
win   = par(n, N, *(wa(n)));

// Identity spectral stage: window -> FFT -> IFFT -> real part.
idFX(NN) = win : par(i,NN,(_,0)) : an.fft(NN) : an.ifft(NN) : par(i,NN,(_,!));

// Constant 1.0 source -> [dry, OLA reconstruction]. wet -> 1.0 in steady state.
process = 1 <: _, il.interleave_hop(N, hop, idFX(N));
