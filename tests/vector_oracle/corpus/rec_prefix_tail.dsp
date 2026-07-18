// P0 case: pure prefix (scaling) -> recursive core -> pure tail (sin).
// Exercises prefix/tail fission around one serial recursion.
process = _ : *(0.5) : (+ ~ *(0.9)) : *(2.0) : sin;
