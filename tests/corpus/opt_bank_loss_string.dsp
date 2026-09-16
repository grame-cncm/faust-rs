// optimizers.lib: the waveguide string learned through the eight-band bank
// loss, next to the waveform error.
//
// The string of ddsp_fad_waveguide_string_pitch.dsp from 224 Hz, 4 Hz above
// the hidden 220 Hz. `bank_log_energy_loss` (eight bands, 150 to 4 800 Hz)
// slopes monotonically toward 220 Hz from about 218 to 226 Hz (measured
// with opt_landscape_string.dsp), and SGD on it reaches 220 Hz. The rate is
// 1e-4, well under the 1 - a = 1e-3 of the loss's smoothing, the tutorial's
// rule on smoothed losses: 5e-4 oscillates. The waveform error, whose well
// is +-1 Hz wide, reaches 220 Hz from 224 Hz too, carried by the slope of
// its plateau (it captures from above up to 228 Hz and drifts away from
// below): on this string the bank's wider well does not buy a start the
// waveform error cannot handle, the harmonic alignments at 214 and 216 Hz
// blocking both from below.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [pitch in Hz through the bank loss, pitch in Hz through the waveform error]

import("stdfaust.lib");
op = library("optimizers.lib");

MAXD = 512;
x = 0.1 * no.noise;
string(d, g, s) = (+(s) : de.fdelay4(MAXD, d - 1.0) : *(g) : si.smooth(0.3)) ~ _;

f_star = 220.0;
target = string(ma.SR / f_star, 0.95, x);
mdl(d, s) = string(d, 0.95, s);
f0 = 224.0;

bank_loss(d) = op.bank_log_energy_loss(8, 150.0, 4800.0, 0.999, 0.000000001, mdl(d, x), target);
d_bank = op.descend_1D(bank_loss, op.sgd_g(0.0001), 100.0, 400.0, ma.SR / f0, 0.0);
d_wave = op.lsq_1D(mdl, op.nlms(0.02, 0.000001, 0.99), 100.0, 400.0, ma.SR / f0, 0.0, target, x);

process = ma.SR / d_bank, ma.SR / d_wave;
