// Control inputs as boxes (faust-rs extension, no C++ equivalent: the C++
// compiler stops on `undefined symbol : cinputs`), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// cinput(i, e) is (widget, init, min, max, step); a button is (widget, 0, 0, 1, 1).
// Outputs: b = 0.5, 0.5, 0, 1, 0.1, then go = 0, 0, 0, 1, 1.
e = hslider("b", 0.5, 0, 1, 0.1) * 10 + hslider("a", 0.25, 0, 2, 0.1) * 100
  + hgroup("z", hslider("c", 3, 0, 10, 1)) * 1000 + hslider("[0]y", 0, 0, 1, 0.1)
  + hgroup("g", nentry("d", 1, 0, 4, 1)) * 10000 + button("go") * 100000
  : hbargraph("m", -1, 1);
process = cinput(2, e), cinput(4, e);
