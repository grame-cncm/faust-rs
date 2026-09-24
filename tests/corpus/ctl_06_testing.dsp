// controls.lib (faust-rs library, no C++ equivalent: it needs cinputs, cinput
// and the "*" modulation target), run and checked by
// crates/compiler/tests/controls_lib.rs. Requires -I libraries only: the
// library imports nothing.
// Testing. sweep(100, e) moves a and b over [0, 1], b half a period behind
// a: e = 10 a + b goes from 1 (a = 0, b = 1) to 10 (a = 1, b = 0) and back.
// sweep_one(0, 100, e) moves a alone, b on its knob 0.25. Then the number
// of non-finite values of 1 / d as d sweeps [-1, 1] in 100 samples: 1 on
// the samples where d is 0 (25 and 75 of each period), 0 elsewhere.
ct = library("controls.lib");
e = hslider("a", 0.5, 0, 1, 0.1) * 10 + hslider("b", 0.25, 0, 1, 0.1);
inv = 1 / hslider("d", 0.5, -1, 1, 0.01);
process = ct.sweep(100, e), ct.sweep_one(0, 100, e), (ct.sweep(100, inv) : ct.nonfinite(1));
