// F04 — writable table seeded at instanceConstants time, then written per sample.
import("stdfaust.lib");
N = 4096;
process = rwtable(N, sin(2.0 * ma.PI * float(ba.time) / float(N)),
                  int(os.phasor(N, 1)), _, int(os.phasor(N, 1)));
