import("stdfaust.lib");
process = fi.resonlp(hslider("cutoff", 1000, 20, 20000, 1), hslider("q", 50, 0.5, 200, 0.01), 1);
