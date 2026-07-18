// The recursive bank is lockstep-vectorized; the final reduction is not a lane.
body = + : *(0.9) : +(0.1) : *(0.8) : -(0.2) : *(0.7) : +(0.3)
     : *(0.6) : +(0.4) : *(0.5) : -(0.1) : *(0.4) : +(0.2);
cell(g) = body ~ *(g);
bank = cell(0.5), cell(0.5), cell(0.5), cell(0.5);
process = bank :> *(0.25);
