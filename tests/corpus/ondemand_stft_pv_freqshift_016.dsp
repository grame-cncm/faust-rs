// Phase-vocoder with INTER-FRAME phase accumulation, built on the frame-rate
// `ondemand` block (il.interleave_hop). This is the piece the earlier spectral
// fixtures could not express: per-bin state that updates ONCE PER FRAME inside
// the ondemand block — a one-frame phase memory (ph') and a synthesis phase
// accumulator (psi = incr : +~_), both running on the per-domain (frame) clock.
//
// Per bin k, each frame (hop samples):
//   mag  = |X_k|,  ph = atan2(Im, Re)
//   incr = expected_k + princarg(ph - ph' - expected_k)   // true per-hop advance
//   psi += <increment>                                     // frame-rate recursion
//   bin  = mag * (cos psi, sin psi)
// with expected_k = 2*pi*k*hop/N and princarg = wrap to (-pi, pi].
//
// NOTE on time-stretch: changing DURATION (analysis hop != synthesis hop) is not
// expressible in a synchronous 1-in/1-out Faust process at constant rate. The
// inter-frame phase machinery itself — the actual gap vs SuperCollider/Csound —
// is what these fixtures demonstrate; its correct constant-rate product is a
// frequency shifter (below). A duration-changing stretch needs an external
// rate-decoupling buffer (out of the synchronous model).
//
// Requires -I <faustlibraries> (analyzers, maths, oscillators) and -I . (interleave.lib).

il = library("interleave.lib");
an = library("analyzers.lib");
ma = library("maths.lib");
N = 16; hop = 16/2;
shift = 2.0*ma.PI/16.0;                             // +fs/16 rad/sample (swap for an hslider in Hz)

wa(n) = 0.5 - 0.5*cos(2.0*ma.PI*n/N);
win   = par(n, N, *(wa(n)));
tocart(mag, ph) = mag*cos(ph), mag*sin(ph);
wrap(p) = p - 2.0*ma.PI*floor(p/(2.0*ma.PI) + 0.5);

// Add a constant per-sample phase rate to every bin: the whole spectrum's fine
// structure shifts by a constant frequency (single-sideband frequency shift).
pvbin(k, re, im) = tocart(mag, psi)
with {
    expc = 2.0*ma.PI*k*hop/N;
    mag  = sqrt(re*re + im*im);
    ph   = atan2(im, re);
    incr = expc + wrap((ph - ph') - expc);
    psi  = (incr + shift*hop) : (+ ~ _);            // true advance + constant shift
};
pv = par(k, N, pvbin(k));
pvFX(NN) = win : par(i,NN,(_,0)) : an.fft(NN) : pv : an.ifft(NN) : par(i,NN,(_,!));

// Verified: DC input -> pure tone at exactly the shift frequency (impulse-runner).
process = il.interleave_hop(N, hop, pvFX(N));
