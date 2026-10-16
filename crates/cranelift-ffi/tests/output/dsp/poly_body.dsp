// The shape of a piano: a voice of three channels (its regions) and a body
// that makes them stereo. `poly-dsp.h`'s adaptor has no rule past two
// channels; faustprobe chains an effect of the voices' width whatever it is.
gain = hslider("gain", 0.5, 0, 1, 0.01);
gate = button("gate");
process = gate * gain <: _, *(0.5), *(0.25);
effect = _, _, _ <: (_, !, *(0.5) :> _), (!, _, *(0.5) :> _) : *(wet), *(wet)
with { wet = hslider("wet", 1, 0, 1, 0.01); };
