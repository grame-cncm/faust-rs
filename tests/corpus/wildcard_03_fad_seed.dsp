// Wildcard modulation target `"*"` (faust-rs extension: the C++ compiler
// parses it as a label and matches nothing), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// A fad seed that is a widget of e is the same control as the body's use of
// it: both are rebound to the one extra input. Inputs g = 3, x = 5.
// Outputs: primal x*g = 15, tangent d/dg = x = 5.
g = hslider("g", 2, 0, 4, 0.1);
e = fad(_ * g, g);
process = ["*": (!, _) -> e];
