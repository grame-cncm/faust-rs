gain = hslider("gain", 0.5, 0, 1, 0.01);
gate = button("gate");
freq = hslider("freq", 440, 20, 20000, 1);
process = (gate * gain) : + ~ *(hslider("fb", 0.5, 0, 4, 0.01));
