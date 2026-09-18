import("stdfaust.lib");
cutoff = hslider("cutoff [unit:Hz]", 1000, 20, 20000, 1);
q = hslider("q", 1, 0.5, 20, 0.01);
level = _ <: attach(_, abs : ba.slidingMax(64, 64) : hbargraph("level", 0, 1));
process = _ <: (fi.resonlp(cutoff, q, 1) : level), (fi.highpass(1, cutoff));
