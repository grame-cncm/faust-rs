// Zero over zero, and zero modulo zero: the simplifier used to absorb the first
// (`0 / x` is 0) and cancel the second (`x % x` is 0), and the program compiled
// to a silent 0 where the reference reports a division, then a remainder, by 0.
process = 0 / 0;
