
# ---------------------------------------------------------------------------
# runimpulsemojogpubatch.mojo — many voices of one DSP on the GPU with the
# batch contract: one struct for N voices, state as a structure of arrays
# (`MojoOptions::batch`; GPU batch plan G3.5,
# porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md).
#
# Concatenated after `impulsemojo.mojo` and the output of the compiler's
# `mojo_batch` example. Build with `-D DFAUST=DType.float32` and optionally
# `-D NVOICES=<n>` (default 64, even).
#
# The checks of `runimpulsemojogpu.mojo`, on the batch contract: even voices
# get the impulse protocol of the CPU lane, odd voices silence, buttons are
# held during the first block for all (`set_control`, through the control
# table); the output is voice 0. The harness fails unless every even voice is
# bit-identical to voice 0 and every odd one to voice 1, voices 0 and 1 match
# a two-voice batch instance run on the host within GPU_CPU_TOLERANCE times
# max(1, peak) (skipped with `-D GPU_CPU_CHECK=0`), and voice 0 is not silent
# where the host run is not. That the batch layout itself preserves every
# voice is checked on the CPU, bit for bit, by
# crates/compiler/tests/batch_voices_interp.rs (G3.4).
#
# Every buffer a kernel reaches is one of its arguments (the arenas behind
# the struct, the channel data behind the channel table): on Metal a kernel
# reading a buffer that is not one of its arguments read zeros and stale
# values (Mojo 1.3.0.dev2026110705).
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext
from max.gpu import global_idx
from std.sys import stderr

comptime NVOICES = get_defined_int["NVOICES", 64]()
comptime GPU_BLOCK_DIM = 64
comptime GPU_CPU_CHECK = get_defined_int["GPU_CPU_CHECK", 1]()
comptime GPU_CPU_TOLERANCE = 1e-4


# Records, in control-table order, which controls are buttons.
struct ControlGui(FaustGui):
    var count: Int
    var buttons: List[Int]

    def __init__(out ui):
        ui.count = 0
        ui.buttons = List[Int]()

    def add_button(mut ui, var label: String, mut zone: FaustFloat) -> None:
        ui.buttons.append(ui.count)
        ui.count += 1

    def add_check_button(mut ui, var label: String, mut zone: FaustFloat) -> None:
        ui.count += 1

    def add_vertical_slider(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.count += 1

    def add_horizontal_slider(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.count += 1

    def add_num_entry(
        mut ui, var label: String, mut zone: FaustFloat, var init: FaustFloat,
        var min: FaustFloat, var max: FaustFloat, var step: FaustFloat,
    ) -> None:
        ui.count += 1

    def add_vertical_bargraph(
        mut ui, var label: String, mut zone: FaustFloat, var min: FaustFloat, var max: FaustFloat
    ) -> None:
        ui.count += 1

    def add_horizontal_bargraph(
        mut ui, var label: String, mut zone: FaustFloat, var min: FaustFloat, var max: FaustFloat
    ) -> None:
        ui.count += 1


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


def control_kernel(
    dsps: Ptr[UInt8, MutAnyOrigin],
    state: Ptr[UInt8, MutAnyOrigin],
    nvoices: S32,
    index: Int32,
    value: FaustFloat,
) -> None:
    var v = Int(global_idx.x)
    if v >= Int(nvoices):
        return
    dsps.unsafe_bitcast[mydsp]()[].set_control(S32(v), Int(index), value)


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


def grid(n: Int) -> Int:
    return (n + GPU_BLOCK_DIM - 1) // GPU_BLOCK_DIM


def main() raises -> None:
    comptime assert dfaust == f32, "the Apple GPU computes in f32: -D DFAUST=DType.float32"
    comptime assert NVOICES >= 2 and NVOICES % 2 == 0, "NVOICES must be even"
    var ctx = DeviceContext()

    # The device: state for NVOICES voices, the struct built over it here,
    # class_init here (shared fields), then the struct copied over.
    var state = ctx.enqueue_create_buffer[DType.uint8](max(mydsp.state_bytes(NVOICES), 8))
    ctx.synchronize()
    var host = unsafe_alloc[mydsp](1)
    host.unsafe_write(mydsp(S32(NVOICES), state.unsafe_ptr().unsafe_origin_cast[MUT_NOTRK]()))
    host[].class_init(SAMPLE_RATE)
    var gui = ControlGui()
    host[].build_user_interface(gui)
    var dsps = ctx.enqueue_create_buffer[DType.uint8](size_of[mydsp]())
    ctx.enqueue_copy(dst_buf=dsps, src_ptr=host.unsafe_bitcast[UInt8]())
    ctx.enqueue_function[init_kernel](
        dsps, state, S32(NVOICES), SAMPLE_RATE,
        grid_dim=grid(NVOICES), block_dim=GPU_BLOCK_DIM,
    )
    for i in gui.buttons:
        ctx.enqueue_function[control_kernel](
            dsps, state, S32(NVOICES), Int32(i), FaustFloat(1.0),
            grid_dim=grid(NVOICES), block_dim=GPU_BLOCK_DIM,
        )

    # The host reference: a two-voice batch instance, voice 0 fed the impulse
    # and voice 1 silence, buttons held for both during the first block.
    var ref_state = unsafe_alloc[UInt8](max(mydsp.state_bytes(2), 8))
    var cpu = unsafe_alloc[mydsp](1)
    cpu.unsafe_write(mydsp(S32(2), ref_state.unsafe_origin_cast[MUT_NOTRK]()))
    cpu[].class_init(SAMPLE_RATE)
    for v in range(2):
        cpu[].instance_init(S32(v), SAMPLE_RATE)
        for i in gui.buttons:
            cpu[].set_control(S32(v), i, FaustFloat(1.0))

    var n_ins = Int(host[].get_num_inputs())
    var n_outs = Int(host[].get_num_outputs())
    var n_chans = n_ins + n_outs
    var frame_floats = max(n_chans, 1) * BLOCK_SIZE

    var data = ctx.enqueue_create_buffer[dfaust](NVOICES * frame_floats)
    var staging = ctx.enqueue_create_host_buffer[dfaust](NVOICES * frame_floats)
    var channels = ctx.enqueue_create_buffer[DType.uint64](NVOICES * max(n_chans, 1))
    ctx.enqueue_function[channels_kernel](
        channels, data, S32(NVOICES), S32(n_chans),
        grid_dim=grid(NVOICES), block_dim=GPU_BLOCK_DIM,
    )
    ctx.synchronize()

    var cpu_heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](2 * max(n_chans, 1))
    var cpu_data = unsafe_alloc[FaustFloat](2 * frame_floats)
    for v in range(2):
        for c in range(n_chans):
            cpu_heads.unsafe_offset(v * n_chans + c).unsafe_write(
                cpu_data.unsafe_offset((v * n_chans + c) * BLOCK_SIZE).unsafe_origin_cast[
                    MUT_NOTRK
                ]()
            )

    var out = String("number_of_inputs  : ") + pad_left(n_ins, 3) + "\n"
    out += String("number_of_outputs : ") + pad_left(n_outs, 3) + "\n"
    out += String("number_of_frames  : ") + pad_left(FRAMES, 6) + "\n"

    var line = 0
    var block = 0
    var remaining = FRAMES
    var max_diff = List[F64](length=2, fill=0.0)
    var cpu_peak = F64(0.0)
    var voice0_energy = F64(0.0)
    var cpu0_energy = F64(0.0)
    while remaining > 0:
        var count = min(BLOCK_SIZE, remaining)
        for i in range(NVOICES * frame_floats):
            staging[i] = FaustFloat(0.0)
        for i in range(2 * frame_floats):
            cpu_data[unsafe_offset=i] = FaustFloat(0.0)
        if block == 0:
            for v in range(0, NVOICES, 2):
                for c in range(n_ins):
                    staging[(v * n_chans + c) * BLOCK_SIZE] = FaustFloat(1.0)
            for c in range(n_ins):
                cpu_data[unsafe_offset=c * BLOCK_SIZE] = FaustFloat(1.0)
        if block == 1:
            for i in gui.buttons:
                ctx.enqueue_function[control_kernel](
                    dsps, state, S32(NVOICES), Int32(i), FaustFloat(0.0),
                    grid_dim=grid(NVOICES), block_dim=GPU_BLOCK_DIM,
                )
                for v in range(2):
                    cpu[].set_control(S32(v), i, FaustFloat(0.0))
        ctx.enqueue_copy(dst_buf=data, src_buf=staging)
        ctx.enqueue_function[voices_kernel](
            dsps, state, data, channels, S32(NVOICES), S32(n_ins), S32(n_outs), S32(count),
            grid_dim=grid(NVOICES), block_dim=GPU_BLOCK_DIM,
        )
        ctx.enqueue_copy(dst_buf=staging, src_buf=data)
        ctx.synchronize()
        for v in range(2):
            var heads = cpu_heads.unsafe_offset(v * n_chans).unsafe_origin_cast[MUT_NOTRK]()
            cpu[].compute(
                S32(v),
                S32(count),
                heads.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]](),
                heads.unsafe_offset(n_ins).unsafe_origin_cast[MUT_NOTRK](),
            )

        for c in range(n_outs):
            for i in range(count):
                var s0 = staging[(0 * n_chans + n_ins + c) * BLOCK_SIZE + i]
                var s1 = staging[(1 * n_chans + n_ins + c) * BLOCK_SIZE + i]
                for v in range(2, NVOICES):
                    var want = s0 if v % 2 == 0 else s1
                    if staging[(v * n_chans + n_ins + c) * BLOCK_SIZE + i] != want:
                        raise Error(
                            String("voice ") + String(v) + " differs from voice "
                            + String(v % 2) + " at frame " + String(line + i)
                        )
                for v in range(2):
                    var got = F64(staging[(v * n_chans + n_ins + c) * BLOCK_SIZE + i])
                    var want = F64(cpu_data[unsafe_offset=(v * n_chans + n_ins + c) * BLOCK_SIZE + i])
                    if abs(got - want) > max_diff[v]:
                        max_diff[v] = abs(got - want)
                    if v == 0:
                        voice0_energy += abs(got)
                        cpu0_energy += abs(want)
                        if abs(want) > cpu_peak:
                            cpu_peak = abs(want)

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
        "gpu-vs-cpu impulse", max_diff[0], "silent", max_diff[1], "peak", cpu_peak,
        file=stderr,
    )
    var tolerance = GPU_CPU_TOLERANCE * max(1.0, cpu_peak)
    if GPU_CPU_CHECK != 0 and max_diff[0] > tolerance:
        raise Error(String("voice 0 differs from its CPU run by ") + String(max_diff[0]))
    if GPU_CPU_CHECK != 0 and max_diff[1] > tolerance:
        raise Error(String("voice 1 differs from its CPU run by ") + String(max_diff[1]))
    if cpu0_energy > 1e-6 and voice0_energy == 0.0:
        raise Error("voice 0 is silent on the GPU and not on the CPU")
    print(out, end="")
    host.unsafe_free()
    cpu.unsafe_free()
    ref_state.unsafe_free()
    cpu_heads.unsafe_free()
    cpu_data.unsafe_free()
