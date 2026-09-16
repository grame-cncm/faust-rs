// optimizers.lib: the `fad` tangent of `bank_log_energy_loss` against a
// central finite difference.
//
// The loss is the eight-band bank loss between `g x` and `0.7 x` as a
// function of the gain `g` (held at 0.5); `fad` gives d(loss)/dg through
// the eight band-pass filters and the smoothed log energies, and the
// finite difference `(loss(g + h) - loss(g - h)) / 2 h` with `h = 1e-3`
// must agree with it to the order of `h^2`.
//
// Requires -I libraries (project-local optimizers.lib) and the directory of
// the Faust standard libraries on the import path (-I <faustlibraries>).
//
// Outputs: [fad tangent, finite difference]

import("stdfaust.lib");
op = library("optimizers.lib");

x = no.noise;
target = 0.7 * x;
loss(g) = op.bank_log_energy_loss(8, 150.0, 4800.0, 0.999, 0.000000001, g * x, target);
g = 0.5;
h = 0.001;

process = (fad(loss(g), g) : !, _), (loss(g + h) - loss(g - h)) / (2.0 * h);
