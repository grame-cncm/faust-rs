// GPU batch voice: one partial of an additive bank, a sine oscillator with its
// own frequency and gain. Thousands of instances form the bank, summed on the
// device (porting/faust-gpu-audio-synthesis-2026-11-07-en.md §5, additive
// synthesis). Self-contained (no library); written for 48 kHz.
//
// The program describes one instance; the number of instances is not in it.
// It is chosen by the host at run time: the `<voices>` argument of the bench
// binary (swept by `BENCH_VOICES`, 64 to 65,536), passed to the struct as
// `mydsp(voices, state)` and launched as one GPU thread per instance. The
// bench leaves every instance at its default controls (the cost does not
// depend on their values); an instrument sets each instance's controls with
// `set_control(voice, index, value)`.
//
//   make -C tests/impulse-tests mojo-gpu-bench-batch BENCH_DSPDIR=../corpus \
//     BENCH_PROGRAMS=gpu_partial_sine BENCH_NAME=additive

freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.01, 0, 1, 0.0001);
wrap(x) = x - floor(x);
phase = (freq / 48000.0) : (+ : wrap) ~ _;
process = sin(phase * 6.2831853) * gain;
