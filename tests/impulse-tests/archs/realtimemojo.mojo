
# ---------------------------------------------------------------------------
# realtimemojo.mojo — the real-time options and the per-block timing split of
# the GPU players (polymojogpubatch.mojo, choirmojogpubatch.mojo,
# bellsmojogpubatch.mojo), and the audio output they share: PortAudio's
# blocking API or cpal.
#
# Concatenated before a player, after `impulsemojo.mojo` and the generated
# programs. macOS only: linked with `-lportaudio` (the CoreAudio host API,
# for `PaMacCore_GetStreamOutputDevice`), `-lcpal_bridge`
# (tools/cpal-bridge, a C ABI over the cpal crate) and `-framework
# CoreAudio`.
#
# The options are words on a player's command line:
#
#   rt=off    (default) the loop runs on the main thread at timeshare
#             priority.
#   rt=tc     the main thread takes the Mach time-constraint policy before
#             the loop (`thread_policy_set`, THREAD_TIME_CONSTRAINT_POLICY on
#             `pthread_mach_thread_np(pthread_self())`): period and
#             constraint one block, computation half a block, preemptible.
#   rt=wg     the main thread joins the output device's IO workgroup once the
#             stream runs (`PaMacCore_GetStreamOutputDevice`, then
#             `kAudioDevicePropertyIOThreadOSWorkgroup` read with
#             `AudioObjectGetPropertyData`, then `os_workgroup_join`), and
#             leaves it before the stream stops.
#   rt=tc+wg  both.
#   pipeline  the GPU work of block n + 1 is submitted before block n is
#             written: the loop waits for the event recorded after block n
#             instead of the whole queue, so the GPU computes block n + 1
#             while `Pa_WriteStream` blocks. The output carries one block more
#             of latency.
#   latency=<ms>  the suggested output latency (two blocks by default).
#             PortAudio's blocking ring holds twice that, rounded up to a
#             power of two frames, and at least three blocks: 1,024 frames
#             (21.3 ms) for the default at 256 frames. That ring is the
#             cushion against a late block; the output latency PortAudio
#             reports (43 ms there) adds the device's own buffers to it.
#   audio=pa  (default) PortAudio's blocking API (`Pa_WriteStream`), which
#             waits for room in its ring by sleeping 5 ms at a time.
#   audio=cpal  cpal (tools/cpal-bridge): the loop writes into a ring that
#             cpal's output callback reads on the CoreAudio IO thread; a
#             write that finds the ring full parks until the next callback.
#   ring=<frames>  with cpal, the ring's size (default: the size PortAudio
#             would choose for the same latency, so that both start with the
#             same cushion). It starts full of silence, as PortAudio's does.
#   buffer=<frames>  with cpal, the device buffer (`BufferSize::Fixed`; one
#             block by default, 0 for the device's default).
#
# The timing split: each block of the loop is cut into four timed parts,
# printed on a `timing` line as median, 99th percentile and maximum in µs:
#
#   enqueue   the host's control updates and the enqueueing of the block's
#             copies and kernels;
#   wait      the wait for the GPU (`ctx.synchronize()`, or the block's event
#             with `pipeline`);
#   host      the host's work between the wait and the write (scaling, the
#             CPU reverb of the choir, the busy wait of `stress_us`);
#   write     `Pa_WriteStream`, which blocks while PortAudio's ring is full
#             (the first write, which also waits for the device to start
#             playing, is printed apart as `first_write_us`);
#
# and `gap`, the time from the return of one `Pa_WriteStream` to the next
# call, during which PortAudio's ring is not refilled; `fill`, what the ring
# still holds just before each write (`Pa_GetStreamWriteAvailable`), printed
# as its minimum and 1st percentile in ms beside the ring's size (`ring_ms`).
# A block under-runs when the ring empties: a gap longer than the fill left
# after the previous write. `over_budget` counts the gaps longer than one
# block period; `thread_cpu_us` is the main thread's CPU time per block. The
# line also gives the status of each request: `tc_status`
# (`thread_policy_set`'s kern_return_t), `wg_status` (`os_workgroup_join`'s
# result, or the CoreAudio status when the workgroup could not be read), -1
# when not asked. With cpal it adds the bridge's counters: callbacks, those
# that found the ring short (`cpal_short_callbacks`, the underruns the
# callback saw), CoreAudio overloads (`cpal_xruns`), the callback sizes, the
# longest time between two callbacks, the lowest fill a callback saw, the
# device buffer and the device latency (cpal's playback minus callback
# instant: device buffer, device latency and safety offset).
#
# With rt=wg, the workgroup is that of PortAudio's stream device, or of the
# default output device (the one cpal opens).
# ---------------------------------------------------------------------------

from max.gpu.host import DeviceContext, DeviceEvent, HostBuffer
from std.ffi import external_call
from std.time import perf_counter_ns

comptime RT_PA_FLOAT32 = UInt64(1)
comptime RT_THREAD_TIME_CONSTRAINT_POLICY = UInt32(2)
comptime RT_CLOCK_THREAD_CPUTIME_ID = UInt32(16)
# 'oswg', 'glob' and the main element of AudioObjectPropertyAddress.
comptime RT_IO_THREAD_OS_WORKGROUP = UInt32(0x6F737767)
comptime RT_SCOPE_GLOBAL = UInt32(0x676C6F62)
# 'dOut': kAudioHardwarePropertyDefaultOutputDevice of the system object (1).
comptime RT_DEFAULT_OUTPUT_DEVICE = UInt32(0x644F7574)
comptime RT_PA_OUTPUT_UNDERFLOWED = Int32(-9980)
# The 11 u64 counters of CpalBridgeStats (tools/cpal-bridge/src/lib.rs).
comptime RT_CPAL_STATS = 11


def rt_address[T: AnyType](p: Ptr[T, MUT_NOTRK]) -> UInt64:
    return UInt64(Int(p.__int__()))


# The options a player reads from its command line.
struct RtOptions(ImplicitlyCopyable, Movable):
    var tc: Bool
    var wg: Bool
    var pipeline: Bool
    var latency_ms: F64
    var cpal: Bool
    var ring: Int
    var buffer: Int

    def __init__(out o):
        o.tc = False
        o.wg = False
        o.pipeline = False
        o.latency_ms = -1.0
        o.cpal = False
        o.ring = -1
        o.buffer = -1

    # The suggested output latency in seconds: `latency=`, or two blocks.
    def latency_s(o, block: Int, rate: S32) -> F64:
        if o.latency_ms > 0.0:
            return o.latency_ms / 1000.0
        return 2.0 * F64(block) / F64(rate)

    # True when `a` is one of the options (and records it).
    def parse(mut o, a: String) raises -> Bool:
        if a == "rt=off":
            o.tc = False
            o.wg = False
        elif a == "rt=tc":
            o.tc = True
            o.wg = False
        elif a == "rt=wg":
            o.tc = False
            o.wg = True
        elif a == "rt=tc+wg":
            o.tc = True
            o.wg = True
        elif a == "pipeline":
            o.pipeline = True
        elif a.startswith("latency="):
            o.latency_ms = atof(a.split("=")[1])
        elif a == "audio=pa":
            o.cpal = False
        elif a == "audio=cpal":
            o.cpal = True
        elif a.startswith("ring="):
            o.ring = atol(a.split("=")[1])
        elif a.startswith("buffer="):
            o.buffer = atol(a.split("=")[1])
        elif a.startswith("audio="):
            raise Error("unknown option " + a + " (audio=pa or audio=cpal)")
        elif a.startswith("rt="):
            raise Error("unknown option " + a + " (rt=off, rt=tc, rt=wg or rt=tc+wg)")
        else:
            return False
        return True

    def describe(o) -> String:
        var rt = String("off")
        if o.tc and o.wg:
            rt = "tc+wg"
        elif o.tc:
            rt = "tc"
        elif o.wg:
            rt = "wg"
        var s = "rt=" + rt + (" pipeline" if o.pipeline else "")
        if o.latency_ms > 0.0:
            s += " latency=" + String(o.latency_ms)
        if o.cpal:
            s += " audio=cpal"
            if o.ring > 0:
                s += " ring=" + String(o.ring)
            if o.buffer >= 0:
                s += " buffer=" + String(o.buffer)
        else:
            s += " audio=pa"
        return s


# Opens and starts the default output, stereo f32 at `rate`, `block` frames
# per write, with a suggested latency of `latency_s`.
def rt_open_output(rate: S32, block: Int, latency_s: F64) raises -> UInt64:
    if external_call["Pa_Initialize", Int32]() != 0:
        raise Error("Pa_Initialize failed")
    # PaStreamParameters: PaDeviceIndex device; int channelCount;
    # PaSampleFormat sampleFormat; PaTime suggestedLatency;
    # void *hostApiSpecificStreamInfo (32 bytes on arm64).
    var params = unsafe_alloc[UInt64](4)
    params.unsafe_bitcast[Int32]()[unsafe_offset=0] = external_call["Pa_GetDefaultOutputDevice", Int32]()
    params.unsafe_bitcast[Int32]()[unsafe_offset=1] = Int32(2)
    params[unsafe_offset=1] = RT_PA_FLOAT32
    params.unsafe_bitcast[F64]()[unsafe_offset=2] = latency_s
    params[unsafe_offset=3] = UInt64(0)
    var stream = UInt64(0)
    var err = external_call["Pa_OpenStream", Int32](
        rt_address(Ptr(to=stream).unsafe_origin_cast[MUT_NOTRK]()),
        UInt64(0),
        rt_address(params.unsafe_origin_cast[MUT_NOTRK]()),
        F64(rate), UInt64(block), UInt64(0), UInt64(0), UInt64(0),
    )
    params.unsafe_free()
    if err != 0:
        raise Error(String("Pa_OpenStream failed: ") + String(err))
    if external_call["Pa_StartStream", Int32](stream) != 0:
        raise Error("Pa_StartStream failed")
    return stream


# The output latency PortAudio granted, in seconds (-1 if unknown).
def rt_output_latency(stream: UInt64) -> F64:
    # PaStreamInfo: int structVersion; PaTime inputLatency;
    # PaTime outputLatency; double sampleRate.
    var info = external_call["Pa_GetStreamInfo", UInt64](stream)
    if info == 0:
        return -1.0
    return Ptr[F64](unsafe_from_address=Int(info) + 16)[]


# The default output device (the one cpal opens), 0 if unknown.
def rt_default_output_device() -> UInt32:
    var addr = unsafe_alloc[UInt32](3)
    addr[unsafe_offset=0] = RT_DEFAULT_OUTPUT_DEVICE
    addr[unsafe_offset=1] = RT_SCOPE_GLOBAL
    addr[unsafe_offset=2] = UInt32(0)
    var size = unsafe_alloc[UInt32](1)
    size[unsafe_offset=0] = UInt32(4)
    var device = unsafe_alloc[UInt32](1)
    device[unsafe_offset=0] = 0
    _ = external_call["AudioObjectGetPropertyData", Int32](
        UInt32(1), rt_address(addr.unsafe_origin_cast[MUT_NOTRK]()), UInt32(0), UInt64(0),
        rt_address(size.unsafe_origin_cast[MUT_NOTRK]()),
        rt_address(device.unsafe_origin_cast[MUT_NOTRK]()),
    )
    var d = device[unsafe_offset=0]
    addr.unsafe_free()
    size.unsafe_free()
    device.unsafe_free()
    return d


def rt_close_output(stream: UInt64):
    _ = external_call["Pa_StopStream", Int32](stream)
    _ = external_call["Pa_CloseStream", Int32](stream)
    _ = external_call["Pa_Terminate", Int32]()


def rt_sorted(xs: List[Int]) -> List[Int]:
    var s = xs.copy()
    for i in range(1, len(s)):
        var x = s[i]
        var j = i - 1
        while j >= 0 and s[j] > x:
            s[j + 1] = s[j]
            j -= 1
        s[j + 1] = x
    return s^


def rt_ms(frames: Int, rate: S32) -> F64:
    return F64(frames) * 1000.0 / F64(rate)


# "<name>_median_us x <name>_p99_us y <name>_max_us z"
def rt_stats(name: String, xs: List[Int]) -> String:
    if len(xs) == 0:
        return name + "_median_us - " + name + "_p99_us - " + name + "_max_us -"
    var s = rt_sorted(xs)
    var n = len(s)
    var p99 = s[min(n - 1, (n * 99) // 100)]
    return (
        name + "_median_us " + String(F64(s[n // 2]) / 1000.0)
        + " " + name + "_p99_us " + String(F64(p99) / 1000.0)
        + " " + name + "_max_us " + String(F64(s[n - 1]) / 1000.0)
    )


# The real-time state of a player's loop and its timing split.
struct RtLoop:
    var opts: RtOptions
    var period_ns: Int
    var tc_status: Int
    var wg_status: Int
    var wg: UInt64
    var token: Ptr[UInt64]
    var enqueue: List[Int]
    var wait: List[Int]
    var host: List[Int]
    var write: List[Int]
    var gap: List[Int]
    var fill: List[Int]
    var ring_frames: Int
    var block: Int
    var rate: S32
    var last_write_end: Int
    var first_write: Int
    var cpu0: Int
    var stream: UInt64
    var latency_s: F64
    var cpal_stats: Ptr[UInt64]

    def __init__(out r, opts: RtOptions, block: Int, rate: S32):
        r.opts = opts
        r.period_ns = Int(F64(block) * 1e9 / F64(rate))
        r.tc_status = -1
        r.wg_status = -1
        r.wg = 0
        # os_workgroup_join_token_s: uint32_t sig; char opaque[36].
        r.token = unsafe_alloc[UInt64](8)
        for i in range(8):
            r.token[unsafe_offset=i] = 0
        r.enqueue = List[Int]()
        r.wait = List[Int]()
        r.host = List[Int]()
        r.write = List[Int]()
        r.gap = List[Int]()
        r.fill = List[Int]()
        r.ring_frames = 0
        r.block = block
        r.rate = rate
        r.last_write_end = -1
        r.first_write = 0
        r.cpu0 = 0
        r.stream = 0
        r.latency_s = -1.0
        r.cpal_stats = unsafe_alloc[UInt64](RT_CPAL_STATS)
        for i in range(RT_CPAL_STATS):
            r.cpal_stats[unsafe_offset=i] = 0

    # One block of GPU work in flight with `pipeline`, none without.
    def depth(r) -> Int:
        return 1 if r.opts.pipeline else 0

    # Before the loop: the time-constraint policy (rt=tc).
    def start(mut r):
        if r.opts.tc:
            # mach_timebase_info_data_t: uint32_t numer, denom; absolute time
            # = ns * denom / numer.
            var tb = unsafe_alloc[UInt32](2)
            _ = external_call["mach_timebase_info", Int32](rt_address(tb.unsafe_origin_cast[MUT_NOTRK]()))
            var to_abs = F64(tb[unsafe_offset=1]) / F64(tb[unsafe_offset=0])
            tb.unsafe_free()
            # thread_time_constraint_policy: period, computation, constraint
            # (uint32_t, absolute time units), preemptible (boolean_t, int).
            var policy = unsafe_alloc[UInt32](4)
            policy[unsafe_offset=0] = UInt32(F64(r.period_ns) * to_abs)
            policy[unsafe_offset=1] = UInt32(F64(r.period_ns) * 0.5 * to_abs)
            policy[unsafe_offset=2] = UInt32(F64(r.period_ns) * to_abs)
            policy[unsafe_offset=3] = UInt32(1)
            var thread = external_call["pthread_mach_thread_np", UInt32](
                external_call["pthread_self", UInt64]()
            )
            r.tc_status = Int(
                external_call["thread_policy_set", Int32](
                    thread, RT_THREAD_TIME_CONSTRAINT_POLICY,
                    rt_address(policy.unsafe_origin_cast[MUT_NOTRK]()), UInt32(4),
                )
            )
            policy.unsafe_free()

    # Opens and starts the output, with the real-time requests: the
    # time-constraint policy (rt=tc) before, the device's IO workgroup
    # (rt=wg) after; starts the CPU-time count.
    def open(mut r) raises:
        r.start()
        var suggested = r.opts.latency_s(r.block, r.rate)
        # PortAudio's computeRingBufferSize (pa_mac_core_utilities.c): twice
        # the suggested latency, at least three buffers, rounded up to a
        # power of two. The ring starts full of zeros.
        var want = max(Int(suggested * F64(r.rate) * 2.0 + 0.5), 3 * r.block, 4)
        r.ring_frames = 1
        while r.ring_frames < want:
            r.ring_frames *= 2
        var device = UInt32(0)
        if r.opts.cpal:
            if r.opts.ring > 0:
                r.ring_frames = r.opts.ring
            var buffer = r.opts.buffer if r.opts.buffer >= 0 else r.block
            r.stream = external_call["cpal_bridge_open", UInt64](
                UInt32(r.rate), UInt32(buffer), UInt32(r.ring_frames)
            )
            if r.stream == 0:
                var msg = external_call["cpal_bridge_last_error", UInt64]()
                raise Error(
                    "cpal_bridge_open failed: "
                    + (String(unsafe_from_utf8_ptr=Ptr[UInt8](unsafe_from_address=Int(msg))) if msg != 0 else "")
                )
            if r.opts.wg:
                device = rt_default_output_device()
        else:
            r.stream = rt_open_output(r.rate, r.block, suggested)
            r.latency_s = rt_output_latency(r.stream)
            if r.opts.wg:
                device = external_call["PaMacCore_GetStreamOutputDevice", UInt32](r.stream)
        r.cpu0 = Int(external_call["clock_gettime_nsec_np", UInt64](RT_CLOCK_THREAD_CPUTIME_ID))
        if r.opts.wg:
            r.join(device)

    def join(mut r, device: UInt32):
        # AudioObjectPropertyAddress: selector, scope, element.
        var addr = unsafe_alloc[UInt32](3)
        addr[unsafe_offset=0] = RT_IO_THREAD_OS_WORKGROUP
        addr[unsafe_offset=1] = RT_SCOPE_GLOBAL
        addr[unsafe_offset=2] = UInt32(0)
        var size = unsafe_alloc[UInt32](1)
        size[unsafe_offset=0] = UInt32(8)
        var wg = unsafe_alloc[UInt64](1)
        wg[unsafe_offset=0] = 0
        var status = external_call["AudioObjectGetPropertyData", Int32](
            device, rt_address(addr.unsafe_origin_cast[MUT_NOTRK]()), UInt32(0), UInt64(0),
            rt_address(size.unsafe_origin_cast[MUT_NOTRK]()),
            rt_address(wg.unsafe_origin_cast[MUT_NOTRK]()),
        )
        r.wg = wg[unsafe_offset=0]
        addr.unsafe_free()
        size.unsafe_free()
        wg.unsafe_free()
        if status != 0 or r.wg == 0:
            r.wg_status = Int(status) if status != 0 else -2
            r.wg = 0
            return
        r.wg_status = Int(
            external_call["os_workgroup_join", Int32](r.wg, rt_address(r.token.unsafe_origin_cast[MUT_NOTRK]()))
        )

    # What the ring holds now, in frames: call it just before a write.
    def ring_fill(r) -> Int:
        if r.opts.cpal:
            return Int(external_call["cpal_bridge_fill", UInt64](r.stream))
        return r.ring_frames - Int(external_call["Pa_GetStreamWriteAvailable", Int64](r.stream))

    # Writes one block of interleaved stereo frames, waiting for room: 0, 1
    # when the output under-ran since the previous write, -1 on an error.
    def write_block(mut r, frames: Ptr[F32, MUT_NOTRK]) -> Int:
        if r.opts.cpal:
            return Int(external_call["cpal_bridge_write", Int32](r.stream, rt_address(frames), UInt32(r.block)))
        var e = external_call["Pa_WriteStream", Int32](r.stream, rt_address(frames), UInt64(r.block))
        if e == RT_PA_OUTPUT_UNDERFLOWED:
            return 1
        return 0 if e == 0 else -1

    # Leaves the workgroup and closes the output. With cpal, reads the
    # bridge's counters first; the output latency is then its ring plus the
    # device latency of the last callback.
    def close(mut r):
        if r.wg != 0:
            if r.wg_status == 0:
                external_call["os_workgroup_leave", NoneType](
                    r.wg, rt_address(r.token.unsafe_origin_cast[MUT_NOTRK]())
                )
            external_call["os_release", NoneType](r.wg)
            r.wg = 0
        if r.stream == 0:
            return
        if r.opts.cpal:
            external_call["cpal_bridge_stats", NoneType](
                r.stream, rt_address(r.cpal_stats.unsafe_origin_cast[MUT_NOTRK]())
            )
            external_call["cpal_bridge_close", NoneType](r.stream)
            r.latency_s = rt_ms(r.ring_frames, r.rate) / 1000.0 + F64(r.cpal_stats[unsafe_offset=9]) / 1e9
        else:
            rt_close_output(r.stream)
        r.stream = 0

    # The output latency: granted by PortAudio, or ring plus device with
    # cpal (known after `close`).
    def output_latency(r) -> F64:
        return r.latency_s

    # The four parts of one played block, the ring's fill before its write
    # and the gap before it (`write_start` is perf_counter_ns() just before
    # Pa_WriteStream).
    def record(mut r, enqueue: Int, wait: Int, host: Int, fill: Int, write_start: Int, write_end: Int):
        r.fill.append(fill)
        r.enqueue.append(enqueue)
        r.wait.append(wait)
        r.host.append(host)
        if r.last_write_end >= 0:
            r.write.append(write_end - write_start)
            r.gap.append(write_start - r.last_write_end)
        else:
            r.first_write = write_end - write_start
        r.last_write_end = write_end

    def summary(r) -> String:
        var over = 0
        for g in r.gap:
            if g > r.period_ns:
                over += 1
        var cpu = Int(external_call["clock_gettime_nsec_np", UInt64](RT_CLOCK_THREAD_CPUTIME_ID)) - r.cpu0
        var fs = rt_sorted(r.fill)
        return (
            "timing " + r.opts.describe()
            + " tc_status " + String(r.tc_status) + " wg_status " + String(r.wg_status)
            + " " + rt_stats("enqueue", r.enqueue) + " " + rt_stats("wait", r.wait)
            + " " + rt_stats("host", r.host) + " " + rt_stats("write", r.write)
            + " first_write_us " + String(F64(r.first_write) / 1000.0)
            + " " + rt_stats("gap", r.gap) + " over_budget " + String(over)
            + " ring_ms " + String(rt_ms(r.ring_frames, r.rate))
            + " fill_min_ms " + String(rt_ms(fs[0] if len(fs) > 0 else 0, r.rate))
            + " fill_p1_ms " + String(rt_ms(fs[len(fs) // 100] if len(fs) > 0 else 0, r.rate))
            + " thread_cpu_us " + String(F64(cpu) / 1000.0 / F64(max(len(r.enqueue), 1)))
            + (r.cpal_summary() if r.opts.cpal else "")
        )

    def cpal_summary(r) -> String:
        var c = r.cpal_stats
        return (
            " cpal_callbacks " + String(c[unsafe_offset=0])
            + " cpal_short_callbacks " + String(c[unsafe_offset=1])
            + " cpal_short_frames " + String(c[unsafe_offset=2])
            + " cpal_xruns " + String(c[unsafe_offset=3])
            + " cpal_errors " + String(c[unsafe_offset=4])
            + " cpal_callback_frames " + String(c[unsafe_offset=5]) + "-" + String(c[unsafe_offset=6])
            + " cpal_max_interval_ms " + String(F64(c[unsafe_offset=7]) / 1e6)
            + " cpal_min_fill_ms " + String(rt_ms(Int(c[unsafe_offset=8]), r.rate))
            + " cpal_device_latency_ms " + String(F64(c[unsafe_offset=9]) / 1e6)
            + " cpal_buffer_frames " + String(c[unsafe_offset=10])
        )

    def free(mut r):
        r.token.unsafe_free()
        r.cpal_stats.unsafe_free()
