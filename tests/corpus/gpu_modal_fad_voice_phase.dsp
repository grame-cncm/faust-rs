// GPU batch voice for modal sound matching against a recording: one mode of
// gpu_modal_fad_voice.dsp with a free phase, and its four tangents.
//
//   y[n] = r^n * (gain * sin(w n) + gainc * cos(w n))
//
// A struck object seen through a microphone does not start every mode at
// the same phase. Two amplitudes, of the sine and of the cosine of the same
// damped rotation, give each mode its own amplitude and phase while keeping
// both linear in the parameters: a (gain, phase) pair would have an
// undefined phase at zero gain and a tangent that vanishes there.
//
// `process` outputs y, dy/dfreq, dy/dt60, dy/dgain, dy/dgainc (the `fad`
// contract). The real-recording fit of tests/impulse-tests
// (`make mojo-gpu-modal-fad-bell`) runs one instance per mode; the number of
// modes and every mode's controls are the host's.

v = library("gpu_modal_fad_voice.dsp");

freq = hslider("freq", 440, 1, 24000, 0.0001);
t60 = hslider("t60", 1, 0.001, 60, 0.0001);
gain = hslider("gain", 0.1, -10, 10, 0.000001);
gainc = hslider("gainc", 0, -10, 10, 0.000001);

mode = v.rotor(freq, t60) : (*(gainc), *(gain)) :> _;
process = fad(mode, (freq, t60, gain, gainc));
