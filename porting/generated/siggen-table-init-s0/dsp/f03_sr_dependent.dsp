// F03 — sample-rate-dependent table content: the constant must be computed in
// the sub-container's instanceInit, which is why folding cannot express it.
import("stdfaust.lib");
process = rdtable(1024, exp(0.0 - float(ba.time) / ma.SR), int(os.phasor(1024, 2)));
