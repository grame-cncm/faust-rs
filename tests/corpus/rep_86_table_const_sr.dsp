// `--table-init const --table-init-sample-rate 48000` folds this table to
// literal 48000 Hz values. `--table-init runtime` instead uses the sample
// rate supplied to the generated DSP's init(sample_rate).
//
// Const mode intentionally rejects this program if the explicit compile-time
// sample-rate option is omitted.
sr = fconstant(int fSamplingFreq, <math.h>);
process = rdtable(16, sr, int(_));
