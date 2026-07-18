// P0 case: ondemand clock-domain wrapper (research branch construct).
// The plan records that this C++ branch rejects -vec with ondemand; the
// vector captures for this case are expected to be error artifacts.
process = _, _ : ondemand(*(0.5));
