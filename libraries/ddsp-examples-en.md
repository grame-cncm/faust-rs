# Six DDSP examples with `fad` and `rad`

Six complete differentiable-DSP programs, each one a task an audio engineer
recognises, written with the two automatic-differentiation primitives of
`faust-rs` and the loops of [optimizers.lib](optimizers.lib). Three use
`fad`, forward mode, where the exact derivative through a recursion is what
makes the method work; three use `rad`, reverse mode, where one scalar loss
depends on many parameters or where the gradient leaves the graph for a host.
Every program lives in `tests/corpus/ddsp_*.dsp`, is run by the test suite
([crates/compiler/tests/ddsp_examples.rs](../crates/compiler/tests/ddsp_examples.rs)),
and can be watched with `faustprobe`:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 40000 --every 5000 tests/corpus/ddsp_fad_adaptive_notch.dsp
```

`-n` renders that many frames, `--every` prints one frame out of N, `--quiet`
prints statistics only, `--skip N` excludes the first N frames from them.
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

**Try.** Replace the target by a recording and the loss by a spectral one
computed by the host: the DSP stays the same. Batch several excitations per
update. Train the five coefficients of a biquad (`rad(loss, (b0, b1, b2, a1,
a2))`): one more lane each, one sweep.

## How the tests check them

Each program renders through the interpreter on a fresh instance (the
standard libraries are found through `FAUST_RS_FAUSTLIBRARIES_ROOT` or the
default checkout; the tests skip when they are absent), and the checks are
the numbers above with a margin: the notch within 0.5 Hz and the residual
under 0.02 rms, the mode within 0.5 Hz and 0.1 in Q, the amp within 2 % on
the means of the last 4 000 samples, the echo canceller above 30 dB of ERLE,
the network 20 dB under the target with a fivefold improvement over its
start, the host loop within 0.02 of the target with a 30 dB loss reduction
after the finite-difference check. The programs run in single precision
there and in double under `faustprobe`; both converge.

## Where the gradients come from

`fad` expands during propagation into the augmented-state recursion
described in [docs/fad-note-en.md](../docs/fad-note-en.md); `rad` into the
block reverse sweep of [docs/rad-note-en.md](../docs/rad-note-en.md), whose
carries, tapes and horizons are what examples 4 to 6 exercise. The bus loops
and the engines are documented function by function in
[optimizers.lib](optimizers.lib).
