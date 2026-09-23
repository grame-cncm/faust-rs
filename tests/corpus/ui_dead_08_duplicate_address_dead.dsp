// A widget no signal reads leaves the interface, as in the reference compiler,
// whose interface is built while generating code: arity and interface frozen
// from faust 2.88.1 in crates/compiler/tests/dead_widgets.rs (2026-09-23).
// two sliders at one address, one of them dead: no conflict, the reference
// checks the addresses of the widgets it shows.
process = _ : *(hslider("gain", 0.5, 0, 1, 0.01)) : *(1 + 0 * hslider("gain", 0.25, 0, 1, 0.01));
