// P0 case: read/write table (mutable shared resource; effect ordering).
process = rwtable(100, 0.0, int(_ * 99.0), _, int(_ * 99.0));
