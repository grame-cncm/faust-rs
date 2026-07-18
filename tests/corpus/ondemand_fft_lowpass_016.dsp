// Complete FFT round-trip (time -> spectrum -> spectral effect -> time) built on
// the `interleave.lib` S2 primitive `il.interleave(N, FX)` (commit c088ced):
// serialize_in -> ondemand(FX) -> serialize_out (zero-stuff + overlap-add).
// FX is the N-in/N-out frame operator below; the round-trip latency is N-1
// samples (il.interleave(N, id) == @(N-1), proven in ondemand_pipeline.rs).
//
// Framing is rectangular, non-overlapping (hop = N): each sample belongs to
// exactly one frame, so reconstruction with an identity FX is exact (COLA holds
// trivially). A real windowed / overlap-add STFT would use il.interleave_hop.
//
// Requires -I <faustlibraries> (analyzers.lib) and -I . (interleave.lib).

il = library("interleave.lib");
an = library("analyzers.lib");
N  = 16;
kc = 4;                               // brickwall cutoff bin (keep |m| <= kc)

// Real per-bin gain, Hermitian-symmetric so the IFFT output stays real.
gain(m) = float(min(m, N-m) <= kc);
mul(g)  = *(g), *(g);                      // scale complex bin (re,im) by real g
mask    = par(m, N, mul(gain(m)));

lpFX(NN) = par(i,NN,(_,0)) : an.fft(NN) : mask : an.ifft(NN) : par(i,NN,(_,!));
process  = il.interleave(N, lpFX(N));
