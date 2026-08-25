//! Prepared-signal and artifact ID indexing shared by the checked stages.
//!
//! Every vector stage builds `key -> record` maps over certificate rows.
//! The raw `.map(|x| (x.id, x)).collect::<BTreeMap<_, _>>()` idiom silently
//! keeps the *last* row on a duplicate key; these helpers reject duplicates
//! instead, so a malformed artifact fails loudly at the indexing boundary.
//! On artifacts that already passed their uniqueness checks the behavior is
//! identical.

use std::collections::BTreeMap;

use crate::signal_prepare::VerifiedPreparedSignals;
use signals::SigId;

/// Maps every prepared signal's raw `u32` id back to its [`SigId`].
///
/// The prepared type map is keyed by `SigId`, so this index is total and
/// duplicate-free by construction.
pub(crate) fn prepared_signal_ids(prepared: &VerifiedPreparedSignals) -> BTreeMap<u32, SigId> {
    prepared
        .sig_types_map()
        .keys()
        .map(|&sig| (sig.as_u32(), sig))
        .collect()
}
