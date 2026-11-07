// GPU batch voice: the strike of one bell, the `pm.strikeModel` of
// physmodels.lib (filtered noise under a short envelope), as
// `pm.standardBell` drives its modes.
//
// The carillon of tests/impulse-tests (`make mojo-gpu-bells`) runs one
// instance per bell; the host strikes a bell by raising its `gate`, and every
// mode of that bell reads this output as its input.

import("stdfaust.lib");

cutoff = hslider("cutoff", 6500, 20, 20000, 1);
sharpness = hslider("sharpness", 0.5, 0.01, 5, 0.01);
gain = hslider("gain", 1, 0, 1, 0.01);
gate = button("gate");

process = pm.strikeModel(10, cutoff, sharpness, gain, gate);
