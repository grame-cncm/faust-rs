// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// one label matching two widgets: one shared extra input.
e2 = hgroup("top", (_ : *(vgroup("in", hslider("x", 0.5, 0, 1, 0.01)))), (_ : *(vgroup("out", hslider("x", 0.25, 0, 1, 0.01)))) :> _);
process = ["x" -> e2];
