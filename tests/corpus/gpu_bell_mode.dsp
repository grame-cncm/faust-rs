// GPU batch voice: one mode of a bell, the `pm.modeFilter` of physmodels.lib
// (a state-variable realisation that keeps its poles in float) with its own
// frequency, 60 dB decay time and gain, driven by its bell's strike.
//
// The carillon of tests/impulse-tests (`make mojo-gpu-bells`) runs one instance
// per mode of every bell: the number of instances (50 per bell) and each
// instance's `freq`, `t60` and `gain`, taken from `pm.standardBellModel`, are
// set by the host at run time, not written here.

import("stdfaust.lib");

freq = hslider("freq", 490.25, 20, 20000, 0.001);
t60 = hslider("t60", 30, 0.001, 60, 0.001);
gain = hslider("gain", 0.5, 0, 1, 0.0001);

process = pm.modeFilter(freq, t60, gain);
