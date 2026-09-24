// Wildcard modulation target `"*"` (faust-rs extension: the C++ compiler
// parses it as a label and matches nothing), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// A one-input modulator transforms every matched control; bargraphs are
// never matched. Outputs: 2*0.5 + 2*0.25 + 1 = 2.5, 0.5 + (0.25 - 1) + 1 = 0.75.
e = hslider("a", 0.5, 0, 1, 0.1) + hgroup("g", hslider("b", 0.25, 0, 1, 0.1)) + hbargraph("m", 0, 1)(1);
process = ["*": *(2) -> e], ["g/*": -(1) -> e];
