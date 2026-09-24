// Wildcard modulation target `"*"` (faust-rs extension: the C++ compiler
// parses it as a label and matches nothing), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// A wildcard target matching no control input is an error, FRS-EVAL-0010
// (a literal target matching nothing is only the warning FRS-EVAL-0008).
process = ["amp/*": (!, _) -> hslider("x", 0, 0, 1, 0.1)];
