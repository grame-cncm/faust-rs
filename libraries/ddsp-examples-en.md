# Twelve DDSP examples with `fad` and `rad`

Twelve complete differentiable-DSP programs, each one a task an audio engineer
recognises, written with the two automatic-differentiation primitives of
`faust-rs` and the loops of [optimizers.lib](optimizers.lib). Three use
`fad`, forward mode, where the exact derivative through a recursion is what
makes the method work; three use `rad`, reverse mode, where one scalar loss
depends on many parameters or where the gradient leaves the graph for a host.
Three more, at the end, are the state of the art of their fields: a diode
clipper whose components are learned through its implicit solver, an FDN
reverb calibrated to a target decay, a recurrent neural amplifier trained by
truncated backpropagation through time. The last two are the fragile ones,
kept because of what they teach: the pitch of a string learned through its
fractional delay, and the harmonic synthesizer of DDSP fitted through a
spectral loss computed frame by frame inside an `ondemand` block. The
twelfth is the FDN reverb again, calibrating itself and then switching its
learning off, so that it costs a reverb once done.
Every program lives in `tests/corpus/ddsp_*.dsp`, is run by the test suite
([crates/compiler/tests/ddsp_examples.rs](../crates/compiler/tests/ddsp_examples.rs)),
and can be watched with `faustprobe`:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 40000 --every 5000 tests/corpus/ddsp_fad_adaptive_notch.dsp
```

`-n` renders that many frames, `--every` prints one frame out of N, `--quiet`
prints statistics only, `--skip N` excludes the first N frames from them.
The columns are the program's outputs in the order its header gives; in the
statistics, `dc` (the mean) is the reading for a parameter and `rms` for a
residual. Each section below gives the command for its program and what it
prints; the commands run from the repository root.
This document says what each program does, what is differentiated and why in
that mode, which optimizer it uses and why, and what the numbers are. The
reader new to the vocabulary will find it in
[optimizers-overview-en.md](optimizers-overview-en.md); the step-by-step
introduction is [optimizers-ddsp-tutorial-en.md](optimizers-ddsp-tutorial-en.md).

| | Program | Task | Mode | Loop and engine | Result |
|---|---|---|---|---|---|
| 1 | `ddsp_fad_adaptive_notch` | remove a hum of unknown frequency | `fad` | `lsq_1D` + `nlms` | 1000.0 ± 0.2 Hz from 1400 Hz, residual at the noise floor |
| 2 | `ddsp_fad_modal_resonator_lm` | calibrate a mode (frequency, Q) | `fad` | `lm_2D` (Gauss-Newton) | (800.000, 25.001) from (600, 10) |
| 3 | `ddsp_fad_amp_model` | learn an amp (drive, gain, tone) end to end | `fad` | `descend_3D` + Adam | (3.98, 0.701, 0.800) for (4, 0.7, 0.8) |
| 4 | `ddsp_rad_echo_canceller_64` | cancel a 64-tap acoustic echo | `rad` | `lsq_N_rad` + `nlms` | residual echo below 1e-9 (ERLE > 100 dB) |
| 5 | `ddsp_rad_mlp_waveshaper` | train a small neural network to a soft clipper | `rad` | `descend_N_rad` + Adam | residual 46 dB under the target |
| 6 | `ddsp_rad_host_block_resonator` | block gradients of a resonator for a host | `rad`, public | Adam in the host (Rust) | gradient = finite differences to five digits, (−1.20000, 0.72000) recovered |
| 7 | `ddsp_fad_diode_clipper_newton` | learn the components of a diode clipper through its implicit solver | `fad` in `fad` | `lm_2D` | (τ, k) exact within 8 000 samples; unrolled = implicit derivative to 2e-7 |
| 8 | `ddsp_fad_fdn_reverb_lm` | calibrate an FDN reverb to a target decay | `fad` | `lm_2D` | (T60, damping) = (0.600, 0.300) from (0.3, 0) |
| 9 | `ddsp_rad_gru_amp_host` | train a GRU amplifier model by block-truncated BPTT | `rad`, public | Adam in the host (Rust) | gradients = finite differences to four digits; residual 29 dB under the target |
| 10 | `ddsp_fad_waveguide_string_pitch` | tune the pitch of a waveguide string through its fractional delay | `fad` | `lsq_1D` + `nlms` | 228 → 220.000000 Hz; the well is ±1 Hz wide, capture only from above |
| 11 | `ddsp_rad_harmonic_spectral_frame` | fit 16 harmonic amplitudes through a per-frame spectral loss | `rad` in `ondemand` | Adam per frame, in an `ondemand` block | all amplitudes within 2.5e-4 of 1/h in 100 frames |
| 12 | `ddsp_fad_fdn_gated` | calibrate the FDN, then switch its learning off | `fad` in `gated` | `lm_2D` in an `ondemand` gated by `stop_below`, gains by `on_change` | (0.600, 0.300) frozen at the sixth period; the learning then costs nothing |

**Where the optimizer runs.** Eight examples take one step per audio sample
inside the graph, through the loops of the library (`lsq_1D`, `lm_2D`,
`descend_3D`, `lsq_N_rad`, `descend_N_rad`): 1, 2, 3, 4, 5, 7, 8 and 10.
Example 11 is the only one whose optimizer runs in an `ondemand` block: its
loss is computed once per 256-sample frame inside the block and Adam steps
there, at frame rate, in a block written by hand around `frame_sum` and
`adam_g`. The clocked loops of the library (`descend_1D_clocked` …
`descend_5D_clocked`, `descend_N_clocked`, `descend_N_rad_clocked`) package
the other clocked pattern, a loss computed at audio rate and its gradient
averaged over the frame, one step per firing; none of the first eleven uses
them, section 11 of the tutorial and the fixtures `opt_descend_clocked_gain.dsp`
and `opt_descend_in_ondemand_gain.dsp` do. Example 12 runs the loop of
example 8 at audio rate inside `op.gated`, an `ondemand` block that its own
flag stops: the only one whose learning ends, and the one that uses the
gating helpers of the library, `stop_below` for the flag and `on_change` for
the coefficients of the rendered reverb. Examples 6 and 9 use no
`ondemand` at all: their optimizer is the host's, one Adam step per
`compute` block on the summed gradient lanes.

## 1. Hum cancellation with an adaptive notch (`fad`)

**What it does.** The input is a 1 kHz hum (a sine of amplitude 0.5) with a
little noise. A notch filter removes one frequency; the program learns which
one by minimising the power of its own output. This is the adaptive notch of
Rao & Kung (1984) and Nehorai (1985), the standard way to track and remove an
interfering line without knowing its frequency.

**Model.** The notch is constrained by construction: zeros on the unit
circle at ±w, poles at radius r = 0.95 just behind them,

```text
H(z) = (1 − 2c z⁻¹ + z⁻²) / (1 − 2rc z⁻¹ + r² z⁻²),   c = cos w.
```

The learned parameter is c, so any value in [−1, 1] is a valid notch; the
frequency in hertz is read back with `acos`. `r` sets the width of the null:
a narrower notch (r closer to 1) attenuates less around the hum but has a
narrower basin of attraction.

**What is differentiated, and why forward mode.** The loop is `lsq_1D` with
the notch as the model and a target of zero: each sample, `fad` returns the
notch output and its sensitivity `j = d(output)/dc`. The notch is recursive,
so `j` at sample n depends on the whole past of the filter; `fad` carries
that derivative along with the filter state (the RTRL derivative), which is
exactly the quantity the classic derivations approximate with a "simplified
gradient". One parameter, one tangent: forward mode costs one extra filter.

**Optimizer.** `nlms(0.002, 1e-6, 0.99)`: the step `mu · r · j / E[j²]` is
proportional to the residual, so it settles by itself once the null is on
the hum. An Adam step, normalised to about `lr` per sample, keeps
random-walking at the optimum: the same program with `descend_1D` and Adam
jitters by ±25 Hz.

**What you see.** From 1400 Hz: 966 Hz after 1 000 samples, 995.7 after
2 000, 999.9 after 4 000, then 1000.0 ± 0.2 Hz. The residual falls from the
hum level (rms 0.35) to rms 0.0118, the floor of the added noise (0.02 uniform:
rms 0.0115): the hum is gone and the noise untouched.

**With faustprobe.** Column 2 is the frequency of the null, column 1 the
cleaned signal:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --every 4000 tests/corpus/ddsp_fad_adaptive_notch.dsp
```

prints 1400 at frame 0, 999.9 at 4 000, 1000.05 at 8 000, then within 0.1 Hz
of 1000. Add `--quiet --skip 16000` for the statistics of the last 4 000
frames: `out0` rms 0.0120, the floor of the added noise, and `out1` dc
1000.00, the mean of the tracked frequency.

**Try.** Move `f0` during the run (it is a constant here; make it a slider):
the notch follows. Lower `r` to 0.9 to widen the capture range, raise it to
0.99 to hear how narrow a null can be. Replace the sine by two sines: one
notch tracks one of them; two cascaded notches with two parameters
(`lsq_2D`) track both.

## 2. Calibrating a mode by Gauss-Newton (`fad`)

**What it does.** A mode of a modal synthesiser is a resonant band-pass with
a frequency and a quality factor (its decay). Given the response of a hidden
mode to noise, the program identifies both parameters of a model mode. Modal
calibration from recordings is the bread and butter of physical-modelling
DDSP; this is one mode of it, with the second-order method.

**Model.** `fi.resonbp(f, q, 1)` with f in hertz and q without unit; the
target is `(800, 25)`, the model starts at `(600, 10)`.

**What is differentiated, and why forward mode.** `lm_2D` differentiates the
*model* with respect to its two parameters: each sample, `fad` gives the two
sensitivities of the resonator output, exact through its recursion, and the
loop solves the 2×2 normal equations built from them (a damped Gauss-Newton
step, Levenberg-Marquardt), with a forgetting factor of 0.99 and a Marquardt
damping of 0.1. Two parameters with incompatible units — hertz and Q — take
steps of the right scale without any tuning; a first-order engine would need
a log domain or per-parameter rates (tutorial, section 5). Forward mode is
the natural way to get a Jacobian row per sample: two tangents.

**What you see.** f reaches 799.87 Hz within 8 000 samples and q 24.3, both
exact (800.000, 25.001) by 24 000; the residual falls to rms 2.6e-5. Q
briefly hits its upper bound (60) on the way: the damped step is bold while
the Jacobian is small, and the bound is what keeps it honest.

**With faustprobe.** Columns f, q, residual:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --every 4000 tests/corpus/ddsp_fad_modal_resonator_lm.dsp
```

f overshoots to 808.6 at 4 000 and is 799.9 at 8 000; q closes in more
slowly, 22.4, 24.3, 24.7, 25.3 at the printed frames, 25.0 on average; the
residual column is a few 1e-3 at most.

**Try.** Excite with an impulse train instead of noise (the calibration then
only learns during the decays). Add a third parameter, the mode's gain, with
`lm_3D`. Two modes: two `lm_2D` loops on the same target cannot separate
them; a five-parameter `descend_5D` on the sum can.

## 3. An amplifier model learned end to end (`fad`)

**What it does.** The smallest "amp": a drive into a `tanh` saturation, a
tone control (a one-pole low-pass), a gain. Given the output of a hidden amp
on noise, the three parameters are learned from the waveform error. It is
the shape of every neural-amp-modelling task, reduced to a model with three
interpretable knobs.

**Model.** `amp(ldrive, gain, tone, x) = gain · tanh(e^ldrive · x) : si.smooth(tone)`.
The drive is learned in the log domain (a multiplicative parameter, whose
useful range spans a decade), the tone as the pole coefficient bounded in
[0, 0.95] (a stable filter by construction), the gain in [0, 2]. Target
`(4, 0.7, 0.8)`, start `(1, 1, 0.5)`.

**What is differentiated, and why forward mode.** `descend_3D` differentiates
the loss `mse(amp(p, x), target)` with respect to the three parameters. `fad`
goes through the foreign `tanh` (`maths.lib`'s `ffunction`) and through the
one-pole recursion: the derivative of the output with respect to the pole
coefficient depends on the whole past of the filter, and `fad` carries it
exactly. Three tangents through a small model: forward mode is cheap here,
and it is consumed immediately in the graph.

**Optimizer.** One `adam_g(0.002, 0.9, 0.999, 1e-8)` per parameter: the
drive, the gain and the tone have different sensitivities, and Adam
normalises each step separately. Adam keeps a small jitter at the optimum
(the drive's mean over the last 4 000 samples is 3.98, its peak 4.27); the
test reads the means. A schedule (`lr_exp`) or `polyak` averaging removes the
jitter for a deployed model.

**What you see.** `(4.00, 0.700, 0.800)` by 8 000 samples, a residual rms of
4e-3 on a target of amplitude 0.8.

**With faustprobe.** Columns drive, gain, tone, residual:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --every 4000 tests/corpus/ddsp_fad_amp_model.dsp
```

`(2.68, 0.80, 0.81)` at 4 000, `(4.000, 0.700, 0.800)` at 8 000 and 16 000.
A printed frame can catch Adam's jitter (4.14 at 12 000 in one run), which
is why the test averages the last 4 000 samples: `--quiet --skip 16000`
gives that mean in the `dc` column.

**Try.** Replace the noise by a guitar-like excitation (a decaying sawtooth
sum) and watch identifiability go: the drive is only learned where the
signal saturates. Learn a second stage (a `tanh` after the tone) with
`descend_5D`. Replace `tanh` by a table-based waveshaper: `fad`
differentiates read-only tables by finite differences on the index.

## 4. A 64-tap acoustic echo canceller (`rad`)

**What it does.** The far-end signal goes to a loudspeaker; the microphone
picks up its echo through the room. The canceller learns an FIR replica of
the room's response and subtracts it from the microphone signal — the
normalised-LMS echo canceller of every conferencing system (Haykin, *Adaptive
Filter Theory*). The room here is a synthetic 64-tap response,
`h_i = sin(1.7 i + 0.3) · e^(−i/12)`.

**Model.** `fir`, a block whose first 64 inputs are the taps and the last the
far-end signal, applied to the delayed far-end samples; `lsq_N_rad(64, fir,
nlms(0.01, 1e-6, 0.99), −2, 2, 0, 0, mic, far)`.

**What is differentiated, and why reverse mode.** The sensitivity of the FIR
output to tap i is the delayed far-end sample x[n−i]: 64 sensitivities, one
output. Reverse mode gives all of them from one sweep per sample, where
`lsq_N` would carry 64 tangents — on a 16-tap FIR the reverse loop compiles
to 3× fewer interpreter instructions, at 64 taps 7× (overview, section 5).
The body is feed-forward in the taps, so the one-sample horizon of an
in-graph `rad` loses nothing: the gradient is exact.

**Optimizer.** `nlms` per tap (the library normalises each tap by the power
of its own sensitivity), `mu = 0.01`: with 64 taps sharing the step, the
stability bound of the classic NLMS (`mu < 2/N` in these units) is what
sets it.

**What you see.** The residual starts at the echo level (rms 1.8 over the
first 2 000 samples, with a transient peak of 25 while the taps overshoot),
and is below 1e-9 by 8 000 samples: an echo return loss enhancement beyond
100 dB on this noiseless room. Add near-end noise and the residual settles
at its level.

**With faustprobe.** Column 1 is the residual echo, column 2 the microphone:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 1000 --quiet tests/corpus/ddsp_rad_echo_canceller_64.dsp
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 12000 --skip 8000 --quiet tests/corpus/ddsp_rad_echo_canceller_64.dsp
```

The first run shows the residual at the echo level, rms 2.6 with a
transient peak of 25; the second, over frames 8 000 to 12 000, an `out0` rms
of 0 to the printed precision against an `out1` rms of 1.03: the ERLE is
beyond 100 dB on this noiseless room.

**Try.** Change the room while running (make the response depend on a
slider): the canceller re-converges. Add a near-end talker: the classic
double-talk problem — the taps drift; gate the update with `gate_g` on a
double-talk detector. Compare with `lsq_N` (forward mode): same residual,
seven times the code.

## 5. A small neural network learns a waveshaper (`rad`)

**What it does.** A one-hidden-layer network with four `tanh` units (13
parameters) is trained inside the graph to imitate a soft clipper,
`0.8 · tanh(3x) + 0.1x`. This is neural amp modelling at its smallest: a
scalar loss, a network, gradient descent on the waveform error.

**Model.** `net`, a block of its 13 parameters `(w1 × 4, b1 × 4, w2 × 4, b2)`:
`y = Σ_j w2_j · tanh((w1_j + w1⁰_j) x + b1_j + b1⁰_j) + b2`. A bus loop
starts every parameter from the same value, which would leave the four
hidden units identical for ever; the model adds fixed, distinct offsets
`w1⁰_j = 1 + 0.5 j`, `b1⁰_j = −0.6 + 0.4 j` to the learned weights, so the
parameters are learned from zero around a deterministic initialisation.

**What is differentiated, and why reverse mode.** `descend_N_rad(13, net_loss,
adam_g(0.003, 0.9, 0.999, 1e-8), −4, 4, 0, 0)`: the loss `mse(net(p), target)`
is differentiated by one reverse sweep per sample for the 13 gradients.
Reverse mode *is* backpropagation: one scalar loss, many parameters, the
adjoint flowing back through the output layer to each unit. The network is
feed-forward, so the in-graph sweep is exact.

**Optimizer.** Adam, shared by the 13 parameters (one engine expression, one
state per parameter): the units have different sensitivities and Adam
equalises them.

**What you see.** The residual falls from rms 0.105 over the first 2 000
samples (the offsets' initial function is 17 dB under the target) to 0.0034
over the last 4 000: 46 dB under the target, a 30 dB improvement.

**With faustprobe.** Column 1 is the residual, column 2 the target:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 2000 --quiet tests/corpus/ddsp_rad_mlp_waveshaper.dsp
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --skip 16000 --quiet tests/corpus/ddsp_rad_mlp_waveshaper.dsp
```

rms 0.105 over the first 2 000 frames, 0.0037 over frames 16 000 to 20 000
against a target of rms 0.71: 46 dB down.

**Try.** More units (`H = 8`): the bus loop only needs the constant. A
harder target with memory — a one-pole after the clipper — and the network
cannot follow (it has no state): add a learned one-pole after `net`, or feed
`x` and `x'` to the units. Learn the offsets away: start `w1⁰` at 0 and see
the units collapse.

## 6. Block gradients of a resonator, handed to a host (`rad`, public)

**What it does.** The two denominator coefficients of a resonant filter are
sliders. The program outputs, sample by sample, the squared error against a
hidden resonator and the two gradients of that error with respect to the
sliders — and learns nothing itself. The host (the Rust test, a plugin, a
Python script) sums the gradient lanes over each block and updates the
sliders with Adam. This is the host-driven pattern of
[docs/rad-usage-en.md](../docs/rad-usage-en.md), on a recursive model.

**Model.** `resonator(c1, c2, x) = fi.tf2(1, 0, 0, c1, c2, x)`; target
`(−1.2, 0.72)` (poles at radius 0.85, 45°), sliders starting at `(−0.8, 0.5)`.
`process = rad(loss, (a1, a2))` with `loss = (target − model)²`: three
outputs, `[loss, ∂loss/∂a1, ∂loss/∂a2]`.

**What is differentiated, and why reverse mode.** Because the gradient lanes
leave the graph, the reverse sweep runs backwards over the whole `compute()`
block: the adjoint of the resonator's state is carried from sample to sample
within the block (zero terminal adjoint at the block end), so the sum of a
lane over the block is the exact gradient of the block's loss. The host can
check that against finite differences, and the test does: at `(−0.8, 0.5)`
on a block of 256, the summed lanes are 299.609 and 198.821 where central
differences on the sliders give 299.605 and 198.821 (single-precision
interpreter, `h = 1e-3`). Consumed inside the graph,
the same `rad` would see one sample and return the direct term (overview,
section 4.7); this example is the one whose gradient is exact through the
recursion *and* comes from a reverse sweep — at the price of the host loop.

**Optimizer.** Adam in Rust, `lr = 0.01` per block of 256 samples, bias
corrected, the poles kept in the stability triangle (`|a2| < 1`, `|a1| < 1 + a2`).
The sliders are written through their heap offsets (`set_real_zone`), the
excitation is the corpus LCG noise.

**What you see.** In 600 blocks (3.5 s of audio) the sliders reach
`(−1.20000, 0.72000)` and the mean block loss falls from 0.53 to 2.6e-14.

**With faustprobe.** The program needs an input and a host; `faustprobe`
provides the input and shows the lanes the host would sum, but does not
run the Adam loop (that is the Rust test's part):

```sh
faustprobe --double -I libraries -I <faustlibraries> --list-params tests/corpus/ddsp_rad_host_block_resonator.dsp
faustprobe --double -I libraries -I <faustlibraries> --in white:1 -n 256 --quiet tests/corpus/ddsp_rad_host_block_resonator.dsp
faustprobe --double -I libraries -I <faustlibraries> --in white:1 -n 256 --quiet --set /ddsp_rad_host_block_resonator/a1=-1.2 --set /ddsp_rad_host_block_resonator/a2=0.72 tests/corpus/ddsp_rad_host_block_resonator.dsp
```

The first lists the two sliders and their paths. The second, on one block
of 256 frames of white noise at the initial `(-0.8, 0.5)`, gives in the `dc`
column the mean per-sample contributions, loss 0.46, gradients 1.78 and
1.27; times 256 they are the block loss and the block gradient a host would
step on. The third sets the sliders to the hidden `(-1.2, 0.72)`: the three
lanes are exactly 0.

**Try.** Replace the target by a recording and the loss by a spectral one
computed by the host: the DSP stays the same. Batch several excitations per
update. Train the five coefficients of a biquad (`rad(loss, (b0, b1, b2, a1,
a2))`): one more lane each, one sweep.

## State of the art: three more

The six programs above are the textbook of adaptive audio; the three below
are what the differentiable-DSP literature of the last five years does, and
each of them relies on something a tensor framework does not give: the exact
derivative through an implicit solver, through thousands of samples of
feedback, or the reverse sweep through a recurrent cell without rewriting
the model.

## 7. A diode clipper learned through its implicit solver (`fad` in `fad`)

**What it does.** The circuit of every overdrive pedal: a resistor, a
capacitor and a diode pair (Yeh, Abel & Smith 2007),
`dv/dt = (x − v)/(RC) − (2 Is/C) sinh(v/(2 n Vt))`. Discretised by backward
Euler it is an implicit equation in v[n], `G(v) = v − v[n−1] − h f(v, x[n]) = 0`,
solved at every sample by four safeguarded Newton iterations whose slope
`G'(v)` comes from an inner `fad` — a zero-delay-feedback virtual-analog
model in the usual sense. Two component values, τ = RC and k = 2 Is/C, are
then learned from the output of a hidden clipper: white-box virtual-analog
modelling (Esqueda, Kuznetsov & Parker 2021), in the audio thread.

**Model.** A guitar-like excitation (three partials and band-limited noise,
about ±1.5 V, so the diodes conduct on the peaks); `h = 1/SR`,
`2 n Vt = 0.09 V`; hidden `(τ, k) = (1e-4 s, 0.1)`, i.e. 2.2 kΩ · 47 nF;
the model starts at `(3e-4, 0.03)`, both in the log domain. The Newton
iteration starts from an explicit-Euler predictor and keeps its iterate
within ±2 V.

**What is differentiated, and why forward mode.** `lm_2D` differentiates
the clipper output with respect to `(log τ, log k)`: the outer `fad` goes
through the four unrolled Newton steps — each holding an inner `fad` for
the slope — and through the state recursion: `fad` inside `fad` inside a
recursion, all expanded at compile time. The program checks the result
against the implicit-function theorem: the derivative of the solved v with
respect to k propagated through the recursion,
`s[n] = −(G_k + G_vprev · s[n−1]) / G_v`, agrees with the unrolled derivative
to 2e-7, in both precisions, while the Newton residual stays below 1e-8
(1.2e-7 in single). Forward mode: two tangents through a solver whose
Jacobian the inner `fad` already provides. Two things had to hold for this
to work in single precision, and both are now in the compiler and the
pitfalls: a recursion the seed does not reach is not augmented (the whole
`lm_2D` loop used to be copied into the inner `fad`, with tangent slots
exactly zero in theory and `inf · 0` in `f32`), and the iteration must not
start from the very signal the equation holds fixed — seeds are matched by
identity, `fad(G(vprev, v), v)` with `v = vprev` differentiates both.

**Optimizer.** `lm_2D(mdl, 0.01, 0.1, 0.99, …)`: damped Gauss-Newton with
the exact Jacobian through the solver.

**What you see.** `(τ, k) → (1.0000e-4, 0.1000)` within 8 000 samples, the
residual against the hidden clipper at 1.7e-7 rms in single precision.

**With faustprobe.** Columns τ × 1e4, k, residual, Newton residual,
derivative gap, derivative:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --every 4000 tests/corpus/ddsp_fad_diode_clipper_newton.dsp
```

τ × 1e4 goes from 3.0 to 1.000 and k from 0.03 to 0.1000 by the frame
4 000 line already; the residual, the Newton residual and the gap between
the two derivatives print as 0 at nine decimals; the last column, dv/dk
itself, grows with the signal from 0.05 to 0.56, the scale against which
the gap is read.

**Try.** Learn `2 n Vt` as well (`lm_3D`); an asymmetric clipper (one
diode, `exp` instead of `sinh`); a second RC stage; feed a recording and
watch identifiability depend on how hard the input drives the diodes.

## 8. An FDN reverb calibrated to a target decay (`fad`)

**What it does.** A four-line feedback delay network (Jot 1991): prime
delays of 1051, 1327, 1597 and 1801 samples (24 to 41 ms), an orthogonal
Hadamard matrix (scaled by 1/2), a per-line gain set by a reverberation
time, `gain_i = 10^(−3 len_i / (T60 · SR))`, and a per-line one-pole damping
that shortens the decay of high frequencies. Given the responses of a hidden
FDN to an impulse train, the program learns its T60 and its damping:
differentiable artificial reverberation (Lee, Choi & Lee 2022).

**Model.** Impulses every 16 384 samples; hidden `(T60, d) = (0.6 s, 0.3)`;
start `(0.3 s, 0)`, T60 in the log domain.

**What is differentiated, and why forward mode.** `fad` carries a tangent
through the four delay lines, the damping filters and the feedback matrix,
sample by sample: the derivative of a reverb tail with respect to its decay
parameters, exact through recursions thousands of samples long, where a
tensor framework unrolls or approximates. Two tangents.

**Optimizer.** `lm_2D` with a forgetting factor of 0.999: the gradient is
informative only during the decays, and Gauss-Newton with a forgetting
factor keeps the last decay in its information matrix. Adam with a schedule
reaches `(0.60, 0.30)` as well, then wanders between impulses when the
gradient carries no information (the fixture says so).

**What you see.** `(0.574, 0.289)` after 8 000 samples, `(0.6000, 0.3000)`
by 60 000 (four impulses), the residual at 4.8e-7 rms at 80 000.

**With faustprobe.** Columns T60, damping, residual; `--every 16384`
prints one line per impulse:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 80000 --every 16384 tests/corpus/ddsp_fad_fdn_reverb_lm.dsp
```

T60 0.568 after the first period, 0.6002 after the second, 0.6000 from
the third; damping 0.298, 0.2998, 0.29995, 0.30000; the residual falls to
3e-7.

**Try.** Learn one gain per line (`descend_N`); make the target a
*different* reverb and the loss `log_energy_loss` on the decay; eight lines;
a frequency-dependent T60 with a target measured from a room.

## 9. A GRU amplifier model trained by block-truncated BPTT (`rad`, public)

**What it does.** A GRU cell with two hidden units and a linear readout,
27 parameters — the architecture of real-time neural amp modelling (Wright &
Välimäki 2020) — is trained to imitate a hidden amplifier (a tone control
into a `tanh` saturation). The parameters are sliders; the program outputs
the squared error and its 27 gradients, sample by sample; the host (the Rust
test) sums each lane over the block and takes an Adam step: truncated
backpropagation through time, with the block as the truncation length.

**Model.** `z = σ(W_z x + U_z h + b_z)`, `r = σ(W_r x + U_r h + b_r)`,
`c = tanh(W_h x + U_h (r ∘ h) + b_h)`, `h' = (1 − z) ∘ h + z ∘ c`,
`y = W_o h' + b_o`, two units; one slider per parameter with a fixed
initial value (the parser wants literal labels). Hidden amplifier
`0.8 · tanh(3 · si.smooth(0.7, x))`. `process = rad(loss, params)`: 28 lanes.

**What is differentiated, and why reverse mode.** One loss, 27 parameters:
one reverse sweep. Because the lanes leave the graph, the sweep runs
backwards over the whole block through the gates, the `tanh` candidate and
the two fed-back states, with a zero terminal adjoint at the block end: the
sum of a lane is the exact gradient of the block loss with the initial state
held fixed — BPTT truncated at the block, which the test checks against
central finite differences on three parameters of different kinds: an input
weight 0.3671 (0.3670), a recurrent weight 0.0288 (0.0288), a readout weight
−3.2342 (−3.2342). Consumed inside the graph, the same `rad` would see one
sample, and a recurrent model cannot be trained on that; hence the host.

**Optimizer.** Adam in Rust, `lr = 0.005` per block of 256 samples, 2 000
blocks (11.6 s of audio), the state carried across blocks.

**What you see.** The mean block loss falls from 4.7e-3 (first 100 blocks)
to 2.4e-4 (last 100); on fresh noise, from a fresh instance, the residual is
0.0148 for a target of rms 0.43: 29 dB under the target.

**With faustprobe.** As for example 6, `faustprobe` shows what the host
would read, not the training:

```sh
faustprobe --double -I libraries -I <faustlibraries> --list-params tests/corpus/ddsp_rad_gru_amp_host.dsp
faustprobe --double -I libraries -I <faustlibraries> --in white:3 -n 256 --quiet tests/corpus/ddsp_rad_gru_amp_host.dsp
```

27 sliders; on one block of white noise at the initial weights, `out0` has
a `dc` of 0.0119, the mean block loss, and `out1` to `out27` the mean
gradient contributions of the 27 parameters in the order of the `params`
list; the host sums each over the block and steps.

**Try.** Four hidden units (more sliders, same host loop); an LSTM cell;
several excitations per update; a recording of a real amplifier as the
hidden model — the DSP does not change, only the host's target.

## The two fragile ones: pitch through a delay, spectra through a frame

## 10. A waveguide string learns its pitch through a fractional delay (`fad`)

**What it does.** A plucked-string model — a loop with a fractional delay
(4th-order Lagrange interpolation), a loss gain and a one-pole damping —
driven by noise; the delay length, that is the pitch, is learned from a
hidden string at 220 Hz by normalised least squares on the waveform.

**What is differentiated, and why forward mode.** `fad` differentiates the
loop output with respect to the delay length: through the interpolation
(the derivative of an interpolated read with respect to the read position
is the local slope of the signal) and through the feedback, sample by
sample. Tensor frameworks have no derivative with respect to a delay
length; here it costs one tangent.

**What the landscape allows.** The waveform error between two strings is a
well ±1 Hz wide around 220 Hz on a flat plateau: residual rms 0.10–0.11
from 150 to 300 Hz, 0.09 at ±1 Hz, 0 at 220. On the plateau the gradient
is not zero: the loop filter's group delay shifts the string's
autocorrelation peak off the delay length, so the model's own output power
depends on `d` and the normalised step drifts toward *lower* pitch whatever
the target (a correlation loss, `−model · target`, removes that bias and
has no pull on the plateau either). Fine tuning works — from 228 Hz the
pitch locks to 220.000000 Hz within 60 000 samples, and from 264 Hz too
when the model's damping is annealed from 0.70 to 0.95 (broad resonances
first) — and from below it does not (200 → 190 Hz, 176 → 168). That is why
DDSP systems estimate f0 with a detector and let the gradient refine it.

**Optimizer.** `lsq_1D` with `nlms(0.02, 1e-6, 0.99)`.

**What you see.** 228 → 219.99 Hz at 20 000 samples, 220.000000 at 60 000,
residual 3e-8.

**With faustprobe.** Columns pitch in Hz, residual:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 60000 --every 10000 tests/corpus/ddsp_fad_waveguide_string_pitch.dsp
```

228, 223.8, 219.99, 220.0007, 219.99999, 220.0000004 at the printed frames;
the residual goes from 0.08 to 2e-7.

**Try.** Learn the damping as well (`lsq_2D`); replace the noise by plucks
and watch the well narrow; start a fifth away and watch the drift; feed a
pitch detector's estimate as `init`.

## 11. A harmonic synthesizer fitted through a per-frame spectral loss (`rad` in an `ondemand` block)

**What it does.** The harmonic oscillator bank of DDSP (Engel et al. 2020):
sixteen harmonics of 440 Hz whose amplitudes are learned, positive by
construction (`a_h = exp(p_h)`), fitted to a target signal through a
spectral loss computed once per 256-sample frame. The target here is a
hidden harmonic tone with amplitudes 1/h, but any audio would do: it is
analysed at audio rate — windowed correlations at the sixteen harmonics,
accumulated over the frame with `frame_sum` — and enters the block as
inputs.

**What is differentiated, why reverse mode, and why a block.** Inside the
block, fired once per frame, the synthesizer's frame is computed from the
log-amplitudes and the frame start, its magnitudes at the harmonics are
compared with the target's, and `rad` on that frame loss gives the sixteen
gradients from one reverse sweep — in the block's own domain, at frame
rate, on a feed-forward loss; one Adam step per frame. The magnitude loss
is blind to the sign of an amplitude (a harmonic converges to −a as readily
as to a), hence the exponentials, as in DDSP. Three things had to hold in
the compiler and the library: the reverse sweep treats a block's boundary
inputs and the foreign constants (`ma.SR`) as leaves and passes through the
clocked wrapper, so a `rad` can live *inside* a block (across the boundary
it is still rejected); and a loop's state is best kept as a deviation from
`init`, with no first-sample detection at all, which is what
`optimizers.lib` 0.7.1 does.

**What you see.** All sixteen amplitudes within 2.5e-4 (relative) of 1/h
in 100 frames, 0.6 s of audio; the resynthesis residual is 1.2e-3 rms for
a target of rms 0.8. The frame graph — 256 × 16 sines, 32 correlations of
256 terms — normalises in one second in a release build and in two minutes
in an unoptimised one (the add-term factorisation C++ Faust also runs), so
its test runs under `cargo test --release`.

**With faustprobe.** Columns 1 to 16 are the amplitudes, held between
frames, column 17 the residual; the statistics of the last 4 200 frames are
the reading:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 51200 --skip 47000 --quiet tests/corpus/ddsp_rad_harmonic_spectral_frame.dsp
```

`out0` dc 0.9998 (1/1), `out1` 0.4999 (1/2), ..., `out15` 0.06250 (1/16);
`out16` rms 4e-4. The run takes about fifteen seconds: the normalisation of
the frame graph, a sum of 256 products of 16-term sums, dominates.

**Try.** A recording as the target (`--in`); more harmonics; DDSP's other
half, a noise band through a learned filter; a multi-resolution loss (two
frame sizes, two blocks).

## 12. A reverb that calibrates itself, then stops paying for it (`gated`, `stop_below`, `on_change`)

**What it does.** The FDN of example 8, the same hidden target, the same
Gauss-Newton loop, with two additions from the "Gating and Stopping" section
of the library. The whole learning, the model carrying the two tangents,
`lm_2D` and the residual, lives in `op.gated(learn)`, an `ondemand` domain
whose clock the block's own flag switches off: once the flag is raised
nothing of the learning is computed any more and the parameters hold. And
the reverberator that renders the output takes its four gains from
`op.on_change`, which recomputes `10^(−3 len_i / (T60 · SR))` only when T60
changes, once per learning step and never after the stop, instead of four
`pow` per sample.

**The flag.** `op.stop_below(clock, 1e-7)` on the squared residual: raised
at the end of the first period whose residual energy is under 1e-7, a
residual of 2.5e-6 rms, an exact match for an effect, and kept raised. A
threshold rather than `stop_relative` because the target is exact: the
residual has no floor, it keeps falling geometrically and its relative
change never settles. On a measured target, with a noise floor,
`stop_relative` is the criterion; the calibration of a reverberator to
measured rooms in section 7 of the overview uses it. Gauss-Newton with a
forgetting factor of 0.999 is also why the target is exact here: noise at
−60 dB is enough to make its steps wander (try it).

**What you see.** `(0.5688, 0.2980)` at the end of the first period,
`(0.6000, 0.3000)` by the fourth; the residual energy per period falls from
3.9e-2 to 1.6e-8 at the sixth, where the flag rises on the period's last
sample (98 303) and the parameters freeze at `(0.600002, 0.300000)`; the
reverb rendered on the held gains then matches the target to 5e-9 rms. Until
the flag a gated block is bit-identical to the same block outside the gate
(the fixtures of the library check it); after it, the learning costs
nothing and the reverb costs a reverb.

**With faustprobe.** Columns T60, damping, done, residual of the rendered
reverb; one line per period:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 160000 --every 16384 tests/corpus/ddsp_fad_fdn_gated.dsp
```

T60 and damping follow example 8 line for line; `done` is 0 for five
periods and 1 from frame 98 304, the sixth period, on; from that line T60
prints 0.600001606 on every following line, bit for bit, and the residual
is 1e-9. Nothing of the learning runs any more.

**Try.** `gated_when(button("learn"), learn)` to relearn on demand; a target
that changes every hundred periods, with `gated_when` re-enabling the
learning when the residual energy rises; `stop_after(clock, 8)` as a plain
budget.

## How the tests check them

Each program renders through the interpreter on a fresh instance (the
standard libraries are found through `FAUST_RS_FAUSTLIBRARIES_ROOT` or the
default checkout; the tests skip when they are absent), and the checks are
the numbers above with a margin: the notch within 0.5 Hz and the residual
under 0.02 rms, the mode within 0.5 Hz and 0.1 in Q, the amp within 2 % on
the means of the last 4 000 samples, the echo canceller above 30 dB of ERLE,
the network 20 dB under the target with a fivefold improvement over its
start, the host loop within 0.02 of the target with a 30 dB loss reduction
after the finite-difference check; the diode clipper within 1 % of τ and k
with a Newton residual under 1e-4 and the two derivatives within 1e-3 of
each other, the FDN within 0.01 of T60 and damping, the GRU's gradients
within 2 % of finite differences, its loss cut tenfold and its residual 20 dB
under the target; the string within 0.05 Hz of 220 with a residual under
1e-3; the harmonic amplitudes within 2 % of 1/h with a resynthesis residual
under 0.01 (in release builds); the gated FDN within 0.01 of T60 and
damping when its flag rises, on a period boundary between the fourth and
the twentieth period, its parameters bit-constant afterwards and the
rendered residual under 1e-4 rms. The programs run in single precision there
and in double under `faustprobe`; both converge.

## Where the gradients come from

`fad` expands during propagation into the augmented-state recursion
described in [docs/fad-note-en.md](../docs/fad-note-en.md); `rad` into the
block reverse sweep of [docs/rad-note-en.md](../docs/rad-note-en.md), whose
carries, tapes and horizons are what examples 4 to 6 exercise. The bus loops
and the engines are documented function by function in
[optimizers.lib](optimizers.lib).
