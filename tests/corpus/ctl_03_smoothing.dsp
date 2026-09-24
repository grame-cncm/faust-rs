// controls.lib (faust-rs library, no C++ equivalent: it needs cinputs, cinput
// and the "*" modulation target), run and checked by
// crates/compiler/tests/controls_lib.rs. Requires -I libraries only: the
// library imports nothing.
// Smoothing. smooth starts every control at its default, so e = 10 a + b
// reads 5.25 from the first sample; smoother(t) from 0 towards a constant 1
// is 1 - p^(n + 1), p = exp(-1 / (t SR)): at 48 kHz and t = 1 ms,
// p = exp(-1 / 48).
ct = library("controls.lib");
e = hslider("a", 0.5, 0, 1, 0.1) * 10 + hslider("b", 0.25, 0, 1, 0.1);
process = ct.smooth(0.001, e), (1 : ct.smoother(0.001));
