// P0 case: a verySimple sample expression used both directly and delayed.
// Exercises the precedence trap: maxDelay > 0 dominates verySimple
// (C++ needSeparateLoop priority 1 over priority 2).
process = _ : *(0.5) <: _, @(3) :> _;
