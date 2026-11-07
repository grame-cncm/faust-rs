// One voice of the polyphonic runner of the GPU batch plan (G4,
// porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md): a sawtooth through
// two one-pole low-passes and a short comb, under an envelope on `gate`.
// Self-contained (no library), so that the voice is the same wherever the
// runner is built. Written for 48 kHz.

declare name "poly_voice";

freq = hslider("freq", 220, 20, 4000, 0.01);
gain = hslider("gain", 0.1, 0, 1, 0.001);
gate = button("gate");

smooth(c) = *(1.0 - c) : + ~ *(c);
wrap(x) = x - floor(x);

phase = (freq / 48000.0) : (+ : wrap) ~ _;
saw = phase * 2.0 - 1.0;
comb(d, g) = + ~ (@(d) : *(g));
env = gate : smooth(0.999);

process = saw : smooth(0.7) : smooth(0.7) : comb(113, 0.6) : *(env * gain);
