// A widget no signal reads leaves the interface, as in the reference compiler,
// whose interface is built while generating code: arity and interface frozen
// from faust 2.88.1 in crates/compiler/tests/dead_widgets.rs (2026-09-23).
// a group whose every widget is dead leaves with them; the live group stays.
process = _ : *(hgroup("live", hslider("g", 0.5, 0, 1, 0.01))) : *(1 + 0 * vgroup("dead", hslider("d", 0.5, 0, 1, 0.01) + hslider("e", 0.5, 0, 1, 0.01)));
