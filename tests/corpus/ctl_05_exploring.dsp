// controls.lib (faust-rs library, no C++ equivalent: it needs cinputs, cinput
// and the "*" modulation target), run and checked by
// crates/compiler/tests/controls_lib.rs. Requires -I libraries only: the
// library imports nothing.
// Exploring settings, on e = 10 a + b (a = 0.5, b = 0.25). Outputs:
// morph halfway to the preset (1, 0): a = 0.75, b = 0.125, 7.625;
// offset by a tenth of each range: a = 0.6, b = 0.35, 6.35;
// then randomize (amount 1, a draw every 4 samples) on a slider in [0, 1]
// step 0.1, a numeric entry in [0, 8] step 1 and a checkbox: their
// defaults 0.5, 3, 0 until the first draw (sample 3), then values on each
// grid, in each range.
ct = library("controls.lib");
e = hslider("a", 0.5, 0, 1, 0.1) * 10 + hslider("b", 0.25, 0, 1, 0.1);
r = hslider("x", 0.5, 0, 1, 0.1), nentry("n", 3, 0, 8, 1), checkbox("c");
trig = (+(1) ~ _) % 4 == 0;
process = ct.morph(0.5, (1, 0), e), ct.offset(0.1, e), ct.randomize(trig, 1, r);
