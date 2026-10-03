// DDSP example (fad): a whole effect, written with its sliders as a player
// would write it, learns all six of them from a recording of another
// setting, without being rewritten (optimizers.lib's `adaptive_fad`).
//
// `pedal` is an ordinary program: a tight high-pass, a drive into tanh, a
// tone low-pass, a Regalia-Mitra mid peak and a level, six sliders in their
// own units (Hz, dB) under a group. Nothing in it mentions learning. The
// "recorded" pedal is the same program at a hidden setting, given by named
// literal modulations; the excitation is a sawtooth with a little noise
// under a slow envelope, so that the tanh is driven at several depths and
// drive and level stay separable.
//
// `adaptive_fad` counts the controls with `cinputs`, reads their ranges and
// defaults with `cinput`, rebinds them with the `"*"` modulation and runs
// `descend_N_fad_clocked`: one Adam step every 512 samples on the frame
// mean of the six `fad` gradients of the squared error, each step 1 % of
// the slider's own range (`ct.by_range` of controls.lib), each parameter
// bounded by its slider's range and started at its default.
//
// The mid peak is `fi.peak_eq_rm`, smooth in its gain: `fi.peak_eq` takes
// `abs` of the gain, whose derivative at its 0 dB default is not a number.
//
// Convergence: the six sliders within 1e-4 of the hidden setting (20 dB,
// -6 dB, 1200 Hz, 5 dB, 150 Hz, 1800 Hz) from 175 000 samples, residual
// 2e-7 rms over the last 20 000 of 200 000.
//
// Requires -I libraries (project-local optimizers.lib, controls.lib) and the directory of the
// Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [drive, level, mid_freq, mid_gain, tight, tone, residual], the
// sliders in the order of the interface (`cinputs`).

import("stdfaust.lib");
op = library("optimizers.lib");
ct = library("controls.lib");

// the effect, as written for a player
pedal = hgroup("pedal",
      fi.highpass(1, hslider("tight", 80, 20, 400, 1))
    : *(ba.db2linear(hslider("drive", 12, 0, 30, 0.1)))
    : ma.tanh
    : fi.lowpass(1, hslider("tone", 3000, 500, 8000, 1))
    : fi.peak_eq_rm(hslider("mid_gain", 0, -12, 12, 0.1), hslider("mid_freq", 800, 200, 3000, 1), tan(ma.PI * 400 / ma.SR))
    : *(ba.db2linear(hslider("level", -12, -40, 0, 0.1))));

// the "recorded" pedal: the same program at another setting
hidden = ["tight": 150, "drive": 20, "tone": 1800, "mid_gain": 5, "mid_freq": 1200, "level": -6 -> pedal];

x = 0.3 * env * (0.7 * os.sawtooth(110) + 0.3 * no.noise)
with { env = 0.55 + 0.45 * sin(2 * ma.PI * os.phasor(1, 1.3)); };
target = x : hidden;

// one Adam per slider, its step 1 % of the slider's range
upd = ct.by_range(\(lr).(op.adam_g(lr, 0.9, 0.999, 1e-8)), 0.01, pedal);
clock = (ba.time % 512) == 511;

// adaptive_fad outputs the pedal's audio, then the six learned sliders. The
// lambda names the audio `y`; its body's free inputs, the `si.bus` of the
// six sliders, follow `y` as inputs of the lambda, so the six pass through
// unchanged and the audio is replaced by the residual `y - target`.
// `ct.count(pedal)` keeps the bus as wide as the pedal has controls.
process = op.adaptive_fad(pedal, op.mse, upd, clock, 0, x, target) : \(y).(si.bus(ct.count(pedal)), y - target);
