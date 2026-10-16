// A voice of three channels and an effect of two inputs, which no adaptor
// reconciles: the effect is left out, and faustprobe says so.
gain = hslider("gain", 0.5, 0, 1, 0.01);
gate = button("gate");
process = gate * gain <: _, *(0.5), *(0.25);
effect = _, _ :> _;
