import("stdfaust.lib");
process = _ * (hslider("gain", 0.5, 0, 1, 0.001) : si.smoo);
