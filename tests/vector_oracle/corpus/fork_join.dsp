// P0 case: asymmetric fork/join expression graph (distinct -ss orders).
process = _ <: (*(0.5) : sin), (*(0.25) : cos : *(3.0)) :> _;
