// controls.lib (faust-rs library, no C++ equivalent: it needs cinputs, cinput
// and the "*" modulation target), run and checked by
// crates/compiler/tests/controls_lib.rs. Requires -I libraries only: the
// library imports nothing.
// Rebinding the controls, on e = 10 a + b (a = 0.5, b = 0.25 in [0, 1]).
// Inputs: external (a, b), normalized (a, b), cv (a, b), six in all.
// With inputs 0.3, 0.4, 0.2, 0.5, 0.1, -0.1 the outputs are:
// map (a, 2 b): 5.5; external: 3.4; normalized: 2.5; cv (depth 1): 6.15.
ct = library("controls.lib");
e = hslider("a", 0.5, 0, 1, 0.1) * 10 + hslider("b", 0.25, 0, 1, 0.1);
f(i, w) = w * (i + 1);
process = ct.map(f, e), ct.external(e), ct.normalized(e), ct.cv(1, e);
