
# ---------------------------------------------------------------------------
# runimpulsemojogpu.mojo — many voices of one DSP on the GPU, one thread per
# voice (porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md, G1).
#
# Concatenated after `impulsemojo.mojo` and the generated DSP, so `mydsp`,
# the aliases, `ImpulseGui`, `format_sample` and `pad_left` are in scope.
# Build with `-D DFAUST=DType.float32` (the Apple GPU has no f64) and
# optionally `-D NVOICES=<n>` (default 64, even).
#
# Every voice is an ordinary `mydsp` initialised on the host and copied to
# the device; the kernel calls the unchanged scalar `compute` of its voice.
# Even voices get the impulse protocol of the CPU lane, odd voices get
# silence, buttons are held during the first block for all. The output is
# voice 0, printed as the CPU runner prints it, for the comparison with the
# C++ reference. The harness itself fails (non-zero exit) unless:
# - every even voice is bit-identical to voice 0, every odd voice to voice
#   1 (a voice reading another voice's state breaks one of the two);
# - voices 0 and 1 match the same f32 DSP run on the CPU, with the impulse
#   and with zero input, within GPU_CPU_TOLERANCE times max(1, the CPU peak
#   of voice 0) (absolute for signals within [-1, 1]): the device is
#   checked against the same program on the host, not against the f64 C++
#   reference, which the CPU lanes cover. `-D GPU_CPU_CHECK=0` skips this
#   check for programs whose GPU and CPU runs drift apart from a
#   rounding-level start (the code generation differs, the GPU flushes
#   denormals) amplified by feedback; the other checks still apply;
# - voice 0 is not silent where its CPU run is not (a kernel can return
#   without writing anything, with no error raised).
#
# The channel data is passed to the compute kernel although it reaches it
# through the channel table: on Metal a kernel reading a buffer that is not
# one of its arguments read zeros and stale values (found in G3.5).
#
# Channel pointers live in device memory: a pointer into the thread's
# private memory passed to a `compute` that is not inlined loses its
# writes on Metal (Mojo 1.3.0.dev2026110705), silently.
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext
from max.gpu import global_idx
from std.sys import stderr

comptime NVOICES = get_defined_int["NVOICES", 64]()
comptime GPU_BLOCK_DIM = 64
comptime GPU_CPU_CHECK = get_defined_int["GPU_CPU_CHECK", 1]()
comptime GPU_CPU_TOLERANCE = 1e-4


def voices_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin],
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


# Writes the device address of every channel of every voice into `channels`.
def channels_kernel(
    channels: Ptr[UInt64, MutAnyOrigin],
    data: Ptr[FaustFloat, MutAnyOrigin],
    nvoices: S32,
    n_chans: S32,
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
            (v * Int(n_chans) + c) * BLOCK_SIZE
        ).unsafe_origin_cast[MUT_NOTRK]()


# Sets one control zone, at `offset` bytes into every voice, to `value`.
def zone_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin],
    nvoices: S32,
    stride: S64,
    offset: S64,
    value: FaustFloat,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(nvoices):
        return
    dsps.unsafe_offset(v * Int(stride) + Int(offset)).unsafe_bitcast[
        FaustFloat
    ]()[] = value


def grid(n: Int) -> Int:
    return (n + GPU_BLOCK_DIM - 1) // GPU_BLOCK_DIM


def main() raises -> None:
    comptime assert dfaust == f32, "the Apple GPU computes in f32: -D DFAUST=DType.float32"
    comptime assert NVOICES >= 2 and NVOICES % 2 == 0, "NVOICES must be even"
    var ctx = DeviceContext()

    # Host voices, plus one CPU instance run with zero input for voice 1.
    var host = unsafe_alloc[mydsp](NVOICES + 1)
    for v in range(NVOICES + 1):
        host.unsafe_offset(v).unsafe_write(mydsp())
        host.unsafe_offset(v)[].init(SAMPLE_RATE)
    var ui = ImpulseGui()
    host[].build_user_interface(ui)
    var cpu_ui = ImpulseGui()
    host.unsafe_offset(NVOICES)[].build_user_interface(cpu_ui)
    var stride = size_of[mydsp]()
    var base = Int(host.unsafe_bitcast[UInt8]().unsafe_origin_cast[MUT_NOTRK]().__int__())
    var button_offsets = List[Int]()
    for button in ui.buttons:
        button_offsets.append(Int(button.__int__()) - base)
    ui.set_buttons(True)
    for v in range(1, NVOICES):
        for off in button_offsets:
            host.unsafe_bitcast[UInt8]().unsafe_offset(v * stride + off).unsafe_bitcast[
                FaustFloat
            ]()[] = FaustFloat(1.0)

    var n_ins = Int(host[].get_num_inputs())
    var n_outs = Int(host[].get_num_outputs())
    var n_chans = n_ins + n_outs
    var frame_floats = max(n_chans, 1) * BLOCK_SIZE

    var dsps = ctx.enqueue_create_buffer[DType.uint8](stride * NVOICES)
    ctx.enqueue_copy(dst_buf=dsps, src_ptr=host.unsafe_bitcast[UInt8]())
    var data = ctx.enqueue_create_buffer[dfaust](NVOICES * frame_floats)
    var staging = ctx.enqueue_create_host_buffer[dfaust](NVOICES * frame_floats)
    var channels = ctx.enqueue_create_buffer[DType.uint64](NVOICES * max(n_chans, 1))
    ctx.enqueue_function[channels_kernel](
        channels, data, S32(NVOICES), S32(n_chans),
        grid_dim=grid(NVOICES), block_dim=GPU_BLOCK_DIM,
    )
    ctx.synchronize()

    # CPU channels for the zero-input run.
    var cpu_heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](max(n_chans, 1))
    var cpu_data = unsafe_alloc[FaustFloat](frame_floats)
    for c in range(n_chans):
        cpu_heads.unsafe_offset(c).unsafe_write(
            cpu_data.unsafe_offset(c * BLOCK_SIZE).unsafe_origin_cast[MUT_NOTRK]()
        )
    var cpu_in = cpu_heads.unsafe_origin_cast[MUT_NOTRK]()
    var cpu_out = cpu_heads.unsafe_offset(n_ins).unsafe_origin_cast[MUT_NOTRK]()

    var out = String("number_of_inputs  : ") + pad_left(n_ins, 3) + "\n"
    out += String("number_of_outputs : ") + pad_left(n_outs, 3) + "\n"
    out += String("number_of_frames  : ") + pad_left(FRAMES, 6) + "\n"

    var line = 0
    var block = 0
    var remaining = FRAMES
    var max_silent_diff = F64(0.0)
    var max_impulse_diff = F64(0.0)
    var cpu_peak = F64(0.0)
    var voice0_energy = F64(0.0)
    var cpu_impulse_energy = F64(0.0)
    var cpu_silent = host.unsafe_offset(NVOICES)
    var cpu2 = unsafe_alloc[mydsp](1)
    cpu2.unsafe_write(mydsp())
    cpu2[].init(SAMPLE_RATE)
    var cpu2_ui = ImpulseGui()
    cpu2[].build_user_interface(cpu2_ui)
    var cpu2_heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](max(n_chans, 1))
    var cpu2_data = unsafe_alloc[FaustFloat](frame_floats)
    for c in range(n_chans):
        cpu2_heads.unsafe_offset(c).unsafe_write(
            cpu2_data.unsafe_offset(c * BLOCK_SIZE).unsafe_origin_cast[MUT_NOTRK]()
        )
    var cpu2_in = cpu2_heads.unsafe_origin_cast[MUT_NOTRK]()
    var cpu2_out = cpu2_heads.unsafe_offset(n_ins).unsafe_origin_cast[MUT_NOTRK]()

    while remaining > 0:
        var count = min(BLOCK_SIZE, remaining)
        # Inputs: impulse on even voices in the first block, silence otherwise.
        for i in range(NVOICES * frame_floats):
            staging[i] = FaustFloat(0.0)
        if block == 0:
            for v in range(0, NVOICES, 2):
                for c in range(n_ins):
                    staging[(v * n_chans + c) * BLOCK_SIZE] = FaustFloat(1.0)
        if block == 1:
            for off in button_offsets:
                ctx.enqueue_function[zone_kernel](
                    dsps, S32(NVOICES), S64(stride), S64(off), FaustFloat(0.0),
                    grid_dim=grid(NVOICES), block_dim=GPU_BLOCK_DIM,
                )
        ctx.enqueue_copy(dst_buf=data, src_buf=staging)
        ctx.enqueue_function[voices_kernel](
            dsps, data, channels, S32(NVOICES), S32(n_ins), S32(n_outs), S32(count),
            grid_dim=grid(NVOICES), block_dim=GPU_BLOCK_DIM,
        )
        ctx.enqueue_copy(dst_buf=staging, src_buf=data)
        ctx.synchronize()

        # The same block on the CPU: zero input (voice 1) and impulse (voice 0).
        for i in range(frame_floats):
            cpu_data[unsafe_offset=i] = FaustFloat(0.0)
            cpu2_data[unsafe_offset=i] = FaustFloat(0.0)
        if block == 0:
            for c in range(n_ins):
                cpu2_in[unsafe_offset=c][unsafe_offset=0] = FaustFloat(1.0)
        cpu_ui.set_buttons(block == 0)
        cpu2_ui.set_buttons(block == 0)
        cpu_silent[].compute(
            S32(count), cpu_in.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]](), cpu_out
        )
        cpu2[].compute(
            S32(count), cpu2_in.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]](), cpu2_out
        )

        for c in range(n_outs):
            for i in range(count):
                var at0 = (0 * n_chans + n_ins + c) * BLOCK_SIZE + i
                var at1 = (1 * n_chans + n_ins + c) * BLOCK_SIZE + i
                var s0 = staging[at0]
                var s1 = staging[at1]
                for v in range(2, NVOICES):
                    var at = (v * n_chans + n_ins + c) * BLOCK_SIZE + i
                    var want = s0 if v % 2 == 0 else s1
                    if staging[at] != want:
                        raise Error(
                            String("voice ") + String(v) + " differs from voice "
                            + String(v % 2) + " at frame " + String(line + i)
                        )
                var d = abs(F64(s1) - F64(cpu_out[unsafe_offset=c][unsafe_offset=i]))
                if d > max_silent_diff:
                    max_silent_diff = d
                voice0_energy += abs(F64(s0))
                var want0 = F64(cpu2_out[unsafe_offset=c][unsafe_offset=i])
                var d0 = abs(F64(s0) - want0)
                if d0 > max_impulse_diff:
                    max_impulse_diff = d0
                if abs(want0) > cpu_peak:
                    cpu_peak = abs(want0)
                cpu_impulse_energy += abs(
                    F64(cpu2_out[unsafe_offset=c][unsafe_offset=i])
                )

        for i in range(count):
            out += pad_left(line, 6) + " : "
            line += 1
            for c in range(n_outs):
                var sample = F64(staging[(n_ins + c) * BLOCK_SIZE + i])
                if isnan(sample) or isinf(sample):
                    raise Error("non-finite DSP output")
                if abs(sample) < 0.000001:
                    sample = 0.0
                out += " " + format_sample(sample)
            out += "\n"
        remaining -= count
        block += 1

    print(
        "gpu-vs-cpu impulse", max_impulse_diff, "silent", max_silent_diff,
        "peak", cpu_peak, file=stderr,
    )
    var tolerance = GPU_CPU_TOLERANCE * max(1.0, cpu_peak)
    if GPU_CPU_CHECK != 0 and max_impulse_diff > tolerance:
        raise Error(
            String("voice 0 differs from its CPU run by ") + String(max_impulse_diff)
        )
    if GPU_CPU_CHECK != 0 and max_silent_diff > tolerance:
        raise Error(
            String("voice 1 differs from its CPU run by ") + String(max_silent_diff)
        )
    if cpu_impulse_energy > 1e-6 and voice0_energy == 0.0:
        raise Error("voice 0 is silent on the GPU and not on the CPU")
    print(out, end="")
    host.unsafe_free()
    cpu2.unsafe_free()
    cpu_heads.unsafe_free()
    cpu_data.unsafe_free()
    cpu2_heads.unsafe_free()
    cpu2_data.unsafe_free()
