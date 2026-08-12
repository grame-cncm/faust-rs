import("stdfaust.lib");
declare name "LibImport";
declare author "GRAME";
freq = hslider("freq", 440, 20, 20000, 1);
process = os.osc(freq) <: _, _;
