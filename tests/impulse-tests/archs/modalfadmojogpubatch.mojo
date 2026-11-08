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
from std.sys import argv
from std.time import perf_counter_ns

comptime GPU_BLOCK_DIM = 64
comptime RATE = S32(48000)
comptime SIGNAL_FRAMES = 49152
comptime GROUP = 64
comptime NACC = 9
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
# first sample), its three controls set from `params` (freq, t60, gain, each
# n long), its sums zeroed.
def reset_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    params: Ptr[FaustFloat, MutAnyOrigin], acc: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, ifreq: Int32, it60: Int32, igain: Int32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(n):
        return
    ref d = dsps.unsafe_bitcast[modaldsp]()[]
    d.instance_clear(S32(v))
    d.set_control(S32(v), Int(ifreq), params[unsafe_offset=v])
    d.set_control(S32(v), Int(it60), params[unsafe_offset=Int(n) + v])
    d.set_control(S32(v), Int(igain), params[unsafe_offset=2 * Int(n) + v])
    for j in range(NACC):
        acc[unsafe_offset=j * Int(n) + v] = FaustFloat(0.0)


# Mode v writes its four outputs at outs + (4 v + c) * block.
def channels_kernel(
    channels: Ptr[UInt64, MutAnyOrigin], outs: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, block: S32,
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        for c in range(4):
            channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]()[unsafe_offset=4 * v + c] = outs.unsafe_offset(
                (4 * v + c) * Int(block)
            ).unsafe_origin_cast[MUT_NOTRK]()


# Mode v's sums over one block: e . dy/dtheta_j (j = 0..2) and, with
# nacc == 9, the six entries of its Gram matrix.
@always_inline
def accumulate(
    outs: Ptr[FaustFloat, MutAnyOrigin], e: Ptr[FaustFloat, MutAnyOrigin],
    acc: Ptr[FaustFloat, MutAnyOrigin], v: Int, n: Int, eoff: Int, count: Int, block: Int,
    nacc: Int,
):
    var base = 4 * v * block
    var a0 = FaustFloat(0.0)
    var a1 = FaustFloat(0.0)
    var a2 = FaustFloat(0.0)
    if nacc == 3:
        for i in range(count):
            var ee = e[unsafe_offset=eoff + i]
            a0 += ee * outs[unsafe_offset=base + block + i]
            a1 += ee * outs[unsafe_offset=base + 2 * block + i]
            a2 += ee * outs[unsafe_offset=base + 3 * block + i]
    else:
        var h00 = FaustFloat(0.0)
        var h11 = FaustFloat(0.0)
        var h22 = FaustFloat(0.0)
        var h01 = FaustFloat(0.0)
        var h02 = FaustFloat(0.0)
        var h12 = FaustFloat(0.0)
        for i in range(count):
            var ee = e[unsafe_offset=eoff + i]
            var d0 = outs[unsafe_offset=base + block + i]
            var d1 = outs[unsafe_offset=base + 2 * block + i]
            var d2 = outs[unsafe_offset=base + 3 * block + i]
            a0 += ee * d0
            a1 += ee * d1
            a2 += ee * d2
            h00 += d0 * d0
            h11 += d1 * d1
            h22 += d2 * d2
            h01 += d0 * d1
            h02 += d0 * d2
            h12 += d1 * d2
        acc[unsafe_offset=3 * n + v] += h00
        acc[unsafe_offset=4 * n + v] += h11
        acc[unsafe_offset=5 * n + v] += h22
        acc[unsafe_offset=6 * n + v] += h01
        acc[unsafe_offset=7 * n + v] += h02
        acc[unsafe_offset=8 * n + v] += h12
    acc[unsafe_offset=v] += a0
    acc[unsafe_offset=n + v] += a1
    acc[unsafe_offset=2 * n + v] += a2


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
    var heads = channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]().unsafe_offset(4 * v).unsafe_origin_cast[MUT_NOTRK]()
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
        acc += outs[unsafe_offset=4 * v * Int(block) + i]
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


# theta = (freq / bw0, log t60, gain / |gain0|); scale = (bw0, |gain0|).
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
    params[unsafe_offset=2 * n + v] = theta[unsafe_offset=2 * n + v] * scale[unsafe_offset=n + v]


# Adam on the loss gradient in theta coordinates; bc1, bc2 the bias
# corrections of this step; gscale = -2 / sum target^2.
def adam_kernel(
    params: Ptr[FaustFloat, MutAnyOrigin], theta: Ptr[FaustFloat, MutAnyOrigin],
    m1: Ptr[FaustFloat, MutAnyOrigin], m2: Ptr[FaustFloat, MutAnyOrigin],
    scale: Ptr[FaustFloat, MutAnyOrigin], acc: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, lr_f: F32, lr_t: F32, lr_g: F32, bc1: F32, bc2: F32, gscale: F32,
) -> None:
    var v = Int(global_idx.x)
    var nn = Int(n)
    if v >= nn:
        return
    var c0 = scale[unsafe_offset=v]
    var c1 = params[unsafe_offset=nn + v]
    var c2 = scale[unsafe_offset=nn + v]
    for j in range(3):
        var c = c0 if j == 0 else (c1 if j == 1 else c2)
        var lr = lr_f if j == 0 else (lr_t if j == 1 else lr_g)
        var g = FaustFloat(gscale) * c * acc[unsafe_offset=j * nn + v]
        var k = j * nn + v
        var a = FaustFloat(BETA1) * m1[unsafe_offset=k] + FaustFloat(1.0 - BETA1) * g
        var b = FaustFloat(BETA2) * m2[unsafe_offset=k] + FaustFloat(1.0 - BETA2) * g * g
        m1[unsafe_offset=k] = a
        m2[unsafe_offset=k] = b
        var mh = a / FaustFloat(bc1)
        var vh = b / FaustFloat(bc2)
        theta[unsafe_offset=k] -= FaustFloat(lr) * mh / (sqrt(vh) + FaustFloat(1e-20))
    store_params(params, theta, scale, v, nn)


# Damped Gauss-Newton on the mode's own three parameters, against the shared
# residual: (A + lambda I) z = b in the Jacobi-normalised coordinates.
def gn_kernel(
    params: Ptr[FaustFloat, MutAnyOrigin], theta: Ptr[FaustFloat, MutAnyOrigin],
    scale: Ptr[FaustFloat, MutAnyOrigin], acc: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, eta: F32, lam: F32,
) -> None:
    var v = Int(global_idx.x)
    var nn = Int(n)
    if v >= nn:
        return
    var c0 = scale[unsafe_offset=v]
    var c1 = params[unsafe_offset=nn + v]
    var c2 = scale[unsafe_offset=nn + v]
    # J'e and J'J in theta coordinates.
    var b0 = c0 * acc[unsafe_offset=v]
    var b1 = c1 * acc[unsafe_offset=nn + v]
    var b2 = c2 * acc[unsafe_offset=2 * nn + v]
    var h00 = c0 * c0 * acc[unsafe_offset=3 * nn + v]
    var h11 = c1 * c1 * acc[unsafe_offset=4 * nn + v]
    var h22 = c2 * c2 * acc[unsafe_offset=5 * nn + v]
    var h01 = c0 * c1 * acc[unsafe_offset=6 * nn + v]
    var h02 = c0 * c2 * acc[unsafe_offset=7 * nn + v]
    var h12 = c1 * c2 * acc[unsafe_offset=8 * nn + v]
    if not (h00 > FaustFloat(0.0) and h11 > FaustFloat(0.0) and h22 > FaustFloat(0.0)):
        return
    var d0 = sqrt(h00)
    var d1 = sqrt(h11)
    var d2 = sqrt(h22)
    var a00 = FaustFloat(1.0) + FaustFloat(lam)
    var a01 = h01 / (d0 * d1)
    var a02 = h02 / (d0 * d2)
    var a12 = h12 / (d1 * d2)
    var r0 = b0 / d0
    var r1 = b1 / d1
    var r2 = b2 / d2
    # Cramer's rule on the symmetric 3x3 (A + lambda I), diagonal a00.
    var m00 = a00 * a00 - a12 * a12
    var m01 = a01 * a00 - a12 * a02
    var m02 = a01 * a12 - a00 * a02
    var det = a00 * m00 - a01 * m01 + a02 * m02
    if not (abs(det) > FaustFloat(1e-12)):
        return
    var z0 = (r0 * m00 - a01 * (r1 * a00 - a12 * r2) + a02 * (r1 * a12 - a00 * r2)) / det
    var z1 = (a00 * (r1 * a00 - a12 * r2) - r0 * m01 + a02 * (a01 * r2 - r1 * a02)) / det
    var z2 = (a00 * (a00 * r2 - r1 * a12) - a01 * (a01 * r2 - r1 * a02) + r0 * m02) / det
    var s0 = max(FaustFloat(-0.5), min(FaustFloat(0.5), FaustFloat(eta) * z0 / d0))
    var s1 = max(FaustFloat(-0.2), min(FaustFloat(0.2), FaustFloat(eta) * z1 / d1))
    var s2 = max(FaustFloat(-0.5), min(FaustFloat(0.5), FaustFloat(eta) * z2 / d2))
    theta[unsafe_offset=v] += s0
    theta[unsafe_offset=nn + v] += s1
    theta[unsafe_offset=2 * nn + v] += s2
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

    def __init__(out b, n: Int):
        b.n = n
        b.freq = List[F64]()
        b.t60 = List[F64]()
        b.gain = List[F64]()


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
        b.gain.append(g)
    return b^


# The starting point of a fit: the hidden bank, `sigma` bandwidths away.
def perturbed(h: Bank, sigma: F64) -> Bank:
    var b = Bank(h.n)
    for k in range(h.n):
        b.freq.append(h.freq[k] + sigma * bandwidth(h.t60[k]) * gauss(k, 10))
        b.t60.append(h.t60[k] * exp(0.3 * sigma * gauss(k, 20)))
        b.gain.append(h.gain[k] * (1.0 + 0.3 * sigma * gauss(k, 30)))
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


# The closed form of the voice in f64: y, dy/dfreq, dy/dt60, dy/dgain at
# sample i (the impulse at i = 0).
def closed_form(f: F64, t60: F64, g: F64, i: Int, c: Int) -> F64:
    var w = TWO_PI * f / F64(RATE)
    var lr = -LN1000 / (t60 * F64(RATE))
    var ni = F64(i)
    var env = exp(lr * ni)
    var s = env * sin(w * ni)
    if c == 0:
        return g * s
    if c == 1:
        return g * env * cos(w * ni) * ni * TWO_PI / F64(RATE)
    if c == 2:
        return g * s * ni * LN1000 / (t60 * t60 * F64(RATE))
    return s


comptime MODE_VALIDATE = 0
comptime MODE_BENCH = 1
comptime MODE_FIT = 2


# One forward-and-gradient pass over the signal with the current params.
# With `keep`, every block of outputs is appended to `kept` (validate).
def epoch(
    ctx: DeviceContext, dsps: DeviceBuffer[DType.uint8], state: DeviceBuffer[DType.uint8],
    params: DeviceBuffer[dfaust], acc: DeviceBuffer[dfaust], outs: DeviceBuffer[dfaust],
    chan: DeviceBuffer[DType.uint64], gsum: DeviceBuffer[dfaust], target: DeviceBuffer[dfaust],
    e: DeviceBuffer[dfaust], outs_host: HostBuffer[dfaust],
    n: Int, block: Int, nblocks: Int, groups: Int, nacc: Int, ifreq: Int, it60: Int, igain: Int,
    keep: Bool, mut kept: List[FaustFloat],
) raises:
    ctx.enqueue_function[reset_kernel](
        dsps, state, params, acc, S32(n), Int32(ifreq), Int32(it60), Int32(igain),
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
            for i in range(4 * n * block):
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
# the hidden bank, then run `epochs` epochs from the perturbed start.
def session(
    n: Int, block: Int, epochs: Int, sigma: F64, use_gn: Bool, lr: F64, mode: Int,
    frames_arg: Int = SIGNAL_FRAMES,
) raises -> F64:
    var frames = frames_arg
    if frames % block != 0:
        raise Error("the block must divide the signal length")
    var nblocks = frames // block
    var groups = (n + GROUP - 1) // GROUP
    var nacc = 9 if use_gn else 3
    var hidden = make_bank(n)
    var start = perturbed(hidden, sigma)

    var ctx = DeviceContext()
    var state = ctx.enqueue_create_buffer[DType.uint8](max(modaldsp.state_bytes(n), 8))
    ctx.synchronize()
    var host = unsafe_alloc[modaldsp](1)
    host.unsafe_write(modaldsp(S32(n), state.unsafe_ptr().unsafe_origin_cast[MUT_NOTRK]()))
    host[].class_init(RATE)
    var gui = LabelGui()
    host[].build_user_interface(gui)
    var ifreq = index_of(gui.labels, "freq")
    var it60 = index_of(gui.labels, "t60")
    var igain = index_of(gui.labels, "gain")
    var dsps = ctx.enqueue_create_buffer[DType.uint8](size_of[modaldsp]())
    ctx.enqueue_copy(dst_buf=dsps, src_ptr=host.unsafe_bitcast[UInt8]())
    ctx.enqueue_function[init_kernel](dsps, state, S32(n), RATE, grid_dim=grid(n), block_dim=GPU_BLOCK_DIM)
    var outs = ctx.enqueue_create_buffer[dfaust](4 * n * block)
    var chan = ctx.enqueue_create_buffer[DType.uint64](4 * n)
    ctx.enqueue_function[channels_kernel](chan, outs, S32(n), S32(block), grid_dim=grid(n), block_dim=GPU_BLOCK_DIM)
    var gsum = ctx.enqueue_create_buffer[dfaust](groups * block)
    var e = ctx.enqueue_create_buffer[dfaust](frames)
    var e_host = ctx.enqueue_create_host_buffer[dfaust](frames)
    var target = ctx.enqueue_create_buffer[dfaust](frames)
    var target_host = ctx.enqueue_create_host_buffer[dfaust](frames)
    var params = ctx.enqueue_create_buffer[dfaust](3 * n)
    var params_host = ctx.enqueue_create_host_buffer[dfaust](3 * n)
    var theta = ctx.enqueue_create_buffer[dfaust](3 * n)
    var theta_host = ctx.enqueue_create_host_buffer[dfaust](3 * n)
    var scale = ctx.enqueue_create_buffer[dfaust](2 * n)
    var scale_host = ctx.enqueue_create_host_buffer[dfaust](2 * n)
    var m1 = ctx.enqueue_create_buffer[dfaust](3 * n)
    var m2 = ctx.enqueue_create_buffer[dfaust](3 * n)
    var theta_prev = ctx.enqueue_create_buffer[dfaust](3 * n)
    var params_prev = ctx.enqueue_create_buffer[dfaust](3 * n)
    var acc = ctx.enqueue_create_buffer[dfaust](NACC * n)
    var acc_host = ctx.enqueue_create_host_buffer[dfaust](NACC * n)
    var outs_host = ctx.enqueue_create_host_buffer[dfaust](4 * n * block)
    ctx.synchronize()

    # The target: the hidden bank against a zero target gives e = -mix.
    var none = List[FaustFloat]()
    ctx.synchronize()
    for i in range(frames):
        target_host[i] = FaustFloat(0.0)
    ctx.enqueue_copy(dst_buf=target, src_buf=target_host)
    upload(ctx, params, params_host, hidden)
    epoch(ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, n, block, nblocks, groups, nacc, ifreq, it60, igain, False, none)
    ctx.enqueue_copy(dst_buf=e_host, src_buf=e)
    ctx.synchronize()
    var energy = F64(0.0)
    for i in range(frames):
        target_host[i] = -e_host[i]
        energy += F64(e_host[i]) * F64(e_host[i])
    ctx.enqueue_copy(dst_buf=target, src_buf=target_host)

    # The start, and its coordinates.
    upload(ctx, params, params_host, start)
    ctx.synchronize()
    for k in range(n):
        var s_f = bandwidth(start.t60[k])
        var s_g = max(abs(start.gain[k]), 1e-9)
        scale_host[k] = FaustFloat(s_f)
        scale_host[n + k] = FaustFloat(s_g)
        theta_host[k] = FaustFloat(start.freq[k] / s_f)
        theta_host[n + k] = FaustFloat(log(start.t60[k]))
        theta_host[2 * n + k] = FaustFloat(start.gain[k] / s_g)
    ctx.enqueue_copy(dst_buf=scale, src_buf=scale_host)
    ctx.enqueue_copy(dst_buf=theta, src_buf=theta_host)
    # A host buffer is reused only once its copy has run.
    ctx.synchronize()
    for k in range(3 * n):
        theta_host[k] = FaustFloat(0.0)
    ctx.enqueue_copy(dst_buf=m1, src_buf=theta_host)
    ctx.enqueue_copy(dst_buf=m2, src_buf=theta_host)
    ctx.synchronize()

    var gscale = F32(-2.0 / energy)
    var result: F64

    if mode == MODE_VALIDATE:
        var kept = List[FaustFloat]()
        epoch(ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, n, block, nblocks, groups, nacc, ifreq, it60, igain, True, kept)
        ctx.enqueue_copy(dst_buf=acc_host, src_buf=acc)
        ctx.enqueue_copy(dst_buf=e_host, src_buf=e)
        ctx.synchronize()
        # The CPU reference: one scalar instance per mode.
        var cpu = List[FaustFloat]()
        for _ in range(4 * n * frames):
            cpu.append(FaustFloat(0.0))
        var buf = unsafe_alloc[FaustFloat](4 * block)
        var heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](4)
        for c in range(4):
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
            for b in range(nblocks):
                inst[].compute(
                    S32(block),
                    heads.unsafe_origin_cast[MUT_NOTRK]().unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]](),
                    heads.unsafe_origin_cast[MUT_NOTRK](),
                )
                for c in range(4):
                    for i in range(block):
                        cpu[(c * n + k) * frames + b * block + i] = buf[unsafe_offset=c * block + i]
        # Outputs: GPU against CPU and against the closed form, per channel,
        # relative to the channel's peak over all modes.
        var worst = F64(0.0)
        for c in range(4):
            var peak = F64(0.0)
            var d_cpu = F64(0.0)
            var d_cf = F64(0.0)
            for k in range(n):
                for b in range(nblocks):
                    for i in range(block):
                        var t = b * block + i
                        var got = F64(kept[b * 4 * n * block + (4 * k + c) * block + i])
                        var want = F64(cpu[(c * n + k) * frames + t])
                        var cf = closed_form(start.freq[k], start.t60[k], start.gain[k], t, c)
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
        for j in range(3):
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
            ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host,
            n, block, nblocks, groups, nacc, ifreq, it60, igain, False, none,
        )
        var l0 = loss_now(ctx, e, e_host, frames, energy)
        if mode != MODE_FIT:
            ctx.enqueue_copy(dst_buf=acc_host, src_buf=acc)
            ctx.synchronize()
            var gsum_abs = F64(0.0)
            for i in range(3 * n):
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
                ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host,
                n, block, nblocks, groups, nacc, ifreq, it60, igain, False, none,
            )
            var logged = mode == MODE_FIT and ep % log_every == 0
            var lnow = F64(-1.0)
            if mode == MODE_FIT and (use_gn or logged):
                lnow = loss_now(ctx, e, e_host, frames, energy)
            if logged:
                print("step", ep, "loss", lnow, "eta", eta_now if use_gn else F64(0.0))
            if use_gn:
                var take = True
                if mode == MODE_FIT:
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
                    F32(lr_f * r), F32(lr_f * r), F32(lr_f * r), F32(bc1), F32(bc2), gscale,
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
        if mode == MODE_FIT:
            # A last pass for the loss after the last step, then recovery
            # mode by mode.
            epoch(ctx, dsps, state, params, acc, outs, chan, gsum, target, e, outs_host, n, block, nblocks, groups, nacc, ifreq, it60, igain, False, none)
            var lf = loss_now(ctx, e, e_host, frames, energy)
            if use_gn and lf > best:
                ctx.enqueue_copy(dst_buf=params, src_buf=params_prev)
                lf = best
            print("step", epochs, "loss", lf)
            ctx.enqueue_copy(dst_buf=params_host, src_buf=params)
            ctx.synchronize()
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
                var eg = abs(F64(params_host[2 * n + k]) / hidden.gain[k] - 1.0)
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
                var sg = abs(start.gain[k] / hidden.gain[k] - 1.0)
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
    else:
        raise Error("unknown mode")
