// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// `in/x`: the group name selects one of two widgets named `x`.
e2 = hgroup("top", (_ : *(vgroup("in", hslider("x", 0.5, 0, 1, 0.01)))), (_ : *(vgroup("out", hslider("x", 0.25, 0, 1, 0.01)))) :> _);
process = ["in/x" -> e2];
