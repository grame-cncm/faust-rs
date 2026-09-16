// optimizers.lib: the two basin-widening losses are symmetric in their
// arguments and at their floor when the arguments coincide.
//
// On a noise and its low-passed copy: `corr_loss(y, t) - corr_loss(t, y)`
// and `bank_log_energy_loss(y, t) - bank_log_energy_loss(t, y)` are zero
// bit for bit (the products under the correlation and the squared log
// ratio commute); `corr_loss(t, t)` tends to -1 and the bank loss of a
// signal with itself is exactly 0.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [corr asymmetry, bank asymmetry, corr_loss(t, t), bank loss(t, t)]

import("stdfaust.lib");
op = library("optimizers.lib");

y = no.noise;
t = no.noise : fi.lowpass(2, 800.0);
corr(u, v) = op.corr_loss(0.999, 0.000000001, u, v);
bank(u, v) = op.bank_log_energy_loss(8, 150.0, 4800.0, 0.999, 0.000000001, u, v);

process = corr(y, t) - corr(t, y), bank(y, t) - bank(t, y), corr(t, t), bank(t, t);
