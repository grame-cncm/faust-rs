// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// the body defined in a `with`.
process = ["cut" -> lp] with { lp = _ : +~(_ : *(hslider("cut", 0.9, 0, 0.999, 0.001))); };
