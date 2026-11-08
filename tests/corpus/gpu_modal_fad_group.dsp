// The CPU baseline of the GPU modal fit (tests/impulse-tests,
// `make mojo-gpu-modal-fad`): four modes of gpu_modal_fad_voice.dsp in one
// program, each with its own controls (`freq0` ... `gain3`).
//
// On a CPU one mode per instance is bound by the latency of its recursion
// (a few dependent multiply-adds per sample); four independent modes in one
// sample loop fill the pipeline and halve the cost per mode. Measured with
// clang -O3 on an Apple M5: 9.1 ns per mode and sample with one mode per
// instance, 4.7 with four or eight, 4.9 with sixteen and 9.8 with
// thirty-two (likely spilling registers), one core, `plain` and `process` together.
//
// `process` outputs, for each mode k, y_k and its three tangents (16
// signals); `plain` (`-pn plain`) the sum of the four modes without tangents.

v = library("gpu_modal_fad_voice.dsp");
SR = min(192000.0, max(1.0, fconstant(int fSamplingFreq, <math.h>)));

freq(k) = hslider("freq%k", 440, 1, 24000, 0.0001);
t60(k) = hslider("t60%k", 1, 0.001, 60, 0.0001);
gain(k) = hslider("gain%k", 0.1, -10, 10, 0.000001);
y(k) = gain(k) * v.resonator(freq(k), t60(k));

plain = par(k, 4, y(k)) :> _;
process = par(k, 4, fad(y(k), (freq(k), t60(k), gain(k))));
