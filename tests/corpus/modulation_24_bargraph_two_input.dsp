// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// a bargraph under the default `*`, one extra input.
e7 = _ <: _, (abs : hbargraph("lev", 0, 1)) : +;
process = ["lev" -> e7];
