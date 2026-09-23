// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// a modulated block in a recursion: the extra input is the first, the one the recursion feeds.
r = + : *(hslider("a", 0.5, 0, 1, 0.01));
process = ["a" -> r] ~ _;
