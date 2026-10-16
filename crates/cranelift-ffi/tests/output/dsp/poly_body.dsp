// The shape of a piano: a voice of three channels (its regions) and a body
// that makes them stereo. `poly-dsp.h`'s adaptor passes the three channels
// through (`bus(3) <: bus(3)`).
gain = hslider("gain", 0.5, 0, 1, 0.01);
gate = button("gate");
process = gate * gain <: _, *(0.5), *(0.25);
effect = _, _, _ <: (_, !, *(0.5) :> _), (!, _, *(0.5) :> _) : *(wet), *(wet)
with { wet = hslider("wet", 1, 0, 1, 0.01); };
