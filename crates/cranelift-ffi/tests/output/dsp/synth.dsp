import("stdfaust.lib");
freq = hslider("freq", 440, 20, 20000, 1);
gain = hslider("gain", 0.5, 0, 1, 0.01);
gate = button("gate");
bright = hslider("bright", 0.5, 0, 1, 0.01);
process = os.sawtooth(freq) * gain * en.adsr(0.001, 0.01, 0.8, 0.02, gate) : fi.lowpass(1, 200 + bright * 8000);
effect = _ * hslider("wet", 0.5, 0, 1, 0.01);
