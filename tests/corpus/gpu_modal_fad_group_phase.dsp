// The CPU baseline of the GPU modal fit against a recording
// (tests/impulse-tests, `make mojo-gpu-modal-fad-bell`): four modes of
// gpu_modal_fad_voice_phase.dsp in one program, each with its own controls
// (`freq0` ... `gainc3`). Four independent recursions in one sample loop
// keep a CPU core busy where one mode per instance waits on its recursion
// (gpu_modal_fad_group.dsp measures the groupings).
//
// `process` outputs, for each mode k, y_k and its four tangents (20 signals);
// `plain` (`-pn plain`) the sum of the four modes without tangents.

v = library("gpu_modal_fad_voice.dsp");

freq(k) = hslider("freq%k", 440, 1, 24000, 0.0001);
t60(k) = hslider("t60%k", 1, 0.001, 60, 0.0001);
gain(k) = hslider("gain%k", 0.1, -10, 10, 0.000001);
gainc(k) = hslider("gainc%k", 0, -10, 10, 0.000001);
y(k) = v.rotor(freq(k), t60(k)) : (*(gainc(k)), *(gain(k))) :> _;

plain = par(k, 4, y(k)) :> _;
process = par(k, 4, fad(y(k), (freq(k), t60(k), gain(k), gainc(k))));
