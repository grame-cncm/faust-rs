// F09 — foreign call inside the fill loop.
import("stdfaust.lib");
myf = ffunction(float myfun(float), <math.h>, "");
process = rdtable(64, myf(float(ba.time)), int(os.phasor(64, 1)));
