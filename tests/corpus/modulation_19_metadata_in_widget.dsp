// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// metadata on the widget's label is ignored by the match.
e5 = _ : *(hslider("a[style:knob]", 0.5, 0, 1, 0.01));
process = ["a" -> e5];
