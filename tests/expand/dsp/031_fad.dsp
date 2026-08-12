gain = hslider("g", 0.5, 0, 1, 0.01);
process = fad(_ * gain, gain);
