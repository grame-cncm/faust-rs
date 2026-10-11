import("expand_test_library.lib");
declare name "LibImport";
declare author "GRAME";
lib = library("expand_test_library.lib");
freq = hslider("freq", 440, 20, 20000, 1);
process = lib.expansion_test_osc(freq) <: _, _;
