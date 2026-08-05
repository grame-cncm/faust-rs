// F10 — integer-typed generator: itbl element type and int* fill signature.
import("stdfaust.lib");
process = rdtable(64, int(ba.time * 2), int(ba.time % 64));
