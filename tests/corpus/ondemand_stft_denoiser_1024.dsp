// Real-time spectral denoiser (Wiener-style gate) on a 1024-point frame-rate
// FFT built entirely on the boolean `ondemand` block via `interleave.lib`.
//
// Why N=1024: at 48 kHz one bin is ~46.9 Hz wide and a frame lands every ~21 ms.
// That fine resolution concentrates a tonal partial's energy into ~one
// high-magnitude bin while broadband noise stays low *per bin* — exactly the
// separation a spectral gate exploits. The effect is only meaningfully useful
// at large N, which is what this example showcases.
//
// Per bin the gain is the Wiener form  g = |X|² / (|X|² + t²)  (t = noise-floor
// estimate): g → 1 for partials well above the floor, g → 0 for bins near/below
// it. It is real and depends only on |X| (Hermitian-symmetric), so the IFFT
// output stays real. Analysis is a periodic Hann window at hop = N/2, which
// satisfies COLA (Σ wa = 1); at t = 0 the gate is transparent and the windowed
// overlap-add reconstructs the input exactly (latency N-1) — the correctness
// anchor (verified: max|wet[t] − dry[t−(N-1)]| ≈ 1e-6).
//
// Requires the raised evaluator budget for the deep FFT recursion at N=1024:
//   FAUST_RS_DEFAULT_EVAL_MAX_DEPTH=4096
// and the O(N log N) FIR CSE (per-scope, commit 03127f30) that keeps the
// generated code tractable (~0.5 M interp lines, ~40 s to compile).
//
// Requires -I <faustlibraries> (analyzers.lib, maths.lib) and -I . (interleave.lib).

il = library("interleave.lib");
an = library("analyzers.lib");
ma = library("maths.lib");

N   = 1024;
hop = 512;                                         // 50 % overlap, Hann COLA
thresh = hslider("noise floor[scale:log]", 0.02, 0.0, 1.0, 0.001);

wa(n) = 0.5 - 0.5*cos(2.0*ma.PI*n/N);              // periodic Hann analysis window
win   = par(n, N, *(wa(n)));

// Wiener gain per complex bin (re, im).
gate(re, im) = re*g, im*g
with {
    p = re*re + im*im;                             // |X|²
    g = p / (p + thresh*thresh);                   // → 1 loud, → 0 quiet
};
denoise = par(m, N, gate);

// time → windowed complex frame → FFT → gate → IFFT → real part
fx(NN) = win : par(i, NN, (_, 0)) : an.fft(NN) : denoise : an.ifft(NN) : par(i, NN, (_, !));

process = il.interleave_hop(N, hop, fx(N));
