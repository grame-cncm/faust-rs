// Only the four recursive branches form a lockstep bundle; `side` stays outside.
body = + : *(0.9) : +(0.1) : *(0.8) : -(0.2) : *(0.7) : +(0.3)
     : *(0.6) : +(0.4) : *(0.5) : -(0.1) : *(0.4) : +(0.2);
cell(g) = body ~ *(g);
side = *(0.25) : +(0.5);
process = cell(0.5), cell(0.5), cell(0.5), cell(0.5), side;
