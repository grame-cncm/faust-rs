// GPU batch voice: one voice of a polyphonic instrument, a sawtooth through
// two one-pole low-passes and a short comb, under an envelope on `gate`.
// Played by `make mojo-gpu-poly` in tests/impulse-tests (G4 of
// porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md; results in
// porting/faust-gpu-audio-synthesis-2026-11-07-en.md §2.5). Self-contained (no
// library); written for 48 kHz.
//
// The program describes one voice; the number of voices is not in it. The
// runner takes it at run time (`POLY_VOICES`, 256 by default), passes it to the
// struct as `mydsp(voices, state)`, launches one GPU thread per voice, and gives
// each voice its own `freq` and scripted `gate` through `set_control`.

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
