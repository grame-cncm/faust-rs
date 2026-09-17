// The divisor is an argument that is 0 at this call, as `householder_coef(N, i, j)`
// of faust-diff-jot was when it received its arguments in the wrong order: the
// case that revealed the panic. The division is inside a function, and one of
// the two `par` branches is sound.
coef(n, i, j) = float(i == j) - 2.0 / n;
process = par(i, 2, _ * coef(i, 1, 1));
