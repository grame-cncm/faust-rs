// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// `a/a/x` and `a/x` under two nested groups named `a`.
e13 = hgroup("a", hgroup("a", _ : *(hslider("x", 0.5, 0, 1, 0.01))));
process = ["a/a/x" -> e13], ["a/x" -> e13];
