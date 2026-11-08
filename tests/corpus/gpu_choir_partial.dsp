// GPU batch voice: one partial of an additive choir, a "rich" partial in the
// sense of sinusoids-plus-noise models: a sine with its own vibrato, a slow
// amplitude swell (breath), and a band of noise centred on the partial,
// filtered by a TPT state-variable band-pass that follows the vibrato. Each
// partial is a small signal processor of its own, which an inverse-FFT
// additive resynthesis cannot express: a noise band whose centre moves with a
// per-sample vibrato, a resonance per partial.
//
// The choir of tests/impulse-tests (`make mojo-gpu-choir`) runs one instance
// per partial: S singers times P partials (harmonics of the singer's
// fundamental), every instance with its own controls, set by the host at run
// time; the number of instances is not in this program. At a chord change
// `gain` glides (80 ms), so that no partial clicks, and `freq` jumps with a
// continuous phase: a one-pole glide of `freq` in f32 stops short of its
// target by up to an ulp over its step (1 - c), 2.3e-4 relative here, about
// 0.4 cent, at a point that differs between a device that fuses the
// multiply-add and one that does not; the partial's phase then drifts apart
// by about a radian per second. A singer's partials share its vibrato rate
// and phase (`vrate`, `phase`), so its harmonics move together, and its
// breath; `seed` gives every instance its own noise.
//
// Self-contained (no library). Per sample: three sines (vibrato, swell,
// partial), one tangent (the filter follows the vibrated frequency), a linear
// congruential noise and the filter.

SR = min(192000.0, max(1.0, fconstant(int fSamplingFreq, <math.h>)));
PI = 3.141592653589793;

freq = hslider("freq", 440, 1, 20000, 0.001);
gain = hslider("gain", 0.01, 0, 1, 0.0001);
vrate = hslider("vrate", 5.5, 0, 20, 0.001);
vdepth = hslider("vdepth", 0.006, 0, 0.1, 0.0001);
brate = hslider("brate", 0.15, 0, 20, 0.001);
noisiness = hslider("noise", 0.2, 0, 1, 0.001);
bw = hslider("bw", 0.02, 0.001, 1, 0.0001);
phase = hslider("phase", 0, 0, 1, 0.0001);
seed = hslider("seed", 0, 0, 1000000, 1);

first = 1 - 1';
wrap(x) = x - floor(x);
// One-pole glide of time constant t, starting on its target (no sweep from 0
// at the first sample).
glide(t) = *(1 - c) : + ~ *(c) with { c = exp(-1 / (t * SR)) * (1 - first); };
gaing = gain : glide(0.08);
phasor(f) = (+(f / SR) : wrap) ~ _;
lfo(f, p) = sin(2 * PI * wrap(phasor(f) + p));

// The vibrated frequency, kept under 0.45 SR for the filter.
fv = min(freq * (1 + vdepth * lfo(vrate, phase)), 0.45 * SR);
partial = sin(2 * PI * phasor(fv));
swell = 0.6 + 0.4 * lfo(brate, wrap(phase * 0.7 + 0.25));

// Per-instance white noise in [-1, 1]: the seed enters once, at the first
// sample, so that instances with different seeds draw different sequences.
lcg = (+(12345) : +(int(seed) * first)) ~ *(1103515245);
noise = float(lcg) / 2147483647.0;

// TPT state-variable band-pass (Zavalishin), unity peak gain, centre fv,
// relative bandwidth bw (k = 1/Q = bw); stable for any centre under SR/2.
bandpass(x) = (step ~ (_, _)) : (!, !, _)
with {
    g = tan(PI * fv / SR);
    k = bw;
    a1 = 1 / (1 + g * (g + k));
    a2 = g * a1;
    a3 = g * a2;
    step(s1, s2) = 2 * v1 - s1, 2 * v2 - s2, k * v1
    with {
        v3 = x - s2;
        v1 = a1 * s1 + a2 * v3;
        v2 = s2 + a2 * s1 + a3 * v3;
    };
};

// The band's RMS brought to a sine's (equivalent noise bandwidth of the
// band-pass: pi / 2 * bw * freq; uniform noise has variance 1/3).
nscale = sqrt(1.5 * SR / (PI * bw * max(freq, 1)));

process = gaing * swell * ((1 - noisiness) * partial + noisiness * nscale * bandpass(noise));
