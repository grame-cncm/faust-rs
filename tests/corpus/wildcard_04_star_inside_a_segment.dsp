// Wildcard modulation target `"*"` (faust-rs extension: the C++ compiler
// parses it as a label and matches nothing), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// "stage*" is a literal label, not a wildcard: no match, the reference's
// dangling input and FRS-EVAL-0008, not FRS-EVAL-0010. Input 1. Output 0.5.
process = ["stage*": (!, _) -> hslider("x", 0.5, 0, 1, 0.1)];
