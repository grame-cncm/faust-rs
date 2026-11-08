// GPU batch voice for modal sound matching by forward-mode AD: one mode of
// an impact-excited modal model, with its three tangents.
//
// The mode is a damped rotation (the normalised second-order form of
// `fi.nlf2`, which `pm.modeFilter` uses so that the poles survive in f32),
// struck by a unit impulse at the first sample:
//
//   y[n] = gain * r^n * sin(w n),   w = 2 pi freq / SR,   r = 0.001^(1 / (t60 SR))
//
// `process` outputs y and its derivatives with respect to the voice's own
// controls, in the `fad` contract: y, dy/dfreq, dy/dt60, dy/dgain. The fit of
// tests/impulse-tests (`make mojo-gpu-modal-fad`) runs one instance per mode
// of the model: the number of modes and every mode's controls are set by the
// host at run time, not written here. The host owns the loss (the residual of
// the sum of all modes against a target) and the update; the chain rule to
// its own coordinates (log frequency, log decay) is the host's too.
//
// `plain` is the same mode without tangents (`-pn plain`), the cheaper
// forward pass a CPU fit uses to form the residual; `resonator` is shared
// with gpu_modal_fad_group.dsp, the grouped program of that CPU fit.
//
// Self-contained (no library). Per sample: one complex multiply for the
// primal, three for the tangents (augmented-state recursion).

SR = min(192000.0, max(1.0, fconstant(int fSamplingFreq, <math.h>)));
PI = 3.141592653589793;

freq = hslider("freq", 440, 1, 24000, 0.0001);
t60 = hslider("t60", 1, 0.001, 60, 0.0001);
gain = hslider("gain", 0.1, -10, 10, 0.000001);

// The mode, struck at the first sample: the state (u, v) = r^n (cos wn, sin wn).
resonator(freq, t60) = (rot ~ (_, _)) : (!, _)
with {
    w = 2 * PI * freq / SR;
    r = pow(0.001, 1 / (t60 * SR));
    imp = 1 - 1';
    rot(u, v) = r * (cos(w) * u - sin(w) * v) + imp, r * (sin(w) * u + cos(w) * v);
};

plain = gain * resonator(freq, t60);
process = fad(gain * resonator(freq, t60), (freq, t60, gain));
