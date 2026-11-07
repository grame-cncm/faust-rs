
# ---------------------------------------------------------------------------
# benchmojogpu.mojo — cost of many voices of one DSP on the GPU against the
# CPU (porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md, G2).
#
# Concatenated after `impulsemojo.mojo` and the generated DSP, like
# `runimpulsemojogpu.mojo`; built with `-D DFAUST=DType.float32`.
#
#   <binary> <program> <voices> <block> [<blocks>]
#
# prints one CSV row (header with `--header`):
# - gpu_full_us: one block on the device, as a host would run it: copy the
#   inputs in, launch, copy the outputs out, synchronise;
# - gpu_kernel_us: the launch and its synchronisation alone;
# - gpu_copy_us: the two copies and a synchronisation alone;
# - gpu_empty_us: an empty kernel and a synchronisation (the fixed floor);
# - cpu1_us, cpu4_us, cpu10_us: the same voices on 1 thread, and split in
#   4 and 10 slices run by `parallelize` (the M5 has 4 performance and 6
#   efficiency cores; the OS places the threads);
# - cpu_extrapolated: 1 when the CPU timings were taken on CPU_CAP voices
#   and scaled to <voices> (voices are independent, so the cost is linear
#   up to cache effects, which the scaling ignores);
# - budget_us: the real-time duration of one block at 44.1 kHz;
# - max_gpu_cpu_diff: largest gap between voice 0 on the device and on the
#   CPU over the timed blocks (the same inputs, so a sanity check).
# Every timing is the median over <blocks> blocks (default 100) after
# WARMUP blocks: over 20 blocks the GPU has not reached its clock yet, and
# the 1,024-voice APF block measured 360 us against 235 us over 200.
# `--state-bytes` prints the size of one voice, for the sweep to skip the
# voice counts the device memory cannot hold.
# Inputs are a fixed low-level signal on every channel, not an impulse, so
# that no state decays into denormals during the measurement.
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext
from max.gpu import global_idx
from max.algorithm import parallelize
from std.sys import argv
from std.time import perf_counter_ns

comptime GPU_BLOCK_DIM = 64
comptime CPU_CAP = 4096
comptime WARMUP = 20


def voices_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin],
    channels: Ptr[UInt64, MutAnyOrigin],
    nvoices: S32,
    n_ins: S32,
    n_outs: S32,
    count: S32,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(nvoices):
        return
    var dsp = dsps.unsafe_bitcast[mydsp]().unsafe_offset(v)
    var heads = (
        channels.unsafe_bitcast[Ptr[FaustFloat, MUT_NOTRK]]()
        .unsafe_offset(v * Int(n_ins + n_outs))
        .unsafe_origin_cast[MUT_NOTRK]()
    )
    dsp[].compute(
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


# Runs `compute` for voices [first, last) of `dsps`, each on the shared
# channel buffers of one worker (the outputs are overwritten; only the
# cost is wanted).
def run_voices(
    dsps: Ptr[mydsp, MUT_NOTRK],
    first: Int,
    last: Int,
    heads: Ptr[Ptr[FaustFloat, MUT_NOTRK], MUT_NOTRK],
    n_ins: Int,
    block: Int,
):
    for v in range(first, last):
        dsps.unsafe_offset(v)[].compute(
            S32(block),
            heads.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]]()
            .unsafe_mut_cast[False]()
            .unsafe_origin_cast[IMM_NOTRK](),
            heads.unsafe_offset(n_ins),
        )


# The cost of one block of `nvoices` voices on the CPU, `workers` slices
# run by `parallelize`, measured on `cpu_voices` voices and scaled.
def cpu_block(
    hosts: Ptr[mydsp, MUT_NOTRK],
    heads_all: Ptr[Ptr[FaustFloat, MUT_NOTRK], MUT_NOTRK],
    cpu_voices: Int,
    nvoices: Int,
    n_ins: Int,
    n_chans: Int,
    block: Int,
    workers: Int,
) -> Int:
    var t0 = perf_counter_ns()
    if workers == 1:
        run_voices(hosts, 0, cpu_voices, heads_all, n_ins, block)
    else:

        def slice(
            w: Int,
        ) {var hosts, var heads_all, var cpu_voices, var workers, var n_ins, var n_chans, var block}:
            var first = cpu_voices * w // workers
            var last = cpu_voices * (w + 1) // workers
            run_voices(
                hosts, first, last, heads_all.unsafe_offset(w * n_chans), n_ins, block
            )

        parallelize(slice, workers)
    var t1 = perf_counter_ns()
    return Int(t1 - t0) * nvoices // cpu_voices


def main() raises -> None:
    comptime assert dfaust == f32, "the Apple GPU computes in f32: -D DFAUST=DType.float32"
    var args = argv()
    if len(args) > 1 and args[1] == "--header":
        print(
            "program,voices,block,state_bytes,gpu_full_us,gpu_kernel_us,gpu_copy_us,"
            + "gpu_empty_us,cpu1_us,cpu4_us,cpu10_us,cpu_extrapolated,budget_us,"
            + "max_gpu_cpu_diff"
        )
        return
    if len(args) > 1 and args[1] == "--state-bytes":
        print(size_of[mydsp]())
        return
    if len(args) < 4:
        raise Error("usage: <binary> <program> <voices> <block> [<blocks>]")
    var program = String(args[1])
    var nvoices = atol(args[2])
    var block = atol(args[3])
    var nblocks = atol(args[4]) if len(args) > 4 else 100

    var ctx = DeviceContext()
    var host = unsafe_alloc[mydsp](nvoices)
    for v in range(nvoices):
        host.unsafe_offset(v).unsafe_write(mydsp())
        host.unsafe_offset(v)[].init(SAMPLE_RATE)
    var stride = size_of[mydsp]()
    var n_ins = Int(host[].get_num_inputs())
    var n_outs = Int(host[].get_num_outputs())
    var n_chans = max(n_ins + n_outs, 1)
    var floats = nvoices * n_chans * block

    var dsps = ctx.enqueue_create_buffer[DType.uint8](stride * nvoices)
    ctx.enqueue_copy(dst_buf=dsps, src_ptr=host.unsafe_bitcast[UInt8]())
    var data = ctx.enqueue_create_buffer[dfaust](floats)
    var staging = ctx.enqueue_create_host_buffer[dfaust](floats)
    var channels = ctx.enqueue_create_buffer[DType.uint64](nvoices * n_chans)
    ctx.enqueue_function[channels_kernel](
        channels, data, S32(nvoices), S32(n_chans), S32(block),
        grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
    )
    ctx.synchronize()

    # A fixed low-level input on every input channel of every voice.
    var signal = unsafe_alloc[FaustFloat](block)
    for i in range(block):
        signal[unsafe_offset=i] = FaustFloat(0.1 * sin(F64(i) * 0.05))

    # CPU side: separate voices (the host copies, already sent to the
    # device), CPU_CAP of them at most; one channel set per worker.
    var cpu_voices = min(nvoices, CPU_CAP)
    var extrapolated = 1 if cpu_voices < nvoices else 0
    comptime MAX_WORKERS = 10
    var cpu_heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](MAX_WORKERS * n_chans)
    var cpu_data = unsafe_alloc[FaustFloat](MAX_WORKERS * n_chans * block)
    for w in range(MAX_WORKERS):
        for c in range(n_chans):
            cpu_heads.unsafe_offset(w * n_chans + c).unsafe_write(
                cpu_data.unsafe_offset((w * n_chans + c) * block).unsafe_origin_cast[
                    MUT_NOTRK
                ]()
            )
        for c in range(n_ins):
            for i in range(block):
                cpu_data[unsafe_offset=(w * n_chans + c) * block + i] = signal[unsafe_offset=i]
    var hosts = host.unsafe_origin_cast[MUT_NOTRK]()
    var heads_all = cpu_heads.unsafe_origin_cast[MUT_NOTRK]()


    # GPU: the full block, the kernel alone, the copies alone, an empty
    # kernel. Voice 0 of the device is compared with voice 0 run alone on a
    # fresh CPU instance fed the same blocks.
    var ref_dsp = unsafe_alloc[mydsp](1)
    ref_dsp.unsafe_write(mydsp())
    ref_dsp[].init(SAMPLE_RATE)
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
            dsps, channels, S32(nvoices), S32(n_ins), S32(n_outs), S32(block),
            grid_dim=grid(nvoices), block_dim=GPU_BLOCK_DIM,
        )
        ctx.enqueue_copy(dst_buf=staging, src_buf=data)
        ctx.synchronize()
        var t1 = perf_counter_ns()
        # The reference voice, on the CPU, same block.
        for c in range(n_chans):
            for i in range(block):
                ref_data[unsafe_offset=c * block + i] = (
                    signal[unsafe_offset=i] if c < n_ins else FaustFloat(0.0)
                )
        ref_dsp[].compute(
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
            dsps, channels, S32(nvoices), S32(n_ins), S32(n_outs), S32(block),
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

    var cpu1 = List[Int]()
    var cpu4 = List[Int]()
    var cpu10 = List[Int]()
    var cpu_blocks = max(3, min(nblocks, 10))
    for b in range(cpu_blocks + 1):
        var a = cpu_block(hosts, heads_all, cpu_voices, nvoices, n_ins, n_chans, block, 1)
        var c4 = cpu_block(hosts, heads_all, cpu_voices, nvoices, n_ins, n_chans, block, 4)
        var c10 = cpu_block(hosts, heads_all, cpu_voices, nvoices, n_ins, n_chans, block, 10)
        if b >= 1:
            cpu1.append(a)
            cpu4.append(c4)
            cpu10.append(c10)

    var budget_ns = block * 1_000_000_000 // Int(SAMPLE_RATE)
    print(
        program + "," + String(nvoices) + "," + String(block) + "," + String(stride)
        + "," + us(median(full)) + "," + us(median(kernel)) + "," + us(median(copy))
        + "," + us(median(empty)) + "," + us(median(cpu1)) + "," + us(median(cpu4))
        + "," + us(median(cpu10)) + "," + String(extrapolated) + "," + us(budget_ns)
        + "," + String(max_diff)
    )
    host.unsafe_free()
    ref_dsp.unsafe_free()
    ref_heads.unsafe_free()
    ref_data.unsafe_free()
    cpu_heads.unsafe_free()
    cpu_data.unsafe_free()
    signal.unsafe_free()
