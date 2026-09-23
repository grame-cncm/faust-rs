// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// `(!, _)`: the external signal replaces the widget, whose signal reaches nothing.
e = hgroup("g", _ : *(hslider("a", 0.5, 0, 1, 0.01)) : +(hslider("b", 0.1, 0, 1, 0.01)));
process = ["a": (!, _) -> e];
