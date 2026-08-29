// rad with a seed that does not appear in the differentiated expression.
// Expected behaviour: gradient lane for `y` is exactly zero — phase-1 RAD
// must not fabricate a non-zero adjoint when the seed is unreachable from
// any primal output.
x = hslider("x", 0.3, -1.0, 1.0, 0.01);
y = hslider("y", 0.7, -1.0, 1.0, 0.01);
process = rad(sin(x), y);
