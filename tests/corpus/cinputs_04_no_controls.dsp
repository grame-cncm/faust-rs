// Control inputs as boxes (faust-rs extension, no C++ equivalent: the C++
// compiler stops on `undefined symbol : cinputs`), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// A program without controls: cinputs is the empty box 0 : ! (no input, no
// output). Outputs: 0, 0, 0.
e = _ + 1;
process = outputs(cinputs(e)), inputs(cinputs(e)), outputs(coutputs(e));
