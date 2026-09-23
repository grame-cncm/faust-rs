// A widget no signal reads leaves the interface, as in the reference compiler,
// whose interface is built while generating code: arity and interface frozen
// from faust 2.88.1 in crates/compiler/tests/dead_widgets.rs (2026-09-23).
// a slider in the dead branch of a `select2` with a constant selector; the other branch's slider stays.
process = _ : *(select2(0, hslider("kept", 0.5, 0, 1, 0.01), hslider("dropped", 0.25, 0, 1, 0.01)));
