// Complete FFT round-trip (time -> spectrum -> spectral effect -> time) via the
// `interleave.lib` S2 primitive `il.interleave(N, FX)` (commit c088ced):
// serialize_in -> ondemand(FX) -> serialize_out. FX is N-in/N-out; round-trip
// latency is N-1 samples (il.interleave(N, id) == @(N-1)).
//
// Rectangular, non-overlapping framing (hop = N): spectral products realize a
// *circular* convolution per frame. A linear-convolution / windowed STFT needs
// il.interleave_hop + a COLA window (see ondemand_stft_pv_*).
//
// Requires -I <faustlibraries> (analyzers.lib, maths.lib) and -I . (interleave.lib).

il = library("interleave.lib");
an = library("analyzers.lib");
N   = 16;
klo = 2;
khi = 5;                               // keep bins with klo <= min(m,N-m) <= khi

band(m) = min(m, N-m);
gain(m) = float((band(m) >= klo) & (band(m) <= khi));
mul(g)  = *(g), *(g);
mask    = par(m, N, mul(gain(m)));

bpFX(NN) = par(i,NN,(_,0)) : an.fft(NN) : mask : an.ifft(NN) : par(i,NN,(_,!));
process  = il.interleave(N, bpFX(N));
