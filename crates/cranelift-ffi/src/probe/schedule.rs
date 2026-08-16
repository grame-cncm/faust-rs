//! Frame-scheduled events: parameter changes and note on/off.
//!
//! # Why this phase exists
//! Up to P4 the probe could set a control before a render and nothing after.
//! That is enough to characterise a static operating point, and not enough for
//! anything whose whole behaviour is a response to change: an envelope's
//! release time, a smoother's glide, a filter's crossfade when its type
//! switches, a delay's ping-pong seeded by one note.
//!
//! The reference impulse protocol holds every button for the first block and
//! then releases it — 64 samples. Measuring a 0.5 s release needs a gate held
//! for tens of thousands, which is why this is a separate phase rather than a
//! flag on the old path.
//!
//! # Timing
//! Events are sample-exact. The render loop shortens its block so that a
//! block boundary always falls on the next scheduled frame, rather than
//! rounding events to the block grid: at block 64 a rounding scheme would
//! place a note-off up to 63 frames late, which is small against a 0.5 s
//! release and not small against a 5 ms one.

use std::collections::BTreeMap;

/// Something to do at a given frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Write `value` to the control matching `path`.
    ///
    /// Resolved the same way `--set` is, so a trailing fragment works.
    SetParam { path: String, value: f64 },
    /// Note on. Requires a polyphonic render.
    NoteOn { pitch: i32, velocity: i32 },
    /// Note off. Requires a polyphonic render.
    NoteOff { pitch: i32 },
}

impl Event {
    /// Whether this event needs the polyphonic wrapper.
    #[must_use]
    pub const fn needs_poly(&self) -> bool {
        matches!(self, Self::NoteOn { .. } | Self::NoteOff { .. })
    }
}

/// Events ordered by frame.
///
/// A `BTreeMap` keyed by frame keeps the render loop's "when is the next
/// boundary?" query cheap and keeps events at the same frame in insertion
/// order, which matters when a note-off and a note-on for the same pitch
/// coincide: the off must win first or the on would be cancelled.
#[derive(Debug, Clone, Default)]
pub struct Schedule {
    events: BTreeMap<usize, Vec<Event>>,
}

impl Schedule {
    /// Empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Paths written by scheduled `SetParam` events, in frame order.
    ///
    /// Used to reject a schedule that writes a control a sweep is also
    /// driving: the later write would silently win and the swept axis would
    /// report values the render never used.
    #[must_use]
    pub fn param_paths(&self) -> Vec<&str> {
        self.events
            .values()
            .flatten()
            .filter_map(|e| match e {
                Event::SetParam { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Whether nothing is scheduled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Total number of events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.values().map(Vec::len).sum()
    }

    /// Add `event` at `frame`.
    pub fn push(&mut self, frame: usize, event: Event) {
        self.events.entry(frame).or_default().push(event);
    }

    /// Events at exactly `frame`, in insertion order.
    #[must_use]
    pub fn at(&self, frame: usize) -> &[Event] {
        self.events.get(&frame).map_or(&[], Vec::as_slice)
    }

    /// The next frame strictly after `frame` that carries an event.
    ///
    /// The render loop uses this to clamp its block length so no event is
    /// ever crossed mid-block.
    #[must_use]
    pub fn next_after(&self, frame: usize) -> Option<usize> {
        self.events.range(frame + 1..).next().map(|(f, _)| *f)
    }

    /// Whether any event requires a polyphonic render.
    #[must_use]
    pub fn needs_poly(&self) -> bool {
        self.events.values().flatten().any(Event::needs_poly)
    }

    /// The last frame carrying an event, if any.
    #[must_use]
    pub fn last_frame(&self) -> Option<usize> {
        self.events.keys().next_back().copied()
    }
}

/// Parse `--at FRAME PATH=VALUE`, given the two arguments already split.
///
/// # Errors
/// Returns a message when the frame is not a number, or the assignment is
/// malformed.
pub fn parse_at(frame: &str, assignment: &str) -> Result<(usize, Event), String> {
    let frame: usize = frame
        .parse()
        .map_err(|_| format!("`{frame}` is not a frame index in `--at {frame} {assignment}`"))?;
    let (path, value) = assignment
        .split_once('=')
        .ok_or_else(|| format!("expected PATH=VALUE, got `{assignment}`"))?;
    let value: f64 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a number in `{assignment}`"))?;
    Ok((
        frame,
        Event::SetParam {
            path: path.to_owned(),
            value,
        },
    ))
}

/// Parse `PITCH:VEL@ON..OFF` or `PITCH@ON..OFF` into a note-on/note-off pair.
///
/// Velocity defaults to 100, the value the reference `Key2Midi` helper uses.
/// `OFF` may be omitted (`PITCH@ON..`), leaving the note held to the end of
/// the render — useful for measuring an attack without a release in the way.
///
/// # Errors
/// Returns a message naming the offending field.
pub fn parse_note(text: &str) -> Result<Vec<(usize, Event)>, String> {
    let (head, span) = text
        .split_once('@')
        .ok_or_else(|| format!("expected PITCH[:VEL]@ON[..OFF], got `{text}`"))?;
    let (pitch_text, vel_text) = head
        .split_once(':')
        .map_or((head, None), |(p, v)| (p, Some(v)));
    let pitch: i32 = pitch_text
        .trim()
        .parse()
        .map_err(|_| format!("`{pitch_text}` is not a MIDI pitch in `{text}`"))?;
    let velocity: i32 = match vel_text {
        Some(v) => v
            .trim()
            .parse()
            .map_err(|_| format!("`{v}` is not a velocity in `{text}`"))?,
        None => 100,
    };
    if !(0..=127).contains(&pitch) {
        return Err(format!(
            "pitch {pitch} out of MIDI range 0..=127 in `{text}`"
        ));
    }
    if !(0..=127).contains(&velocity) {
        return Err(format!(
            "velocity {velocity} out of MIDI range 0..=127 in `{text}`"
        ));
    }

    let (on_text, off_text) = span
        .split_once("..")
        .map_or((span, None), |(a, b)| (a, Some(b)));
    let on: usize = on_text
        .trim()
        .parse()
        .map_err(|_| format!("`{on_text}` is not a frame index in `{text}`"))?;

    let mut out = vec![(on, Event::NoteOn { pitch, velocity })];
    match off_text.map(str::trim) {
        None | Some("") => {}
        Some(off_text) => {
            let off: usize = off_text
                .parse()
                .map_err(|_| format!("`{off_text}` is not a frame index in `{text}`"))?;
            if off < on {
                return Err(format!(
                    "note-off frame {off} precedes note-on frame {on} in `{text}`"
                ));
            }
            out.push((off, Event::NoteOff { pitch }));
        }
    }
    Ok(out)
}

/// Parse `P1,P2,...:VEL@ON..OFF` into one note per pitch.
///
/// # Errors
/// As [`parse_note`], plus a message when the pitch list is empty.
pub fn parse_chord(text: &str) -> Result<Vec<(usize, Event)>, String> {
    let (pitches, rest) = text
        .split_once(':')
        .or_else(|| text.split_once('@'))
        .ok_or_else(|| format!("expected P1,P2,...[:VEL]@ON[..OFF], got `{text}`"))?;
    // Re-attach the separator that `split_once` consumed so `parse_note` sees
    // a well-formed single-note spec.
    let separator = if text[pitches.len()..].starts_with(':') {
        ':'
    } else {
        '@'
    };
    let mut out = Vec::new();
    let mut any = false;
    for pitch in pitches.split(',') {
        let pitch = pitch.trim();
        if pitch.is_empty() {
            return Err(format!("empty pitch in `{text}`"));
        }
        any = true;
        out.extend(parse_note(&format!("{pitch}{separator}{rest}"))?);
    }
    if !any {
        return Err(format!("no pitches in `{text}`"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_parses_pitch_velocity_and_span() {
        let events = parse_note("60:90@100..20000").unwrap();
        assert_eq!(
            events,
            vec![
                (
                    100,
                    Event::NoteOn {
                        pitch: 60,
                        velocity: 90
                    }
                ),
                (20_000, Event::NoteOff { pitch: 60 }),
            ]
        );
    }

    #[test]
    fn note_velocity_defaults_to_100() {
        let events = parse_note("60@0..10").unwrap();
        assert_eq!(
            events[0].1,
            Event::NoteOn {
                pitch: 60,
                velocity: 100
            }
        );
    }

    #[test]
    fn note_without_an_off_is_held_to_the_end() {
        // Measuring an attack should not require inventing a release that
        // would then be in the way.
        assert_eq!(parse_note("60@100..").unwrap().len(), 1);
        assert_eq!(parse_note("60@100").unwrap().len(), 1);
    }

    #[test]
    fn note_rejects_reversed_span() {
        // Silently swapping the ends would render something the caller did
        // not ask for and quietly measure the wrong thing.
        assert!(parse_note("60@200..100").is_err());
    }

    #[test]
    fn note_rejects_out_of_range_midi_values() {
        assert!(parse_note("128@0..10").is_err());
        assert!(parse_note("60:200@0..10").is_err());
        assert!(parse_note("-1@0..10").is_err());
    }

    #[test]
    fn note_rejects_malformed_input() {
        assert!(parse_note("60").is_err());
        assert!(parse_note("x@0..10").is_err());
        assert!(parse_note("60@x..10").is_err());
    }

    #[test]
    fn chord_expands_to_one_note_per_pitch() {
        let events = parse_chord("60,64,67:90@100..20000").unwrap();
        assert_eq!(events.len(), 6); // three on, three off
        assert_eq!(
            events[0].1,
            Event::NoteOn {
                pitch: 60,
                velocity: 90
            }
        );
        assert_eq!(events[5].1, Event::NoteOff { pitch: 67 });
    }

    #[test]
    fn chord_without_velocity_still_parses() {
        assert_eq!(parse_chord("60,64@0..10").unwrap().len(), 4);
    }

    #[test]
    fn chord_rejects_an_empty_pitch() {
        assert!(parse_chord("60,,64:90@0..10").is_err());
    }

    #[test]
    fn at_parses_frame_and_assignment() {
        let (frame, event) = parse_at("4800", "cutoff=1000").unwrap();
        assert_eq!(frame, 4800);
        assert_eq!(
            event,
            Event::SetParam {
                path: "cutoff".to_owned(),
                value: 1000.0
            }
        );
    }

    #[test]
    fn at_rejects_malformed_input() {
        assert!(parse_at("x", "cutoff=1").is_err());
        assert!(parse_at("0", "cutoff").is_err());
        assert!(parse_at("0", "cutoff=loud").is_err());
    }

    #[test]
    fn schedule_orders_by_frame_and_finds_the_next_boundary() {
        let mut s = Schedule::new();
        s.push(100, Event::NoteOff { pitch: 60 });
        s.push(
            10,
            Event::NoteOn {
                pitch: 60,
                velocity: 100,
            },
        );
        assert_eq!(s.next_after(0), Some(10));
        assert_eq!(s.next_after(10), Some(100));
        assert_eq!(s.next_after(100), None);
        assert_eq!(s.last_frame(), Some(100));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn schedule_keeps_same_frame_events_in_insertion_order() {
        // A note-off and a note-on for the same pitch at one frame: the off
        // must be applied first, or the on it was meant to precede is undone.
        let mut s = Schedule::new();
        s.push(50, Event::NoteOff { pitch: 60 });
        s.push(
            50,
            Event::NoteOn {
                pitch: 60,
                velocity: 100,
            },
        );
        assert_eq!(s.at(50)[0], Event::NoteOff { pitch: 60 });
    }

    #[test]
    fn needs_poly_only_for_note_events() {
        let mut s = Schedule::new();
        s.push(
            0,
            Event::SetParam {
                path: "g".into(),
                value: 1.0,
            },
        );
        assert!(!s.needs_poly());
        s.push(
            1,
            Event::NoteOn {
                pitch: 60,
                velocity: 100,
            },
        );
        assert!(s.needs_poly());
    }

    /// `--at` targets must be discoverable so a sweep can refuse to be
    /// overridden by one.
    #[test]
    fn param_paths_lists_only_scheduled_writes() {
        let mut sched = Schedule::new();
        sched.push(
            10,
            Event::SetParam {
                path: "freq".to_owned(),
                value: 440.0,
            },
        );
        sched.push(
            20,
            Event::NoteOn {
                pitch: 60,
                velocity: 100,
            },
        );
        sched.push(
            30,
            Event::SetParam {
                path: "gain".to_owned(),
                value: 0.5,
            },
        );
        assert_eq!(sched.param_paths(), vec!["freq", "gain"]);
    }

    #[test]
    fn param_paths_is_empty_without_a_schedule() {
        assert!(Schedule::new().param_paths().is_empty());
    }
}
