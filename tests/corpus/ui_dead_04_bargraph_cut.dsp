// A widget no signal reads leaves the interface, as in the reference compiler,
// whose interface is built while generating code: arity and interface frozen
// from faust 2.88.1 in crates/compiler/tests/dead_widgets.rs (2026-09-23).
// a bargraph whose output is cut.
process = _ <: _, (abs : hbargraph("lev", 0, 1) : !);
