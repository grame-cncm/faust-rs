// An evaluation cycle through a function argument (FRS-EVAL-0013,
// grame-cncm/faust-rs#22): `cut` passes `process` to `fade`, `process` uses
// `effect` and `cut`, `effect` uses `cut`. The C++ reference reports `ERROR :
// after <n> evaluation steps, the compiler has detected an endless evaluation
// cycle of <k> steps`; faust-rs names the definitions of the cycle.
// Checked by crates/compiler/tests/diagnostic_errors.rs.
fade(g) = *(g);
cut = hslider("Cut", 0, 0, 1, 0.01) : fade(process);
effect = _ * (1 - cut);
process = effect, cut;
