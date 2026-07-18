// P0 case: two independent recursion groups feeding one sum.
// Candidate pair for lockstep bundling (structurally distinct here: different
// coefficients only, same shape).
process = _ <: (+ ~ *(0.9)), (+ ~ *(0.5)) :> _;
