// F13 — int and real generated tables in one program. Sole direct evidence for
// plan §5.6 rule 1: the tbl counter is shared across types (itbl0, ftbl1, itbl2),
// the i/f letter being a prefix rather than part of the counter key.
import("stdfaust.lib");
process = rdtable(64, int(ba.time * 2), int(ba.time % 64)),
          rdtable(32, sin(float(ba.time)), int(ba.time % 32)),
          rdtable(16, int(ba.time * 3), int(ba.time % 16));
