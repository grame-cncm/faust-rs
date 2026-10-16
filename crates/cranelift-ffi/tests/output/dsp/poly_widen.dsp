// A one-channel voice and a stereo effect: `poly-dsp.h`'s adaptor splits the
// voices' channel into the effect's two inputs (`adapt(1,2) = _ <: _,_`).
gain = hslider("gain", 0.5, 0, 1, 0.01);
gate = button("gate");
process = gate * gain;
effect = *(0.25), *(0.75);
