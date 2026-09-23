// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// the target's signal is smoothed after the widget: the modulation sits before the smoothing.
smoo = *(0.001) : +~*(0.999);
e11 = _ : *(hslider("a", 0.5, 0, 1, 0.01) : smoo);
process = ["a" -> e11];
