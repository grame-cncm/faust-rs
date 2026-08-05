// F11 — two reads of one generator tree: one table, one sub-container, one fill.
import("stdfaust.lib");
gen = sin(2.0 * ma.PI * float(ba.time) / 64.0);
process = rdtable(64, gen, int(ba.time % 64)),
          rdtable(64, gen, int((ba.time + 7) % 64));
