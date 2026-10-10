// A recursion whose numeric-pattern argument is a UI control
// (FRS-EVAL-0012, grame-cncm/faust-rs#21). `bp` has the shape of the
// `bpbsr` helper of fi.bandpass: base cases 0 and 1 for the order, step 2.
// No numeric rule can match a slider, so the general rule recurses until a
// depth budget stops it. The C++ reference reports `ERROR : stack overflow in
// eval` (2.84.3) or runs until its timeout; faust-rs names the argument.
// Checked by crates/compiler/tests/diagnostic_errors.rs.
bp(s, 0, nh) = _;
bp(s, 1, nh) = *(0.5);
bp(s, o, nh) = bp(s, o-2, nh) : *(0.25);
process = bp(0, hslider("order", 4, 1, 8, 1), 4);
