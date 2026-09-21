// A seed computed from another seed of the same fad is refused
// (FRS-PROP-0005). A seed is differentiated as an independent variable,
// so the lanes of `x` and `y` would be 0 wherever the body reads `x + y`:
// the program would return `1, 0, 0` where `1, 1, 1` was meant. The two
// spellings that mean something are fad(x + y, (x, y)) and fad(x + y, x + y).
process(x, y) = fad(x + y, (x + y, x, y));
