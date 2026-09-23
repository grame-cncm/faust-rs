// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// the same widget box used twice is one control, one extra input.
w = hslider("a", 0.5, 0, 1, 0.01);
e4 = _ : *(w) : +(w);
process = ["a" -> e4];
