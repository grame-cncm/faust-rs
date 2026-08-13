gain = hslider("gain", 0.75, 0, 1, 0.01);
process = *(gain) : + ~ *(0.25);
