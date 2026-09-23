// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// vslider and nentry targets.
e10 = _ : *(vslider("v", 0.5, 0, 1, 0.01)) : *(nentry("n", 2, 0, 4, 1));
process = ["v", "n": + -> e10];
