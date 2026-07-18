// P0 case: a slow (control-rate) value used with a delay.
// Block-rate + maxDelay > 0 must still separate a loop (priority 1 over 2).
s = hslider("s", 0.5, 0, 1, 0.01);
process = _ * (s : @(100));
