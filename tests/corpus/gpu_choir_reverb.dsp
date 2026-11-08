// The hall of the GPU choir (tests/impulse-tests, `make mojo-gpu-choir`): the
// `re.zita_rev1_stereo` of reverbs.lib, a church-like decay, mixed with the
// dry signal. One instance on the stereo mix, so it runs on the CPU: the GPU
// computes the tens of thousands of partials and sums them, and this single
// program, which a GPU would run 30 to 100 times slower than one CPU core,
// takes the two channels that come back.

import("stdfaust.lib");

// A large, wet hall by default (a cathedral-like 5 s). At steady state
// `zita_rev1` returns about 0.29 of its input on each channel (-11 dB), so a
// wet gain of 2 puts the hall about 3 dB above the dry choir at 0.4. The host
// can set `dry`, `wet` and `t60` (the harness: `play ... reverb wet=3 t60=7`).
dry = hslider("dry", 0.4, 0, 2, 0.01);
wet = hslider("wet", 2.0, 0, 4, 0.01);
t60 = hslider("t60", 5.0, 1, 10, 0.1);

process = _, _ <: (*(dry), *(dry)), (re.zita_rev1_stereo(60, 200, 6000, t60 * 1.3, t60, 48000) : *(wet), *(wet))
    :> _, _;
