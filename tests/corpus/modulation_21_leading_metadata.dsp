// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// a label that starts with metadata, `[1] Wet [tooltip: ...]` (a freeverb_demo slider): matched by `Wet`.
e6 = hgroup("Freeverb", (_, _) : (*(wet), *(wet))) with { wet = vslider("[1] Wet [tooltip: The amount of reverb applied to the signal
 between 0 and 1 with 1 for the maximum amount of reverb.]", 0.3333, 0, 1, 0.025); };
process = ["Wet" -> e6];
