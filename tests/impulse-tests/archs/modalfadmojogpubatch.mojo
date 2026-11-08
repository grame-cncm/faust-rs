# ---------------------------------------------------------------------------
# modalfadmojogpubatch.mojo — modal sound matching by forward-mode AD on the
# GPU: a bank of N modes, one GPU instance per mode of
# tests/corpus/gpu_modal_fad_voice.dsp (an impulse-struck damped rotation
# whose `fad` outputs y and dy/dfreq, dy/dt60, dy/dgain), is fitted to a
# target signal by gradient descent on the squared error of the whole mix
# (porting/faust-gpu-audio-synthesis-2026-11-07-en.md, §5).
#
# Concatenated after `impulsemojo.mojo` and two generated programs:
# `modaldsp` (the batch contract of gpu_modal_fad_voice.dsp) and `refdsp`
# (its scalar contract, one ordinary instance per mode, the CPU reference of
# `validate`). Built with `-D DFAUST=DType.float32`.
#
#   <binary> validate                                     GPU against CPU and closed form
#   <binary> bench <modes> [<block>] [adam|gn]            time per epoch
#   <binary> fit <modes> <epochs> <sigma> [adam|gn] [<lr>] [<block>]
#   <binary> fit-file <target.f32> <modes.txt> <epochs> <out> [adam|gn] [<lr>] [<block>]
#
# NPARAM (`-D NPARAM=3`, the default, or 4) is the number of controls the
# voice differentiates. With 3 the voice is gpu_modal_fad_voice.dsp (freq,
# t60, gain: every mode starts on a sine); with 4 it is
# gpu_modal_fad_voice_phase.dsp, which adds `gainc`, the amplitude of the
# cosine, so that each mode has its own phase (a recording). Everything below
# that names "gain" applies to every amplitude: gain, and gainc with 4.
#
# `fit-file` fits a recording: the target is a raw native-endian f32 file at
# 48 kHz, cut to a whole number of blocks; the starting modes are the lines
# "freq t60 gain [gainc]" of a text file (tools/modal_init.py writes them
# from the recording's spectrum). It writes the fitted modes to
# <out>.modes.txt (same format), and the model and the residual as raw f32 to
# <out>.model.f32 and <out>.residual.f32.
#
# The target is synthetic: a hidden bank of N modes (`make_bank`), rendered
# by the same voice on the GPU, so that recovery is checked mode by mode.
# Frequencies on a jittered grid from 100 Hz to 16 kHz (neighbours at least
# 0.2 grid steps apart), T60 from about 2.5 s at 100 Hz to 0.4 s at 16 kHz,
# gains of random sign over 24 dB. The fit starts from the hidden bank
# perturbed (`perturbed`): each frequency moved by `sigma` times the mode's
# bandwidth (ln(1000) / (pi T60), Gaussian), each T60 by a factor
# exp(0.3 sigma N(0,1)), each gain by (1 + 0.3 sigma N(0,1)). `sigma` is the
# distance from the truth in bandwidths: the fit measures the basin of the
# loss, not a blind search (no peak picking).
#
# The loss: L = sum_t e(t)^2 / sum_t target(t)^2 with e = target - sum_k y_k,
# over SIGNAL_FRAMES samples (1.024 s at 48 kHz). The gradient of mode k is
# -2 sum_t e(t) dy_k/dtheta_k / sum_t target^2.
#
# One epoch is ONE pass over the signal, block by block: the loss is a sum
# over time of a function of the mix at that time, so e on a block needs only
# that block of every mode. Per block: the voice kernel first reduces its own
# tangents of the previous block against that block's e (the outputs are its
# own, still in place), then computes the new block; a first mix kernel sums
# the modes by groups of 64 (one thread per group and frame), a second sums
# the groups and writes e = target - mix (one thread per frame). After the
# last block a gradient kernel reduces it, and the update kernel steps every
# mode. No tangent is stored beyond one block: memory is N x 4 x block
# floats, 64 MB for 4,096 modes at 1,024 frames.
#
# Two updates, one GPU thread per mode, both in the coordinates
# theta = (freq / bw0, log t60, gain / |gain0|), bw0 and gain0 the starting
# bandwidth and gain of the mode:
#   adam  Adam (beta 0.9, 0.999) on the gradient, the rate `lr` (default 0.1,
#         in those units) decayed to 1 % over the run on a cosine.
#   gn    damped Gauss-Newton per mode: the kernel also accumulates the 3x3
#         Gram matrix of the mode's tangents (6 more sums) and solves
#         (J'J + 0.1 diag(J'J)) delta = J'e for its own three parameters,
#         every mode against the same residual (a block-Jacobi step), scaled
#         by a factor eta (at most `lr`, default 1) and clamped to 0.5 bw,
#         0.2 in log T60 and 0.5 in gain. Overlapping modes correct the same
#         residual at once and overshoot together, so `fit` globalises the
#         step: a step that raises the loss is undone and eta halved, an
#         accepted one grows eta by 25 %. `bench` times the step at a fixed
#         eta (the globalisation adds one 192 KB copy per epoch).
#
# `fit` prints the loss every epochs / 20 steps, then the modes recovered:
# strict (frequency within 0.01 bandwidth, T60 and gain within 1 %) and loose
# (0.1 bandwidth, 10 %), the same counts at the start, and how many modes are
# isolated (both neighbours at least 2 (bw + 1/T) away) and recovered strict.
#
# The batch code keeps the `compute` state in locals for the sample loop
# (`fir::batch::promote_state`); `make mojo-gpu-modal-fad` also builds the
# earlier form, every state access in the voice arenas
# (`mojo_batch --no-registers`), and validates and times both.
#
# `validate` runs 48 modes for 0.5 s from a perturbed start and fails above
# 2e-3 of relative difference on any of: the four outputs of every mode
# against the scalar program on the CPU, against the closed form in f64, the
# residual e against the CPU's, and the three gradient sums against the same
# sums formed in f64 on the CPU outputs.
#
# Every buffer a kernel reaches is one of its arguments (Metal); kernel
# scalars are fixed-width.
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext, DeviceBuffer, HostBuffer
from max.gpu import global_idx
from std.ffi import external_call
from std.sys import argv
from std.time import perf_counter_ns

comptime GPU_BLOCK_DIM = 64
comptime RATE = S32(48000)
comptime SIGNAL_FRAMES = 49152
comptime GROUP = 64
# The voice's differentiated controls, its outputs (y and one tangent per
# control), the entries of a mode's Gram matrix, and the sums per mode.
comptime NP = get_defined_int["NPARAM", 3]()
comptime NOUT = NP + 1
comptime NGRAM = NP * (NP + 1) // 2
comptime NACC = NP + NGRAM
comptime LN1000 = 6.907755278982137
comptime TWO_PI = 6.283185307179586
comptime FLO = 100.0
comptime FHI = 16000.0
comptime BETA1 = 0.9
comptime BETA2 = 0.999


# Records each control's label, in control-table order.
struct LabelGui(FaustGui):
    var labels: List[String]

    def __init__(out ui):
        ui.labels = List[String]()

    def add_horizontal_slider(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.labels.append(label)


def index_of(labels: List[String], label: String) raises -> Int:
    for i in range(len(labels)):
        if labels[i] == label:
            return i
    raise Error(String("no control `") + label + "`")


# Keeps the zones of a scalar instance, by label.
struct ZoneGui(FaustGui):
    var labels: List[String]
    var zones: List[Ptr[FaustFloat, MUT_NOTRK]]

    def __init__(out ui):
        ui.labels = List[String]()
        ui.zones = List[Ptr[FaustFloat, MUT_NOTRK]]()

    def add_horizontal_slider(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.labels.append(label)
        ui.zones.append(Ptr(to=zone).unsafe_bitcast[FaustFloat]().unsafe_origin_cast[MUT_NOTRK]())


# ------------------------------------------------------------------- kernels

def init_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin], n: S32, rate: S32
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        dsps.unsafe_bitcast[modaldsp]()[].instance_init(S32(v), rate)


# The start of an epoch: every mode's state cleared (the impulse at the next
# first sample), its NP controls set from `params` (freq, t60, gain[, gainc],
# each n long; `index` holds their control indices), its sums zeroed.
def reset_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    params: Ptr[FaustFloat, MutAnyOrigin], acc: Ptr[FaustFloat, MutAnyOrigin],
    index: Ptr[Int32, MutAnyOrigin], n: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(n):
        return
    ref d = dsps.unsafe_bitcast[modaldsp]()[]
    d.instance_clear(S32(v))
    comptime for j in range(NP):
        d.set_control(S32(v), Int(index[unsafe_offset=j]), params[unsafe_offset=j * Int(n) + v])
    for j in range(NACC):
        acc[unsafe_offset=j * Int(n) + v] = FaustFloat(0.0)


# Mode v writes its NOUT outputs at outs + (NOUT v + c) * block.
def channels_kernel(
    channels: Ptr[UInt64, MutAnyOrigin], outs: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, block: S32,
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        for c in range(NOUT):
            channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]()[unsafe_offset=NOUT * v + c] = outs.unsafe_offset(
                (NOUT * v + c) * Int(block)
            ).unsafe_origin_cast[MUT_NOTRK]()


# Mode v's sums over one block: e . dy/dtheta_j (j < NP) and, with
# nacc == NACC, the NGRAM entries of its Gram matrix, (p, q) for p <= q in
# row order after the NP gradient sums.
@always_inline
def accumulate(
    outs: Ptr[FaustFloat, MutAnyOrigin], e: Ptr[FaustFloat, MutAnyOrigin],
    acc: Ptr[FaustFloat, MutAnyOrigin], v: Int, n: Int, eoff: Int, count: Int, block: Int,
    nacc: Int,
):
    var base = NOUT * v * block
    var a = Arr[FaustFloat, NACC](fill=FaustFloat(0.0))
    if nacc == NP:
        for i in range(count):
            var ee = e[unsafe_offset=eoff + i]
            comptime for j in range(NP):
                a[j] += ee * outs[unsafe_offset=base + (j + 1) * block + i]
    else:
        for i in range(count):
            var ee = e[unsafe_offset=eoff + i]
            var d = Arr[FaustFloat, NP](fill=FaustFloat(0.0))
            comptime for j in range(NP):
                d[j] = outs[unsafe_offset=base + (j + 1) * block + i]
                a[j] += ee * d[j]
            var g = NP
            comptime for p in range(NP):
                comptime for q in range(p, NP):
                    a[g] += d[p] * d[q]
                    g += 1
    for j in range(nacc):
        acc[unsafe_offset=j * n + v] += a[j]


# Reduces the previous block (eprev >= 0, its offset in e), then computes the
# next one in place.
def voice_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    outs: Ptr[FaustFloat, MutAnyOrigin], channels: Ptr[UInt64, MutAnyOrigin],
    e: Ptr[FaustFloat, MutAnyOrigin], acc: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, count: S32, block: S32, eprev: Int32, nacc: Int32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(n):
        return
    if eprev >= 0:
        accumulate(outs, e, acc, v, Int(n), Int(eprev), Int(count), Int(block), Int(nacc))
    var heads = channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]().unsafe_offset(NOUT * v).unsafe_origin_cast[MUT_NOTRK]()
    dsps.unsafe_bitcast[modaldsp]()[].compute(
        S32(v), count,
        heads.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]().unsafe_mut_cast[False]().unsafe_origin_cast[IMM_NOTRK](),
        heads,
    )


def grad_kernel(
    outs: Ptr[FaustFloat, MutAnyOrigin], e: Ptr[FaustFloat, MutAnyOrigin],
    acc: Ptr[FaustFloat, MutAnyOrigin], n: S32, count: S32, block: S32, eoff: Int32, nacc: Int32,
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        accumulate(outs, e, acc, v, Int(n), Int(eoff), Int(count), Int(block), Int(nacc))


# The mix, in two stages: one thread per (group, frame) sums the group's
# primal outputs; one thread per frame sums the groups and writes the
# residual at e[toff + i].
def group_mix_kernel(
    outs: Ptr[FaustFloat, MutAnyOrigin], gsum: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, groups: S32, count: S32, block: S32,
) -> None:
    var t = Int(global_idx.x)
    if t >= Int(groups) * Int(block):
        return
    var g = t // Int(block)
    var i = t % Int(block)
    if i >= Int(count):
        return
    var acc = FaustFloat(0.0)
    for v in range(g * GROUP, min(Int(n), (g + 1) * GROUP)):
        acc += outs[unsafe_offset=NOUT * v * Int(block) + i]
    gsum[unsafe_offset=g * Int(block) + i] = acc


def residual_kernel(
    gsum: Ptr[FaustFloat, MutAnyOrigin], target: Ptr[FaustFloat, MutAnyOrigin],
    e: Ptr[FaustFloat, MutAnyOrigin], groups: S32, count: S32, block: S32, toff: Int32,
) -> None:
    var i = Int(global_idx.x)
    if i >= Int(count):
        return
    var s = FaustFloat(0.0)
    for g in range(Int(groups)):
        s += gsum[unsafe_offset=g * Int(block) + i]
    e[unsafe_offset=Int(toff) + i] = target[unsafe_offset=Int(toff) + i] - s


# theta = (freq / bw0, log t60, gain / |gain0|[, gainc / |gain0|]); scale =
# (bw0, |gain0|), |gain0| the starting amplitude of the mode.
@always_inline
def store_params(
    params: Ptr[FaustFloat, MutAnyOrigin], theta: Ptr[FaustFloat, MutAnyOrigin],
    scale: Ptr[FaustFloat, MutAnyOrigin], v: Int, n: Int,
):
    var fmax = FaustFloat(0.45) * FaustFloat(RATE)
    var f = max(FaustFloat(20.0), min(fmax, theta[unsafe_offset=v] * scale[unsafe_offset=v]))
    var lt = max(FaustFloat(-4.6), min(FaustFloat(3.4), theta[unsafe_offset=n + v]))
    theta[unsafe_offset=v] = f / scale[unsafe_offset=v]
    theta[unsafe_offset=n + v] = lt
    params[unsafe_offset=v] = f
    params[unsafe_offset=n + v] = FaustFloat(exp(F32(lt)))
    for j in range(2, NP):
        params[unsafe_offset=j * n + v] = theta[unsafe_offset=j * n + v] * scale[unsafe_offset=n + v]


# The scale of parameter j of mode v in theta coordinates.
@always_inline
def coord_scale(
    params: Ptr[FaustFloat, MutAnyOrigin], scale: Ptr[FaustFloat, MutAnyOrigin], j: Int, v: Int, n: Int,
) -> FaustFloat:
    if j == 0:
        return scale[unsafe_offset=v]
    if j == 1:
        return params[unsafe_offset=n + v]
    return scale[unsafe_offset=n + v]


# Adam on the loss gradient in theta coordinates; bc1, bc2 the bias
# corrections of this step; gscale = -2 / sum target^2.
def adam_kernel(
    params: Ptr[FaustFloat, MutAnyOrigin], theta: Ptr[FaustFloat, MutAnyOrigin],
    m1: Ptr[FaustFloat, MutAnyOrigin], m2: Ptr[FaustFloat, MutAnyOrigin],
    scale: Ptr[FaustFloat, MutAnyOrigin], acc: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, lr: F32, bc1: F32, bc2: F32, gscale: F32,
) -> None:
    var v = Int(global_idx.x)
    var nn = Int(n)
    if v >= nn:
        return
    for j in range(NP):
        var g = FaustFloat(gscale) * coord_scale(params, scale, j, v, nn) * acc[unsafe_offset=j * nn + v]
        var k = j * nn + v
        var a = FaustFloat(BETA1) * m1[unsafe_offset=k] + FaustFloat(1.0 - BETA1) * g
        var b = FaustFloat(BETA2) * m2[unsafe_offset=k] + FaustFloat(1.0 - BETA2) * g * g
        m1[unsafe_offset=k] = a
        m2[unsafe_offset=k] = b
        var mh = a / FaustFloat(bc1)
        var vh = b / FaustFloat(bc2)
        theta[unsafe_offset=k] -= FaustFloat(lr) * mh / (sqrt(vh) + FaustFloat(1e-20))
    store_params(params, theta, scale, v, nn)


# Damped Gauss-Newton on the mode's own NP parameters, against the shared
# residual: (A + lambda I) z = b in the Jacobi-normalised coordinates, solved
# by Cholesky (A + lambda I is symmetric positive definite).
def gn_kernel(
    params: Ptr[FaustFloat, MutAnyOrigin], theta: Ptr[FaustFloat, MutAnyOrigin],
    scale: Ptr[FaustFloat, MutAnyOrigin], acc: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, eta: F32, lam: F32,
) -> None:
    var v = Int(global_idx.x)
    var nn = Int(n)
    if v >= nn:
        return
    var c = Arr[FaustFloat, NP](fill=FaustFloat(0.0))
    var d = Arr[FaustFloat, NP](fill=FaustFloat(0.0))
    var r = Arr[FaustFloat, NP](fill=FaustFloat(0.0))
    var m = Arr[FaustFloat, NP * NP](fill=FaustFloat(0.0))
    for j in range(NP):
        c[j] = coord_scale(params, scale, j, v, nn)
    # J'J in theta coordinates, then its diagonal.
    var g = NP
    for p in range(NP):
        for q in range(p, NP):
            var h = c[p] * c[q] * acc[unsafe_offset=g * nn + v]
            m[p * NP + q] = h
            m[q * NP + p] = h
            g += 1
    for j in range(NP):
        if not (m[j * NP + j] > FaustFloat(0.0)):
            return
        d[j] = sqrt(m[j * NP + j])
    for p in range(NP):
        r[p] = c[p] * acc[unsafe_offset=p * nn + v] / d[p]
        for q in range(NP):
            m[p * NP + q] = m[p * NP + q] / (d[p] * d[q])
        m[p * NP + p] = FaustFloat(1.0) + FaustFloat(lam)
    # Cholesky: m = L L' (L in the lower triangle of m), then the two
    # triangular solves into r.
    for p in range(NP):
        for q in range(p + 1):
            var s = m[p * NP + q]
            for k in range(q):
                s -= m[p * NP + k] * m[q * NP + k]
            if p == q:
                if not (s > FaustFloat(1e-12)):
                    return
                m[p * NP + p] = sqrt(s)
            else:
                m[p * NP + q] = s / m[q * NP + q]
    for p in range(NP):
        var s = r[p]
        for k in range(p):
            s -= m[p * NP + k] * r[k]
        r[p] = s / m[p * NP + p]
    for pp in range(NP):
        var p = NP - 1 - pp
        var s = r[p]
        for k in range(p + 1, NP):
            s -= m[k * NP + p] * r[k]
        r[p] = s / m[p * NP + p]
    for j in range(NP):
        var bound = FaustFloat(0.2) if j == 1 else FaustFloat(0.5)
        var step = max(-bound, min(bound, FaustFloat(eta) * r[j] / d[j]))
        theta[unsafe_offset=j * nn + v] += step
    store_params(params, theta, scale, v, nn)


def grid(n: Int) -> Int:
    return (n + GPU_BLOCK_DIM - 1) // GPU_BLOCK_DIM


# ------------------------------------------------------------------- the bank

# A deterministic value in [0, 1) from two integers (a small integer hash;
# archs/modalfadcpu.cpp computes the same).
def hash01(a: Int, b: Int) -> F64:
    var x = UInt64(a) * UInt64(0x9E3779B97F4A7C15) ^ UInt64(b + 1) * UInt64(0xC2B2AE3D27D4EB4F)
    x ^= x >> 31
    x *= UInt64(0xBF58476D1CE4E5B9)
    x ^= x >> 29
    return F64(Int(x >> 11)) / 9007199254740992.0


def gauss(a: Int, b: Int) -> F64:
    var u1 = max(hash01(a, b), 1e-12)
    var u2 = hash01(a, b + 1)
    return sqrt(-2.0 * log(u1)) * cos(TWO_PI * u2)


def bandwidth(t60: F64) -> F64:
    return LN1000 / (3.141592653589793 * t60)


struct Bank(Copyable, Movable):
    var n: Int
    var freq: List[F64]
    var t60: List[F64]
    var gain: List[F64]
    # The cosine amplitudes (NP == 4); zeros otherwise.
    var gainc: List[F64]

    def __init__(out b, n: Int):
        b.n = n
        b.freq = List[F64]()
        b.t60 = List[F64]()
        b.gain = List[F64]()
        b.gainc = List[F64]()

    # The amplitude of mode k.
    def amplitude(self, k: Int) -> F64:
        return sqrt(self.gain[k] * self.gain[k] + self.gainc[k] * self.gainc[k])


# The hidden bank of n modes.
def make_bank(n: Int) -> Bank:
    var b = Bank(n)
    var step = (FHI - FLO) / F64(n)
    var level = 1.0 / sqrt(F64(n))
    for k in range(n):
        var f = FLO + step * (F64(k) + 0.5 + 0.8 * (hash01(k, 1) - 0.5))
        var t = 2.5 * (f / FLO) ** -0.35 * exp(0.6 * (hash01(k, 2) - 0.5))
        var g = level * 10.0 ** (-24.0 * hash01(k, 3) / 20.0)
        if hash01(k, 4) < 0.5:
            g = -g
        b.freq.append(f)
        b.t60.append(t)
        if NP == 4:
            # A phase of its own: the amplitude split between sine and cosine.
            var phi = TWO_PI * hash01(k, 5)
            b.gain.append(g * cos(phi))
            b.gainc.append(g * sin(phi))
        else:
            b.gain.append(g)
            b.gainc.append(0.0)
    return b^


# The starting point of a fit: the hidden bank, `sigma` bandwidths away.
def perturbed(h: Bank, sigma: F64) -> Bank:
    var b = Bank(h.n)
    for k in range(h.n):
        b.freq.append(h.freq[k] + sigma * bandwidth(h.t60[k]) * gauss(k, 10))
        b.t60.append(h.t60[k] * exp(0.3 * sigma * gauss(k, 20)))
        b.gain.append(h.gain[k] * (1.0 + 0.3 * sigma * gauss(k, 30)))
        b.gainc.append(h.gainc[k] * (1.0 + 0.3 * sigma * gauss(k, 40)))
    return b^


def median(mut xs: List[Int]) -> Int:
    for i in range(1, len(xs)):
        var x = xs[i]
        var j = i - 1
        while j >= 0 and xs[j] > x:
            xs[j + 1] = xs[j]
            j -= 1
        xs[j + 1] = x
    return xs[len(xs) // 2]


# The closed form of the voice in f64: y, dy/dfreq, dy/dt60, dy/dgain and,
# with NP == 4, dy/dgainc, at sample i (the impulse at i = 0).
def closed_form(f: F64, t60: F64, g: F64, gc: F64, i: Int, c: Int) -> F64:
    var w = TWO_PI * f / F64(RATE)
    var lr = -LN1000 / (t60 * F64(RATE))
    var ni = F64(i)
    var env = exp(lr * ni)
    var s = env * sin(w * ni)
    var co = env * cos(w * ni)
    if c == 0:
        return g * s + gc * co
    if c == 1:
        return env * (g * cos(w * ni) - gc * sin(w * ni)) * ni * TWO_PI / F64(RATE)
    if c == 2:
        return (g * s + gc * co) * ni * LN1000 / (t60 * t60 * F64(RATE))
    if c == 3:
        return s
    return co


# The control labels of the NP differentiated controls, in parameter order.
def control_labels() -> List[String]:
    var labels: List[String] = ["freq", "t60", "gain"]
    if NP == 4:
        labels.append("gainc")
    return labels^


# Raw file I/O through the C library.
def address[T: AnyType](p: Ptr[T, MUT_NOTRK]) -> UInt64:
    return UInt64(Int(p.__int__()))


def c_string(s: String) -> Ptr[UInt8, MUT_NOTRK]:
    var b = s.as_bytes()
    var p = unsafe_alloc[UInt8](len(b) + 1).unsafe_origin_cast[MUT_NOTRK]()
    for i in range(len(b)):
        p[unsafe_offset=i] = b[i]
    p[unsafe_offset=len(b)] = UInt8(0)
    return p


def read_bytes(path: String) raises -> List[UInt8]:
    var cpath = c_string(path)
    var cmode = c_string("rb")
    var f = external_call["fopen", UInt64](address(cpath), address(cmode))
    cpath.unsafe_free()
    cmode.unsafe_free()
    if f == 0:
        raise Error(String("cannot open ") + path)
    _ = external_call["fseek", Int32](f, Int64(0), Int32(2))
    var size = Int(external_call["ftell", Int64](f))
    _ = external_call["fseek", Int32](f, Int64(0), Int32(0))
    var buf = unsafe_alloc[UInt8](max(size, 1)).unsafe_origin_cast[MUT_NOTRK]()
    var got = Int(external_call["fread", UInt64](address(buf), UInt64(1), UInt64(size), f))
    _ = external_call["fclose", Int32](f)
    var out = List[UInt8](capacity=got)
    for i in range(got):
        out.append(buf[unsafe_offset=i])
    buf.unsafe_free()
    return out^


def write_f32(path: String, xs: List[F32]) raises:
    var cpath = c_string(path)
    var cmode = c_string("wb")
    var f = external_call["fopen", UInt64](address(cpath), address(cmode))
    cpath.unsafe_free()
    cmode.unsafe_free()
    if f == 0:
        raise Error(String("cannot open ") + path)
    var buf = unsafe_alloc[F32](max(len(xs), 1)).unsafe_origin_cast[MUT_NOTRK]()
    for i in range(len(xs)):
        buf[unsafe_offset=i] = xs[i]
    var written = external_call["fwrite", UInt64](address(buf), UInt64(4), UInt64(len(xs)), f)
    _ = external_call["fclose", Int32](f)
    buf.unsafe_free()
    if Int(written) != len(xs):
        raise Error(String("short write to ") + path)


def write_text(path: String, text: String) raises:
    var cpath = c_string(path)
    var cmode = c_string("wb")
    var f = external_call["fopen", UInt64](address(cpath), address(cmode))
    cpath.unsafe_free()
    cmode.unsafe_free()
    if f == 0:
        raise Error(String("cannot open ") + path)
    var b = text.as_bytes()
    var buf = unsafe_alloc[UInt8](max(len(b), 1)).unsafe_origin_cast[MUT_NOTRK]()
    for i in range(len(b)):
        buf[unsafe_offset=i] = b[i]
    _ = external_call["fwrite", UInt64](address(buf), UInt64(1), UInt64(len(b)), f)
    _ = external_call["fclose", Int32](f)
    buf.unsafe_free()


# A raw native-endian f32 file.
def read_f32(path: String) raises -> List[F32]:
    var bytes = read_bytes(path)
    var n = len(bytes) // 4
    var buf = unsafe_alloc[UInt8](max(4 * n, 4)).unsafe_origin_cast[MUT_NOTRK]()
    for i in range(4 * n):
        buf[unsafe_offset=i] = bytes[i]
    var fp = buf.unsafe_bitcast[F32]()
    var out = List[F32](capacity=n)
    for i in range(n):
        out.append(fp[unsafe_offset=i])
    buf.unsafe_free()
    return out^


# The modes of a text file: one line "freq t60 gain [gainc]" per mode.
def read_modes(path: String) raises -> Bank:
    var bytes = read_bytes(path)
    var text = String(unsafe_from_utf8=Span(bytes))
    var b = Bank(0)
    for line in text.split("\n"):
        var fields = List[String]()
        for w in String(line).split(" "):
            if String(w).byte_length() > 0:
                fields.append(String(w))
        if len(fields) < 3 or fields[0].startswith("#"):
            continue
        b.freq.append(atof(fields[0]))
        b.t60.append(atof(fields[1]))
        b.gain.append(atof(fields[2]))
        b.gainc.append(atof(fields[3]) if len(fields) > 3 and NP == 4 else 0.0)
    b.n = len(b.freq)
    return b^


comptime MODE_VALIDATE = 0
comptime MODE_BENCH = 1
comptime MODE_FIT = 2
comptime MODE_FILE = 3


# One forward-and-gradient pass over the signal with the current params.
# With `keep`, every block of outputs is appended to `kept` (validate).
def epoch(
    ctx: DeviceContext, dsps: DeviceBuffer[DType.uint8], state: DeviceBuffer[DType.uint8],
    params: DeviceBuffer[dfaust], acc: DeviceBuffer[dfaust], outs: DeviceBuffer[dfaust],
    chan: DeviceBuffer[DType.uint64], gsum: DeviceBuffer[dfaust], target: DeviceBuffer[dfaust],
    e: DeviceBuffer[dfaust], outs_host: HostBuffer[dfaust], index: DeviceBuffer[DType.int32],
    n: Int, block: Int, nblocks: Int, groups: Int, nacc: Int,
    keep: Bool, mut kept: List[FaustFloat],
) raises:
    ctx.enqueue_function[reset_kernel](
        dsps, state, params, acc, index, S32(n),
        grid_dim=grid(n), block_dim=GPU_BLOCK_DIM,
    )
    for b in range(nblocks):
        var eprev = Int32((b - 1) * block) if b > 0 else Int32(-1)
        ctx.enqueue_function[voice_kernel](
            dsps, state, outs, chan, e, acc, S32(n), S32(block), S32(block), eprev, Int32(nacc),
            grid_dim=grid(n), block_dim=GPU_BLOCK_DIM,
        )
        ctx.enqueue_function[group_mix_kernel](
            outs, gsum, S32(n), S32(groups), S32(block), S32(block),
            grid_dim=grid(groups * block), block_dim=GPU_BLOCK_DIM,
        )
        ctx.enqueue_function[residual_kernel](
            gsum, target, e, S32(groups), S32(block), S32(block), Int32(b * block),
            grid_dim=grid(block), block_dim=GPU_BLOCK_DIM,
        )
        if keep:
            ctx.enqueue_copy(dst_buf=outs_host, src_buf=outs)
            ctx.synchronize()
            for i in range(NOUT * n * block):
                kept.append(outs_host[i])
    ctx.enqueue_function[grad_kernel](
        outs, e, acc, S32(n), S32(block), S32(block), Int32((nblocks - 1) * block), Int32(nacc),
        grid_dim=grid(n), block_dim=GPU_BLOCK_DIM,
    )


def upload(
    ctx: DeviceContext, params: DeviceBuffer[dfaust], params_host: HostBuffer[dfaust], bank: Bank
) raises:
    ctx.synchronize()
    var n = bank.n
    for k in range(n):
        params_host[k] = FaustFloat(bank.freq[k])
        params_host[n + k] = FaustFloat(bank.t60[k])
        params_host[2 * n + k] = FaustFloat(bank.gain[k])
        if NP == 4:
            params_host[3 * n + k] = FaustFloat(bank.gainc[k])
    ctx.enqueue_copy(dst_buf=params, src_buf=params_host)


def loss_now(
    ctx: DeviceContext, e: DeviceBuffer[dfaust], e_host: HostBuffer[dfaust], frames: Int, energy: F64
) raises -> F64:
    ctx.enqueue_copy(dst_buf=e_host, src_buf=e)
    ctx.synchronize()
    var s = F64(0.0)
    for i in range(frames):
        s += F64(e_host[i]) * F64(e_host[i])
    return s / energy


# Everything runs here: set up the bank on the device, render the target from
# the hidden bank (or read it, MODE_FILE), then run `epochs` epochs from the
# perturbed start (or the modes of `modes_path`).
def session(
    n_arg: Int, block: Int, epochs: Int, sigma: F64, use_gn: Bool, lr: F64, mode: Int,
    frames_arg: Int = SIGNAL_FRAMES, target_path: String = "", modes_path: String = "",
    out_prefix: String = "",
) raises -> F64:
    var n = n_arg
    var frames = frames_arg
    var hidden = Bank(0)
    var start: Bank
    var recorded = List[F32]()
    if mode == MODE_FILE:
        recorded = read_f32(target_path)
        frames = (len(recorded) // block) * block
        start = read_modes(modes_path)
        n = start.n
        if n == 0 or frames == 0:
            raise Error("no mode or no signal")
    else:
        hidden = make_bank(n)
        start = perturbed(hidden, sigma)
    if frames % block != 0:
        raise Error("the block must divide the signal length")
    var nblocks = frames // block
    var groups = (n + GROUP - 1) // GROUP
    var nacc = NACC if use_gn else NP

    var ctx = DeviceContext()
    var state = ctx.enqueue_create_buffer[DType.uint8](max(modaldsp.state_bytes(n), 8))
    ctx.synchronize()
    var host = unsafe_alloc[modaldsp](1)
    host.unsafe_write(modaldsp(S32(n), state.unsafe_ptr().unsafe_origin_cast[MUT_NOTRK]()))
    host[].class_init(RATE)
    var gui = LabelGui()
    host[].build_user_interface(gui)
    var labels = control_labels()
    var index = ctx.enqueue_create_buffer[DType.int32](NP)
    var index_host = ctx.enqueue_create_host_buffer[DType.int32](NP)
    var dsps = ctx.enqueue_create_buffer[DType.uint8](size_of[modaldsp]())
    ctx.enqueue_copy(dst_buf=dsps, src_ptr=host.unsafe_bitcast[UInt8]())
    ctx.enqueue_function[init_kernel](dsps, state, S32(n), RATE, grid_dim=grid(n), block_dim=GPU_BLOCK_DIM)
    var outs = ctx.enqueue_create_buffer[dfaust](NOUT * n * block)
    var chan = ctx.enqueue_create_buffer[DType.uint64](NOUT * n)
    ctx.enqueue_function[channels_kernel](chan, outs, S32(n), S32(block), grid_dim=grid(n), block_dim=GPU_BLOCK_DIM)
    var gsum = ctx.enqueue_create_buffer[dfaust](groups * block)
    var e = ctx.enqueue_create_buffer[dfaust](frames)
    var e_host = ctx.enqueue_create_host_buffer[dfaust](frames)
    var target = ctx.enqueue_create_buffer[dfaust](frames)
    var target_host = ctx.enqueue_create_host_buffer[dfaust](frames)
    var params = ctx.enqueue_create_buffer[dfaust](NP * n)
    var params_host = ctx.enqueue_create_host_buffer[dfaust](NP * n)
    var theta = ctx.enqueue_create_buffer[dfaust](NP * n)
    var theta_host = ctx.enqueue_create_host_buffer[dfaust](NP * n)
    var scale = ctx.enqueue_create_buffer[dfaust](2 * n)
    var scale_host = ctx.enqueue_create_host_buffer[dfaust](2 * n)
    var m1 = ctx.enqueue_create_buffer[dfaust](NP * n)
    var m2 = ctx.enqueue_create_buffer[dfaust](NP * n)
    var theta_prev = ctx.enqueue_create_buffer[dfaust](NP * n)
    var params_prev = ctx.enqueue_create_buffer[dfaust](NP * n)
    var acc = ctx.enqueue_create_buffer[dfaust](NACC * n)
    var acc_host = ctx.enqueue_create_host_buffer[dfaust](NACC * n)
    var outs_host = ctx.enqueue_create_host_buffer[dfaust](NOUT * n * block)
    ctx.synchronize()
    for j in range(NP):
        index_host[j] = Int32(index_of(gui.labels, labels[j]))
    ctx.enqueue_copy(dst_buf=index, src_buf=index_host)

    # The target: read, or the hidden bank against a zero target (e = -mix).
    var none = List[FaustFloat]()
    ctx.synchronize()
    var energy = F64(0.0)
    if mode == MODE_FILE:
        for i in range(frames):
            target_host[i] = FaustFloat(recorded[i])
            energy += F64(recorded[i]) * F64(recorded[i])
        ctx.enqueue_copy(dst_buf=target, src_buf=target_host)
    else:
        for i in range(frames):
            target_host[i] = FaustFloat(0.0)
        ctx.enqueue_copy(dst_buf=target, src_buf=target_host)
        upload(ctx, params, params_host, hidden)
        epoch(ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, index, n, block, nblocks, groups, nacc, False, none)
        ctx.enqueue_copy(dst_buf=e_host, src_buf=e)
        ctx.synchronize()
        for i in range(frames):
            target_host[i] = -e_host[i]
            energy += F64(e_host[i]) * F64(e_host[i])
        ctx.enqueue_copy(dst_buf=target, src_buf=target_host)

    # The start, and its coordinates.
    upload(ctx, params, params_host, start)
    ctx.synchronize()
    for k in range(n):
        var s_f = bandwidth(start.t60[k])
        var s_g = max(start.amplitude(k), 1e-9)
        scale_host[k] = FaustFloat(s_f)
        scale_host[n + k] = FaustFloat(s_g)
        theta_host[k] = FaustFloat(start.freq[k] / s_f)
        theta_host[n + k] = FaustFloat(log(start.t60[k]))
        theta_host[2 * n + k] = FaustFloat(start.gain[k] / s_g)
        if NP == 4:
            theta_host[3 * n + k] = FaustFloat(start.gainc[k] / s_g)
    ctx.enqueue_copy(dst_buf=scale, src_buf=scale_host)
    ctx.enqueue_copy(dst_buf=theta, src_buf=theta_host)
    # A host buffer is reused only once its copy has run.
    ctx.synchronize()
    for k in range(NP * n):
        theta_host[k] = FaustFloat(0.0)
    ctx.enqueue_copy(dst_buf=m1, src_buf=theta_host)
    ctx.enqueue_copy(dst_buf=m2, src_buf=theta_host)
    ctx.synchronize()

    var gscale = F32(-2.0 / energy)
    var result: F64

    if mode == MODE_VALIDATE:
        var kept = List[FaustFloat]()
        epoch(ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, index, n, block, nblocks, groups, nacc, True, kept)
        ctx.enqueue_copy(dst_buf=acc_host, src_buf=acc)
        ctx.enqueue_copy(dst_buf=e_host, src_buf=e)
        ctx.synchronize()
        # The CPU reference: one scalar instance per mode.
        var cpu = List[FaustFloat]()
        for _ in range(NOUT * n * frames):
            cpu.append(FaustFloat(0.0))
        var buf = unsafe_alloc[FaustFloat](NOUT * block)
        var heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](NOUT)
        for c in range(NOUT):
            heads.unsafe_offset(c).unsafe_write(buf.unsafe_offset(c * block).unsafe_origin_cast[MUT_NOTRK]())
        var inst = unsafe_alloc[refdsp](1)
        for k in range(n):
            inst.unsafe_write(refdsp())
            inst[].init(RATE)
            var zg = ZoneGui()
            inst[].build_user_interface(zg)
            zg.zones[index_of(zg.labels, "freq")][] = FaustFloat(start.freq[k])
            zg.zones[index_of(zg.labels, "t60")][] = FaustFloat(start.t60[k])
            zg.zones[index_of(zg.labels, "gain")][] = FaustFloat(start.gain[k])
            if NP == 4:
                zg.zones[index_of(zg.labels, "gainc")][] = FaustFloat(start.gainc[k])
            for b in range(nblocks):
                inst[].compute(
                    S32(block),
                    heads.unsafe_origin_cast[MUT_NOTRK]().unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]](),
                    heads.unsafe_origin_cast[MUT_NOTRK](),
                )
                for c in range(NOUT):
                    for i in range(block):
                        cpu[(c * n + k) * frames + b * block + i] = buf[unsafe_offset=c * block + i]
        # Outputs: GPU against CPU and against the closed form, per channel,
        # relative to the channel's peak over all modes.
        var worst = F64(0.0)
        for c in range(NOUT):
            var peak = F64(0.0)
            var d_cpu = F64(0.0)
            var d_cf = F64(0.0)
            for k in range(n):
                for b in range(nblocks):
                    for i in range(block):
                        var t = b * block + i
                        var got = F64(kept[b * NOUT * n * block + (NOUT * k + c) * block + i])
                        var want = F64(cpu[(c * n + k) * frames + t])
                        var cf = closed_form(start.freq[k], start.t60[k], start.gain[k], start.gainc[k], t, c)
                        peak = max(peak, abs(cf))
                        d_cpu = max(d_cpu, abs(got - want))
                        d_cf = max(d_cf, abs(got - cf))
            print(
                "output", c, "peak", peak, "gpu_vs_cpu", d_cpu / peak, "gpu_vs_closed_form", d_cf / peak,
            )
            worst = max(worst, max(d_cpu, d_cf) / peak)
        # The residual and the gradient sums against the CPU outputs, in f64.
        var d_e = F64(0.0)
        var peak_e = F64(0.0)
        var ecpu = List[F64]()
        for t in range(frames):
            var s = F64(0.0)
            for k in range(n):
                s += F64(cpu[k * frames + t])
            var x = F64(target_host[t]) - s
            ecpu.append(x)
            peak_e = max(peak_e, abs(x))
            d_e = max(d_e, abs(F64(e_host[t]) - x))
        print("residual peak", peak_e, "gpu_vs_cpu", d_e / peak_e)
        worst = max(worst, d_e / peak_e)
        for j in range(NP):
            var d_g = F64(0.0)
            var peak_g = F64(0.0)
            for k in range(n):
                var s = F64(0.0)
                for t in range(frames):
                    s += ecpu[t] * F64(cpu[((j + 1) * n + k) * frames + t])
                peak_g = max(peak_g, abs(s))
                d_g = max(d_g, abs(F64(acc_host[j * n + k]) - s))
            print("gradient", j, "peak", peak_g, "gpu_vs_cpu_f64", d_g / peak_g)
            worst = max(worst, d_g / peak_g)
        print("modes", n, "frames", frames, "loss_at_start", loss_now(ctx, e, e_host, frames, energy), "worst_relative", worst)
        buf.unsafe_free()
        heads.unsafe_free()
        inst.unsafe_free()
        result = worst
    else:
        var times = List[Int]()
        var lr_f = lr if lr > 0.0 else 0.1
        var eta = lr if lr > 0.0 else 1.0
        var lam = 0.1
        var log_every = max(1, epochs // 20)
        # The loss of the start (no step), and with `bench` the sum of the
        # gradient sums' magnitudes, which archs/modalfadcpu.cpp prints too.
        epoch(
            ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, index,
            n, block, nblocks, groups, nacc, False, none,
        )
        var l0 = loss_now(ctx, e, e_host, frames, energy)
        if mode == MODE_BENCH:
            ctx.enqueue_copy(dst_buf=acc_host, src_buf=acc)
            ctx.synchronize()
            var gsum_abs = F64(0.0)
            for i in range(NP * n):
                gsum_abs += abs(F64(acc_host[i]))
            print("loss_at_start", l0, "grad_abs_sum", gsum_abs)
        # With `fit`, the Gauss-Newton step is globalised: when the loss of
        # the stepped parameters is above the best so far, the step is undone
        # and the factor halved; otherwise it grows by 25 % up to `lr`.
        var eta_now = eta
        var best = F64(1e300)
        for ep in range(epochs):
            var t0 = perf_counter_ns()
            # The pass at the parameters after `ep` steps.
            epoch(
                ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, index,
                n, block, nblocks, groups, nacc, False, none,
            )
            var logged = (mode == MODE_FIT or mode == MODE_FILE) and ep % log_every == 0
            var lnow = F64(-1.0)
            var fitting = mode == MODE_FIT or mode == MODE_FILE
            if fitting and (use_gn or logged):
                lnow = loss_now(ctx, e, e_host, frames, energy)
            if logged:
                print(
                    "step", ep, "loss", lnow, "snr_db", -10.0 * log10(max(lnow, 1e-30)),
                    "eta", eta_now if use_gn else F64(0.0),
                )
            if use_gn:
                var take = True
                if fitting:
                    if lnow > best * (1.0 + 1e-6):
                        take = False
                        ctx.enqueue_copy(dst_buf=theta, src_buf=theta_prev)
                        ctx.enqueue_copy(dst_buf=params, src_buf=params_prev)
                        eta_now *= 0.5
                    else:
                        best = lnow
                        ctx.enqueue_copy(dst_buf=theta_prev, src_buf=theta)
                        ctx.enqueue_copy(dst_buf=params_prev, src_buf=params)
                        eta_now = min(eta, eta_now * 1.25)
                if take:
                    ctx.enqueue_function[gn_kernel](
                        params, theta, scale, acc, S32(n), F32(eta_now), F32(lam),
                        grid_dim=grid(n), block_dim=GPU_BLOCK_DIM,
                    )
            else:
                # Cosine decay of the rate to 1 % over the run.
                var frac = F64(ep) / F64(max(epochs - 1, 1))
                var r = 0.01 + 0.99 * 0.5 * (1.0 + cos(3.141592653589793 * frac))
                var bc1 = 1.0 - BETA1 ** F64(ep + 1)
                var bc2 = 1.0 - BETA2 ** F64(ep + 1)
                ctx.enqueue_function[adam_kernel](
                    params, theta, m1, m2, scale, acc, S32(n),
                    F32(lr_f * r), F32(bc1), F32(bc2), gscale,
                    grid_dim=grid(n), block_dim=GPU_BLOCK_DIM,
                )
            ctx.synchronize()
            times.append(Int(perf_counter_ns() - t0))
        var warm = List[Int]()
        for i in range(min(3, len(times) - 1), len(times)):
            warm.append(times[i])
        var med = F64(median(warm)) / 1e6
        print(
            "modes", n, "block", block, "frames", frames, "update", "gn" if use_gn else "adam",
            "epoch_ms", med, "ns_per_mode_sample", med * 1e6 / (F64(n) * F64(frames)),
        )
        result = med
        if mode == MODE_FIT or mode == MODE_FILE:
            # A last pass for the loss after the last step (the best
            # parameters again if that step was worse), then recovery mode by
            # mode, or the files of `fit-file`.
            epoch(ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, index, n, block, nblocks, groups, nacc, False, none)
            var lf = loss_now(ctx, e, e_host, frames, energy)
            if use_gn and lf > best:
                ctx.enqueue_copy(dst_buf=params, src_buf=params_prev)
                epoch(ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, index, n, block, nblocks, groups, nacc, False, none)
                lf = loss_now(ctx, e, e_host, frames, energy)
            print("step", epochs, "loss", lf, "snr_db", -10.0 * log10(max(lf, 1e-30)))
            ctx.enqueue_copy(dst_buf=params_host, src_buf=params)
            ctx.synchronize()
            if mode == MODE_FILE:
                var text = String("# freq t60 gain gainc\n")
                var moved = List[F64]()
                var silenced = 0
                for k in range(n):
                    var f = F64(params_host[k])
                    var a = F64(params_host[2 * n + k])
                    var ac = F64(params_host[3 * n + k]) if NP == 4 else 0.0
                    text += String(f) + " " + String(F64(params_host[n + k])) + " " + String(a) + " " + String(ac) + "\n"
                    moved.append(abs(f - start.freq[k]) / bandwidth(start.t60[k]))
                    if sqrt(a * a + ac * ac) < 1e-3 * start.amplitude(k):
                        silenced += 1
                write_text(out_prefix + ".modes.txt", text)
                var model = List[F32](capacity=frames)
                var residual = List[F32](capacity=frames)
                for i in range(frames):
                    residual.append(F32(e_host[i]))
                    model.append(F32(target_host[i]) - F32(e_host[i]))
                write_f32(out_prefix + ".model.f32", model)
                write_f32(out_prefix + ".residual.f32", residual)
                # The median frequency move, in starting bandwidths.
                for i in range(1, len(moved)):
                    var x = moved[i]
                    var jj = i - 1
                    while jj >= 0 and moved[jj] > x:
                        moved[jj + 1] = moved[jj]
                        jj -= 1
                    moved[jj + 1] = x
                print(
                    "modes", n, "frames", frames, "snr_db from", -10.0 * log10(max(l0, 1e-30)),
                    "to", -10.0 * log10(max(lf, 1e-30)), "median freq move (bw)", moved[n // 2],
                    "max", moved[n - 1], "silenced", silenced, "wrote", out_prefix + ".{modes.txt,model.f32,residual.f32}",
                )
                host.unsafe_free()
                return lf
            # Recovered: frequency within 0.01 bandwidth, T60 and gain within
            # 1 % (strict), or within 0.1 bandwidth and 10 % (loose); the same
            # counts at the start, for reference.
            # Isolated: both neighbours at least 2 (bw + 1 / T) away, a mode
            # the signal resolves from its neighbours.
            var strict = 0
            var loose = 0
            var isolated = 0
            var iso_strict = 0
            var res = F64(RATE) / F64(frames)
            var start_strict = 0
            var start_loose = 0
            for k in range(n):
                var bw = bandwidth(hidden.t60[k])
                var ef = abs(F64(params_host[k]) - hidden.freq[k]) / bw
                var et = abs(F64(params_host[n + k]) / hidden.t60[k] - 1.0)
                var dg = F64(params_host[2 * n + k]) - hidden.gain[k]
                var dgc = F64(params_host[3 * n + k]) - hidden.gainc[k] if NP == 4 else 0.0
                var eg = sqrt(dg * dg + dgc * dgc) / hidden.amplitude(k)
                if ef < 0.01 and et < 0.01 and eg < 0.01:
                    strict += 1
                var gap = F64(1e9)
                if k > 0:
                    gap = min(gap, hidden.freq[k] - hidden.freq[k - 1])
                if k + 1 < n:
                    gap = min(gap, hidden.freq[k + 1] - hidden.freq[k])
                if gap >= 2.0 * (bw + res):
                    isolated += 1
                    if ef < 0.01 and et < 0.01 and eg < 0.01:
                        iso_strict += 1
                if ef < 0.1 and et < 0.1 and eg < 0.1:
                    loose += 1
                var sf = abs(start.freq[k] - hidden.freq[k]) / bw
                var st = abs(start.t60[k] / hidden.t60[k] - 1.0)
                var sdg = start.gain[k] - hidden.gain[k]
                var sdgc = start.gainc[k] - hidden.gainc[k]
                var sg = sqrt(sdg * sdg + sdgc * sdgc) / hidden.amplitude(k)
                if sf < 0.01 and st < 0.01 and sg < 0.01:
                    start_strict += 1
                if sf < 0.1 and st < 0.1 and sg < 0.1:
                    start_loose += 1
            print(
                "final loss", lf, "from", l0, "modes", n, "recovered strict", strict, "loose", loose,
                "(at start: strict", start_strict, "loose", String(start_loose) + ")",
                "isolated", isolated, "of which strict", iso_strict,
            )
            result = lf
    host.unsafe_free()
    return result


def main() raises -> None:
    comptime assert dfaust == f32, "the Apple GPU computes in f32: -D DFAUST=DType.float32"
    var args = argv()
    if len(args) < 2:
        raise Error(
            "usage: validate | bench <modes> [<block>] [adam|gn]"
            + " | fit <modes> <epochs> <sigma> [adam|gn] [<lr>] [<block>]"
            + " | fit-file <target.f32> <modes.txt> <epochs> <out> [adam|gn] [<lr>] [<block>]"
        )
    if args[1] == "validate":
        var worst = session(48, 256, 1, 1.0, True, 0.0, MODE_VALIDATE, 24064)
        if worst > 2e-3:
            raise Error("the GPU differs from the CPU or from the closed form")
    elif args[1] == "bench":
        var block = atol(args[3]) if len(args) > 3 else 1024
        var gn = len(args) > 4 and args[4] == "gn"
        _ = session(atol(args[2]), block, 23, 1.0, gn, 0.0, MODE_BENCH)
    elif args[1] == "fit":
        var gn = len(args) > 5 and args[5] == "gn"
        var lr = atof(args[6]) if len(args) > 6 else 0.0
        var block = atol(args[7]) if len(args) > 7 else 1024
        _ = session(atol(args[2]), block, atol(args[3]), atof(args[4]), gn, lr, MODE_FIT)
    elif args[1] == "fit-file":
        if len(args) < 6:
            raise Error("usage: fit-file <target.f32> <modes.txt> <epochs> <out> [adam|gn] [<lr>] [<block>]")
        var gn = not (len(args) > 6 and args[6] == "adam")
        var lr = atof(args[7]) if len(args) > 7 else 0.0
        var block = atol(args[8]) if len(args) > 8 else 1024
        _ = session(
            0, block, atol(args[4]), 0.0, gn, lr, MODE_FILE, 0,
            String(args[2]), String(args[3]), String(args[5]),
        )
    else:
        raise Error("unknown mode")
