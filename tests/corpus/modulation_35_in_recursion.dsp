// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// a modulated block in a recursion: the extra input is the first, the one the recursion feeds.
// The block adds the slider: with `*(hslider(...))`, the recursion y = a*(x1+x2)*y'
// starts at 0 and stays 0, which C++ 2.90.4 folds to 0, dropping the slider.
r = + : +(hslider("a", 0.5, 0, 1, 0.01));
process = ["a" -> r] ~ _;
