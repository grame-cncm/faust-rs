// The rad twin of err_fad_dependent_seed.dsp: a seed computed from another
// seed of the same rad is refused (FRS-PROP-0005). No adjoint flows below a
// seed, so the gradient lanes of `x` and `y` would be 0 through `2 * (x + y)`.
process(x, y) = rad(2 * (x + y), (x + y, x));
