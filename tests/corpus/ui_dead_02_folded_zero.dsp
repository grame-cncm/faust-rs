// A widget no signal reads leaves the interface, as in the reference compiler,
// whose interface is built while generating code: arity and interface frozen
// from faust 2.88.1 in crates/compiler/tests/dead_widgets.rs (2026-09-23).
// a slider absorbed by a folded zero, `0 * slider + 1`.
process = _ : *(0 * hslider("zeroed", 0.5, 0, 1, 0.01) + 1);
