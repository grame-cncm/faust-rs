
# ---------------------------------------------------------------------------
# bellsmojogpubatch.mojo — a carillon on the GPU: B bells, each a strike and
# the 50 modes of `pm.standardBellModel`, every mode its own GPU instance with
# its own frequency, decay and gain (porting/faust-gpu-audio-synthesis-
# 2026-11-07-en.md, modal synthesis).
#
# Concatenated after `impulsemojo.mojo`, the bell data that
# `tools/bell_modes.py` reads from the installed physmodels.lib, three
# generated programs: `strikedsp` and `modedsp` (the batch contract of
# tests/corpus/gpu_bell_strike.dsp and gpu_bell_mode.dsp) and `refdsp` (the
# scalar contract of gpu_bell_reference.dsp, the original `pm.standardBell`),
# and `realtimemojo.mojo`.
# Built with `-D DFAUST=DType.float32`, linked with `-lportaudio -framework
# CoreAudio`.
#
# `rt=`, `pipeline` and `latency=` are the real-time options of `play`
# (realtimemojo.mojo): the time-constraint policy for the loop's thread, the
# output device's IO workgroup, the GPU work submitted one block ahead, and
# the suggested output latency. `play` prints a second line, `timing`: each
# block split into enqueue, GPU wait, host work and `Pa_WriteStream`, the
# gaps between writes and the fill of PortAudio's ring.
#
#   <binary> validate                         one bell against pm.standardBell
#   <binary> bench <bells> [<block>]          GPU against the CPU threads
#   <binary> play <bells> <seconds> [audible] [<block>] [cloud|melody|chime]
#                 [rt=off|tc|wg|tc+wg] [pipeline] [latency=<ms>]
#
# The scores of `play`: `cloud` (the default) strikes bell b every 96 blocks,
# 7 blocks after bell b - 1, a dense cloud of bells; `melody` plays the two
# phrases of Beethoven's "Ode to Joy" and `chime` the Westminster Quarters
# (five changes on four bells, then four strikes of a low hour bell), both in
# the public domain, slow, looped. A note of semitone s (0 is the model, about
# B4) strikes the bell nearest to that pitch: with 37 bells, one per semitone
# over three octaves, every note has its own bell.
#
# `validate` fails above 1e-3 of relative difference (measured: 1.5e-4, the
# mode parameters being computed in f64 on the host where pm.standardBell
# computes them in f32, and the modes summed in another order); `play` fails
# on an underrun.
#
# Bell b of B: its size scales the model's frequencies by 2^(semitones/12),
# from an octave below the model to two octaves above (semitones -12 to +24);
# a smaller bell rings shorter (T60 divided by the scale); it is struck at
# position b % 7, which selects the model's row of mode gains. Modes above
# 0.45 times the sample rate are muted. Bell b sits at pan b / (B - 1).
#
# Per block on the GPU: the bells' gates are set; the B strikes compute; the
# 50 B modes compute, each reading its own bell's strike output directly (its
# channel table points into the strike data, no copy); a mix kernel sums the
# modes into two channels, each bell divided by 50 as the model divides.
#
# Every buffer a kernel reaches is one of its arguments (Metal).
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext, DeviceEvent, HostBuffer
from max.gpu import global_idx
from max.algorithm import parallelize
from std.ffi import external_call
from std.sys import argv
from std.time import perf_counter_ns

comptime GPU_BLOCK_DIM = 64
comptime RATE = S32(48000)
comptime WARMUP = 20
comptime paOutputUnderflowed = Int32(-9980)


# Records each control's label, in control-table order.
struct LabelGui(FaustGui):
    var labels: List[String]

    def __init__(out ui):
        ui.labels = List[String]()

    def add_button(mut ui, var label: String, mut zone: FaustFloat) -> None:
        ui.labels.append(label)

    def add_check_button(mut ui, var label: String, mut zone: FaustFloat) -> None:
        ui.labels.append(label)

    def add_vertical_slider(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.labels.append(label)

    def add_horizontal_slider(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.labels.append(label)

    def add_num_entry(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.labels.append(label)


def index_of(labels: List[String], label: String) raises -> Int:
    for i in range(len(labels)):
        if labels[i] == label:
            return i
    raise Error(String("no control `") + label + "`")


# Keeps the zones of the reference, by label.
struct ZoneGui(FaustGui):
    var labels: List[String]
    var zones: List[Ptr[FaustFloat, MUT_NOTRK]]

    def __init__(out ui):
        ui.labels = List[String]()
        ui.zones = List[Ptr[FaustFloat, MUT_NOTRK]]()

    def keep(mut ui, var label: String, mut zone: FaustFloat):
        ui.labels.append(label)
        ui.zones.append(Ptr(to=zone).unsafe_bitcast[FaustFloat]().unsafe_origin_cast[MUT_NOTRK]())

    def add_button(mut ui, var label: String, mut zone: FaustFloat) -> None:
        ui.keep(label, zone)

    def add_horizontal_slider(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.keep(label, zone)

    def add_num_entry(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.keep(label, zone)

    def set(mut ui, label: String, value: FaustFloat) raises:
        ui.zones[index_of(ui.labels, label)][] = value


# ------------------------------------------------------------------- kernels

def strike_init_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin], n: S32, rate: S32
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        dsps.unsafe_bitcast[strikedsp]()[].instance_init(S32(v), rate)


def mode_init_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin], n: S32, rate: S32
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        dsps.unsafe_bitcast[modedsp]()[].instance_init(S32(v), rate)


def strike_control_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    values: Ptr[FaustFloat, MutAnyOrigin], n: S32, index: Int32,
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        dsps.unsafe_bitcast[strikedsp]()[].set_control(S32(v), Int(index), values[unsafe_offset=v])


def mode_control_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    values: Ptr[FaustFloat, MutAnyOrigin], n: S32, index: Int32,
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        dsps.unsafe_bitcast[modedsp]()[].set_control(S32(v), Int(index), values[unsafe_offset=v])


# Strike b writes its block at strikes + b * block (no input).
def strike_channels_kernel(
    channels: Ptr[UInt64, MutAnyOrigin], strikes: Ptr[FaustFloat, MutAnyOrigin],
    n: S32, block: S32,
) -> None:
    var v = Int(global_idx.x)
    if v < Int(n):
        channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]()[unsafe_offset=v] = strikes.unsafe_offset(
            v * Int(block)
        ).unsafe_origin_cast[MUT_NOTRK]()


# Mode v reads its bell's strike (v / modes) and writes at outs + v * block.
def mode_channels_kernel(
    channels: Ptr[UInt64, MutAnyOrigin], strikes: Ptr[FaustFloat, MutAnyOrigin],
    outs: Ptr[FaustFloat, MutAnyOrigin], n: S32, modes: S32, block: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(n):
        return
    var heads = channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]().unsafe_offset(2 * v)
    heads[unsafe_offset=0] = strikes.unsafe_offset((v // Int(modes)) * Int(block)).unsafe_origin_cast[MUT_NOTRK]()
    heads[unsafe_offset=1] = outs.unsafe_offset(v * Int(block)).unsafe_origin_cast[MUT_NOTRK]()


def strike_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    strikes: Ptr[FaustFloat, MutAnyOrigin], channels: Ptr[UInt64, MutAnyOrigin],
    n: S32, count: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(n):
        return
    var heads = channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]().unsafe_offset(v).unsafe_origin_cast[MUT_NOTRK]()
    dsps.unsafe_bitcast[strikedsp]()[].compute(
        S32(v), count,
        heads.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]().unsafe_mut_cast[False]().unsafe_origin_cast[IMM_NOTRK](),
        heads,
    )


def mode_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin],
    strikes: Ptr[FaustFloat, MutAnyOrigin], outs: Ptr[FaustFloat, MutAnyOrigin],
    channels: Ptr[UInt64, MutAnyOrigin], n: S32, count: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(n):
        return
    var heads = channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]().unsafe_offset(2 * v).unsafe_origin_cast[MUT_NOTRK]()
    dsps.unsafe_bitcast[modedsp]()[].compute(
        S32(v), count,
        heads.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]().unsafe_mut_cast[False]().unsafe_origin_cast[IMM_NOTRK](),
        heads.unsafe_offset(1),
    )


# The mix, in two stages: one thread per (bell, frame) sums the bell's
# modes; then one thread per frame sums the bells, panned, each divided by
# `modes` as the model divides. A single stage, one thread per frame summing
# every mode, left 256 threads walking up to 51,200 modes each and made the
# carillon slower on the GPU than on the CPU.
def bell_mix_kernel(
    outs: Ptr[FaustFloat, MutAnyOrigin], bellsum: Ptr[FaustFloat, MutAnyOrigin],
    bells: S32, modes: S32, count: S32, block: S32,
) -> None:
    var t = Int(global_idx.x)
    if t >= Int(bells) * Int(block):
        return
    var b = t // Int(block)
    var i = t % Int(block)
    if i >= Int(count):
        return
    var acc = FaustFloat(0.0)
    for m in range(Int(modes)):
        acc += outs[unsafe_offset=(b * Int(modes) + m) * Int(block) + i]
    bellsum[unsafe_offset=b * Int(block) + i] = acc


def mix_kernel(
    bellsum: Ptr[FaustFloat, MutAnyOrigin], pans: Ptr[FaustFloat, MutAnyOrigin],
    mix: Ptr[FaustFloat, MutAnyOrigin], bells: S32, modes: S32, count: S32, block: S32,
) -> None:
    var i = Int(global_idx.x)
    if i >= Int(count):
        return
    var left = FaustFloat(0.0)
    var right = FaustFloat(0.0)
    for b in range(Int(bells)):
        var x = bellsum[unsafe_offset=b * Int(block) + i]
        left += x * pans[unsafe_offset=2 * b]
        right += x * pans[unsafe_offset=2 * b + 1]
    mix[unsafe_offset=2 * i] = left / FaustFloat(Int(modes))
    mix[unsafe_offset=2 * i + 1] = right / FaustFloat(Int(modes))


def grid(n: Int) -> Int:
    return (n + GPU_BLOCK_DIM - 1) // GPU_BLOCK_DIM


# ------------------------------------------------------------------ the scene

# The parameters of every mode of every bell (header).
struct Carillon:
    var bells: Int
    var modes: Int
    var freq: List[FaustFloat]
    var t60: List[FaustFloat]
    var gain: List[FaustFloat]
    var pans: List[FaustFloat]

    def __init__(out c, bells: Int, position: Int):
        c.bells = bells
        c.modes = BELL_MODES
        c.freq = List[FaustFloat]()
        c.t60 = List[FaustFloat]()
        c.gain = List[FaustFloat]()
        c.pans = List[FaustFloat]()
        var freqs = bell_freqs()
        var gains = bell_gains()
        for b in range(bells):
            var semitones = 0.0 if bells == 1 else -12.0 + 36.0 * F64(b) / F64(bells - 1)
            var scale = 2.0 ** (semitones / 12.0)
            var p = position if position >= 0 else b % BELL_POSITIONS
            for m in range(BELL_MODES):
                var f = freqs[m]
                var t = BELL_T60 * (1.0 - (f / BELL_T60_FMAX) * BELL_T60_RATIO) ** BELL_T60_SLOPE
                var fb = f * scale
                c.freq.append(FaustFloat(fb))
                c.t60.append(FaustFloat(t / scale))
                var g = gains[p * BELL_MODES + m] if fb < 0.45 * F64(RATE) else 0.0
                c.gain.append(FaustFloat(g))
            var angle = 0.7853981633974483 if bells == 1 else 1.5707963267948966 * F64(b) / F64(bells - 1)
            c.pans.append(FaustFloat(cos(angle)))
            c.pans.append(FaustFloat(sin(angle)))


# The score: bell b is struck every 96 blocks, 7 blocks after bell b - 1.
def score_gate(b: Int, block: Int) -> FaustFloat:
    return FaustFloat(1.0) if (block - 7 * b) % 96 == 0 else FaustFloat(0.0)


# A score: the bells struck at given blocks of a loop of `period` blocks.
struct Score(Movable):
    var at: List[Int]
    var bell: List[Int]
    var period: Int

    def __init__(out sc):
        sc.at = List[Int]()
        sc.bell = List[Int]()
        sc.period = 1

    # Appends notes (semitones, durations in beats) at `beat_blocks` per beat.
    def notes(mut sc, semitones: List[Int], beats: List[F64], beat_blocks: F64, bells: Int):
        var t = F64(sc.period - 1) if len(sc.at) > 0 else 0.0
        for i in range(len(semitones)):
            var b = Int(round(F64(semitones[i] + 12) * F64(bells - 1) / 36.0))
            sc.at.append(Int(t))
            sc.bell.append(max(0, min(bells - 1, b)))
            t += beats[i] * beat_blocks
        sc.period = Int(t) + 1

    def rest(mut sc, beats: F64, beat_blocks: F64):
        sc.period += Int(beats * beat_blocks)

    def gates(self, k: Int, mut out: List[FaustFloat]):
        for b in range(len(out)):
            out[b] = FaustFloat(0.0)
        var t = k % self.period
        for i in range(len(self.at)):
            if self.at[i] == t:
                out[self.bell[i]] = FaustFloat(1.0)


comptime SCORE_CLOUD = 0
comptime SCORE_MELODY = 1
comptime SCORE_CHIME = 2


# Ode to Joy (Beethoven), its two phrases, in C major around the model's
# pitch (C = 1, D = 3, E = 5, F = 6, G = 8 semitones), a beat of 0.45 s.
def melody(bells: Int) -> Score:
    var sc = Score()
    var beat = 0.45 * F64(RATE) / 256.0
    var a: List[Int] = [5, 5, 6, 8, 8, 6, 5, 3, 1, 1, 3, 5, 5, 3, 3]
    var a2: List[Int] = [5, 5, 6, 8, 8, 6, 5, 3, 1, 1, 3, 5, 3, 1, 1]
    var d: List[F64] = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1.5, 0.5, 2]
    sc.notes(a, d, beat, bells)
    sc.notes(a2, d, beat, bells)
    sc.rest(2.0, beat)
    return sc^


# The Westminster Quarters, in E major an octave above the hour bell (B = 0,
# E = 5, F# = 7, G# = 9 semitones): the five changes, each note a beat and
# the last two, two beats of rest between changes, then the hour bell (E, -7)
# struck four times three beats apart; a beat of 0.7 s.
def chime(bells: Int) -> Score:
    var sc = Score()
    var beat = 0.7 * F64(RATE) / 256.0
    var changes: List[List[Int]] = [
        [9, 7, 5, 0], [5, 9, 7, 0], [5, 7, 9, 5], [9, 5, 7, 0], [0, 7, 9, 5]
    ]
    var d: List[F64] = [1, 1, 1, 2]
    for c in changes:
        sc.notes(c, d, beat, bells)
        sc.rest(2.0, beat)
    var hour: List[Int] = [-7, -7, -7, -7]
    var hd: List[F64] = [3, 3, 3, 3]
    sc.notes(hour, hd, beat, bells)
    sc.rest(6.0, beat)
    return sc^


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


# The carillon on the device, run for `blocks` blocks. MODE_VALIDATE compares
# the (single) bell with pm.standardBell struck at `position` and returns the
# relative difference; MODE_BENCH returns the median block in ns; MODE_PLAY
# writes the mix to PortAudio and returns the number of underruns.
def run_scene(
    scene: Carillon, block: Int, blocks: Int, mode: Int, position: Int, audible: Bool,
    score: Int = SCORE_CLOUD, rt: RtOptions = RtOptions(),
) raises -> F64:
    var bells = scene.bells
    var n = bells * scene.modes
    var ctx = DeviceContext()
    var sstate = ctx.enqueue_create_buffer[DType.uint8](max(strikedsp.state_bytes(bells), 8))
    var mstate = ctx.enqueue_create_buffer[DType.uint8](max(modedsp.state_bytes(n), 8))
    ctx.synchronize()
    var shost = unsafe_alloc[strikedsp](1)
    shost.unsafe_write(strikedsp(S32(bells), sstate.unsafe_ptr().unsafe_origin_cast[MUT_NOTRK]()))
    shost[].class_init(RATE)
    var sgui = LabelGui()
    shost[].build_user_interface(sgui)
    var gate_index = index_of(sgui.labels, "gate")
    var mhost = unsafe_alloc[modedsp](1)
    mhost.unsafe_write(modedsp(S32(n), mstate.unsafe_ptr().unsafe_origin_cast[MUT_NOTRK]()))
    mhost[].class_init(RATE)
    var mgui = LabelGui()
    mhost[].build_user_interface(mgui)
    var sdsps = ctx.enqueue_create_buffer[DType.uint8](size_of[strikedsp]())
    var mdsps = ctx.enqueue_create_buffer[DType.uint8](size_of[modedsp]())
    ctx.enqueue_copy(dst_buf=sdsps, src_ptr=shost.unsafe_bitcast[UInt8]())
    ctx.enqueue_copy(dst_buf=mdsps, src_ptr=mhost.unsafe_bitcast[UInt8]())
    ctx.enqueue_function[strike_init_kernel](
        sdsps, sstate, S32(bells), RATE, grid_dim=grid(bells), block_dim=GPU_BLOCK_DIM
    )
    ctx.enqueue_function[mode_init_kernel](
        mdsps, mstate, S32(n), RATE, grid_dim=grid(n), block_dim=GPU_BLOCK_DIM
    )
    var strikes = ctx.enqueue_create_buffer[dfaust](bells * block)
    var outs = ctx.enqueue_create_buffer[dfaust](n * block)
    var bellsum = ctx.enqueue_create_buffer[dfaust](bells * block)
    var schan = ctx.enqueue_create_buffer[DType.uint64](bells)
    var mchan = ctx.enqueue_create_buffer[DType.uint64](2 * n)
    ctx.enqueue_function[strike_channels_kernel](
        schan, strikes, S32(bells), S32(block), grid_dim=grid(bells), block_dim=GPU_BLOCK_DIM
    )
    ctx.enqueue_function[mode_channels_kernel](
        mchan, strikes, outs, S32(n), S32(scene.modes), S32(block),
        grid_dim=grid(n), block_dim=GPU_BLOCK_DIM,
    )
    # Every mode's parameters, one control at a time.
    var values = ctx.enqueue_create_buffer[dfaust](n)
    var values_host = ctx.enqueue_create_host_buffer[dfaust](n)
    var labels: List[String] = ["freq", "t60", "gain"]
    for k in range(3):
        ctx.synchronize()
        for v in range(n):
            values_host[v] = scene.freq[v] if k == 0 else (scene.t60[v] if k == 1 else scene.gain[v])
        ctx.enqueue_copy(dst_buf=values, src_buf=values_host)
        ctx.enqueue_function[mode_control_kernel](
            mdsps, mstate, values, S32(n), Int32(index_of(mgui.labels, labels[k])),
            grid_dim=grid(n), block_dim=GPU_BLOCK_DIM,
        )
    var pans = ctx.enqueue_create_buffer[dfaust](2 * bells)
    var pans_host = ctx.enqueue_create_host_buffer[dfaust](2 * bells)
    ctx.synchronize()
    for i in range(2 * bells):
        pans_host[i] = scene.pans[i]
    ctx.enqueue_copy(dst_buf=pans, src_buf=pans_host)
    var mix = ctx.enqueue_create_buffer[dfaust](2 * block)
    var gates = ctx.enqueue_create_buffer[dfaust](bells)
    ctx.synchronize()

    # The reference (MODE_VALIDATE): pm.standardBell, one program, on the CPU.
    var refi = unsafe_alloc[refdsp](1)
    refi.unsafe_write(refdsp())
    refi[].init(RATE)
    var zones = ZoneGui()
    refi[].build_user_interface(zones)
    zones.set("position", FaustFloat(max(position, 0)))
    var ref_out = unsafe_alloc[FaustFloat](block)
    var ref_heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](1)
    ref_heads.unsafe_write(ref_out.unsafe_origin_cast[MUT_NOTRK]())
    var diff = F64(0.0)
    var ref_peak = F64(0.0)

    # PortAudio (MODE_PLAY).
    var out = unsafe_alloc[F32](2 * block)
    var stream = UInt64(0)
    var underflows = 0
    var peak = F64(0.0)
    var latency_s = F64(-1.0)

    var warmup = 0 if mode == MODE_VALIDATE else WARMUP
    var times = List[Int]()
    # The sum of squares of the played mix: the same with and without
    # `pipeline` (a check that the double buffers hold the right block).
    var energy = F64(0.0)
    var sc = melody(bells) if score == SCORE_MELODY else (chime(bells) if score == SCORE_CHIME else Score())
    var row = List[FaustFloat](length=bells, fill=FaustFloat(0.0))
    # Iteration k submits block k and processes block k - depth: with
    # `pipeline` in `play` (depth 1), block k + 1 is on the GPU while block k
    # is written. Two of each host buffer a block in flight reads or writes,
    # and an event after each block.
    var rl = RtLoop(rt, block, RATE)
    var depth = rl.depth() if mode == MODE_PLAY else 0
    var gates_hosts = List[HostBuffer[dfaust]]()
    var mix_hosts = List[HostBuffer[dfaust]]()
    var events = List[DeviceEvent]()
    for _ in range(2):
        gates_hosts.append(ctx.enqueue_create_host_buffer[dfaust](bells))
        mix_hosts.append(ctx.enqueue_create_host_buffer[dfaust](2 * block))
        events.append(ctx.create_event())
    ctx.synchronize()
    var enqueue_ns = List[Int](length=2, fill=0)
    var total = blocks + warmup
    for k in range(total + depth):
        if k < total:
            var kb = k - warmup
            var si = k % 2
            # The tune starts with the stream, not during the warm-up.
            if mode != MODE_VALIDATE and score != SCORE_CLOUD and kb >= 0:
                sc.gates(kb, row)
            for b in range(bells):
                if mode == MODE_VALIDATE:
                    gates_hosts[si][b] = FaustFloat(1.0) if kb == 0 else FaustFloat(0.0)
                elif score == SCORE_CLOUD:
                    gates_hosts[si][b] = score_gate(b, k)
                else:
                    gates_hosts[si][b] = row[b] if kb >= 0 else FaustFloat(0.0)
            var t0 = Int(perf_counter_ns())
            ctx.enqueue_copy(dst_buf=gates, src_buf=gates_hosts[si])
            ctx.enqueue_function[strike_control_kernel](
                sdsps, sstate, gates, S32(bells), Int32(gate_index),
                grid_dim=grid(bells), block_dim=GPU_BLOCK_DIM,
            )
            ctx.enqueue_function[strike_kernel](
                sdsps, sstate, strikes, schan, S32(bells), S32(block),
                grid_dim=grid(bells), block_dim=GPU_BLOCK_DIM,
            )
            ctx.enqueue_function[mode_kernel](
                mdsps, mstate, strikes, outs, mchan, S32(n), S32(block),
                grid_dim=grid(n), block_dim=GPU_BLOCK_DIM,
            )
            ctx.enqueue_function[bell_mix_kernel](
                outs, bellsum, S32(bells), S32(scene.modes), S32(block), S32(block),
                grid_dim=grid(bells * block), block_dim=GPU_BLOCK_DIM,
            )
            ctx.enqueue_function[mix_kernel](
                bellsum, pans, mix, S32(bells), S32(scene.modes), S32(block), S32(block),
                grid_dim=grid(block), block_dim=GPU_BLOCK_DIM,
            )
            ctx.enqueue_copy(dst_buf=mix_hosts[si], src_buf=mix)
            if depth > 0:
                ctx.stream().record_event(events[si])
            enqueue_ns[si] = Int(perf_counter_ns()) - t0
        var j = k - depth
        if j < 0:
            continue
        var jb = j - warmup
        var so = j % 2
        var t1 = Int(perf_counter_ns())
        if depth > 0:
            events[so].synchronize()
        else:
            ctx.synchronize()
        var t2 = Int(perf_counter_ns())
        if jb < 0:
            continue
        times.append(enqueue_ns[so] + t2 - t1)
        if mode == MODE_VALIDATE:
            zones.set("gate", gates_hosts[so][0])
            refi[].compute(
                S32(block),
                ref_heads.unsafe_origin_cast[MUT_NOTRK]().unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]](),
                ref_heads.unsafe_origin_cast[MUT_NOTRK](),
            )
            for i in range(block):
                # One bell is centred: each channel carries it times cos(pi/4).
                var got = F64(mix_hosts[so][2 * i]) / 0.7071067811865476
                var want = F64(ref_out[unsafe_offset=i])
                diff = max(diff, abs(got - want))
                ref_peak = max(ref_peak, abs(want))
        elif mode == MODE_PLAY:
            for i in range(2 * block):
                var x = F32(mix_hosts[so][i])
                peak = max(peak, abs(F64(x)))
                energy += F64(x) * F64(x)
                # The cloud's peak grows like sqrt(bells) (31 at 1,024 bells);
                # a tune rings one or two bells at a time (peak about 2.1).
                var gain = 0.8 / sqrt(F64(bells)) if score == SCORE_CLOUD else 0.4
                out[unsafe_offset=i] = x * F32(gain) if audible else F32(0.0)
            var t3 = Int(perf_counter_ns())
            if jb == 0:
                # The stream starts once the GPU is warm, the real-time
                # requests with it.
                rl.start()
                var suggested = rt.latency_s(block, RATE)
                stream = rt_open_output(RATE, block, suggested)
                latency_s = rt_output_latency(stream)
                rl.joined(stream, suggested)
            var fill = rl.ring_fill(stream)
            var w0 = Int(perf_counter_ns())
            var r = external_call["Pa_WriteStream", Int32](
                stream, address(out.unsafe_origin_cast[MUT_NOTRK]()), UInt64(block)
            )
            rl.record(enqueue_ns[so], t2 - t1, t3 - t2, fill, w0, Int(perf_counter_ns()))
            if r == paOutputUnderflowed:
                underflows += 1

    var result = F64(0.0)
    if mode == MODE_VALIDATE:
        print("position", position, "peak", ref_peak, "max_diff", diff, "relative", diff / ref_peak)
        result = diff / ref_peak
    elif mode == MODE_BENCH:
        result = F64(median(times))
    else:
        rl.leave()
        rt_close_output(stream)
        var worst = 0
        for t in times:
            worst = max(worst, t)
        print(
            "bells", bells, "modes", n, "block", block, "blocks", blocks,
            "underflows", underflows, "compute_median_us", F64(median(times)) / 1000.0,
            "compute_max_us", F64(worst) / 1000.0, "budget_us", F64(block) * 1e6 / F64(RATE),
            "peak", peak, "energy", energy, "audible", audible, "output_latency_ms", latency_s * 1000.0,
        )
        print(rl.summary())
        result = F64(underflows)
    rl.free()
    shost.unsafe_free()
    mhost.unsafe_free()
    refi.unsafe_free()
    ref_out.unsafe_free()
    ref_heads.unsafe_free()
    out.unsafe_free()
    return result


# ----------------------------------------------------------------- the modes

def validate() raises:
    # One bell per strike position, against pm.standardBell on the CPU, 2 s.
    var worst = F64(0.0)
    for p in range(BELL_POSITIONS):
        worst = max(worst, run_scene(Carillon(1, p), 256, 375, MODE_VALIDATE, p, False))
    print("worst relative difference", worst)
    if worst > 1e-3:
        raise Error("a GPU bell differs from pm.standardBell")


# The batch code of the GPU run on the CPU: host instances over host state,
# the strikes then the modes split in `workers` slices; returns the median
# block in ns. Not the baseline to beat (`cpu_faust_block_ns` is): it shows
# what the GPU's layout costs a CPU.
def cpu_block_ns(scene: Carillon, block: Int, workers: Int, blocks: Int) raises -> Int:
    var bells = scene.bells
    var n = bells * scene.modes
    var sstate = unsafe_alloc[UInt8](max(strikedsp.state_bytes(bells), 8))
    var mstate = unsafe_alloc[UInt8](max(modedsp.state_bytes(n), 8))
    var s = unsafe_alloc[strikedsp](1)
    s.unsafe_write(strikedsp(S32(bells), sstate.unsafe_origin_cast[MUT_NOTRK]()))
    var m = unsafe_alloc[modedsp](1)
    m.unsafe_write(modedsp(S32(n), mstate.unsafe_origin_cast[MUT_NOTRK]()))
    s[].class_init(RATE)
    m[].class_init(RATE)
    var mgui = LabelGui()
    m[].build_user_interface(mgui)
    var sgui = LabelGui()
    s[].build_user_interface(sgui)
    var i_freq = index_of(mgui.labels, "freq")
    var i_t60 = index_of(mgui.labels, "t60")
    var i_gain = index_of(mgui.labels, "gain")
    var i_gate = index_of(sgui.labels, "gate")
    for b in range(bells):
        s[].instance_init(S32(b), RATE)
    for v in range(n):
        m[].instance_init(S32(v), RATE)
        m[].set_control(S32(v), i_freq, scene.freq[v])
        m[].set_control(S32(v), i_t60, scene.t60[v])
        m[].set_control(S32(v), i_gain, scene.gain[v])
    var strikes = unsafe_alloc[FaustFloat](bells * block)
    var outs = unsafe_alloc[FaustFloat](n * block)
    var sheads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](bells)
    var mheads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](2 * n)
    for b in range(bells):
        sheads.unsafe_offset(b).unsafe_write(strikes.unsafe_offset(b * block).unsafe_origin_cast[MUT_NOTRK]())
    for v in range(n):
        mheads.unsafe_offset(2 * v).unsafe_write(strikes.unsafe_offset((v // scene.modes) * block).unsafe_origin_cast[MUT_NOTRK]())
        mheads.unsafe_offset(2 * v + 1).unsafe_write(outs.unsafe_offset(v * block).unsafe_origin_cast[MUT_NOTRK]())
    # The mix: each worker sums its modes, panned, into its own stereo buffer;
    # the buffers are then added, as the GPU's mix does.
    var acc = unsafe_alloc[FaustFloat](workers * 2 * block)
    var mix = unsafe_alloc[FaustFloat](2 * block)
    var pan = unsafe_alloc[FaustFloat](2 * bells)
    for i in range(2 * bells):
        pan[unsafe_offset=i] = scene.pans[i]
    var accp = acc.unsafe_origin_cast[MUT_NOTRK]()
    var panp = pan.unsafe_origin_cast[MUT_NOTRK]()
    var outp = outs.unsafe_origin_cast[MUT_NOTRK]()
    var modes = scene.modes
    var sp = s.unsafe_origin_cast[MUT_NOTRK]()
    var mp = m.unsafe_origin_cast[MUT_NOTRK]()
    var shp = sheads.unsafe_origin_cast[MUT_NOTRK]()
    var mhp = mheads.unsafe_origin_cast[MUT_NOTRK]()
    var times = List[Int]()
    for k in range(blocks):
        for b in range(bells):
            s[].set_control(S32(b), i_gate, score_gate(b, k))
        var t0 = perf_counter_ns()

        def strike_slice(w: Int) {var sp, var shp, var bells, var workers, var block}:
            for b in range(bells * w // workers, bells * (w + 1) // workers):
                var h = shp.unsafe_offset(b)
                sp[].compute(S32(b), S32(block), h.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]().unsafe_mut_cast[False]().unsafe_origin_cast[IMM_NOTRK](), h)

        def mode_slice(
            w: Int,
        ) {var mp, var mhp, var n, var workers, var block, var accp, var panp, var outp, var modes}:
            var a = accp.unsafe_offset(w * 2 * block)
            for i in range(2 * block):
                a[unsafe_offset=i] = FaustFloat(0.0)
            for v in range(n * w // workers, n * (w + 1) // workers):
                var h = mhp.unsafe_offset(2 * v)
                mp[].compute(S32(v), S32(block), h.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]().unsafe_mut_cast[False]().unsafe_origin_cast[IMM_NOTRK](), h.unsafe_offset(1))
                var b = v // modes
                var l = panp[unsafe_offset=2 * b]
                var r = panp[unsafe_offset=2 * b + 1]
                var o = outp.unsafe_offset(v * block)
                for i in range(block):
                    a[unsafe_offset=2 * i] += o[unsafe_offset=i] * l
                    a[unsafe_offset=2 * i + 1] += o[unsafe_offset=i] * r

        parallelize(strike_slice, workers)
        parallelize(mode_slice, workers)
        for i in range(2 * block):
            var x = FaustFloat(0.0)
            for w in range(workers):
                x += acc[unsafe_offset=w * 2 * block + i]
            mix[unsafe_offset=i] = x / FaustFloat(modes)
        var t1 = perf_counter_ns()
        if k >= 3:
            times.append(Int(t1 - t0))
    s.unsafe_free()
    m.unsafe_free()
    sstate.unsafe_free()
    mstate.unsafe_free()
    strikes.unsafe_free()
    outs.unsafe_free()
    sheads.unsafe_free()
    mheads.unsafe_free()
    acc.unsafe_free()
    mix.unsafe_free()
    pan.unsafe_free()
    return median(times)


# The carillon as a CPU program would be written: one ordinary instance of
# `pm.standardBell` per bell (`refdsp`, the scalar contract: its own struct,
# its 50 modes in one `compute`), the bells split in `workers` slices, each
# worker summing its bells, panned, into its own stereo buffer. This is the
# baseline the GPU has to beat. The batch code run on the CPU
# (`cpu_block_ns`) is 3 to 6 times slower: its voice-interleaved layout,
# made for the GPU, puts each voice's state N elements apart, and every mode
# is a call of its own. Every bell keeps the model's pitch here, which does
# not change the cost (the modes the GPU scene mutes above 0.45 SR are
# computed in both). Returns the median block in ns.
def cpu_faust_block_ns(scene: Carillon, block: Int, workers: Int, blocks: Int) raises -> Int:
    var bells = scene.bells
    var insts = unsafe_alloc[refdsp](bells)
    var gates = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](bells)
    var outs = unsafe_alloc[FaustFloat](bells * block)
    var heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](bells)
    for b in range(bells):
        insts.unsafe_offset(b).unsafe_write(refdsp())
        insts.unsafe_offset(b)[].init(RATE)
        var zones = ZoneGui()
        insts.unsafe_offset(b)[].build_user_interface(zones)
        zones.set("position", FaustFloat(b % BELL_POSITIONS))
        gates.unsafe_offset(b).unsafe_write(zones.zones[index_of(zones.labels, "gate")])
        heads.unsafe_offset(b).unsafe_write(outs.unsafe_offset(b * block).unsafe_origin_cast[MUT_NOTRK]())
    var acc = unsafe_alloc[FaustFloat](workers * 2 * block)
    var mix = unsafe_alloc[FaustFloat](2 * block)
    var pan = unsafe_alloc[FaustFloat](2 * bells)
    for i in range(2 * bells):
        pan[unsafe_offset=i] = scene.pans[i]
    var ip = insts.unsafe_origin_cast[MUT_NOTRK]()
    var hp = heads.unsafe_origin_cast[MUT_NOTRK]()
    var accp = acc.unsafe_origin_cast[MUT_NOTRK]()
    var panp = pan.unsafe_origin_cast[MUT_NOTRK]()
    var outp = outs.unsafe_origin_cast[MUT_NOTRK]()
    var times = List[Int]()
    for k in range(blocks):
        for b in range(bells):
            gates[unsafe_offset=b][] = score_gate(b, k)
        var t0 = perf_counter_ns()

        def bell_slice(
            w: Int,
        ) {var ip, var hp, var bells, var workers, var block, var accp, var panp, var outp}:
            var a = accp.unsafe_offset(w * 2 * block)
            for i in range(2 * block):
                a[unsafe_offset=i] = FaustFloat(0.0)
            for b in range(bells * w // workers, bells * (w + 1) // workers):
                var h = hp.unsafe_offset(b)
                ip.unsafe_offset(b)[].compute(S32(block), h.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]().unsafe_mut_cast[False]().unsafe_origin_cast[IMM_NOTRK](), h)
                var l = panp[unsafe_offset=2 * b]
                var r = panp[unsafe_offset=2 * b + 1]
                var o = outp.unsafe_offset(b * block)
                for i in range(block):
                    a[unsafe_offset=2 * i] += o[unsafe_offset=i] * l
                    a[unsafe_offset=2 * i + 1] += o[unsafe_offset=i] * r

        if workers == 1:
            bell_slice(0)
        else:
            parallelize(bell_slice, workers)
        for i in range(2 * block):
            var x = FaustFloat(0.0)
            for w in range(workers):
                x += acc[unsafe_offset=w * 2 * block + i]
            mix[unsafe_offset=i] = x
        var t1 = perf_counter_ns()
        if k >= 3:
            times.append(Int(t1 - t0))
    insts.unsafe_free()
    gates.unsafe_free()
    outs.unsafe_free()
    heads.unsafe_free()
    acc.unsafe_free()
    mix.unsafe_free()
    pan.unsafe_free()
    return median(times)


# The GPU against two CPU baselines: ordinary instances of pm.standardBell
# (`faust1`, `faust4`, `faust10`: 1, 4 and 10 threads), the baseline to
# beat, and the batch code run on the CPU (`batch4`, `batch10`), kept to
# show what the GPU layout costs a CPU. `gpu_vs_cpu` is against the fastest
# ordinary run.
def bench(bells: Int, block: Int) raises:
    var scene = Carillon(bells, -1)
    var g = Int(run_scene(scene, block, 100, MODE_BENCH, -1, False))
    var f1 = cpu_faust_block_ns(scene, block, 1, 13)
    var f4 = cpu_faust_block_ns(scene, block, 4, 13)
    var f10 = cpu_faust_block_ns(scene, block, 10, 13)
    var b4 = cpu_block_ns(scene, block, 4, 13)
    var b10 = cpu_block_ns(scene, block, 10, 13)
    var budget = F64(block) * 1e6 / F64(RATE)
    print(
        "bells", bells, "modes", bells * BELL_MODES, "block", block, "budget_us", budget,
        "gpu_us", F64(g) / 1000.0, "faust1_us", F64(f1) / 1000.0,
        "faust4_us", F64(f4) / 1000.0, "faust10_us", F64(f10) / 1000.0,
        "batch4_us", F64(b4) / 1000.0, "batch10_us", F64(b10) / 1000.0,
        "gpu_vs_cpu", F64(min(f1, min(f4, f10))) / F64(g),
    )


def play(bells: Int, seconds: F64, audible: Bool, block: Int, score: Int, rt: RtOptions) raises:
    var blocks = Int(seconds * F64(RATE) / F64(block))
    var underflows = run_scene(Carillon(bells, -1), block, blocks, MODE_PLAY, -1, audible, score, rt)
    if underflows > 0:
        raise Error(String(Int(underflows)) + " underruns")


def main() raises -> None:
    comptime assert dfaust == f32, "the Apple GPU computes in f32: -D DFAUST=DType.float32"
    var args = argv()
    if len(args) < 2:
        raise Error(
            "usage: validate | bench <bells> [<block>] | play <bells> <seconds> [audible] [<block>]"
            + " [cloud|melody|chime] [rt=off|tc|wg|tc+wg] [pipeline] [latency=<ms>]"
        )
    if args[1] == "validate":
        validate()
    elif args[1] == "bench":
        bench(atol(args[2]), atol(args[3]) if len(args) > 3 else 256)
    elif args[1] == "play":
        var audible = False
        var block = 256
        var score = SCORE_CLOUD
        var rt = RtOptions()
        for i in range(4, len(args)):
            if rt.parse(String(args[i])):
                continue
            elif args[i] == "audible":
                audible = True
            elif args[i] == "melody":
                score = SCORE_MELODY
            elif args[i] == "chime":
                score = SCORE_CHIME
            elif args[i] == "cloud":
                score = SCORE_CLOUD
            else:
                block = atol(args[i])
        play(atol(args[2]), atof(args[3]), audible, block, score, rt)
    else:
        raise Error("unknown mode")
