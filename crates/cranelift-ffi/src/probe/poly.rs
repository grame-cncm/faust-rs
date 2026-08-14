//! Polyphonic voice bookkeeping: discovery, allocation, stealing, mixing.
//!
//! # Why this module is FFI-free
//! [`engine`](crate::probe::engine) is documented as the only module in this
//! crate that touches FFI. The polyphonic wrapper still needs a JIT — it
//! instantiates N clones of one factory and writes into their zones — but the
//! *decisions* (which voice to allocate, whether a note-on steals, when a
//! releasing voice is reclaimed) are pure functions of small integer state.
//! Keeping them here, taking and returning plain data rather than `Probe`
//! handles, is what makes the allocation policy unit-testable without a JIT:
//! exactly the property the design calls out for "getting it wrong silently
//! detunes every note by an octave-scale factor, so it is worth a test of its
//! own" (design §3.2). [`engine::PolyProbe`](crate::probe::engine::PolyProbe)
//! is the thin FFI layer that carries out what this module decides.
//!
//! # Source
//! Ported from `architecture/faust/dsp/poly-dsp.h` (pinned reference
//! `8eebea429`). Only the parts that affect offline, deterministic rendering
//! are ported; see the module's items for what was intentionally left out.

/// A voice with no note assigned, available for immediate allocation.
///
/// `poly-dsp.h:52`.
pub const FREE_VOICE: i32 = -1;
/// A voice whose gate has been released but which may still be sounding
/// (its envelope tail). `poly-dsp.h:53`.
pub const RELEASE_VOICE: i32 = -2;
/// A voice being stolen: it renders the outgoing note's fading tail and the
/// incoming note's onset within the same block. `poly-dsp.h:54`.
pub const LEGATO_VOICE: i32 = -3;
/// Sentinel returned by a playing-voice search that finds nothing.
/// `poly-dsp.h:55`.
pub const NO_VOICE: i32 = -4;
/// A voice actively sounding a note, keyed by its own `fCurNote` pitch.
/// `poly-dsp.h:51` (`kActiveVoice`); never stored as a sentinel — a voice in
/// this "state" simply carries its MIDI pitch in `cur_note`.
pub const ACTIVE_VOICE: i32 = 0;

/// RMS level below which a releasing voice is reclaimed as free: -90 dB.
/// `poly-dsp.h:57`. Exposed as `--voice-stop-level` because it is the one
/// number here with an audible consequence (design §3.2): too high truncates
/// long releases, too low never reclaims a voice under sustained play.
pub const DEFAULT_VOICE_STOP_LEVEL: f64 = 0.000_031_62;

/// MIDI note number to frequency in Hz: `440 * 2^((note-69)/12)`.
///
/// `dsp_voice::midiToFreq`, `poly-dsp.h:163`.
#[must_use]
pub fn midi_to_freq(note: f64) -> f64 {
    440.0 * 2f64.powf((note - 69.0) / 12.0)
}

/// How a discovered `/freq`-or-`/key`-suffixed path converts a MIDI pitch.
///
/// The suffix that matched selects the function; getting `/freq` and `/key`
/// swapped detunes every note by whatever the difference between a raw MIDI
/// number and its frequency happens to be at that pitch — an octave-scale
/// error, not a rounding one, hence its own test below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyConversion {
    /// `/freq` — MIDI note number to Hz (`dsp_voice::midiToFreq`).
    Freq,
    /// `/key` — identity: the raw MIDI note number.
    Key,
}

impl Default for KeyConversion {
    /// `dsp_voice`'s constructor default (`poly-dsp.h:187`), used verbatim
    /// when a voice declares neither `/freq` nor `/key`.
    fn default() -> Self {
        Self::Freq
    }
}

impl KeyConversion {
    /// Apply the conversion to a MIDI pitch.
    #[must_use]
    pub fn convert(self, pitch: i32) -> f64 {
        match self {
            Self::Freq => midi_to_freq(f64::from(pitch)),
            Self::Key => f64::from(pitch),
        }
    }
}

/// How a discovered `/gain`-or-`/vel`/`/velocity`-suffixed path converts a
/// MIDI velocity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelConversion {
    /// `/gain` — velocity normalised to `[0, 1]` (`vel / 127`).
    Gain,
    /// `/vel` or `/velocity` — identity: the raw MIDI velocity.
    Velocity,
}

impl Default for VelConversion {
    /// `dsp_voice`'s constructor default (`poly-dsp.h:186`).
    fn default() -> Self {
        Self::Gain
    }
}

impl VelConversion {
    /// Apply the conversion to a MIDI velocity.
    #[must_use]
    pub fn convert(self, velocity: i32) -> f64 {
        match self {
            Self::Gain => f64::from(velocity) / 127.0,
            Self::Velocity => f64::from(velocity),
        }
    }
}

/// One voice's control paths, discovered by suffix over its own full-path
/// map, and the conversion functions the suffixes selected.
///
/// Mirrors `dsp_voice::extractPaths` (`poly-dsp.h:233`). Every polyphonic
/// voice is a clone of the same source, so this is identical across voices
/// and is computed once per voice only because nothing guarantees the
/// clones are literally the same `Control` list at different addresses.
#[derive(Debug, Clone, Default)]
pub struct VoiceControlPaths {
    /// Every path ending in `/gate`.
    pub gate: Vec<String>,
    /// Every path ending in `/freq` or `/key`.
    pub freq: Vec<String>,
    /// Every path ending in `/gain`, `/vel` or `/velocity`.
    pub gain: Vec<String>,
    /// Conversion selected by the last `/freq`-or-`/key` path seen.
    pub key_fun: KeyConversion,
    /// Conversion selected by the last `/gain`-or-`/vel`-or-`/velocity` path
    /// seen.
    pub vel_fun: VelConversion,
}

/// Discover a voice's gate/freq/gain paths by suffix, in the order given.
///
/// `paths` must be in the same order C++ `std::map<std::string, ...>`
/// iterates its keys — ascending by full path — because `extractPaths`
/// overwrites `fKeyFun`/`fVelFun` on every match rather than only the first,
/// so a (pathological) voice declaring both `/freq` and `/key` resolves to
/// whichever sorts last. [`crate::probe::params::ControlMap`] is a
/// `BTreeMap`, so its `iter()` already yields this order.
///
/// Mirrors `dsp_voice::extractPaths` (`poly-dsp.h:233`).
#[must_use]
pub fn extract_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> VoiceControlPaths {
    let mut result = VoiceControlPaths::default();
    for path in paths {
        if path.ends_with("/gate") {
            result.gate.push(path.to_owned());
        } else if path.ends_with("/freq") {
            result.key_fun = KeyConversion::Freq;
            result.freq.push(path.to_owned());
        } else if path.ends_with("/key") {
            result.key_fun = KeyConversion::Key;
            result.freq.push(path.to_owned());
        } else if path.ends_with("/gain") {
            result.vel_fun = VelConversion::Gain;
            result.gain.push(path.to_owned());
        } else if path.ends_with("/vel") || path.ends_with("/velocity") {
            result.vel_fun = VelConversion::Velocity;
            result.gain.push(path.to_owned());
        }
    }
    result
}

/// One voice's allocation bookkeeping.
///
/// `cur_note`/`next_note`/`next_vel`/`date` mirror the `dsp_voice` fields of
/// the same name (`poly-dsp.h:169-172`); `level` mirrors `fLevel`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceState {
    /// Pitch currently sounding, or one of the state sentinels
    /// ([`FREE_VOICE`], [`RELEASE_VOICE`], [`LEGATO_VOICE`]).
    pub cur_note: i32,
    /// In [`LEGATO_VOICE`] state, the pitch queued to start mid-buffer.
    pub next_note: i32,
    /// In [`LEGATO_VOICE`] state, the velocity queued to start mid-buffer.
    pub next_vel: i32,
    /// Monotonic allocation counter; higher is more recently allocated.
    pub date: u64,
    /// RMS level of the voice's last rendered block.
    pub level: f64,
}

impl VoiceState {
    /// A voice with no note assigned, as every voice starts.
    #[must_use]
    pub const fn free() -> Self {
        Self {
            cur_note: FREE_VOICE,
            next_note: -1,
            next_vel: -1,
            date: 0,
            level: 0.0,
        }
    }
}

/// Find the oldest voice among `voices` sounding `pitch`, including one
/// mid-steal whose *queued* note is `pitch`.
///
/// Mirrors `getPlayingVoice` (`poly-dsp.h:601`), used by the C++ `keyOff` to
/// find which voice to release for a given pitch — **not** by `keyOn`. A
/// repeated note-on for an already-sounding pitch is not a retrigger of that
/// voice: `mydsp_poly::keyOn` (`poly-dsp.h:900`) calls only
/// [`get_free_voice`], unconditionally allocating a fresh voice every time.
#[must_use]
pub fn get_playing_voice(voices: &[VoiceState], pitch: i32) -> Option<usize> {
    let mut best: Option<(usize, u64)> = None;
    for (i, v) in voices.iter().enumerate() {
        let matches = v.cur_note == pitch || (v.cur_note == LEGATO_VOICE && v.next_note == pitch);
        if matches && best.is_none_or(|(_, d)| v.date < d) {
            best = Some((i, v.date));
        }
    }
    best.map(|(i, _)| i)
}

/// Outcome of [`get_free_voice`]: which voice index to use, and whether
/// taking it requires entering [`LEGATO_VOICE`] rather than sounding
/// immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allocation {
    /// A voice was idle; it enters [`ACTIVE_VOICE`] (i.e. the note pitch)
    /// directly.
    Free(usize),
    /// No voice was idle; the returned voice — the oldest releasing one, or
    /// failing that the oldest playing one — is stolen and must enter
    /// [`LEGATO_VOICE`].
    Steal(usize),
}

/// Choose a voice for a new note-on: first free voice; failing that, the
/// oldest releasing voice; failing that, the oldest playing voice.
///
/// Mirrors `getFreeVoice` (`poly-dsp.h:630`). `poly-dsp.h` asserts the voice
/// table is never empty and always returns a voice; this returns `None` for
/// an empty table instead of the C++ `assert(false)` / UB path, since a test
/// tool should report that condition rather than abort.
#[must_use]
pub fn get_free_voice(voices: &[VoiceState]) -> Option<Allocation> {
    if let Some(i) = voices.iter().position(|v| v.cur_note == FREE_VOICE) {
        return Some(Allocation::Free(i));
    }
    let mut release_best: Option<(usize, u64)> = None;
    let mut playing_best: Option<(usize, u64)> = None;
    for (i, v) in voices.iter().enumerate() {
        if v.cur_note == RELEASE_VOICE {
            if release_best.is_none_or(|(_, d)| v.date < d) {
                release_best = Some((i, v.date));
            }
        } else if playing_best.is_none_or(|(_, d)| v.date < d) {
            playing_best = Some((i, v.date));
        }
    }
    release_best
        .or(playing_best)
        .map(|(i, _)| Allocation::Steal(i))
}

/// A zone write [`PolyState`] decided on; the FFI layer applies it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoiceWrite {
    /// Immediate note-on: every freq/key path to `freq`, every gate path to
    /// 1.0, every gain/vel path to `gain`.
    KeyOn { freq: f64, gain: f64 },
    /// Note-off (soft or hard): every gate path to 0.0.
    KeyOff,
}

/// The allocation and mixing state of every voice in a polyphonic bus.
///
/// Pure state machine: it decides what should happen and returns the write
/// the caller must carry out, but performs no I/O and holds no zone
/// pointers. [`engine::PolyProbe`](crate::probe::engine::PolyProbe) is the
/// thin layer that turns a returned [`VoiceWrite`] into an actual write.
#[derive(Debug, Clone)]
pub struct PolyState {
    /// One entry per voice, in voice-table order.
    pub voices: Vec<VoiceState>,
}

impl PolyState {
    /// `n` idle voices, as a freshly constructed polyphonic bus has.
    ///
    /// # Panics
    /// Panics if `n` is zero: `poly-dsp.h`'s constructor asserts `nvoices >
    /// 0` (`poly-dsp.h:725`), and every allocation function here assumes at
    /// least one voice exists.
    #[must_use]
    pub fn new(n: usize) -> Self {
        assert!(n > 0, "a polyphonic bus needs at least one voice");
        Self {
            voices: vec![VoiceState::free(); n],
        }
    }

    /// Note on: allocate via [`get_free_voice`] and either sound the note
    /// immediately or, for a stolen voice, queue it for the mid-buffer
    /// legato transition.
    ///
    /// Mirrors `mydsp_poly::keyOn` (`poly-dsp.h:900`) composed with
    /// `allocVoice` (`poly-dsp.h:622`) and `dsp_voice::keyOn`
    /// (`poly-dsp.h:273`): `allocVoice` bumps `date` and sets `cur_note` to
    /// the *placeholder* state ([`ACTIVE_VOICE`] or [`LEGATO_VOICE`]) before
    /// `dsp_voice::keyOn` decides, from that placeholder, whether the
    /// assignment is immediate or deferred. Returns the voice index and the
    /// write to apply now, or `None` for a stolen voice — that voice's
    /// [`VoiceWrite`] is produced later by [`Self::apply_legato`], once the
    /// outgoing note's tail has actually rendered.
    #[must_use]
    pub fn key_on(
        &mut self,
        pitch: i32,
        velocity: i32,
        key_fun: KeyConversion,
        vel_fun: VelConversion,
    ) -> (usize, Option<VoiceWrite>) {
        let allocation =
            get_free_voice(&self.voices).expect("PolyState::new guarantees at least one voice");
        match allocation {
            Allocation::Free(i) => {
                self.voices[i].date += 1;
                self.voices[i].cur_note = pitch;
                let write = VoiceWrite::KeyOn {
                    freq: key_fun.convert(pitch),
                    gain: vel_fun.convert(velocity),
                };
                (i, Some(write))
            }
            Allocation::Steal(i) => {
                self.voices[i].date += 1;
                self.voices[i].cur_note = LEGATO_VOICE;
                self.voices[i].next_note = pitch;
                self.voices[i].next_vel = velocity;
                (i, None)
            }
        }
    }

    /// Note off: find the oldest voice sounding `pitch` and release it.
    ///
    /// Mirrors `mydsp_poly::keyOff` (`poly-dsp.h:911`) composed with
    /// `dsp_voice::keyOff` (`poly-dsp.h:299`). `hard` mirrors the `hard`
    /// parameter of `dsp_voice::keyOff` (used by `allNotesOff(true)` /
    /// panic): a hard release frees the voice immediately rather than
    /// waiting for its envelope to decay below the stop level. Returns
    /// `None` when no voice is sounding `pitch`, mirroring the C++
    /// `kNoVoice` case (which only logs and does nothing further).
    #[must_use]
    pub fn key_off(&mut self, pitch: i32, hard: bool) -> Option<(usize, VoiceWrite)> {
        let i = get_playing_voice(&self.voices, pitch)?;
        self.voices[i].cur_note = if hard { FREE_VOICE } else { RELEASE_VOICE };
        Some((i, VoiceWrite::KeyOff))
    }

    /// Apply the note queued on a [`LEGATO_VOICE`] voice, at the point in the
    /// block where the outgoing note's tail has finished rendering.
    ///
    /// Mirrors the `keyOn(fNextNote, fNextVel)` call inside `computeLegato`
    /// (`poly-dsp.h:226`), which — called with its default `legato = false`
    /// — resolves to the *immediate* two-argument `dsp_voice::keyOn`
    /// (`poly-dsp.h:284`): `cur_note` becomes the new pitch right away, not
    /// another placeholder.
    #[must_use]
    pub fn apply_legato(
        &mut self,
        voice: usize,
        key_fun: KeyConversion,
        vel_fun: VelConversion,
    ) -> VoiceWrite {
        let pitch = self.voices[voice].next_note;
        let velocity = self.voices[voice].next_vel;
        self.voices[voice].cur_note = pitch;
        VoiceWrite::KeyOn {
            freq: key_fun.convert(pitch),
            gain: vel_fun.convert(velocity),
        }
    }

    /// Record a voice's just-rendered block level, and reclaim it if it was
    /// releasing and fell below `stop_level`.
    ///
    /// Mirrors the bookkeeping at the end of `mydsp_poly::compute`'s
    /// per-voice branch (`poly-dsp.h:850-853`).
    pub fn record_level(&mut self, voice: usize, level: f64, stop_level: f64) {
        self.voices[voice].level = level;
        if self.voices[voice].cur_note == RELEASE_VOICE && level < stop_level {
            self.voices[voice].cur_note = FREE_VOICE;
        }
    }

    /// Number of voices not in [`FREE_VOICE`] state.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.voices
            .iter()
            .filter(|v| v.cur_note != FREE_VOICE)
            .count()
    }
}

/// Mix `voice`'s rendered block into `out` (summing per sample), and return
/// the block's RMS level across every channel and sample.
///
/// Mirrors `mixCheckVoice` (`poly-dsp.h:552`). The level is what
/// [`PolyState::record_level`] compares against the stop threshold — the
/// returned value must be the *mixed voice's own* level, not `out`'s,
/// because reclamation depends on when this one voice's envelope decays, not
/// on the combined output of every voice playing at once.
///
/// # Panics
/// Panics if `voice` and `out` do not have the same channel count and
/// per-channel length; both always come from the same render call in
/// [`crate::probe::engine::PolyProbe`], so a mismatch is a programming error.
#[must_use]
pub fn mix_check_voice(voice: &[Vec<f64>], out: &mut [Vec<f64>]) -> f64 {
    assert_eq!(voice.len(), out.len(), "channel count mismatch");
    let mut sum_squares = 0.0;
    let mut total = 0usize;
    for (channel, samples) in voice.iter().enumerate() {
        assert_eq!(samples.len(), out[channel].len(), "block length mismatch");
        for (i, &sample) in samples.iter().enumerate() {
            sum_squares += sample * sample;
            out[channel][i] += sample;
        }
        total += samples.len();
    }
    if total == 0 {
        return 0.0;
    }
    (sum_squares / total as f64).sqrt()
}

/// Linearly fade the first `half` samples of every channel in `buffer` from
/// 1.0 toward (but not reaching) 0.0, in place.
///
/// Mirrors `fadeOut` (`poly-dsp.h:540`) as `mydsp_poly::compute` calls it —
/// with `count/2` — on a stolen voice's mix buffer: the outgoing note
/// occupies the first half (see [`PolyState::apply_legato`]'s doc), so
/// fading that half masks the discontinuity a hard cut would leave at the
/// splice.
pub fn fade_out(buffer: &mut [Vec<f64>], half: usize) {
    if half == 0 {
        return;
    }
    let step = 1.0 / half as f64;
    for channel in buffer {
        let mut factor = 1.0;
        for sample in channel.iter_mut().take(half) {
            *sample *= factor;
            factor -= step;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freq_suffix_converts_note_to_hz_not_key() {
        // The distinction the design calls out: mixing these up silently
        // detunes every note by an octave-scale factor rather than failing
        // loudly, so it needs its own explicit check.
        let paths = extract_paths(["/synth/freq"]);
        assert_eq!(paths.key_fun, KeyConversion::Freq);
        // A4 (MIDI 69) must be 440 Hz, not the raw number 69.
        assert!((paths.key_fun.convert(69) - 440.0).abs() < 1e-9);
    }

    #[test]
    fn key_suffix_is_identity_not_hz() {
        let paths = extract_paths(["/synth/key"]);
        assert_eq!(paths.key_fun, KeyConversion::Key);
        assert!((paths.key_fun.convert(69) - 69.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gain_suffix_normalizes_velocity() {
        let paths = extract_paths(["/synth/gain"]);
        assert_eq!(paths.vel_fun, VelConversion::Gain);
        assert!((paths.vel_fun.convert(127) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn vel_and_velocity_suffixes_are_identity_not_normalized() {
        for suffix in ["/synth/vel", "/synth/velocity"] {
            let paths = extract_paths([suffix]);
            assert_eq!(paths.vel_fun, VelConversion::Velocity);
            assert!((paths.vel_fun.convert(64) - 64.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn gate_paths_are_collected_and_do_not_affect_conversions() {
        let paths = extract_paths(["/synth/gate", "/synth/freq", "/synth/gain"]);
        assert_eq!(paths.gate, vec!["/synth/gate"]);
        assert_eq!(paths.freq, vec!["/synth/freq"]);
        assert_eq!(paths.gain, vec!["/synth/gain"]);
    }

    #[test]
    fn unrelated_paths_are_ignored() {
        let paths = extract_paths(["/synth/cutoff", "/synth/resonance"]);
        assert!(paths.gate.is_empty() && paths.freq.is_empty() && paths.gain.is_empty());
    }

    #[test]
    fn default_conversions_match_dsp_voice_constructor() {
        // A voice declaring neither /freq, /key, /gain, /vel nor /velocity
        // still has a defined conversion, matching the C++ default before
        // extractPaths runs (poly-dsp.h:186-187).
        let paths = VoiceControlPaths::default();
        assert_eq!(paths.key_fun, KeyConversion::Freq);
        assert_eq!(paths.vel_fun, VelConversion::Gain);
    }

    #[test]
    fn get_free_voice_picks_the_first_free_slot() {
        let voices = vec![
            VoiceState {
                cur_note: 60,
                ..VoiceState::free()
            },
            VoiceState::free(),
            VoiceState::free(),
        ];
        assert_eq!(get_free_voice(&voices), Some(Allocation::Free(1)));
    }

    #[test]
    fn get_free_voice_steals_oldest_releasing_before_oldest_playing() {
        let voices = vec![
            VoiceState {
                cur_note: 60,
                date: 5,
                ..VoiceState::free()
            }, // playing, newer
            VoiceState {
                cur_note: RELEASE_VOICE,
                date: 1,
                ..VoiceState::free()
            }, // releasing, older
            VoiceState {
                cur_note: RELEASE_VOICE,
                date: 3,
                ..VoiceState::free()
            }, // releasing, newer
        ];
        // Neither voice is free, so a releasing voice is stolen; among the
        // two releasing voices, the oldest (index 1) is taken.
        assert_eq!(get_free_voice(&voices), Some(Allocation::Steal(1)));
    }

    #[test]
    fn get_free_voice_steals_oldest_playing_when_none_releasing() {
        let voices = vec![
            VoiceState {
                cur_note: 60,
                date: 5,
                ..VoiceState::free()
            },
            VoiceState {
                cur_note: 62,
                date: 2,
                ..VoiceState::free()
            },
        ];
        assert_eq!(get_free_voice(&voices), Some(Allocation::Steal(1)));
    }

    #[test]
    fn get_free_voice_returns_none_for_an_empty_table() {
        assert_eq!(get_free_voice(&[]), None);
    }

    #[test]
    fn get_playing_voice_finds_oldest_matching_pitch() {
        let voices = vec![
            VoiceState {
                cur_note: 60,
                date: 5,
                ..VoiceState::free()
            },
            VoiceState {
                cur_note: 60,
                date: 2,
                ..VoiceState::free()
            }, // older, same pitch
            VoiceState {
                cur_note: 61,
                date: 1,
                ..VoiceState::free()
            },
        ];
        assert_eq!(get_playing_voice(&voices, 60), Some(1));
    }

    #[test]
    fn get_playing_voice_matches_a_legato_voices_queued_note() {
        // A stolen voice hasn't sounded its new pitch yet, but a note-off for
        // that pitch must still find it — otherwise a note that is stolen and
        // immediately released could never be released.
        let voices = vec![VoiceState {
            cur_note: LEGATO_VOICE,
            next_note: 64,
            date: 1,
            ..VoiceState::free()
        }];
        assert_eq!(get_playing_voice(&voices, 64), Some(0));
    }

    #[test]
    fn get_playing_voice_returns_none_when_nothing_matches() {
        let voices = vec![VoiceState::free()];
        assert_eq!(get_playing_voice(&voices, 60), None);
    }

    #[test]
    fn key_on_of_an_already_sounding_pitch_allocates_a_new_voice() {
        // Confirms the correction to design §3.2's "retrigger" wording:
        // getPlayingVoice is a keyOff concern (poly-dsp.h:914), not keyOn's
        // (poly-dsp.h:900-904 calls only getFreeVoice). Two note-ons for the
        // same pitch must land on two different voices.
        let mut state = PolyState::new(4);
        let (v1, w1) = state.key_on(60, 100, KeyConversion::Freq, VelConversion::Gain);
        let (v2, w2) = state.key_on(60, 100, KeyConversion::Freq, VelConversion::Gain);
        assert_ne!(v1, v2);
        assert!(matches!(w1, Some(VoiceWrite::KeyOn { .. })));
        assert!(matches!(w2, Some(VoiceWrite::KeyOn { .. })));
    }

    #[test]
    fn key_on_writes_the_converted_frequency_and_gain() {
        let mut state = PolyState::new(1);
        let (_, write) = state.key_on(69, 127, KeyConversion::Freq, VelConversion::Gain);
        match write {
            Some(VoiceWrite::KeyOn { freq, gain }) => {
                assert!((freq - 440.0).abs() < 1e-9);
                assert!((gain - 1.0).abs() < 1e-9);
            }
            other => panic!("expected KeyOn, got {other:?}"),
        }
    }

    #[test]
    fn stealing_a_voice_defers_the_write_to_apply_legato() {
        let mut state = PolyState::new(1);
        let _ = state.key_on(60, 100, KeyConversion::Freq, VelConversion::Gain);
        // Only voice is now active (not free); the next key_on must steal it.
        let (voice, write) = state.key_on(64, 100, KeyConversion::Freq, VelConversion::Gain);
        assert_eq!(voice, 0);
        assert_eq!(write, None);
        assert_eq!(state.voices[0].cur_note, LEGATO_VOICE);
        assert_eq!(state.voices[0].next_note, 64);

        let applied = state.apply_legato(0, KeyConversion::Freq, VelConversion::Gain);
        assert_eq!(state.voices[0].cur_note, 64);
        assert!(matches!(applied, VoiceWrite::KeyOn { .. }));
    }

    #[test]
    fn key_off_releases_the_oldest_matching_voice() {
        let mut state = PolyState::new(2);
        let (v1, _) = state.key_on(60, 100, KeyConversion::Freq, VelConversion::Gain);
        let (v2, _) = state.key_on(61, 100, KeyConversion::Freq, VelConversion::Gain);
        let (released, write) = state.key_off(60, false).unwrap();
        assert_eq!(released, v1);
        assert_eq!(write, VoiceWrite::KeyOff);
        assert_eq!(state.voices[v1].cur_note, RELEASE_VOICE);
        assert_eq!(state.voices[v2].cur_note, 61); // untouched
    }

    #[test]
    fn hard_key_off_frees_immediately_instead_of_releasing() {
        let mut state = PolyState::new(1);
        let _ = state.key_on(60, 100, KeyConversion::Freq, VelConversion::Gain);
        let _ = state.key_off(60, true);
        assert_eq!(state.voices[0].cur_note, FREE_VOICE);
    }

    #[test]
    fn key_off_of_a_silent_pitch_does_nothing() {
        let mut state = PolyState::new(1);
        assert_eq!(state.key_off(60, false), None);
    }

    #[test]
    fn record_level_reclaims_a_releasing_voice_below_the_stop_level() {
        let mut state = PolyState::new(1);
        state.voices[0].cur_note = RELEASE_VOICE;
        state.record_level(0, 0.00001, DEFAULT_VOICE_STOP_LEVEL);
        assert_eq!(state.voices[0].cur_note, FREE_VOICE);
    }

    #[test]
    fn record_level_keeps_a_releasing_voice_above_the_stop_level() {
        let mut state = PolyState::new(1);
        state.voices[0].cur_note = RELEASE_VOICE;
        state.record_level(0, 0.1, DEFAULT_VOICE_STOP_LEVEL);
        assert_eq!(state.voices[0].cur_note, RELEASE_VOICE);
    }

    #[test]
    fn record_level_does_not_reclaim_a_playing_voice() {
        // Only a releasing voice is ever reclaimed by level; a sustained,
        // quiet-but-playing voice must not be silently dropped.
        let mut state = PolyState::new(1);
        state.voices[0].cur_note = 60;
        state.record_level(0, 0.0, DEFAULT_VOICE_STOP_LEVEL);
        assert_eq!(state.voices[0].cur_note, 60);
    }

    #[test]
    fn active_count_excludes_only_free_voices() {
        let mut state = PolyState::new(3);
        state.voices[0].cur_note = 60;
        state.voices[1].cur_note = RELEASE_VOICE;
        // voices[2] stays free.
        assert_eq!(state.active_count(), 2);
    }

    #[test]
    fn mix_check_voice_sums_into_out_and_reports_rms() {
        let voice = vec![vec![1.0, -1.0], vec![1.0, -1.0]];
        let mut out = vec![vec![0.0, 0.0], vec![0.0, 0.0]];
        let level = mix_check_voice(&voice, &mut out);
        assert!((level - 1.0).abs() < f64::EPSILON);
        assert_eq!(out, voice);
    }

    #[test]
    fn mix_check_voice_accumulates_rather_than_overwrites() {
        let voice = vec![vec![1.0]];
        let mut out = vec![vec![2.0]];
        let _ = mix_check_voice(&voice, &mut out);
        assert!((out[0][0] - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fade_out_only_touches_the_first_half() {
        let mut buffer = vec![vec![1.0, 1.0, 1.0, 1.0]];
        fade_out(&mut buffer, 2);
        assert!((buffer[0][0] - 1.0).abs() < f64::EPSILON); // factor starts at 1.0
        assert!(buffer[0][1] < 1.0 && buffer[0][1] > 0.0); // decaying
        assert!((buffer[0][2] - 1.0).abs() < f64::EPSILON); // untouched
        assert!((buffer[0][3] - 1.0).abs() < f64::EPSILON); // untouched
    }

    #[test]
    fn fade_out_of_zero_length_is_a_no_op() {
        let mut buffer = vec![vec![1.0, 1.0]];
        fade_out(&mut buffer, 0);
        assert_eq!(buffer, vec![vec![1.0, 1.0]]);
    }
}
