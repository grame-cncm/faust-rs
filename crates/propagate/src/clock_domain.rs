//! Clock-domain side table (roadmap P0.2).
//!
//! # Source provenance (C++)
//! - `propagate.cpp` (`makeClockEnv(...)`): C++ threads a
//!   `(parent, slotenv, path, box, inputs...)` cons tuple through clocked
//!   wrappers, where `slotenv` + `path` act as the *instance uniqueness*
//!   component (added after the de Bruijn collision bug — see
//!   `porting/ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md` §3.4).
//!
//! # Adaptation status
//! Following the plan's §5.3 recommendation, Rust replaces the structural cons
//! tuple with a dedicated side-table arena: every propagation of one
//! `ondemand` / `upsampling` / `downsampling` wrapper allocates one fresh
//! [`ClockDomain`] entry and embeds only its integer id in the signal graph
//! (as the opaque `SIGCLOCKENV` token, first child of `Clocked(env, y)`).
//!
//! Structural identity for an entity whose whole point is *instance* identity
//! is fragile under hash-consing: two structurally identical wrapper instances
//! in different contexts must still be distinct domains. The allocated id *is*
//! the uniqueness token, so that collision class is gone by construction.
//!
//! # Arena caveat
//! `clock` and `inputs` reference the arena in which propagation ran. Passes
//! that clone the forest into a private arena (e.g. `signal_prepare`) must
//! not dereference these ids against the cloned arena; the in-graph
//! `OnDemand`/`Upsampling`/`Downsampling` payloads carry the same information
//! locally (first payload child is `Clocked(env, clock)`).

use signals::SigId;
use tlib::TreeId;

/// Integer id of one [`ClockDomain`] entry inside a [`ClockDomainTable`].
///
/// The id doubles as the instance-uniqueness token: each propagated wrapper
/// instance allocates a fresh id, so ids compare equal only for the *same*
/// instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClockDomainId(u32);

impl ClockDomainId {
    /// Returns the raw integer id (used to build the `SIGCLOCKENV` token).
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0
    }

    /// Rebuilds an id from a raw integer (used when decoding a `SIGCLOCKENV`
    /// token back into a table lookup).
    #[must_use]
    pub fn from_u32(value: u32) -> Self {
        Self(value)
    }
}

/// Kind of the clocked wrapper that created one domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockDomainKind {
    /// `ondemand(FX)` — fires when the clock is non-zero.
    OnDemand,
    /// `upsampling(FX)` — the body runs `clock` times per outer tick.
    Upsampling,
    /// `downsampling(FX)` — the body runs once every `clock` outer ticks.
    Downsampling,
}

/// One clock-domain instance created while propagating a clocked wrapper.
#[derive(Clone, Debug)]
pub struct ClockDomain {
    /// Enclosing domain, or `None` for a wrapper at the top-level rate.
    pub parent: Option<ClockDomainId>,
    /// Which wrapper primitive created the domain.
    pub kind: ClockDomainKind,
    /// The propagated clock signal (first wrapper input).
    pub clock: SigId,
    /// The wrapper box node (diagnostics / provenance).
    pub wrapper_box: TreeId,
    /// The propagated non-clock wrapper inputs, in order.
    pub inputs: Vec<SigId>,
}

/// Append-only arena of [`ClockDomain`] entries for one propagation run.
#[derive(Debug, Default)]
pub struct ClockDomainTable {
    domains: Vec<ClockDomain>,
}

impl ClockDomainTable {
    /// Creates an empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates one fresh domain instance and returns its unique id.
    pub fn alloc(&mut self, domain: ClockDomain) -> ClockDomainId {
        let id = u32::try_from(self.domains.len())
            .expect("clock-domain table cannot exceed u32::MAX entries");
        self.domains.push(domain);
        ClockDomainId(id)
    }

    /// Returns one domain entry by id.
    #[must_use]
    pub fn get(&self, id: ClockDomainId) -> Option<&ClockDomain> {
        self.domains.get(id.0 as usize)
    }

    /// Number of allocated domains.
    #[must_use]
    pub fn len(&self) -> usize {
        self.domains.len()
    }

    /// Returns `true` when no domain has been allocated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }

    /// Iterates `(id, domain)` pairs in allocation order.
    pub fn iter(&self) -> impl Iterator<Item = (ClockDomainId, &ClockDomain)> {
        self.domains
            .iter()
            .enumerate()
            .map(|(index, domain)| (ClockDomainId(index as u32), domain))
    }
}
