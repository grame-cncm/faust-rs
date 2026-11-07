# ---------------------------------------------------------------------------
# impulsemojo.mojo — impulse-test runtime for the faust-rs Mojo backend.
#
# Self-contained: it needs no file of the Mojo architecture tree. It defines
# the names the generated `-lang mojo` code expects from that tree (the type
# aliases of `conf/prelude.mojo` and the `FaustDsp`, `FaustGui` and `FaustMeta`
# traits of `dsp/dsp.mojo`, `gui/gui.mojo` and `meta/meta.mojo`, as in
# grame-cncm/faust#1293 at 05ac86fb1), then runs the C++ impulse protocol:
# 44.1 kHz, 64-frame blocks, a first-block input impulse, buttons held for
# that block, and the first 15,000 frames printed for the scalar-prefix
# comparison against the C++ oracle.
#
# The generated DSP is concatenated after this file, then
# `runimpulsemojo.mojo`. Build with `-D DFAUST=DType.float64`.
# ---------------------------------------------------------------------------

from std.math import *
from std.memory import *
from std.memory.alloc import *
from std.memory.unsafe_pointer import *
from std.sys.defines import *
from std.sys.info import *

comptime U32 = UInt32
comptime U64 = UInt64
comptime S32 = Int32
comptime S64 = Int64
comptime F32 = Float32
comptime F64 = Float64
comptime s32 = S32.dtype
comptime s64 = S64.dtype
comptime f32 = F32.dtype
comptime f64 = F64.dtype

comptime Arr = Array

comptime dfaust = get_defined_dtype["DFAUST", f32]()
comptime FaustFloat = Scalar[dfaust]

comptime MUT_NOTRK = MutUntrackedOrigin
comptime IMM_NOTRK = ImmUntrackedOrigin
comptime Ptr[T: AnyType = NoneType, ori: Origin = MUT_NOTRK] = Pointer[T, ori]
comptime ImmStreams = Ptr[Ptr[FaustFloat, IMM_NOTRK], IMM_NOTRK]
comptime MutStreams = Ptr[Ptr[FaustFloat, MUT_NOTRK], MUT_NOTRK]


trait FaustMeta:
    @always_inline
    def declare(mut meta, var key: String, var val: String) -> None:
        pass


trait FaustGui:
    @always_inline
    def open_tab_box(mut ui, var label: String) -> None:
        pass

    @always_inline
    def open_horizontal_box(mut ui, var label: String) -> None:
        pass

    @always_inline
    def open_vertical_box(mut ui, var label: String) -> None:
        pass

    @always_inline
    def close_box(mut ui) -> None:
        pass

    @always_inline
    def add_button(mut ui, var label: String, mut zone: FaustFloat) -> None:
        pass

    @always_inline
    def add_check_button(mut ui, var label: String, mut zone: FaustFloat) -> None:
        pass

    @always_inline
    def add_vertical_slider(
        mut ui,
        var label: String,
        mut zone: FaustFloat,
        var init: FaustFloat,
        var min: FaustFloat,
        var max: FaustFloat,
        var step: FaustFloat,
    ) -> None:
        pass

    @always_inline
    def add_horizontal_slider(
        mut ui,
        var label: String,
        mut zone: FaustFloat,
        var init: FaustFloat,
        var min: FaustFloat,
        var max: FaustFloat,
        var step: FaustFloat,
    ) -> None:
        pass

    @always_inline
    def add_num_entry(
        mut ui,
        var label: String,
        mut zone: FaustFloat,
        var init: FaustFloat,
        var min: FaustFloat,
        var max: FaustFloat,
        var step: FaustFloat,
    ) -> None:
        pass

    @always_inline
    def add_vertical_bargraph(
        mut ui,
        var label: String,
        mut zone: FaustFloat,
        var min: FaustFloat,
        var max: FaustFloat,
    ) -> None:
        pass

    @always_inline
    def add_horizontal_bargraph(
        mut ui,
        var label: String,
        mut zone: FaustFloat,
        var min: FaustFloat,
        var max: FaustFloat,
    ) -> None:
        pass

    @always_inline
    def declare(mut ui, mut zone: FaustFloat, var key: String, var val: String) -> None:
        pass


trait FaustDsp:
    def get_num_inputs(imm dsp) -> S32: ...
    def get_num_outputs(imm dsp) -> S32: ...
    def get_sample_rate(imm dsp) -> S32: ...
    def class_init(mut dsp, imm sample_rate: S32) -> None: ...
    def instance_constants(mut dsp, imm sample_rate: S32) -> None: ...
    def instance_reset_user_interface(mut dsp) -> None: ...
    def instance_clear(mut dsp) -> None: ...
    def instance_init(mut dsp, imm sample_rate: S32) -> None: ...
    def init(mut dsp, imm sample_rate: S32) -> None: ...
    def get_json(imm dsp) -> String: ...
    def metadata(imm dsp, mut meta: Some[FaustMeta]) -> None: ...
    def build_user_interface(mut dsp, mut ui: Some[FaustGui]) -> None: ...
    def compute(
        mut dsp, var count: S32, var inputs: ImmStreams, var outputs: MutStreams
    ) -> None: ...


# Keeps the zone of every button, to hold them down during the first block.
struct ImpulseGui(FaustGui):
    var buttons: List[Ptr[FaustFloat, MUT_NOTRK]]

    def __init__(out ui):
        ui.buttons = List[Ptr[FaustFloat, MUT_NOTRK]]()

    def add_button(mut ui, var label: String, mut zone: FaustFloat) -> None:
        ui.buttons.append(
            Ptr(to=zone).unsafe_bitcast[FaustFloat]().unsafe_origin_cast[MUT_NOTRK]()
        )

    def set_buttons(mut ui, var down: Bool) -> None:
        var value = FaustFloat(1.0) if down else FaustFloat(0.0)
        for button in ui.buttons:
            button[] = value


comptime SAMPLE_RATE = S32(44100)
comptime BLOCK_SIZE = 64
comptime FRAMES = 15000


# `printf("%8.6f")` of the C++ runner: six decimals, right-aligned on 8.
def format_sample(value: F64) -> String:
    var negative = value < 0.0
    var magnitude = -value if negative else value
    var scaled = S64(round(magnitude * 1000000.0))
    var text = String("-") if negative and scaled != 0 else String("")
    text += String(scaled // 1000000)
    text += "."
    var frac = String(scaled % 1000000)
    for _ in range(6 - frac.byte_length()):
        text += "0"
    text += frac
    var padded = String("")
    for _ in range(8 - text.byte_length()):
        padded += " "
    return padded + text


def pad_left(value: Int, width: Int) -> String:
    var text = String(value)
    var padded = String("")
    for _ in range(width - text.byte_length()):
        padded += " "
    return padded + text


# The DSP is passed by pointer: `ImpulseGui` holds pointers into it, and a
# `mut` argument made from `dsp[]` would let `compute` run on a copy.
def run_impulse[Dsp: FaustDsp](var dsp: Ptr[Dsp], mut ui: ImpulseGui) raises -> None:
    var n_ins = Int(dsp[].get_num_inputs())
    var n_outs = Int(dsp[].get_num_outputs())
    var n_chans = n_ins + n_outs
    var heads = unsafe_alloc[Ptr[FaustFloat, MUT_NOTRK]](max(n_chans, 1))
    var data = unsafe_alloc[FaustFloat](max(n_chans, 1) * BLOCK_SIZE)
    for c in range(n_chans):
        heads.unsafe_offset(c).unsafe_write(
            data.unsafe_offset(c * BLOCK_SIZE).unsafe_origin_cast[MUT_NOTRK]()
        )
    var inputs = heads.unsafe_origin_cast[MUT_NOTRK]()
    var outputs = heads.unsafe_offset(n_ins).unsafe_origin_cast[MUT_NOTRK]()

    var out = String("number_of_inputs  : ") + pad_left(n_ins, 3) + "\n"
    out += String("number_of_outputs : ") + pad_left(n_outs, 3) + "\n"
    out += String("number_of_frames  : ") + pad_left(FRAMES, 6) + "\n"

    var line = 0
    var block = 0
    var remaining = FRAMES
    while remaining > 0:
        for i in range(n_chans * BLOCK_SIZE):
            data[unsafe_offset=i] = FaustFloat(0.0)
        if block == 0:
            for c in range(n_ins):
                inputs[unsafe_offset=c][unsafe_offset=0] = FaustFloat(1.0)
        ui.set_buttons(block == 0)
        var count = min(BLOCK_SIZE, remaining)
        dsp[].compute(
            S32(count), inputs.unsafe_bitcast[Ptr[FaustFloat, IMM_NOTRK]](), outputs
        )
        for i in range(count):
            out += pad_left(line, 6) + " : "
            line += 1
            for c in range(n_outs):
                var sample = F64(outputs[unsafe_offset=c][unsafe_offset=i])
                if isnan(sample) or isinf(sample):
                    raise Error("non-finite DSP output")
                if abs(sample) < 0.000001:
                    sample = 0.0
                out += " " + format_sample(sample)
            out += "\n"
        remaining -= count
        block += 1
    print(out, end="")
    data.unsafe_free()
    heads.unsafe_free()
