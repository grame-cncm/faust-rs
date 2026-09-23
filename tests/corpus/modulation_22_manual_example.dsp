// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// the manual's example, an LFO on `Wet`, on a stand-in for freeverb_demo.
lfo(f, g) = 1 + g * sin(2 * 3.141592653589793 * phase) with { phase = (+(f / 44100.0) ~ (_ <: _ - floor)); };
freeverb = hgroup("Freeverb", (_, _) : (*(wet), *(wet))) with { wet = vslider("[1] Wet [tooltip: The amount of reverb applied to the signal
 between 0 and 1 with 1 for the maximum amount of reverb.]", 0.3333, 0, 1, 0.025); };
process = lfo(10, 0.5), _, _ : ["Wet" -> freeverb];
