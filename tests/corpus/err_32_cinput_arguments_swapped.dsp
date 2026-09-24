// Control inputs as boxes (faust-rs extension, no C++ equivalent: the C++
// compiler stops on `undefined symbol : cinputs`), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// cinput(e, i) for cinput(i, e): the index is a slider, not a compile-time
// integer, and the expression is a constant, FRS-EVAL-0009 naming the index
// and suggesting `cinput(0, freq1)`.
freq1 = hslider("title1", 0.6, 0, 10, 0.1);
freq2 = hslider("title2", 0.6, 0, 10, 0.1);
process = cinput(freq1, 0);
