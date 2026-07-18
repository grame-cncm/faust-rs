// P0 case: long constant delay (ring-buffer territory).
process = _ <: _, @(3000) :> _;
