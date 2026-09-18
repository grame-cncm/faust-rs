import("stdfaust.lib");
process = os.osc(hslider("freq", 440, 20, 20000, 1)) * hslider("gain", 0.5, 0, 1, 0.001) * button("gate");
