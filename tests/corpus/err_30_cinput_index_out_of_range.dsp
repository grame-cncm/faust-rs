// Control inputs as boxes (faust-rs extension, no C++ equivalent: the C++
// compiler stops on `undefined symbol : cinputs`), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// cinput(i, e) with i at the count: FRS-EVAL-0009.
e = hslider("a", 0.5, 0, 1, 0.1) + hslider("b", 0.5, 0, 1, 0.1);
process = cinput(2, e);
