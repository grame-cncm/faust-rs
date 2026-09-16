// optimizers.lib: the loss landscape of the waveguide string under four
// losses, at a fixed pitch set by the host.
//
// The string of ddsp_fad_waveguide_string_pitch.dsp with its delay set from
// the `pitch` control (no learning); the outputs are, averaged by the host
// over the run, the waveform error `mse`, the normalised correlation
// `corr_loss`, the single-band `log_energy_loss` and the eight-band
// `bank_log_energy_loss` (150 to 4 800 Hz) between the model at that pitch
// and the hidden string at 220 Hz. A host loop sweeps the pitch:
//
//   for f in $(seq 150 2 300); do
//     faustprobe --double -I libraries -I <faustlibraries> --in zero -n 24000 --skip 4000 --quiet --set pitch=$f tests/corpus/opt_landscape_string.dsp
//   done
//
// and reads the `dc` of each output: the width of each loss's well around
// 220 Hz is what decides whether a descent from far away can find it.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [mse, corr_loss, log_energy_loss, bank_log_energy_loss]

import("stdfaust.lib");
op = library("optimizers.lib");

MAXD = 512;
x = 0.1 * no.noise;
string(d, g, s) = (+(s) : de.fdelay4(MAXD, d - 1.0) : *(g) : si.smooth(0.3)) ~ _;

target = string(ma.SR / 220.0, 0.95, x);
pitch = hslider("pitch", 220.0, 100.0, 440.0, 0.01);
model = string(ma.SR / pitch, 0.95, x);

process = op.mse(model, target),
          op.corr_loss(0.999, 0.000000001, model, target),
          op.log_energy_loss(0.999, 0.000000001, model, target),
          op.bank_log_energy_loss(8, 150.0, 4800.0, 0.999, 0.000000001, model, target);
