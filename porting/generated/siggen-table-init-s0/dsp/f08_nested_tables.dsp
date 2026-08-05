// F08 — nested generated tables. Upstream 2.87.1 declares but never fills the
// inner table; the current interpreter folds it correctly. Fill order matters.
import("stdfaust.lib");
inner = rdtable(64, 0.5 * float(ba.time), int(ba.time % 64));
process = rdtable(64, inner, int(os.phasor(64, 1)));
