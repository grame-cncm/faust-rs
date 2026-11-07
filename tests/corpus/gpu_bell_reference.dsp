// The reference of the GPU carillon: the original `pm.standardBell` of
// physmodels.lib (a strike, then 50 `pm.modeFilter` modes summed and divided
// by 50), computed as one program. `make mojo-gpu-bells` checks that one bell
// of the GPU carillon (one gpu_bell_strike.dsp instance and 50
// gpu_bell_mode.dsp instances) reproduces it, on the CPU, for every strike
// position.

import("stdfaust.lib");

position = nentry("position", 0, 0, 6, 1);
cutoff = hslider("cutoff", 6500, 20, 20000, 1);
sharpness = hslider("sharpness", 0.5, 0.01, 5, 0.01);
gain = hslider("gain", 1, 0, 1, 0.01);
gate = button("gate");

process = pm.standardBell(position, cutoff, sharpness, gain, gate);
