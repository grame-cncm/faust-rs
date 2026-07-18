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
ma = library("maths.lib");
N = 16;
L = 2;                                     // kernel length (<= N)
h(0) = 0.5;                                // fixed FIR kernel h = [0.5, 0.5]
h(1) = 0.5;

// Kernel spectrum H(m) = DFT{h}, folded at compile time (Hermitian -> real out).
reH(m) = sum(k, L,       h(k) * cos(2.0*ma.PI*m*k/N));
imH(m) = sum(k, L, 0.0 - h(k) * sin(2.0*ma.PI*m*k/N));
// Per-bin complex product X(m)*H(m).
mulH(m, xr, xi) = xr*reH(m) - xi*imH(m), xr*imH(m) + xi*reH(m);
conv = par(m, N, mulH(m));

convFX(NN) = par(i,NN,(_,0)) : an.fft(NN) : conv : an.ifft(NN) : par(i,NN,(_,!));
process    = il.interleave(N, convFX(N));
