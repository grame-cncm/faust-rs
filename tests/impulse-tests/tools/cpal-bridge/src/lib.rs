//! A C ABI over [cpal](https://crates.io/crates/cpal) for the Mojo GPU players
//! of `tests/impulse-tests` (`archs/realtimemojo.mojo`, option `audio=cpal`).
//!
//! It plays the role PortAudio's blocking API plays for `audio=pa`: the
//! player's loop writes blocks of interleaved stereo `f32` frames into a ring,
//! and cpal's output callback, on the CoreAudio IO thread, reads them.
//!
//! What differs from PortAudio's blocking mode:
//! - the ring's size is chosen by the caller (`ring_frames`), not twice the
//!   suggested latency rounded up to a power of two;
//! - a writer that finds the ring full parks, and the callback unparks it
//!   after every read: the writer wakes when room appears, not after a 5 ms
//!   sleep;
//! - the device buffer (`buffer_frames`) is set through
//!   `BufferSize::Fixed`, and cpal reports CoreAudio overloads
//!   (`kAudioDeviceProcessorOverload`) as `ErrorKind::Xrun`, which are
//!   counted apart from the ring's underruns.
//!
//! The ring is a single-producer single-consumer queue of samples stored as
//! `AtomicU32` bit patterns, with monotonic frame counters: the producer
//! publishes `written` with `Release` after storing its samples, the
//! consumer publishes `read` with `Release` after loading them, and each
//! side reads the other's counter with `Acquire`. Only one thread may call
//! `cpal_bridge_write`.

use std::ffi::{c_char, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, ErrorKind, StreamConfig};

const CHANNELS: usize = 2;

/// The state shared by the writer and cpal's callbacks.
struct Shared {
    /// Interleaved stereo samples, `f32::to_bits`.
    samples: Box<[AtomicU32]>,
    /// The ring's size in frames.
    frames: u64,
    /// Frames written (producer) and read (consumer), monotonic.
    written: AtomicU64,
    read: AtomicU64,
    /// Set by a callback that found fewer frames than it needed; cleared by
    /// the next write, which reports it.
    short: AtomicBool,
    /// The writer's thread, unparked after each callback.
    writer: OnceLock<Thread>,
    callbacks: AtomicU64,
    short_callbacks: AtomicU64,
    short_frames: AtomicU64,
    xruns: AtomicU64,
    other_errors: AtomicU64,
    min_callback_frames: AtomicU64,
    max_callback_frames: AtomicU64,
    max_interval_ns: AtomicU64,
    min_fill_frames: AtomicU64,
    device_latency_ns: AtomicU64,
    /// When the previous callback ran, in ns since `epoch`.
    last_callback_ns: AtomicU64,
    epoch: Instant,
}

impl Shared {
    /// The output callback: copies what the ring holds into `out`, zeros the
    /// rest, and wakes the writer.
    fn fill(&self, out: &mut [f32], device_latency: Duration) {
        let now = self.epoch.elapsed().as_nanos() as u64;
        let last = self.last_callback_ns.swap(now, Ordering::Relaxed);
        if last != 0 {
            self.max_interval_ns
                .fetch_max(now.saturating_sub(last), Ordering::Relaxed);
        }
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.device_latency_ns
            .store(device_latency.as_nanos() as u64, Ordering::Relaxed);
        let need = (out.len() / CHANNELS) as u64;
        self.min_callback_frames.fetch_min(need, Ordering::Relaxed);
        self.max_callback_frames.fetch_max(need, Ordering::Relaxed);

        let read = self.read.load(Ordering::Relaxed);
        let available = self.written.load(Ordering::Acquire) - read;
        self.min_fill_frames.fetch_min(available, Ordering::Relaxed);
        let n = available.min(need);
        let len = self.samples.len() as u64;
        let (filled, rest) = out.split_at_mut(n as usize * CHANNELS);
        for (i, x) in filled.iter_mut().enumerate() {
            let slot = ((read * CHANNELS as u64 + i as u64) % len) as usize;
            *x = f32::from_bits(self.samples[slot].load(Ordering::Relaxed));
        }
        rest.fill(0.0);
        self.read.store(read + n, Ordering::Release);
        if n < need {
            self.short.store(true, Ordering::Relaxed);
            self.short_callbacks.fetch_add(1, Ordering::Relaxed);
            self.short_frames.fetch_add(need - n, Ordering::Relaxed);
        }
        if let Some(writer) = self.writer.get() {
            writer.unpark();
        }
    }

    fn fill_frames(&self) -> u64 {
        self.written.load(Ordering::Acquire) - self.read.load(Ordering::Acquire)
    }
}

/// An open output: the stream and the ring it reads.
pub struct Bridge {
    shared: Arc<Shared>,
    stream: cpal::Stream,
}

/// Counters of a bridge, read by `cpal_bridge_stats`.
#[repr(C)]
pub struct CpalBridgeStats {
    /// Output callbacks run.
    pub callbacks: u64,
    /// Callbacks that found the ring short, and the frames they lacked.
    pub short_callbacks: u64,
    pub short_frames: u64,
    /// CoreAudio overloads reported by cpal (`ErrorKind::Xrun`).
    pub xruns: u64,
    /// Other stream errors.
    pub other_errors: u64,
    /// The smallest and largest callback, in frames.
    pub min_callback_frames: u64,
    pub max_callback_frames: u64,
    /// The longest time between two callbacks, in ns.
    pub max_interval_ns: u64,
    /// The lowest fill of the ring seen by a callback before it read, in
    /// frames.
    pub min_fill_frames: u64,
    /// cpal's playback minus callback instant at the last callback: the
    /// device buffer plus the device's latency and safety offset, in ns.
    pub device_latency_ns: u64,
    /// `Stream::buffer_size()`, the device buffer in frames (0 if unknown).
    pub stream_buffer_frames: u64,
}

static LAST_ERROR: Mutex<Option<CString>> = Mutex::new(None);

fn set_error(message: String) {
    let text = CString::new(message.replace('\0', " ")).unwrap_or_default();
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = Some(text);
    }
}

fn open(rate: u32, buffer_frames: u32, ring_frames: u32) -> Result<Bridge, String> {
    if ring_frames == 0 {
        return Err("the ring needs at least one frame".into());
    }
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no default output device")?;
    let config = StreamConfig {
        channels: CHANNELS as u16,
        sample_rate: rate,
        buffer_size: if buffer_frames == 0 {
            BufferSize::Default
        } else {
            BufferSize::Fixed(buffer_frames)
        },
    };
    let samples: Box<[AtomicU32]> = (0..ring_frames as usize * CHANNELS)
        .map(|_| AtomicU32::new(0.0f32.to_bits()))
        .collect();
    // The ring starts full of silence, as PortAudio's does.
    let shared = Arc::new(Shared {
        samples,
        frames: u64::from(ring_frames),
        written: AtomicU64::new(u64::from(ring_frames)),
        read: AtomicU64::new(0),
        short: AtomicBool::new(false),
        writer: OnceLock::new(),
        callbacks: AtomicU64::new(0),
        short_callbacks: AtomicU64::new(0),
        short_frames: AtomicU64::new(0),
        xruns: AtomicU64::new(0),
        other_errors: AtomicU64::new(0),
        min_callback_frames: AtomicU64::new(u64::MAX),
        max_callback_frames: AtomicU64::new(0),
        max_interval_ns: AtomicU64::new(0),
        min_fill_frames: AtomicU64::new(u64::MAX),
        device_latency_ns: AtomicU64::new(0),
        last_callback_ns: AtomicU64::new(0),
        epoch: Instant::now(),
    });
    let data = Arc::clone(&shared);
    let errors = Arc::clone(&shared);
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out, info| {
                let ts = info.timestamp();
                data.fill(out, ts.playback.duration_since(ts.callback));
            },
            move |error| {
                if error.kind() == ErrorKind::Xrun {
                    errors.xruns.fetch_add(1, Ordering::Relaxed);
                } else {
                    errors.other_errors.fetch_add(1, Ordering::Relaxed);
                }
            },
            None,
        )
        .map_err(|e| format!("build_output_stream: {e}"))?;
    stream.play().map_err(|e| format!("play: {e}"))?;
    Ok(Bridge { shared, stream })
}

/// Opens and starts the default output: stereo `f32` at `rate`, a device
/// buffer of `buffer_frames` (0 for the device's default), and a ring of
/// `ring_frames` frames, full of silence. Returns null on failure (see
/// `cpal_bridge_last_error`).
#[no_mangle]
pub extern "C" fn cpal_bridge_open(rate: u32, buffer_frames: u32, ring_frames: u32) -> *mut Bridge {
    match open(rate, buffer_frames, ring_frames) {
        Ok(bridge) => Box::into_raw(Box::new(bridge)),
        Err(message) => {
            set_error(message);
            std::ptr::null_mut()
        }
    }
}

/// The message of the last failed open, or null.
#[no_mangle]
pub extern "C" fn cpal_bridge_last_error() -> *const c_char {
    match LAST_ERROR.lock() {
        Ok(slot) => slot.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
        Err(_) => std::ptr::null(),
    }
}

/// Writes `frames` interleaved stereo frames from `data`, waiting while the
/// ring is full. Returns 1 if a callback found the ring short since the
/// previous write, 0 otherwise, -1 on a null argument.
///
/// # Safety
/// `bridge` comes from `cpal_bridge_open` and is not closed; `data` points to
/// `2 * frames` readable `f32`. Only one thread calls this function.
#[no_mangle]
pub unsafe extern "C" fn cpal_bridge_write(
    bridge: *mut Bridge,
    data: *const f32,
    frames: u32,
) -> i32 {
    if bridge.is_null() || data.is_null() {
        return -1;
    }
    // SAFETY: the caller guarantees both pointers (see above).
    let (bridge, data) = unsafe {
        (
            &*bridge,
            std::slice::from_raw_parts(data, frames as usize * CHANNELS),
        )
    };
    let shared = &bridge.shared;
    let _ = shared.writer.set(std::thread::current());
    let len = shared.samples.len() as u64;
    let mut done = 0u64;
    let total = u64::from(frames);
    while done < total {
        let written = shared.written.load(Ordering::Relaxed);
        let free = shared.frames - (written - shared.read.load(Ordering::Acquire));
        if free == 0 {
            // Woken by the next callback; the timeout covers a stopped device.
            std::thread::park_timeout(Duration::from_millis(50));
            continue;
        }
        let n = free.min(total - done);
        for i in 0..(n * CHANNELS as u64) {
            let slot = ((written * CHANNELS as u64 + i) % len) as usize;
            let x = data[(done * CHANNELS as u64 + i) as usize];
            shared.samples[slot].store(x.to_bits(), Ordering::Relaxed);
        }
        shared.written.store(written + n, Ordering::Release);
        done += n;
    }
    i32::from(shared.short.swap(false, Ordering::Relaxed))
}

/// The frames the ring holds now.
///
/// # Safety
/// `bridge` comes from `cpal_bridge_open` and is not closed.
#[no_mangle]
pub unsafe extern "C" fn cpal_bridge_fill(bridge: *const Bridge) -> u64 {
    if bridge.is_null() {
        return 0;
    }
    // SAFETY: see above.
    unsafe { &*bridge }.shared.fill_frames()
}

/// Copies the bridge's counters into `out`.
///
/// # Safety
/// `bridge` comes from `cpal_bridge_open` and is not closed; `out` points to
/// a writable `CpalBridgeStats`.
#[no_mangle]
pub unsafe extern "C" fn cpal_bridge_stats(bridge: *const Bridge, out: *mut CpalBridgeStats) {
    if bridge.is_null() || out.is_null() {
        return;
    }
    // SAFETY: see above.
    let bridge = unsafe { &*bridge };
    let s = &bridge.shared;
    let min_or_zero = |a: &AtomicU64| match a.load(Ordering::Relaxed) {
        u64::MAX => 0,
        v => v,
    };
    let stats = CpalBridgeStats {
        callbacks: s.callbacks.load(Ordering::Relaxed),
        short_callbacks: s.short_callbacks.load(Ordering::Relaxed),
        short_frames: s.short_frames.load(Ordering::Relaxed),
        xruns: s.xruns.load(Ordering::Relaxed),
        other_errors: s.other_errors.load(Ordering::Relaxed),
        min_callback_frames: min_or_zero(&s.min_callback_frames),
        max_callback_frames: s.max_callback_frames.load(Ordering::Relaxed),
        max_interval_ns: s.max_interval_ns.load(Ordering::Relaxed),
        min_fill_frames: min_or_zero(&s.min_fill_frames),
        device_latency_ns: s.device_latency_ns.load(Ordering::Relaxed),
        stream_buffer_frames: bridge.stream.buffer_size().map_or(0, u64::from),
    };
    // SAFETY: see above.
    unsafe { out.write(stats) };
}

/// Stops the stream and frees the bridge.
///
/// # Safety
/// `bridge` comes from `cpal_bridge_open` and is not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn cpal_bridge_close(bridge: *mut Bridge) {
    if !bridge.is_null() {
        // SAFETY: see above.
        let bridge = unsafe { Box::from_raw(bridge) };
        let _ = bridge.stream.pause();
    }
}
