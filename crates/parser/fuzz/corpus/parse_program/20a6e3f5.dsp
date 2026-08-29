// Variable delay whose amount is a slider shifted into negative territory.
// hslider("Delay1",10,0,100,1) - 100 has interval [-100,0] (hi=0).
// The delay line is sized to next_power_of_two(1)=1 (zero-delay passthrough),
// matching C++ checkDelayInterval which accepts hi>=0 (not strictly positive).
process = @(100) : @(hslider("Delay1",10,0,100,1) - 100);
