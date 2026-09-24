// Control inputs as boxes (faust-rs extension, no C++ equivalent: the C++
// compiler stops on `undefined symbol : cinputs`), run and checked by
// crates/compiler/tests/control_inputs.rs. See docs/control-inputs-en.md.
// cinputs(f) as the seeds of fad: the same lanes as the widgets written out.
// Input x = 0.75. Outputs: primal x*a*b + a = 1.25, d/da = x*b + 1 = 2.5,
// d/db = x*a = 0.375, then the three differences with fad(f, (a, b)): 0, 0, 0.
a = hslider("a", 0.5, 0, 1, 0.1);
b = hslider("b", 2, 0, 4, 0.1);
f = _ * a * b + a;
process = _ <: fad(f, cinputs(f)), fad(f, (a, b)) : ro_diff
with { ro_diff(p, s, t, p2, s2, t2) = p, s, t, p - p2, s - s2, t - t2; };
