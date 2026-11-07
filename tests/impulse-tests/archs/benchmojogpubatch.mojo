
# ---------------------------------------------------------------------------
# benchmojogpubatch.mojo — cost of N voices of one DSP on the GPU with the
# batch contract: state as a structure of arrays (GPU batch plan G3.5,
# porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md).
#
# Concatenated after `impulsemojo.mojo` and the output of the `mojo_batch`
# example (`MojoOptions::batch`); built with `-D DFAUST=DType.float32`.
#
#   <binary> <program> <voices> <block> [<blocks>]
#
# prints one CSV row with the same GPU columns as `benchmojogpu.mojo`
# (`--header` for the header), so the two layouts compare row by row:
# gpu_full_us (inputs in, launch, outputs out, synchronise), gpu_kernel_us,
# gpu_copy_us, gpu_empty_us; state_bytes is per voice; max_gpu_cpu_diff is the
# largest gap between voice 0 on the device and voice 0 of a one-voice batch
# instance run on the host with the same inputs (a sanity check). The CPU
# columns of the array-of-structures sweep are not repeated: the layout does
# not change the CPU baseline. Medians of <blocks> blocks (default 100) after
# WARMUP blocks.
#
# Every buffer a kernel reaches is passed to it as an argument, even when the
# kernel only reaches it through a pointer stored elsewhere (the arenas, from
# the struct; the channel data, from the channel table): on Metal (Mojo
# 1.3.0.dev2026110705) a kernel reading a buffer that is not one of its
# arguments read zeros and stale values where the previous kernel had written.
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext
from max.gpu import global_idx
from std.sys import argv
from std.time import perf_counter_ns

comptime GPU_BLOCK_DIM = 64
comptime WARMUP = 20


def init_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin],
    state: Ptr[UInt8, MutAnyOrigin],
    nvoices: S32,
    sample_rate: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(nvoices):
        return
    dsps.unsafe_bitcast[mydsp]()[].instance_init(S32(v), sample_rate)


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


def empty_kernel(nvoices: S32) -> None:
    pass


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


def us(ns: Int) -> String:
    return String(F64(ns) / 1000.0)


def main() raises -> None:
    comptime assert dfaust == f32, "the Apple GPU computes in f32: -D DFAUST=DType.float32"
    var args = argv()
    if len(args) > 1 and args[1] == "--header":
        print(
            "program,voices,block,state_bytes,gpu_full_us,gpu_kernel_us,gpu_copy_us,"
            + "gpu_empty_us,budget_us,max_gpu_cpu_diff"
        )
        return
    if len(args) > 1 and args[1] == "--state-bytes":
        print(mydsp.state_bytes(1))
        return
    if len(args) < 4:
        raise Error("usage: <binary> <program> <voices> <block> [<blocks>]")
    var program = String(args[1])
    var nvoices = atol(args[2])
    var block = atol(args[3])
    var nblocks = atol(args[4]) if len(args) > 4 else 100

    var ctx = DeviceContext()
    # The state on the device, the struct built over it on the host (its
    # shared fields filled by class_init there), then copied to the device.
    var state = ctx.enqueue_create_buffer[DType.uint8](max(mydsp.state_bytes(nvoices), 8))
    ctx.synchronize()
    var host = unsafe_alloc[mydsp](1)
    host.unsafe_write(
        mydsp(S32(nvoices), state.unsafe_ptr().unsafe_origin_cast[MUT_NOTRK]())
    )
    host[].class_init(SAMPLE_RATE)
    var dsps = ctx.enqueue_create_buffer[DType.uint8](size_of[mydsp]())
    ctx.enqueue_copy(dst_buf=dsps, src_ptr=host.unsafe_bitcast[UInt8]())
    ctx.enqueue_function[init_kernel](
        dsps, state, S32(nvoices), SAMPLE_RATE,
        grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
    )

    var n_ins = Int(host[].get_num_inputs())
    var n_outs = Int(host[].get_num_outputs())
    var n_chans = max(n_ins + n_outs, 1)
    var floats = nvoices * n_chans * block
    var data = ctx.enqueue_create_buffer[dfaust](floats)
    var staging = ctx.enqueue_create_host_buffer[dfaust](floats)
    var channels = ctx.enqueue_create_buffer[DType.uint64](nvoices * n_chans)
    ctx.enqueue_function[channels_kernel](
        channels, data, S32(nvoices), S32(n_chans), S32(block),
        grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
    )
    ctx.synchronize()

    var signal = unsafe_alloc[FaustFloat](block)
    for i in range(block):
        signal[unsafe_offset=i] = FaustFloat(0.1 * sin(F64(i) * 0.05))

    # The host reference: a one-voice batch instance with its own state.
    var ref_state = unsafe_alloc[UInt8](max(mydsp.state_bytes(1), 8))
    var ref_dsp = unsafe_alloc[mydsp](1)
    ref_dsp.unsafe_write(mydsp(S32(1), ref_state.unsafe_origin_cast[MUT_NOTRK]()))
    ref_dsp[].class_init(SAMPLE_RATE)
    ref_dsp[].instance_init(S32(0), SAMPLE_RATE)
    var ref_heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](n_chans)
    var ref_data = unsafe_alloc[FaustFloat](n_chans * block)
    for c in range(n_chans):
        ref_heads.unsafe_offset(c).unsafe_write(
            ref_data.unsafe_offset(c * block).unsafe_origin_cast[MUT_NOTRK]()
        )
    var max_diff = F64(0.0)

    var full = List[Int]()
    var kernel = List[Int]()
    var copy = List[Int]()
    var empty = List[Int]()
    for b in range(nblocks + WARMUP):
        for v in range(nvoices):
            for c in range(n_ins):
                for i in range(block):
                    staging[(v * n_chans + c) * block + i] = signal[unsafe_offset=i]
        var t0 = perf_counter_ns()
        ctx.enqueue_copy(dst_buf=data, src_buf=staging)
        ctx.enqueue_function[voices_kernel](
            dsps, state, data, channels, S32(nvoices), S32(n_ins), S32(n_outs), S32(block),
            grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
        )
        ctx.enqueue_copy(dst_buf=staging, src_buf=data)
        ctx.synchronize()
        var t1 = perf_counter_ns()
        for c in range(n_chans):
            for i in range(block):
                ref_data[unsafe_offset=c * block + i] = (
                    signal[unsafe_offset=i] if c < n_ins else FaustFloat(0.0)
                )
        ref_dsp[].compute(
            S32(0),
            S32(block),
            ref_heads.unsafe_origin_cast[MUT_NOTRK]()
            .unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]](),
            ref_heads.unsafe_offset(n_ins).unsafe_origin_cast[MUT_NOTRK](),
        )
        for c in range(n_outs):
            for i in range(block):
                var d = abs(
                    F64(staging[(n_ins + c) * block + i])
                    - F64(ref_data[unsafe_offset=(n_ins + c) * block + i])
                )
                if d > max_diff:
                    max_diff = d
        if b >= WARMUP:
            full.append(Int(t1 - t0))

    for b in range(nblocks + WARMUP):
        var t0 = perf_counter_ns()
        ctx.enqueue_function[voices_kernel](
            dsps, state, data, channels, S32(nvoices), S32(n_ins), S32(n_outs), S32(block),
            grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
        )
        ctx.synchronize()
        var t1 = perf_counter_ns()
        ctx.enqueue_copy(dst_buf=data, src_buf=staging)
        ctx.enqueue_copy(dst_buf=staging, src_buf=data)
        ctx.synchronize()
        var t2 = perf_counter_ns()
        ctx.enqueue_function[empty_kernel](
            S32(nvoices), grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM
        )
        ctx.synchronize()
        var t3 = perf_counter_ns()
        if b >= WARMUP:
            kernel.append(Int(t1 - t0))
            copy.append(Int(t2 - t1))
            empty.append(Int(t3 - t2))

    var budget_ns = block * 1_000_000_000 // Int(SAMPLE_RATE)
    print(
        program + "," + String(nvoices) + "," + String(block) + ","
        + String(mydsp.state_bytes(1)) + "," + us(median(full)) + ","
        + us(median(kernel)) + "," + us(median(copy)) + "," + us(median(empty))
        + "," + us(budget_ns) + "," + String(max_diff)
    )
    host.unsafe_free()
    ref_dsp.unsafe_free()
    ref_state.unsafe_free()
    ref_heads.unsafe_free()
    ref_data.unsafe_free()
    signal.unsafe_free()
