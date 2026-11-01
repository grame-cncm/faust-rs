// A read of a writable table in a rad lowered to a block sweep: the reverse
// sweep has no rule for it (DIFF-GAP-006). Refused in the lowering
// (FRS-SFIR-0004).
x = hslider("x", 0.3, -1.0, 1.0, 0.01);
process = rad(rwtable(8, 0.0, 0, x, 1)', x);
