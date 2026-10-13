// The definitions of mini.lib this voice needs, written here rather than
// imported: faustprobe compiles the `effect` below from a string
// (environment{...}, as poly-dsp.h does), where a file beside this one
// cannot be found.
SR = min(192000.0, max(1.0, fconstant(int fSamplingFreq, <math.h>)));
PI = 3.141592653589793;
phasor(f) = (+(f / SR) : \(x).(x - floor(x))) ~ _;
sawtooth(f) = 2.0 * phasor(f) - 1.0;
lowpass1(fc) = _ <: *(k / (1.0 + k)), (mem : *(k / (1.0 + k))) :> + ~ *(0.0 - (k - 1.0) / (1.0 + k))
with { k = tan(PI * fc / SR); };
asr(att, sus, rel, gate) = (\(e).(select2(on, max(0.0, e - sus / (rel * SR)), e + (sus - e) * c))) ~ _
with {
    on = gate > 0;
    c = 1.0 - exp(-1.0 / (att * SR));
};

freq = hslider("freq", 440, 20, 20000, 1);
gain = hslider("gain", 0.5, 0, 1, 0.01);
gate = button("gate");
bright = hslider("bright", 0.5, 0, 1, 0.01);
process = sawtooth(freq) * gain * asr(0.001, 0.8, 0.02, gate) : lowpass1(200 + bright * 8000);
effect = _ * hslider("wet", 0.5, 0, 1, 0.01);
