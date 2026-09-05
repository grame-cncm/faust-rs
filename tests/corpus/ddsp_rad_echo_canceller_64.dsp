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

N = 64;
far = no.noise;
room(i) = sin(1.7 * i + 0.3) * exp(-i / 12.0);
fir = si.bus(N), (_ <: par(i, N, @(i))) : ro.interleave(N, 2) : par(i, N, *) :> _;
mic = (par(i, N, room(i)), far) : fir;

h = op.lsq_N_rad(N, fir, op.nlms(0.01, 0.000001, 0.99), -2.0, 2.0, 0.0, 0.0, mic, far);
residual = mic - ((h, far) : fir);

process = residual, mic;
