// controls.lib (faust-rs library, no C++ equivalent: it needs cinputs, cinput
// and the "*" modulation target), run and checked by
// crates/compiler/tests/controls_lib.rs. Requires -I libraries only: the
// library imports nothing.
// The ranges of the controls (0.2.0), on a frequency in hertz, a gain in
// [-1, 1] and a button, in cinputs order (f, g, t). Outputs: range 0 (f)
// 19980; ranges 19980, 2, 1; by_range with 2 s + 1 at k = 0.01: 400.6,
// 1.04, 1.02.
ct = library("controls.lib");
e = hslider("f", 440, 20, 20000, 1) * hslider("g", 0, -1, 1, 0.01) * button("t");
process = ct.range(0, e), ct.ranges(e), ct.by_range(\(s).(2 * s + 1), 0.01, e);
