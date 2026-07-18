// P0 case: one shared sample expression consumed by two different consumers.
// Exercises multi-occurrence separation (C++ needSeparateLoop priority 5).
process = _ : *(0.5) <: +(1.0), *(2.0);
