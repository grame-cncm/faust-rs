// optimizers.lib: four starts on the waveguide string, the loop following
// the one whose residual is lowest.
//
// The string of ddsp_fad_waveguide_string_pitch.dsp: the waveform loss is a
// well +-1 Hz wide around 220 Hz on a flat plateau, captured from above only
// (228 Hz locks, 200 and 176 Hz drift, 264 Hz wanders). Four least-squares
// NLMS loops run from 176, 200, 228 and 264 Hz; `multistart_lsq_1D`
// outputs the pitch of the one whose smoothed squared residual is lowest.
// The start at 228 Hz locks, its residual vanishes, and from then on the
// index is 2 and the pitch 220 Hz.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [pitch in Hz of the best start, its index]

import("stdfaust.lib");
op = library("optimizers.lib");

MAXD = 512;
x = 0.1 * no.noise;
string(d, g, s) = (+(s) : de.fdelay4(MAXD, d - 1.0) : *(g) : si.smooth(0.3)) ~ _;

f_star = 220.0;
target = string(ma.SR / f_star, 0.95, x);
mdl(d, s) = string(d, 0.95, s);

init(k) = ma.SR / ba.take(k + 1, (176.0, 200.0, 228.0, 264.0));
learned = op.multistart_lsq_1D(4, init, mdl, op.nlms(0.02, 0.000001, 0.99), 100.0, 400.0, 0.999, 0.0, target, x);
d = learned : _, !;
k = learned : !, _;

process = ma.SR / d, k;
