// Widget modulation, one rule of the manual per fixture, the expected arity and
// interface frozen from the reference compiler 2.88.1 in
// crates/compiler/tests/modulation_corpus.rs (samples equal to the bit, 2026-09-23).
// buttons and checkboxes are targets.
e8 = _ : *(button("go")) : *(checkbox("on"));
process = ["go", "on" -> e8];
