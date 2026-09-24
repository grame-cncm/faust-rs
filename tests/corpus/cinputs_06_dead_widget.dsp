// Control inputs as boxes (faust-rs extension, no C++ equivalent: the C++
// compiler stops on `undefined symbol : cinputs`), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// A widget the program no longer reads is still a control input: the list is
// taken on the box, before the interface prunes dead widgets. Output: 2.
e = hslider("dead", 0.5, 0, 1, 0.1) : !, hslider("live", 0.25, 0, 1, 0.1);
process = outputs(cinputs(e));
