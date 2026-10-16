// A voice of two channels and an effect that mixes them down to one: the
// instrument has the effect's single output, not the voices' two.
gain = hslider("gain", 0.5, 0, 1, 0.01);
gate = button("gate");
process = gate * gain <: _, *(0.5);
effect = + : *(2.0);
