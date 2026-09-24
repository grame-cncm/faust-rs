// controls.lib (faust-rs library, no C++ equivalent: it needs cinputs, cinput
// and the "*" modulation target), run and checked by
// crates/compiler/tests/controls_lib.rs. Requires -I libraries only: the
// library imports nothing.
// Rebuilding the interface: knobs(e) shows P0, P1 as knobs with the default,
// range and step of a and b, relabel(entry, e) shows p0, p1 as numeric
// entries in a group; both compute e = 10 a + b = 5.25 on the defaults.
ct = library("controls.lib");
e = hslider("a", 0.5, 0, 1, 0.1) * 10 + hslider("b", 0.25, 0, 1, 0.1);
entry(i, v, a, b, s) = vgroup("params", nentry("p%i", v, a, b, s));
process = hgroup("k", ct.knobs(e)), hgroup("r", ct.relabel(entry, e));
