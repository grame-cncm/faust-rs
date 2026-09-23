// A widget no signal reads leaves the interface, as in the reference compiler,
// whose interface is built while generating code: arity and interface frozen
// from faust 2.88.1 in crates/compiler/tests/dead_widgets.rs (2026-09-23).
// `attach` is what keeps a widget no output reads: the bargraph and the slider stay.
process = _ <: _, (abs : hbargraph("lev", 0, 1)) : attach <: _, hslider("kept_by_attach", 0.5, 0, 1, 0.01) : attach;
