
# ---------------------------------------------------------------------------
# choirmojogpubatch.mojo — an additive choir on the GPU: S singers in four
# parts, every singer a stack of harmonics up to 20 kHz, every harmonic one
# GPU instance of tests/corpus/gpu_choir_partial.dsp (a sine with vibrato, a
# breath swell and a band of noise that follows the vibrato). A choir of 448
# singers is about 48,000 partials (porting/faust-gpu-audio-synthesis-
# 2026-11-07-en.md, §5).
#
# Concatenated after `impulsemojo.mojo` and two generated programs:
# `choirdsp` (the batch contract of gpu_choir_partial.dsp) and `refdsp` (its
# scalar contract, one ordinary instance per partial, the CPU baseline).
# Built with `-D DFAUST=DType.float32`, linked with `-lportaudio`.
#
#   <binary> validate                         a small choir, GPU against CPU
#   <binary> bench <singers> [<block>]        GPU against the CPU threads
#   <binary> play <singers> <seconds> [audible] [<block>]
#
# The score: four chords of four parts (D minor, B flat, F, C; a common
# progression), 5 s each, looped. Singer s sings part s % 4, detuned by up to
# 15 cents, with its own vibrato (4.8 to 6.4 Hz, depth 0.4 to 0.9 %), breath
# (0.08 to 0.25 Hz) and place in the stereo field. Its partials are the
# harmonics of its part's lowest note up to 20 kHz (so that no partial is
# muted at that note), shaped by the five formants of the vowel "a" for its
# part (the Csound formant table), a source tilt, and a noise share that
# rises with frequency. A chord change moves every partial's frequency and
# gain; the program glides the gains over 80 ms.
#
# Per block on the GPU: the partials compute, each writing its own output; a
# first mix kernel sums each singer's partials (one thread per singer and
# frame), a second the panned singers (one thread per frame); only the stereo
# mix returns to the host.
#
# `validate` runs 8 singers (858 partials) for 12 s, across two chord changes,
# against the same choir of ordinary scalar instances on the CPU, and
# compares the two mixes sample by sample; it fails above 1e-3 of relative
# difference. (A frequency glide in the program made the devices drift apart
# after each change, see gpu_choir_partial.dsp; the program glides only the
# gains.) `play` fails on an underrun.
#
# Every buffer a kernel reaches is one of its arguments (Metal).
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext
from max.gpu import global_idx
from max.algorithm import parallelize
from std.ffi import external_call
from std.sys import argv
from std.time import perf_counter_ns

comptime GPU_BLOCK_DIM = 64
comptime RATE = S32(48000)
comptime WARMUP = 20
comptime FMAX = 20000.0
comptime CHORD_SECONDS = 5.0
comptime paFloat32 = UInt64(1)
comptime paOutputUnderflowed = Int32(-9980)

# The controls of a partial, in the order the host sets them.
comptime NCTL = 9


def control_name(k: Int) -> String:
    if k == 0:
        return "freq"
    if k == 1:
        return "gain"
    if k == 2:
        return "vrate"
    if k == 3:
        return "vdepth"
    if k == 4:
        return "brate"
    if k == 5:
        return "noise"
    if k == 6:
        return "bw"
    if k == 7:
        return "phase"
    return "seed"


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
        dsps.unsafe_bitcast[choirdsp]()[].instance_init(S32(v), rate)


def control_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    values: Ptr[FaustFloat, MutAnyOrigin], n: S32, index: Int32,
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        dsps.unsafe_bitcast[choirdsp]()[].set_control(S32(v), Int(index), values[unsafe_offset=v])


# Partial v writes its block at outs + v * block (no input).
def channels_kernel(
    channels: Ptr[UInt64, MutAnyOrigin], outs: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, block: S32,
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]()[unsafe_offset=v] = outs.unsafe_offset(
            v * Int(block)
        ).unsafe_origin_cast[MUT_NOTRK]()


def partial_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    outs: Ptr[FaustFloat, MutAnyOrigin], channels: Ptr[UInt64, MutAnyOrigin],
    n: S32, count: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(n):
        return
    var heads = channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]().unsafe_offset(v).unsafe_origin_cast[MUT_NOTRK]()
    dsps.unsafe_bitcast[choirdsp]()[].compute(
        S32(v), count,
        heads.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]().unsafe_mut_cast[False]().unsafe_origin_cast[IMM_NOTRK](),
        heads,
    )


# The mix, in two stages: one thread per (singer, frame) sums the singer's
# partials (starts[s] to starts[s + 1]); then one thread per frame sums the
# singers, panned and scaled.
def singer_mix_kernel(
    outs: Ptr[FaustFloat, MutAnyOrigin], starts: Ptr[Int32, MutAnyOrigin],
    singersum: Ptr[FaustFloat, MutAnyOrigin], singers: S32, count: S32, block: S32,
) -> None:
    var t = Int(global_idx.x)
    if t >= Int(singers) * Int(block):
        return
    var s = t // Int(block)
    var i = t % Int(block)
    if i >= Int(count):
        return
    var acc = FaustFloat(0.0)
    for v in range(Int(starts[unsafe_offset=s]), Int(starts[unsafe_offset=s + 1])):
        acc += outs[unsafe_offset=v * Int(block) + i]
    singersum[unsafe_offset=s * Int(block) + i] = acc


def mix_kernel(
    singersum: Ptr[FaustFloat, MutAnyOrigin], pans: Ptr[FaustFloat, MutAnyOrigin],
    mix: Ptr[FaustFloat, MutAnyOrigin], singers: S32, count: S32, block: S32,
) -> None:
    var i = Int(global_idx.x)
    if i >= Int(count):
        return
    var left = FaustFloat(0.0)
    var right = FaustFloat(0.0)
    for s in range(Int(singers)):
        var x = singersum[unsafe_offset=s * Int(block) + i]
        left += x * pans[unsafe_offset=2 * s]
        right += x * pans[unsafe_offset=2 * s + 1]
    mix[unsafe_offset=2 * i] = left
    mix[unsafe_offset=2 * i + 1] = right


def grid(n: Int) -> Int:
    return (n + GPU_BLOCK_DIM - 1) // GPU_BLOCK_DIM


# ------------------------------------------------------------------ the choir

# A deterministic value in [0, 1) from two integers (a small integer hash).
def hash01(a: Int, b: Int) -> F64:
    var x = UInt64(a) * UInt64(0x9E3779B97F4A7C15) ^ UInt64(b + 1) * UInt64(0xC2B2AE3D27D4EB4F)
    x ^= x >> 31
    x *= UInt64(0xBF58476D1CE4E5B9)
    x ^= x >> 29
    return F64(Int(x >> 11)) / 9007199254740992.0


def midi_hz(m: F64) -> F64:
    return 440.0 * 2.0 ** ((m - 69.0) / 12.0)


# The chords, as MIDI notes of (bass, tenor, alto, soprano).
def chord_note(c: Int, part: Int) -> Int:
    var dm: List[Int] = [50, 57, 65, 74]
    var bb: List[Int] = [46, 58, 65, 74]
    var f: List[Int] = [41, 57, 65, 72]
    var cm: List[Int] = [48, 55, 64, 72]
    var k = c % 4
    if k == 0:
        return dm[part]
    if k == 1:
        return bb[part]
    if k == 2:
        return f[part]
    return cm[part]


def lowest_note(part: Int) -> Int:
    var lo = 127
    for c in range(4):
        lo = min(lo, chord_note(c, part))
    return lo


# The five formants of the vowel "a" for each part (bass, tenor, alto,
# soprano): centre and bandwidth in Hz, level in dB (the Csound formant
# table), flattened as part * 5 + formant.
def formant_centres() -> List[F64]:
    return [
        600.0, 1040.0, 2250.0, 2450.0, 2750.0,
        650.0, 1080.0, 2650.0, 2900.0, 3250.0,
        800.0, 1150.0, 2800.0, 3500.0, 4950.0,
        800.0, 1150.0, 2900.0, 3900.0, 4950.0,
    ]


def formant_widths() -> List[F64]:
    return [
        60.0, 70.0, 110.0, 120.0, 130.0,
        80.0, 90.0, 120.0, 130.0, 140.0,
        80.0, 90.0, 120.0, 130.0, 140.0,
        80.0, 90.0, 120.0, 130.0, 140.0,
    ]


def formant_levels() -> List[F64]:
    var db: List[F64] = [
        0.0, -7.0, -9.0, -9.0, -20.0,
        0.0, -6.0, -7.0, -8.0, -22.0,
        0.0, -4.0, -20.0, -36.0, -60.0,
        0.0, -6.0, -32.0, -20.0, -50.0,
    ]
    var a = List[F64]()
    for d in db:
        a.append(10.0 ** (d / 20.0))
    return a^


# Every partial's controls, and each singer's range of partials and pan.
struct Choir(Movable):
    var singers: Int
    var n: Int
    var starts: List[Int32]
    var part: List[Int]
    var detune: List[F64]
    var harmonic: List[Int]
    var singer_of: List[Int]
    var ctl: List[List[FaustFloat]]
    var pans: List[FaustFloat]
    var fc: List[F64]
    var fw: List[F64]
    var fa: List[F64]

    def __init__(out c, singers: Int):
        c.singers = singers
        c.starts = List[Int32]()
        c.part = List[Int]()
        c.detune = List[F64]()
        c.harmonic = List[Int]()
        c.singer_of = List[Int]()
        c.ctl = List[List[FaustFloat]]()
        for _ in range(NCTL):
            c.ctl.append(List[FaustFloat]())
        c.pans = List[FaustFloat]()
        c.fc = formant_centres()
        c.fw = formant_widths()
        c.fa = formant_levels()
        var v = 0
        for s in range(singers):
            var p = s % 4
            c.part.append(p)
            c.detune.append((hash01(s, 1) - 0.5) * 0.3)
            c.starts.append(Int32(v))
            var count = Int(FMAX / midi_hz(F64(lowest_note(p))))
            var vrate = 4.8 + 1.6 * hash01(s, 2)
            var vdepth = 0.004 + 0.005 * hash01(s, 3)
            var brate = 0.08 + 0.17 * hash01(s, 4)
            var phase = hash01(s, 5)
            for h in range(1, count + 1):
                c.harmonic.append(h)
                c.singer_of.append(s)
                c.ctl[2].append(FaustFloat(vrate))
                c.ctl[3].append(FaustFloat(vdepth))
                c.ctl[4].append(FaustFloat(brate))
                c.ctl[6].append(FaustFloat(0.02))
                c.ctl[7].append(FaustFloat(phase))
                c.ctl[8].append(FaustFloat(Int(hash01(s, 1000 + h) * 1000000.0)))
                c.ctl[0].append(FaustFloat(0.0))
                c.ctl[1].append(FaustFloat(0.0))
                c.ctl[5].append(FaustFloat(0.0))
                v += 1
            var angle = 1.5707963267948966 * hash01(s, 6)
            c.pans.append(FaustFloat(cos(angle)))
            c.pans.append(FaustFloat(sin(angle)))
        c.starts.append(Int32(v))
        c.n = v
        c.set_chord(0)

    # The gain of a harmonic at `hz` for part `p`: the five formants as
    # Lorentzian bumps over a small floor.
    def formant_gain(self, p: Int, hz: F64) -> F64:
        var g = 0.01
        for i in range(5):
            var x = (hz - self.fc[p * 5 + i]) / (self.fw[p * 5 + i] * 1.5)
            g += self.fa[p * 5 + i] / (1.0 + x * x)
        return g

    # Frequency, gain and noise share of every partial for chord `k`.
    def set_chord(mut c, k: Int):
        var level = 1.0 / sqrt(F64(c.singers))
        var f0s = List[F64]()
        for s in range(c.singers):
            f0s.append(midi_hz(F64(chord_note(k, c.part[s])) + c.detune[s]))
        for v in range(c.n):
            var s = c.singer_of[v]
            var p = c.part[s]
            var h = c.harmonic[v]
            var hz = F64(h) * f0s[s]
            var g = 0.0
            if hz < FMAX:
                # A source tilt of -9 dB per octave under the formants.
                g = level * 0.5 * c.formant_gain(p, hz) / F64(h) ** 1.5
            c.ctl[0][v] = FaustFloat(hz)
            c.ctl[1][v] = FaustFloat(g)
            c.ctl[5][v] = FaustFloat(min(0.9, 0.08 + 0.6 * hz / FMAX))


comptime MODE_VALIDATE = 0
comptime MODE_BENCH = 1
comptime MODE_PLAY = 2


def address[T: AnyType](p: Ptr[T, MUT_NOTRK]) -> UInt64:
    return UInt64(Int(p.__int__()))


def median(mut xs: List[Int]) -> Int:
    for i in range(1, len(xs)):
        var x = xs[i]
        var j = i - 1
        while j >= 0 and xs[j] > x:
            xs[j + 1] = xs[j]
            j -= 1
        xs[j + 1] = x
    return xs[len(xs) // 2]


# The same choir of ordinary scalar instances on the CPU, the partials split
# in `workers` slices, each worker summing its partials, panned, into its own
# stereo buffer. `run(k, block)` computes block k; chord changes are applied
# at the same blocks as on the GPU.
struct CpuChoir:
    var n: Int
    var block: Int
    var workers: Int
    var insts: Ptr[refdsp, MUT_NOTRK]
    var zones: Ptr[Ptr[FaustFloat, MUT_NOTRK], MUT_NOTRK]
    var outs: Ptr[FaustFloat, MUT_NOTRK]
    var heads: Ptr[Ptr[FaustFloat, MUT_NOTRK], MUT_NOTRK]
    var acc: Ptr[FaustFloat, MUT_NOTRK]
    var mix: Ptr[FaustFloat, MUT_NOTRK]
    var pan: Ptr[FaustFloat, MUT_NOTRK]

    def __init__(out cc, choir: Choir, block: Int, workers: Int) raises:
        cc.n = choir.n
        cc.block = block
        cc.workers = workers
        cc.insts = unsafe_alloc[refdsp](cc.n).unsafe_origin_cast[MUT_NOTRK]()
        cc.zones = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](cc.n * NCTL).unsafe_origin_cast[MUT_NOTRK]()
        cc.outs = unsafe_alloc[FaustFloat](cc.n * block).unsafe_origin_cast[MUT_NOTRK]()
        cc.heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](cc.n).unsafe_origin_cast[MUT_NOTRK]()
        cc.acc = unsafe_alloc[FaustFloat](workers * 2 * block).unsafe_origin_cast[MUT_NOTRK]()
        cc.mix = unsafe_alloc[FaustFloat](2 * block).unsafe_origin_cast[MUT_NOTRK]()
        cc.pan = unsafe_alloc[FaustFloat](2 * cc.n).unsafe_origin_cast[MUT_NOTRK]()
        for v in range(cc.n):
            cc.insts.unsafe_offset(v).unsafe_write(refdsp())
            cc.insts.unsafe_offset(v)[].init(RATE)
            var zg = ZoneGui()
            cc.insts.unsafe_offset(v)[].build_user_interface(zg)
            for k in range(NCTL):
                cc.zones.unsafe_offset(v * NCTL + k).unsafe_write(zg.zones[index_of(zg.labels, control_name(k))])
            cc.heads.unsafe_offset(v).unsafe_write(cc.outs.unsafe_offset(v * block))
            var s = choir.singer_of[v]
            cc.pan[unsafe_offset=2 * v] = choir.pans[2 * s]
            cc.pan[unsafe_offset=2 * v + 1] = choir.pans[2 * s + 1]
        cc.set(choir)

    def set(mut cc, choir: Choir):
        for v in range(cc.n):
            for k in range(NCTL):
                cc.zones[unsafe_offset=v * NCTL + k][] = choir.ctl[k][v]

    def run(mut cc):
        var ip = cc.insts
        var hp = cc.heads
        var accp = cc.acc
        var panp = cc.pan
        var outp = cc.outs
        var n = cc.n
        var workers = cc.workers
        var block = cc.block

        def slice(w: Int) {var ip, var hp, var n, var workers, var block, var accp, var panp, var outp}:
            var a = accp.unsafe_offset(w * 2 * block)
            for i in range(2 * block):
                a[unsafe_offset=i] = FaustFloat(0.0)
            for v in range(n * w // workers, n * (w + 1) // workers):
                var h = hp.unsafe_offset(v)
                ip.unsafe_offset(v)[].compute(S32(block), h.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]().unsafe_mut_cast[False]().unsafe_origin_cast[IMM_NOTRK](), h)
                var l = panp[unsafe_offset=2 * v]
                var r = panp[unsafe_offset=2 * v + 1]
                var o = outp.unsafe_offset(v * block)
                for i in range(block):
                    a[unsafe_offset=2 * i] += o[unsafe_offset=i] * l
                    a[unsafe_offset=2 * i + 1] += o[unsafe_offset=i] * r

        if workers == 1:
            slice(0)
        else:
            parallelize(slice, workers)
        for i in range(2 * block):
            var x = FaustFloat(0.0)
            for w in range(workers):
                x += cc.acc[unsafe_offset=w * 2 * block + i]
            cc.mix[unsafe_offset=i] = x

    def free(mut cc):
        cc.insts.unsafe_free()
        cc.zones.unsafe_free()
        cc.outs.unsafe_free()
        cc.heads.unsafe_free()
        cc.acc.unsafe_free()
        cc.mix.unsafe_free()
        cc.pan.unsafe_free()


def chord_at(kb: Int, block: Int) -> Int:
    return Int(F64(kb) * F64(block) / (CHORD_SECONDS * F64(RATE)))


# The choir on the device, run for `blocks` blocks. MODE_VALIDATE compares the
# mix with the CPU choir block by block and returns the relative difference;
# MODE_BENCH returns the median block in ns; MODE_PLAY writes the mix to
# PortAudio and returns the number of underruns.
def run_choir(mut choir: Choir, block: Int, blocks: Int, mode: Int, audible: Bool) raises -> F64:
    var n = choir.n
    var singers = choir.singers
    var ctx = DeviceContext()
    var state = ctx.enqueue_create_buffer[DType.uint8](max(choirdsp.state_bytes(n), 8))
    ctx.synchronize()
    var host = unsafe_alloc[choirdsp](1)
    host.unsafe_write(choirdsp(S32(n), state.unsafe_ptr().unsafe_origin_cast[MUT_NOTRK]()))
    host[].class_init(RATE)
    var gui = LabelGui()
    host[].build_user_interface(gui)
    var index = List[Int]()
    for k in range(NCTL):
        index.append(index_of(gui.labels, control_name(k)))
    var dsps = ctx.enqueue_create_buffer[DType.uint8](size_of[choirdsp]())
    ctx.enqueue_copy(dst_buf=dsps, src_ptr=host.unsafe_bitcast[UInt8]())
    ctx.enqueue_function[init_kernel](dsps, state, S32(n), RATE, grid_dim=grid(n), block_dim=GPU_BLOCK_DIM)
    var outs = ctx.enqueue_create_buffer[dfaust](n * block)
    var singersum = ctx.enqueue_create_buffer[dfaust](singers * block)
    var chan = ctx.enqueue_create_buffer[DType.uint64](n)
    ctx.enqueue_function[channels_kernel](chan, outs, S32(n), S32(block), grid_dim=grid(n), block_dim=GPU_BLOCK_DIM)
    var starts = ctx.enqueue_create_buffer[DType.int32](singers + 1)
    var starts_host = ctx.enqueue_create_host_buffer[DType.int32](singers + 1)
    var pans = ctx.enqueue_create_buffer[dfaust](2 * singers)
    var pans_host = ctx.enqueue_create_host_buffer[dfaust](2 * singers)
    var values = ctx.enqueue_create_buffer[dfaust](n)
    var values_host = ctx.enqueue_create_host_buffer[dfaust](n)
    var mix = ctx.enqueue_create_buffer[dfaust](2 * block)
    var mix_host = ctx.enqueue_create_host_buffer[dfaust](2 * block)
    ctx.synchronize()
    for s in range(singers + 1):
        starts_host[s] = choir.starts[s]
    for i in range(2 * singers):
        pans_host[i] = choir.pans[i]
    ctx.enqueue_copy(dst_buf=starts, src_buf=starts_host)
    ctx.enqueue_copy(dst_buf=pans, src_buf=pans_host)

    # Every partial's controls, one control at a time; a chord change uploads
    # only freq, gain and noise (controls 0, 1 and 5).
    for k in range(NCTL):
        ctx.synchronize()
        for v in range(n):
            values_host[v] = choir.ctl[k][v]
        ctx.enqueue_copy(dst_buf=values, src_buf=values_host)
        ctx.enqueue_function[control_kernel](
            dsps, state, values, S32(n), Int32(index[k]), grid_dim=grid(n), block_dim=GPU_BLOCK_DIM
        )
    ctx.synchronize()
    var changing: List[Int] = [0, 1, 5]

    var cpu = CpuChoir(choir, block, 10) if mode == MODE_VALIDATE else CpuChoir(Choir(4), 1, 1)
    var diff = F64(0.0)
    var peak_ref = F64(0.0)

    var out = unsafe_alloc[F32](2 * block)
    var stream = UInt64(0)
    var underflows = 0
    var peak = F64(0.0)
    var latency_s = F64(-1.0)
    var warmup = 0 if mode == MODE_VALIDATE else WARMUP
    var times = List[Int]()
    var chord = 0
    for k in range(blocks + warmup):
        var kb = max(k - warmup, 0)
        var t0 = perf_counter_ns()
        var c = chord_at(kb, block)
        if c != chord:
            chord = c
            choir.set_chord(c)
            for k in changing:
                ctx.synchronize()
                for v in range(n):
                    values_host[v] = choir.ctl[k][v]
                ctx.enqueue_copy(dst_buf=values, src_buf=values_host)
                ctx.enqueue_function[control_kernel](
                    dsps, state, values, S32(n), Int32(index[k]), grid_dim=grid(n), block_dim=GPU_BLOCK_DIM
                )
            if mode == MODE_VALIDATE:
                cpu.set(choir)
        ctx.enqueue_function[partial_kernel](
            dsps, state, outs, chan, S32(n), S32(block), grid_dim=grid(n), block_dim=GPU_BLOCK_DIM
        )
        ctx.enqueue_function[singer_mix_kernel](
            outs, starts, singersum, S32(singers), S32(block), S32(block),
            grid_dim=grid(singers * block), block_dim=GPU_BLOCK_DIM,
        )
        ctx.enqueue_function[mix_kernel](
            singersum, pans, mix, S32(singers), S32(block), S32(block),
            grid_dim=grid(block), block_dim=GPU_BLOCK_DIM,
        )
        ctx.enqueue_copy(dst_buf=mix_host, src_buf=mix)
        ctx.synchronize()
        var t1 = perf_counter_ns()
        if k < warmup:
            continue
        times.append(Int(t1 - t0))
        if mode == MODE_VALIDATE:
            cpu.run()
            for i in range(2 * block):
                var got = F64(mix_host[i])
                var want = F64(cpu.mix[unsafe_offset=i])
                diff = max(diff, abs(got - want))
                peak_ref = max(peak_ref, abs(want))
        elif mode == MODE_PLAY:
            for i in range(2 * block):
                var x = F32(mix_host[i])
                peak = max(peak, abs(F64(x)))
                out[unsafe_offset=i] = x * F32(0.5) if audible else F32(0.0)
            if kb == 0:
                if external_call["Pa_Initialize", Int32]() != 0:
                    raise Error("Pa_Initialize failed")
                var params = unsafe_alloc[UInt64](4)
                params.unsafe_bitcast[Int32]()[unsafe_offset=0] = external_call["Pa_GetDefaultOutputDevice", Int32]()
                params.unsafe_bitcast[Int32]()[unsafe_offset=1] = Int32(2)
                params[unsafe_offset=1] = paFloat32
                params.unsafe_bitcast[F64]()[unsafe_offset=2] = 2.0 * F64(block) / F64(RATE)
                params[unsafe_offset=3] = UInt64(0)
                var err = external_call["Pa_OpenStream", Int32](
                    address(Ptr(to=stream).unsafe_origin_cast[MUT_NOTRK]()), UInt64(0),
                    address(params.unsafe_origin_cast[MUT_NOTRK]()),
                    F64(RATE), UInt64(block), UInt64(0), UInt64(0), UInt64(0),
                )
                params.unsafe_free()
                if err != 0:
                    raise Error(String("Pa_OpenStream failed: ") + String(err))
                if external_call["Pa_StartStream", Int32](stream) != 0:
                    raise Error("Pa_StartStream failed")
                var info = external_call["Pa_GetStreamInfo", UInt64](stream)
                if info != 0:
                    latency_s = Ptr[F64](unsafe_from_address=Int(info) + 16)[]
            var r = external_call["Pa_WriteStream", Int32](
                stream, address(out.unsafe_origin_cast[MUT_NOTRK]()), UInt64(block)
            )
            if r == paOutputUnderflowed:
                underflows += 1

    var result = F64(0.0)
    if mode == MODE_VALIDATE:
        print("singers", singers, "partials", n, "peak", peak_ref, "max_diff", diff, "relative", diff / peak_ref)
        result = diff / peak_ref
    elif mode == MODE_BENCH:
        result = F64(median(times))
    else:
        _ = external_call["Pa_StopStream", Int32](stream)
        _ = external_call["Pa_CloseStream", Int32](stream)
        _ = external_call["Pa_Terminate", Int32]()
        var worst = 0
        for t in times:
            worst = max(worst, t)
        print(
            "singers", singers, "partials", n, "block", block, "blocks", blocks,
            "underflows", underflows, "compute_median_us", F64(median(times)) / 1000.0,
            "compute_max_us", F64(worst) / 1000.0, "budget_us", F64(block) * 1e6 / F64(RATE),
            "peak", peak, "audible", audible, "output_latency_ms", latency_s * 1000.0,
        )
        result = F64(underflows)
    cpu.free()
    host.unsafe_free()
    out.unsafe_free()
    return result


# ----------------------------------------------------------------- the modes

def validate() raises:
    # 8 singers (two per part) over 12 s, two chord changes.
    var choir = Choir(8)
    var blocks = Int(12.0 * F64(RATE) / 256.0)
    if run_choir(choir, 256, blocks, MODE_VALIDATE, False) > 1e-3:
        raise Error("the GPU choir differs from the CPU choir")


# The median block of the CPU choir on `workers` threads (13 blocks, the
# first 3 dropped).
def cpu_block_ns(choir: Choir, block: Int, workers: Int) raises -> Int:
    var cc = CpuChoir(choir, block, workers)
    var times = List[Int]()
    for k in range(13):
        var t0 = perf_counter_ns()
        cc.run()
        var t1 = perf_counter_ns()
        if k >= 3:
            times.append(Int(t1 - t0))
    cc.free()
    return median(times)


def bench(singers: Int, block: Int) raises:
    var choir = Choir(singers)
    var g = Int(run_choir(choir, block, 100, MODE_BENCH, False))
    var c1 = cpu_block_ns(choir, block, 1)
    var c4 = cpu_block_ns(choir, block, 4)
    var c10 = cpu_block_ns(choir, block, 10)
    var budget = F64(block) * 1e6 / F64(RATE)
    print(
        "singers", singers, "partials", choir.n, "block", block, "budget_us", budget,
        "gpu_us", F64(g) / 1000.0, "cpu1_us", F64(c1) / 1000.0, "cpu4_us", F64(c4) / 1000.0,
        "cpu10_us", F64(c10) / 1000.0, "gpu_vs_cpu", F64(min(c1, min(c4, c10))) / F64(g),
        "gpu_percent", 100.0 * F64(g) / 1000.0 / budget,
        "cpu_best_percent", 100.0 * F64(min(c1, min(c4, c10))) / 1000.0 / budget,
    )


def play(singers: Int, seconds: F64, audible: Bool, block: Int) raises:
    var choir = Choir(singers)
    var blocks = Int(seconds * F64(RATE) / F64(block))
    var underflows = run_choir(choir, block, blocks, MODE_PLAY, audible)
    if underflows > 0:
        raise Error(String(Int(underflows)) + " underruns")


def main() raises -> None:
    comptime assert dfaust == f32, "the Apple GPU computes in f32: -D DFAUST=DType.float32"
    var args = argv()
    if len(args) < 2:
        raise Error("usage: validate | bench <singers> [<block>] | play <singers> <seconds> [audible] [<block>]")
    if args[1] == "validate":
        validate()
    elif args[1] == "bench":
        bench(atol(args[2]), atol(args[3]) if len(args) > 3 else 256)
    elif args[1] == "play":
        var audible = False
        var block = 256
        for i in range(4, len(args)):
            if args[i] == "audible":
                audible = True
            else:
                block = atol(args[i])
        play(atol(args[2]), atof(args[3]), audible, block)
    else:
        raise Error("unknown mode")
