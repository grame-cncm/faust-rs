// Frame-rate FFT (analysis-only) built on the boolean `ondemand` block via the
// `interleave.lib` serialization harness — no dedicated FFT compiler primitive.
// One O(N log N) transform runs per hop of N=64 samples (held between frames)
// instead of per sample. Output: 2N held bin reals [re0, im0, re1, im1, ...] at
// frame rate; consume them directly for a spectral loss / analyzer (no
// resynthesis — see ondemand_fft_lowpass_* for the full time->spectrum->time
// round-trip).
//
// Uses the real `interleave.lib` (il.frame_clock / il.serialize_in), the S2
// primitive from commit c088ced. The DSP core (an.fftb /
// an.c_bit_reverse_shuffle) is reused unchanged.
//
// Semantics: porting/interleave-spectral-primitive-2026-07-07-en.md,
// mirrored by crates/compiler/tests/interleave_fft.rs.
// Requires -I <faustlibraries> (analyzers.lib, signals.lib) and -I . (interleave.lib).

il = library("interleave.lib");
an = library("analyzers.lib");
si = library("signals.lib");

// Spatial DFT core (unchanged): complexify each real tap, shuffle, butterflies.
fftFX(N) = par(i, N, (_, 0)) : an.c_bit_reverse_shuffle(N) : an.fftb(N);

// Analysis-only frame-rate FFT: split the window, then run fftFX once per hop.
process = il.serialize_in(64)
        : (il.frame_clock(64), si.bus(64))
        : ondemand(fftFX(64));
