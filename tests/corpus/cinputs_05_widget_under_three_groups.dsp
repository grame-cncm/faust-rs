// Control inputs as boxes (faust-rs extension, no C++ equivalent: the C++
// compiler stops on `undefined symbol : cinputs`), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// One widget box under three groups is three controls, as in the interface
// (Op 0/g, Op 1/g, Op 2/g, amp/vol), and "*" gives each its own input; a
// group prefix rebinds only its own.
// Outputs: 4, (10 + 20*2 + 30*3) * 40 = 5600, 0.5 + 7*2 + 0.5*3 = 16.
g = hslider("g", 0.5, 0, 1, 0.1);
e = par(i, 3, vgroup("Op %i", g * (i + 1))) :> _ : hgroup("amp", *(hslider("vol", 1, 0, 2, 0.1)));
process = outputs(cinputs(e)), (10, 20, 30, 40 : ["*": (!, _) -> e]), (7 : ["Op 1/*": (!, _) -> e]);
