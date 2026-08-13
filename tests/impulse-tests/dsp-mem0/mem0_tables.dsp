size = 32;
generator = +(1) ~ _;
index = (+(1) ~ _) % size;
process = rdtable(size, generator, index) + (waveform{0.25, 0.5, 0.75, 1.0}, index % 4 : rdtable);
