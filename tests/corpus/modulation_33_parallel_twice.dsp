// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// two modulations of the same expression in parallel: two independent inputs.
e = hgroup("g", _ : *(hslider("a", 0.5, 0, 1, 0.01)) : +(hslider("b", 0.1, 0, 1, 0.01)));
process = ["a" -> e], ["a" -> e];
