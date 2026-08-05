// F12 — two distinct generators: deterministic SIG0/SIG1 and tbl0/tbl1 numbering.
import("stdfaust.lib");
process = rdtable(64, sin(2.0 * ma.PI * float(ba.time) / 64.0), int(ba.time % 64)),
          rdtable(32, cos(2.0 * ma.PI * float(ba.time) / 32.0), int(ba.time % 32));
