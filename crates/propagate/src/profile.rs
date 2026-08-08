//! Opt-in attribution for Box-to-Signal propagation cost.
//!
//! C++ parity source: `compiler/propagate/propagate.cpp` and its
//! `FAUST_PROPAGATE_PROFILE` table. The profiler is deliberately dormant unless
//! that environment variable is present. Normal compiler runs therefore keep
//! no counters and take no clocks; profiled runs print comparable per-Box-kind
//! call counts plus Rust-specific slot, result-bus, lifting, and provenance
//! measurements.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::FlatNodeKind;

const PROFILE_KIND_COUNT: usize = 15;

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub(crate) enum PropagateProfileKind {
    Atom,
    WireCutSlot,
    Prim,
    Ui,
    Group,
    Seq,
    Par,
    Split,
    Merge,
    Rec,
    Route,
    Symbolic,
    Extended,
    Ad,
    Other,
}

impl PropagateProfileKind {
    pub(crate) fn from_flat(kind: FlatNodeKind) -> Self {
        match kind {
            FlatNodeKind::Int
            | FlatNodeKind::Real
            | FlatNodeKind::Waveform
            | FlatNodeKind::FConst
            | FlatNodeKind::FVar
            | FlatNodeKind::Environment => Self::Atom,
            FlatNodeKind::Wire | FlatNodeKind::Cut | FlatNodeKind::Slot => Self::WireCutSlot,
            FlatNodeKind::Prim1
            | FlatNodeKind::Prim2
            | FlatNodeKind::Prim3
            | FlatNodeKind::Prim4
            | FlatNodeKind::Prim5
            | FlatNodeKind::FFun => Self::Prim,
            FlatNodeKind::Button
            | FlatNodeKind::Checkbox
            | FlatNodeKind::VSlider
            | FlatNodeKind::HSlider
            | FlatNodeKind::NumEntry
            | FlatNodeKind::VBargraph
            | FlatNodeKind::HBargraph
            | FlatNodeKind::Soundfile => Self::Ui,
            FlatNodeKind::VGroup { .. }
            | FlatNodeKind::HGroup { .. }
            | FlatNodeKind::TGroup { .. } => Self::Group,
            FlatNodeKind::Seq(..) => Self::Seq,
            FlatNodeKind::Par(..) => Self::Par,
            FlatNodeKind::Split(..) => Self::Split,
            FlatNodeKind::Merge(..) => Self::Merge,
            FlatNodeKind::Rec(..) => Self::Rec,
            FlatNodeKind::Route | FlatNodeKind::Inputs | FlatNodeKind::Outputs => Self::Route,
            FlatNodeKind::Symbolic { .. } => Self::Symbolic,
            FlatNodeKind::Metadata { .. }
            | FlatNodeKind::Ondemand(..)
            | FlatNodeKind::Upsampling(..)
            | FlatNodeKind::Downsampling(..) => Self::Extended,
            FlatNodeKind::ForwardAD { .. } | FlatNodeKind::ReverseAD { .. } => Self::Ad,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Atom => "atom",
            Self::WireCutSlot => "wire/cut/slot",
            Self::Prim => "prim",
            Self::Ui => "ui",
            Self::Group => "group",
            Self::Seq => "seq",
            Self::Par => "par",
            Self::Split => "split",
            Self::Merge => "merge",
            Self::Rec => "rec",
            Self::Route => "route",
            Self::Symbolic => "symbolic",
            Self::Extended => "extended",
            Self::Ad => "ad",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ProfileEntry {
    calls: u64,
    input_signals: u64,
    output_signals: u64,
    slot_bindings: u64,
    total: Duration,
}

/// Per-top-level propagation counters enabled by `FAUST_PROPAGATE_PROFILE`.
///
/// Mapping status: adapted from C++ `PropagateProfileEntry`. Rust additionally
/// records output-bus and slot-environment sizes, lifting-cache behavior, and
/// provenance time because those costs live in Rust's propagation contract.
pub(crate) struct PropagateProfile {
    enabled: bool,
    entries: [ProfileEntry; PROFILE_KIND_COUNT],
    liftn_calls: u64,
    liftn_hits: u64,
    origin_calls: u64,
    origin_time: Duration,
}

impl Default for PropagateProfile {
    fn default() -> Self {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        let enabled =
            *ENABLED.get_or_init(|| std::env::var_os("FAUST_PROPAGATE_PROFILE").is_some());
        Self::new(enabled)
    }
}

impl PropagateProfile {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            entries: [ProfileEntry::default(); PROFILE_KIND_COUNT],
            liftn_calls: 0,
            liftn_hits: 0,
            origin_calls: 0,
            origin_time: Duration::ZERO,
        }
    }

    #[cfg(test)]
    pub(crate) fn enabled_for_test() -> Self {
        Self::new(true)
    }

    #[inline]
    pub(crate) const fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[inline]
    pub(crate) fn start(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    pub(crate) fn record_call(
        &mut self,
        kind: PropagateProfileKind,
        inputs: usize,
        outputs: usize,
        slot_bindings: usize,
        started: Option<Instant>,
    ) {
        if !self.enabled {
            return;
        }
        let entry = &mut self.entries[kind as usize];
        entry.calls += 1;
        entry.input_signals += inputs as u64;
        entry.output_signals += outputs as u64;
        entry.slot_bindings += slot_bindings as u64;
        if let Some(started) = started {
            entry.total += started.elapsed();
        }
    }

    #[inline]
    pub(crate) fn record_liftn_call(&mut self, hit: bool) {
        if self.enabled {
            self.liftn_calls += 1;
            self.liftn_hits += u64::from(hit);
        }
    }

    pub(crate) fn record_origins(&mut self, started: Option<Instant>) {
        if !self.enabled {
            return;
        }
        self.origin_calls += 1;
        if let Some(started) = started {
            self.origin_time += started.elapsed();
        }
    }

    /// Prints one table for traversals large enough to be useful, matching the
    /// C++ profiler's suppression of tiny constant-folding propagations.
    pub(crate) fn print(&self) {
        let total_calls = self.entries.iter().map(|entry| entry.calls).sum::<u64>();
        if !self.enabled || total_calls < 1_000 {
            return;
        }

        eprintln!("\npropagation profile by box kind");
        eprintln!("kind\tcalls\ttotal_s\tavg_in\tavg_out\tavg_slots");
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.calls == 0 {
                continue;
            }
            let kind = match index {
                0 => PropagateProfileKind::Atom,
                1 => PropagateProfileKind::WireCutSlot,
                2 => PropagateProfileKind::Prim,
                3 => PropagateProfileKind::Ui,
                4 => PropagateProfileKind::Group,
                5 => PropagateProfileKind::Seq,
                6 => PropagateProfileKind::Par,
                7 => PropagateProfileKind::Split,
                8 => PropagateProfileKind::Merge,
                9 => PropagateProfileKind::Rec,
                10 => PropagateProfileKind::Route,
                11 => PropagateProfileKind::Symbolic,
                12 => PropagateProfileKind::Extended,
                13 => PropagateProfileKind::Ad,
                _ => PropagateProfileKind::Other,
            };
            let calls = entry.calls as f64;
            eprintln!(
                "{}\t{}\t{:.6}\t{:.3}\t{:.3}\t{:.3}",
                kind.name(),
                entry.calls,
                entry.total.as_secs_f64(),
                entry.input_signals as f64 / calls,
                entry.output_signals as f64 / calls,
                entry.slot_bindings as f64 / calls,
            );
        }
        eprintln!(
            "liftn\tcalls={}\thits={}\thit_rate={:.1}%",
            self.liftn_calls,
            self.liftn_hits,
            percent(self.liftn_hits, self.liftn_calls),
        );
        eprintln!(
            "origins\tcalls={}\ttotal_s={:.6}",
            self.origin_calls,
            self.origin_time.as_secs_f64(),
        );
    }
}

fn percent(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * part as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_profile_records_rust_specific_dimensions() {
        let mut profile = PropagateProfile::enabled_for_test();
        let started = profile.start();
        profile.record_call(PropagateProfileKind::Seq, 2, 1, 3, started);
        profile.record_liftn_call(false);
        profile.record_liftn_call(true);
        profile.record_origins(profile.start());

        let seq = profile.entries[PropagateProfileKind::Seq as usize];
        assert_eq!(seq.calls, 1);
        assert_eq!(seq.input_signals, 2);
        assert_eq!(seq.output_signals, 1);
        assert_eq!(seq.slot_bindings, 3);
        assert_eq!(profile.liftn_calls, 2);
        assert_eq!(profile.liftn_hits, 1);
        assert_eq!(profile.origin_calls, 1);
    }

    #[test]
    fn flat_kinds_match_cpp_profile_families() {
        assert!(matches!(
            PropagateProfileKind::from_flat(FlatNodeKind::Wire),
            PropagateProfileKind::WireCutSlot
        ));
        assert!(matches!(
            PropagateProfileKind::from_flat(FlatNodeKind::Prim1),
            PropagateProfileKind::Prim
        ));
    }
}
