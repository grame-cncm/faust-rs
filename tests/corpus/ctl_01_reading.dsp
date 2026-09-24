// controls.lib (faust-rs library, no C++ equivalent: it needs cinputs, cinput
// and the "*" modulation target), run and checked by
// crates/compiler/tests/controls_lib.rs. Requires -I libraries only: the
// library imports nothing.
// Reading the controls. Outputs: count 2, widget 1 (b) 0.25, init 0.5, lo 0,
// hi 1, step 0.1 (of a), then inits 0.5, 0.25.
ct = library("controls.lib");
e = hslider("a", 0.5, 0, 1, 0.1) * 10 + hslider("b", 0.25, 0, 1, 0.1);
process = ct.count(e), ct.widget(1, e), ct.init(0, e), ct.lo(0, e), ct.hi(0, e), ct.step(0, e), ct.inits(e);
