import("stdfaust.lib");
freq = hslider("freq", 440, 20, 20000, 1);
gain = hslider("gain", 0.5, 0, 4, 0.01);
gate = button("gate");
process = _ * gain * gate + os.osc(freq) * gain * gate * 0.1;
