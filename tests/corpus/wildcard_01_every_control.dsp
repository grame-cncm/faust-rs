// Wildcard modulation target `"*"` (faust-rs extension: the C++ compiler
// parses it as a label and matches nothing), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// "*" with a two-input modulator: one input per control, in cinputs order
// (y, a, b, g/d, go, z/c), equal to one literal target per control listed in
// that order. Inputs 1, 2, 3, 4, 5, 6. Outputs: 1 + 2*100 + 3*10 + 4*10000
// + 5*100000 + 6*1000 = 546231, then its difference with the literal form: 0.
e = hslider("b", 0.5, 0, 1, 0.1) * 10 + hslider("a", 0.25, 0, 2, 0.1) * 100
  + hgroup("z", hslider("c", 3, 0, 10, 1)) * 1000 + hslider("[0]y", 0, 0, 1, 0.1)
  + hgroup("g", nentry("d", 1, 0, 4, 1)) * 10000 + button("go") * 100000
  : hbargraph("m", -1, 1);
process = si_split6 <: ["*": (!, _) -> e],
          ["y": (!, _), "a": (!, _), "b": (!, _), "g/d": (!, _), "go": (!, _), "z/c": (!, _) -> e]
        : \(w, l).(w, w - l)
with { si_split6 = _, _, _, _, _, _; };
