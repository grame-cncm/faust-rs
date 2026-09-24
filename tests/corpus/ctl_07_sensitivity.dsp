// controls.lib (faust-rs library, no C++ equivalent: it needs cinputs, cinput
// and the "*" modulation target), run and checked by
// crates/compiler/tests/controls_lib.rs. Requires -I libraries only: the
// library imports nothing.
// Sensitivity of e(x) = (a x, 10 b x) on x = 1 (a = 2, b = 3). gradient_fad:
// each output then its derivatives, 2, 1, 0, 30, 0, 10. gradient_rad: the
// outputs, then the gradient of their sum, 2, 30, 1, 10.
ct = library("controls.lib");
e = _ <: *(hslider("a", 2, 0, 4, 0.1)), *(hslider("b", 3, 0, 4, 0.1) * 10);
process = (1 : ct.gradient_fad(e)), (1 : ct.gradient_rad(e));
