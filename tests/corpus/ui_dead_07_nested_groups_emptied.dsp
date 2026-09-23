// A widget no signal reads leaves the interface, as in the reference compiler,
// whose interface is built while generating code: arity and interface frozen
// from faust 2.88.1 in crates/compiler/tests/dead_widgets.rs (2026-09-23).
// nested groups emptied from the inside, a live sibling at the top.
process = _ : *(hslider("top", 0.5, 0, 1, 0.01)) : *(1 + 0 * hgroup("a", vgroup("b", tgroup("c", hslider("deep", 0.5, 0, 1, 0.01)))));
