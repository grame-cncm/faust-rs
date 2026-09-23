// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// a widget whose label opens a group, `h:sub/x`: `x` and `sub/x` match, `h:sub/x` does not.
e9 = hgroup("g", _ : *(hslider("h:sub/x", 0.5, 0, 1, 0.01)));
process = ["sub/x" -> e9], ["x" -> e9], ["h:sub/x" -> e9];
