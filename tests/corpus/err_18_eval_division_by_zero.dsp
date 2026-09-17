// A constant division by a constant zero, reals: the reference does not fold
// it to an infinity (`ERROR : division by 0 in 2 / 0`), and neither does
// faust-rs. It used to panic the compiler.
process = 2.0 / 0;
