// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// `%i` in the target, interpolated as in a widget label.
ei(i) = _ : *(hslider("a%i", 0.5, 0, 1, 0.01));
process = par(i, 2, ["a%i" -> ei(i)]);
