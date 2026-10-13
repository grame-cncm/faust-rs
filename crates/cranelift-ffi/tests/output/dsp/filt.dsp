import("mini.lib");
cutoff = hslider("cutoff [unit:Hz]", 1000, 20, 20000, 1);
q = hslider("q", 1, 0.5, 20, 0.01);
level = _ <: attach(_, peak : hbargraph("level", 0, 1));
process = _ <: (resonlp(cutoff, q, 1) : level), highpass1(cutoff);
