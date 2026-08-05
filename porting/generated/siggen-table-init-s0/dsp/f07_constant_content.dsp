// F07 — constant payload. Guards the 127x folding blow-up: upstream emits
// table[i] = 0.5f in a loop, folding emits 65536 identical literals.
import("stdfaust.lib");
process = rdtable(65536, 0.5, int(ba.time % 65536));
