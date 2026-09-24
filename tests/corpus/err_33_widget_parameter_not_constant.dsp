// A widget parameter that is not a compile-time number (FRS-EVAL-0011).
// The lambda applied by `:` binds `x` to an input signal, so the slider's init
// is known only at run time. The C++ reference stops with `ERROR : the
// parameter must be a real constant numerical expression : SigInput[10001]`;
// faust-rs compiled it with an init of 0 until 2026-09-24. Checked by
// crates/compiler/tests/diagnostic_errors.rs.
process = 0.5 : \(x).(hslider("a", x, 0, 1, 0.1));
