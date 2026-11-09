
# ---------------------------------------------------------------------------
# polymojogpubatch.mojo — a polyphonic instrument played in real time: N
# voices of one DSP on the GPU with the batch contract, mixed on the device,
# written to the default audio output through PortAudio (GPU batch plan G4,
# porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md).
#
# Concatenated after `impulsemojo.mojo`, the `mojo_batch` output of a voice
# with `freq`, `gain` and `gate` controls (`poly/voice.dsp`) and
# `realtimemojo.mojo`. Built with `-D DFAUST=DType.float32` and linked with
# `-lportaudio -framework CoreAudio`.
#
#   <binary> <voices> <seconds> [audible] [<block> [<stress_us> [<latency_ms>]]]
#            [rt=off|tc|wg|tc+wg] [pipeline] [latency=<ms>]
#
# `rt=`, `pipeline` and `latency=` (anywhere on the line) are the real-time
# options of realtimemojo.mojo: the time-constraint policy for the loop's
# thread, the output device's IO workgroup, the GPU work submitted one block
# ahead, and the suggested latency (`latency_ms`, if given, wins).
# A second line, `timing`, splits each block into enqueue, GPU wait, host
# work and `Pa_WriteStream` (median, 99th percentile, maximum), with the gap
# between two writes; porting/faust-gpu-audio-synthesis-2026-11-07-en.md
# §2.5 records them under CPU load.
#
# PortAudio's blocking API (`Pa_WriteStream`) paces the loop at the device's
# rate and reports every underrun (`paOutputUnderflowed`): no C callback is
# needed from Mojo. Each block: the scripted gates of all voices are copied
# to the device and set (`set_control`), every voice computes `block` frames
# (one thread per voice), a kernel sums the voices into one buffer, which is
# copied back and written, duplicated to two channels. Muted by default (the
# GPU work is done, silence is written); `audible` writes the mix, scaled.
# `stress_us` adds a busy wait per block, to check that underruns are seen.
# The stream is opened with a suggested output latency (`latency_ms`, two
# blocks by default): `Pa_OpenDefaultStream` takes the device's high-latency
# default (112.6 ms on the machine of record), a cushion that hides late
# blocks. The latency PortAudio grants (`Pa_GetStreamInfo`) is printed.
#
# Notes are scripted, reproducible: voice v plays 55 Hz * 2^((v % 48) / 12),
# its gate on for 64 blocks of every 96, phase-shifted by 7 blocks per voice.
#
# Every buffer a kernel reaches is one of its arguments (Metal, G3.5).
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext, DeviceEvent, HostBuffer
from max.gpu import global_idx
from std.ffi import external_call
from std.sys import argv
from std.time import perf_counter_ns

comptime GPU_BLOCK_DIM = 64
comptime RATE = S32(48000)
comptime WARMUP = 20
comptime paOutputUnderflowed = Int32(-9980)


# Records the control labels, in control-table order.
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

    def add_vertical_bargraph(
        mut ui, var label: String, mut zone: FaustFloat, var min: FaustFloat, var max: FaustFloat
    ) -> None:
        ui.labels.append(label)

    def add_horizontal_bargraph(
        mut ui, var label: String, mut zone: FaustFloat, var min: FaustFloat, var max: FaustFloat
    ) -> None:
        ui.labels.append(label)


def init_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin], state: Ptr[UInt8, MutAnyOrigin], nvoices: S32, rate: S32
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(nvoices):
        return
    dsps.unsafe_bitcast[mydsp]()[].instance_init(S32(v), rate)


# Control `index` of voice v takes values[v].
def control_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin],
    state: Ptr[UInt8, MutAnyOrigin],
    values: Ptr[FaustFloat, MutAnyOrigin],
    nvoices: S32,
    index: Int32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(nvoices):
        return
    dsps.unsafe_bitcast[mydsp]()[].set_control(S32(v), Int(index), values[unsafe_offset=v])


def voices_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin],
    state: Ptr[UInt8, MutAnyOrigin],
    data: Ptr[FaustFloat, MutAnyOrigin],
    channels: Ptr[UInt64, MutAnyOrigin],
    nvoices: S32,
    n_ins: S32,
    n_outs: S32,
    count: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(nvoices):
        return
    var heads = (
        channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]()
        .unsafe_offset(v * Int(n_ins + n_outs))
        .unsafe_origin_cast[MUT_NOTRK]()
    )
    dsps.unsafe_bitcast[mydsp]()[].compute(
        S32(v),
        count,
        heads.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]()
        .unsafe_mut_cast[False]()
        .unsafe_origin_cast[IMM_NOTRK](),
        heads.unsafe_offset(Int(n_ins)),
    )


def channels_kernel(
    channels: Ptr[UInt64, MutAnyOrigin],
    data: Ptr[FaustFloat, MutAnyOrigin],
    nvoices: S32,
    n_chans: S32,
    block: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(nvoices):
        return
    var heads = (
        channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]()
        .unsafe_offset(v * Int(n_chans))
        .unsafe_origin_cast[MUT_NOTRK]()
    )
    for c in range(Int(n_chans)):
        heads[unsafe_offset=c] = data.unsafe_offset(
            (v * Int(n_chans) + c) * Int(block)
        ).unsafe_origin_cast[MUT_NOTRK]()


# One thread per frame: the sum of output 0 of every voice.
def mix_kernel(
    data: Ptr[FaustFloat, MutAnyOrigin],
    mix: Ptr[FaustFloat, MutAnyOrigin],
    nvoices: S32,
    n_chans: S32,
    n_ins: S32,
    block: S32,
) -> None:
    var i = Int(global_idx.x)
    if i >= Int(block):
        return
    var acc = FaustFloat(0.0)
    for v in range(Int(nvoices)):
        acc += data[unsafe_offset=(v * Int(n_chans) + Int(n_ins)) * Int(block) + i]
    mix[unsafe_offset=i] = acc


def grid(n: Int) -> Int:
    return (n + GPU_BLOCK_DIM - 1) // GPU_BLOCK_DIM


def median(mut xs: List[Int]) -> Int:
    for i in range(1, len(xs)):
        var x = xs[i]
        var j = i - 1
        while j >= 0 and xs[j] > x:
            xs[j + 1] = xs[j]
            j -= 1
        xs[j + 1] = x
    return xs[len(xs) // 2]


def address[T: AnyType](p: Ptr[T, MUT_NOTRK]) -> UInt64:
    return UInt64(Int(p.__int__()))


def main() raises -> None:
    comptime assert dfaust == f32, "the Apple GPU computes in f32: -D DFAUST=DType.float32"
    var argl = argv()
    var rt = RtOptions()
    var args = List[String]()
    for i in range(1, len(argl)):
        var a = String(argl[i])
        if not rt.parse(a):
            args.append(a)
    if len(args) < 2:
        raise Error(
            "usage: <binary> <voices> <seconds> [audible] [<block> [<stress_us> [<latency_ms>]]]"
            + " [rt=off|tc|wg|tc+wg] [pipeline] [latency=<ms>]"
        )
    var nvoices = atol(args[0])
    var seconds = atof(args[1])
    var at = 2
    var audible = False
    if len(args) > at and args[at] == "audible":
        audible = True
        at += 1
    var block = atol(args[at]) if len(args) > at else 256
    var stress_us = atol(args[at + 1]) if len(args) > at + 1 else 0
    var latency_ms = (
        atof(args[at + 2]) if len(args) > at + 2 else rt.latency_s(block, RATE) * 1000.0
    )
    var nblocks = Int(seconds * F64(RATE) / F64(block))

    var ctx = DeviceContext()
    var state = ctx.enqueue_create_buffer[DType.uint8](max(mydsp.state_bytes(nvoices), 8))
    ctx.synchronize()
    var host = unsafe_alloc[mydsp](1)
    host.unsafe_write(mydsp(S32(nvoices), state.unsafe_ptr().unsafe_origin_cast[MUT_NOTRK]()))
    host[].class_init(RATE)
    var gui = LabelGui()
    host[].build_user_interface(gui)
    var i_freq = -1
    var i_gate = -1
    for i in range(len(gui.labels)):
        if gui.labels[i] == "freq":
            i_freq = i
        if gui.labels[i] == "gate":
            i_gate = i
    if i_freq < 0 or i_gate < 0:
        raise Error("the voice needs `freq` and `gate` controls")
    var dsps = ctx.enqueue_create_buffer[DType.uint8](size_of[mydsp]())
    ctx.enqueue_copy(dst_buf=dsps, src_ptr=host.unsafe_bitcast[UInt8]())
    ctx.enqueue_function[init_kernel](
        dsps, state, S32(nvoices), RATE, grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM
    )

    var n_ins = Int(host[].get_num_inputs())
    var n_outs = Int(host[].get_num_outputs())
    var n_chans = max(n_ins + n_outs, 1)
    var data = ctx.enqueue_create_buffer[dfaust](nvoices * n_chans * block)
    data.enqueue_fill(FaustFloat(0.0))
    var channels = ctx.enqueue_create_buffer[DType.uint64](nvoices * n_chans)
    ctx.enqueue_function[channels_kernel](
        channels, data, S32(nvoices), S32(n_chans), S32(block),
        grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
    )
    var mix = ctx.enqueue_create_buffer[dfaust](block)
    var values = ctx.enqueue_create_buffer[dfaust](nvoices)
    var values_host = ctx.enqueue_create_host_buffer[dfaust](nvoices)
    # Two of each buffer the host touches while a block is in flight
    # (`pipeline`), and the event recorded after each block.
    var gates_hosts = List[HostBuffer[dfaust]]()
    var mix_hosts = List[HostBuffer[dfaust]]()
    var events = List[DeviceEvent]()
    for _ in range(2):
        gates_hosts.append(ctx.enqueue_create_host_buffer[dfaust](nvoices))
        mix_hosts.append(ctx.enqueue_create_host_buffer[dfaust](block))
        events.append(ctx.create_event())
    ctx.synchronize()
    for v in range(nvoices):
        values_host[v] = FaustFloat(55.0 * (2.0 ** (F64(v % 48) / 12.0)))
    ctx.enqueue_copy(dst_buf=values, src_buf=values_host)
    ctx.enqueue_function[control_kernel](
        dsps, state, values, S32(nvoices), Int32(i_freq),
        grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
    )
    ctx.synchronize()

    var out = unsafe_alloc[F32](2 * block)
    var scale = F32(4.0 / F64(nvoices))

    # One block of the instrument: gates, voices, mix, back to the host.
    # Iteration k submits block k and writes block k - depth: with
    # `pipeline` (depth 1), block k + 1 is on the GPU while block k is
    # written.
    var compute = List[Int]()
    var peak = F64(0.0)
    # The sum of squares of the played mix: the same with and without
    # `pipeline` (a check that the double buffers hold the right block).
    var energy = F64(0.0)
    var stream = UInt64(0)
    var underflows = 0
    var errors = 0
    var t_start = 0
    var latency_s = F64(-1.0)
    var rl = RtLoop(rt, block, RATE)
    var depth = rl.depth()
    var total = nblocks + WARMUP
    var enqueue_ns = List[Int](length=2, fill=0)
    for k in range(total + depth):
        if k < total:
            var si = k % 2
            var t0 = Int(perf_counter_ns())
            for v in range(nvoices):
                var on = ((k + 7 * v) % 96) < 64
                gates_hosts[si][v] = FaustFloat(1.0) if on else FaustFloat(0.0)
            ctx.enqueue_copy(dst_buf=values, src_buf=gates_hosts[si])
            ctx.enqueue_function[control_kernel](
                dsps, state, values, S32(nvoices), Int32(i_gate),
                grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
            )
            ctx.enqueue_function[voices_kernel](
                dsps, state, data, channels, S32(nvoices), S32(n_ins), S32(n_outs), S32(block),
                grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
            )
            ctx.enqueue_function[mix_kernel](
                data, mix, S32(nvoices), S32(n_chans), S32(n_ins), S32(block),
                grid_dim=grid(block), block_dim=GPU_BLOCK_DIM,
            )
            ctx.enqueue_copy(dst_buf=mix_hosts[si], src_buf=mix)
            if depth > 0:
                ctx.stream().record_event(events[si])
            enqueue_ns[si] = Int(perf_counter_ns()) - t0
        var b = k - depth
        if b < 0:
            continue
        var so = b % 2
        var t1 = Int(perf_counter_ns())
        if depth > 0:
            events[so].synchronize()
        else:
            ctx.synchronize()
        var t2 = Int(perf_counter_ns())
        if stress_us > 0:
            var until = t2 + stress_us * 1000
            while Int(perf_counter_ns()) < until:
                continue
        var t_stress = Int(perf_counter_ns())
        for i in range(block):
            var x = F32(mix_hosts[so][i])
            if abs(F64(x)) > peak:
                peak = abs(F64(x))
            var y = x * scale if audible else F32(0.0)
            out[unsafe_offset=2 * i] = y
            out[unsafe_offset=2 * i + 1] = y
        var t3 = Int(perf_counter_ns())
        if b < WARMUP:
            continue
        for i in range(block):
            energy += F64(mix_hosts[so][i]) * F64(mix_hosts[so][i])
        if b == WARMUP:
            # The stream starts once the GPU is warm, the real-time requests
            # with it.
            rl.start()
            stream = rt_open_output(RATE, block, latency_ms / 1000.0)
            t_start = Int(perf_counter_ns())
            latency_s = rt_output_latency(stream)
            rl.joined(stream, latency_ms / 1000.0)
        compute.append(enqueue_ns[so] + t_stress - t1)
        var fill = rl.ring_fill(stream)
        var w0 = Int(perf_counter_ns())
        var r = external_call["Pa_WriteStream", Int32](
            stream, address(out.unsafe_origin_cast[MUT_NOTRK]()), UInt64(block)
        )
        rl.record(enqueue_ns[so], t2 - t1, t3 - t2, fill, w0, Int(perf_counter_ns()))
        if r == paOutputUnderflowed:
            underflows += 1
        elif r != 0:
            errors += 1
    var elapsed = F64(Int(perf_counter_ns()) - t_start) / 1e9
    rl.leave()
    rt_close_output(stream)

    var worst = 0
    for c in compute:
        worst = max(worst, c)
    var budget_us = F64(block) * 1e6 / F64(RATE)
    print(
        "voices", nvoices, "block", block, "rate", RATE, "blocks", nblocks,
        "seconds", elapsed, "underflows", underflows, "errors", errors,
        "compute_median_us", F64(median(compute)) / 1000.0,
        "compute_max_us", F64(worst) / 1000.0, "budget_us", budget_us,
        "mix_peak", peak, "mix_energy", energy, "audible", audible, "stress_us", stress_us,
        "output_latency_ms", latency_s * 1000.0,
    )
    print(rl.summary())
    rl.free()
    host.unsafe_free()
    out.unsafe_free()
    if underflows > 0 and stress_us == 0:
        raise Error(String(underflows) + " underruns")
