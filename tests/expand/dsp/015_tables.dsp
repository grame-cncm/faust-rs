tbl = waveform{0.0, 1.0, 2.0, 3.0};
process = rdtable(tbl, int(_))
        , rwtable(16, 0.0, int(_), _, int(_));
