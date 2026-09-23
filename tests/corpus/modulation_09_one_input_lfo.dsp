// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// a one-input modulator carrying state, an oscillator.
lfo(f, g) = 1 + g * sin(2 * 3.141592653589793 * phase) with { phase = (+(f / 44100.0) ~ (_ <: _ - floor)); };
e = hgroup("g", _ : *(hslider("a", 0.5, 0, 1, 0.01)) : +(hslider("b", 0.1, 0, 1, 0.01)));
process = ["a": *(lfo(3, 0.5)) -> e];
