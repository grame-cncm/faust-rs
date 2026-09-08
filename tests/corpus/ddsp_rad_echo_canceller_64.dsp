// DDSP example (RAD): a 64-tap acoustic echo canceller.
//
// The far-end signal goes to a loudspeaker; the microphone picks up its
// echo through a room response of 64 taps (here a synthetic decaying
// response). The canceller learns an FIR replica of that response and
// subtracts it from the microphone signal, the classic NLMS echo canceller
// (Haykin). With `lsq_N_rad`, the 64 sensitivities of the FIR output --
// the delayed far-end samples -- come from one reverse sweep per sample;
// `lsq_N` would carry 64 tangents for the same trajectory.
//
// The echo return loss enhancement, ERLE = 10 log10(power(mic) /
// power(residual)), passes 30 dB within a few thousand samples.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [residual echo, microphone]

import("stdfaust.lib");
op = library("optimizers.lib");

N = 64;                                   // taps of the room response and of the canceller
far = no.noise;                           // the far-end signal, sent to the loudspeaker
// the room response, tap i: a decaying oscillation, time constant 12 samples
room(i) = sin(1.7 * i + 0.3) * exp(-i / 12.0);
// An FIR as a block of N + 1 inputs: the N taps first, the signal last.
// `_ <: par(i, N, @(i))` makes the N delayed copies x[n - i] of the signal;
// `ro.interleave(N, 2)` pairs tap i with x[n - i]; each pair is multiplied
// and the products are summed. This is the shape the bus loops expect.
fir = si.bus(N), (_ <: par(i, N, @(i))) : ro.interleave(N, 2) : par(i, N, *) :> _;
// the microphone: the far-end signal through the room, i.e. the same FIR
// with the room's taps as coefficients
mic = (par(i, N, room(i)), far) : fir;

// lsq_N_rad(N, mdl, engine, lo, hi, init, reset, target, x): the N taps as a
// bus, least squares of fir(taps, far) against mic; one NLMS engine per tap
// (mu 0.01, eps 1e-6, power smoothing 0.99), each tap normalised by the
// power of its own sensitivity; taps in [-2, 2], starting at 0; no reset.
// The N sensitivities, the delayed far-end samples, come from one reverse
// sweep of `rad` per sample.
h = op.lsq_N_rad(N, fir, op.nlms(0.01, 0.000001, 0.99), -2.0, 2.0, 0.0, 0.0, mic, far);
// what remains of the echo once the replica is subtracted
residual = mic - ((h, far) : fir);

// ---- outputs
// [0] the residual echo, mic - replica: what the far end would hear of
//     itself; falls from the echo's level to the noise floor of the
//     adaptation (ERLE past 30 dB within a few thousand samples)
// [1] the microphone signal, the echo before cancellation, for the ERLE
//     ratio power(mic) / power(residual)
process = residual, mic;
