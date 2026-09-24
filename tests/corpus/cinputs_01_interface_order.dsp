// Control inputs as boxes (faust-rs extension, no C++ equivalent: the C++
// compiler stops on `undefined symbol : cinputs`), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// Six controls declared out of interface order (two groups, a [0] ordering
// prefix, a numentry, a button) and a bargraph. The list follows the
// interface: y, a, b, g/d, go, z/c.
// Outputs: 6 controls, 1 bargraph, then the six defaults in that order:
// 6, 1, 0, 0.25, 0.5, 1, 0, 3.
e = hslider("b", 0.5, 0, 1, 0.1) * 10 + hslider("a", 0.25, 0, 2, 0.1) * 100
  + hgroup("z", hslider("c", 3, 0, 10, 1)) * 1000 + hslider("[0]y", 0, 0, 1, 0.1)
  + hgroup("g", nentry("d", 1, 0, 4, 1)) * 10000 + button("go") * 100000
  : hbargraph("m", -1, 1);
process = outputs(cinputs(e)), outputs(coutputs(e)),
          par(i, outputs(cinputs(e)), cinput(i, e) : !, _, !, !, !);
