// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// `a/x` and `b/x` skip intermediate groups; `h:a/v:c/x` matches nothing.
e3 = hgroup("a", vgroup("b", vgroup("c", _ : *(hslider("x", 0.5, 0, 1, 0.01)))));
process = ["a/x" -> e3], ["b/x" -> e3], ["h:a/v:c/x" -> e3];
