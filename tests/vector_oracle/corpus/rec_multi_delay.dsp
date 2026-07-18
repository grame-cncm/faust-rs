// P0 case: one recursive value read at several delays (0, 2, 50).
// Exercises delay-plan geometry on a recursive carrier.
process = _ : (+ ~ *(0.9)) <: _, @(2), @(50) :> _;
