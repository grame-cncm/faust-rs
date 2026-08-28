//! Signal-level table access protection (`-ct` / check-table).
//!
//! Port of the C++ `SignalTablePromotion` pass
//! (`compiler/transform/sigPromotion.cpp:577-640`), invoked from
//! `simplifyToNormalForm` (`compiler/normalize/normalform.cpp:115`) when
//! `gGlobal->gCheckTable` is set (`-ct`, default 1).
//!
//! The pass rewrites every `SIGRDTBL` / `SIGWRTBL` access whose index the
//! interval analysis cannot prove in-bounds into a clamped form:
//!
//! ```text
//! rdtbl(tbl, ri)              →  rdtbl(tbl, max(0, min(ri, size-1)))
//! wrtbl(size, gen, wi, ws)    →  wrtbl(size, gen, max(0, min(wi, size-1)), ws)
//! ```
//!
//! Accesses whose index interval is already contained in `[0, size-1]` are
//! left untouched (identity), so provably safe programs pay nothing.
//!
//! # Pipeline position
//!
//! Like its C++ counterpart, the pass must run **after** algebraic
//! simplification (so table sizes are integer constants) and **before** the
//! final type annotation consumed by FIR lowering, with a **fresh**
//! `SigId → SigType` map: the clamp decision reads the index interval from
//! that map. The `min`/`max` typing rules
//! (`crates/sigtype/src/rules.rs`) then prove the rewritten index in-bounds,
//! so downstream consumers need no table-specific bounds logic at all.
//!
//! # C++ parity notes
//!
//! - The inserted clamp is always the **full** `max(0, min(ri, size-1))`
//!   pair, exactly like `safeSigRDTbl`, even when the interval would allow
//!   an upper-only `min`. Reference C++ keeps the pair because its
//!   interval-driven `min`/`max` pruning is disabled
//!   (`compiler/extended/maxprim.hh:95`); emitting the same shape keeps the
//!   generated code byte-comparable.
//! - An index with no recorded type (e.g. because an inner table access was
//!   itself rewritten, invalidating the memoized id — the case C++ notes as
//!   "The tree may not be properly typed because of a inner
//!   safeSigRDTbl/safeSigWRTbl call") is treated as
//!   `[INT32_MIN, INT32_MAX]` and clamped.
//! - A non-positive table size is a hard error, mirroring the C++
//!   `faustexception` (`RDTbl size = N should be > 0`).

use std::collections::HashMap;

use signals::{SigBuilder, SigId, SigMatch, match_sig};
use sigtype::SigType;
use tlib::TreeArena;

use crate::normalform::NormalFormError;

/// One out-of-range table access detected (and clamped) by
/// [`promote_table_signals`].
///
/// C++ reports these as `WARNING : RDTbl read index [lo:hi] is outside of
/// table size (N) ...` under `-wall` (`gAllWarning`); the Rust port surfaces
/// them through the semantic-warning channel instead of printing directly.
#[derive(Clone, Debug, PartialEq)]
pub struct TableRangeWarning {
    /// Lower bound of the index interval (`INT32_MIN` when the index had no
    /// recorded type).
    pub lo: f64,
    /// Upper bound of the index interval (`INT32_MAX` when the index had no
    /// recorded type).
    pub hi: f64,
    /// Table size the access was checked against.
    pub size: i32,
    /// The table-access signal that was rewritten (id in the rewritten
    /// forest).
    pub sig: SigId,
    /// `true` for a `SIGWRTBL` write index, `false` for a `SIGRDTBL` read
    /// index.
    pub write: bool,
}

impl std::fmt::Display for TableRangeWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (node, verb) = if self.write {
            ("WRTbl", "write")
        } else {
            ("RDTbl", "read")
        };
        write!(
            f,
            "{node} {verb} index [{}:{}] is outside of table size ({})",
            self.lo, self.hi, self.size
        )
    }
}

/// Rewrites unprovable table accesses of a signal forest into clamped form.
///
/// For every `SIGRDTBL(tbl, ri)` and every writable `SIGWRTBL(size, gen, wi,
/// ws)` reachable from `sigs`, the index is replaced by
/// `max(0, min(index, size-1))` unless its interval in `types` proves it
/// already contained in `[0, size-1]`. Read-only tables
/// (`SIGWRTBL(size, gen, nil, nil)`) have no runtime index and are left
/// untouched.
///
/// `types` is the canonical map produced by `sigtype::TypeAnnotator` on the
/// *current* forest; entries are looked up by `SigId`, so the map must be
/// fresh (see the module docs for the pipeline position). Every rewritten
/// access appends one [`TableRangeWarning`] to `warnings`.
///
/// # Errors
///
/// Returns [`NormalFormError::TableAccess`] when a checked access has a
/// non-positive or non-constant table size. Both are pipeline bugs at this
/// stage — simplification has already folded genuine constant sizes — and
/// match the C++ `faustexception` behavior (`tree2int` / size check in
/// `safeSigRDTbl`).
///
/// C++: `signalTablePromote(sig)` / class `SignalTablePromotion`
/// (`transform/sigPromotion.cpp:577-640`, `transform/sigPromotion.hh`).
pub fn promote_table_signals(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    sigs: &[SigId],
    warnings: &mut Vec<TableRangeWarning>,
) -> Result<Vec<SigId>, NormalFormError> {
    let mut promoter = TablePromoter {
        types,
        cache: HashMap::new(),
        warnings,
    };
    sigs.iter().map(|sig| promoter.map(arena, *sig)).collect()
}

/// Memoized traversal state for one [`promote_table_signals`] run.
///
/// Follows the `sig_map` shape used by `simplify` (itself mirroring C++
/// `sigMap`): depth-first rebuild of every node with a per-node cache, a
/// `Rec` sentinel against cycles, and a local transformation applied to each
/// rebuilt node.
struct TablePromoter<'a> {
    types: &'a HashMap<SigId, SigType>,
    /// `None` = node already visited and unchanged; `Some(r)` = rewritten to
    /// `r`.
    cache: HashMap<SigId, Option<SigId>>,
    warnings: &'a mut Vec<TableRangeWarning>,
}

impl TablePromoter<'_> {
    /// Rebuilds `sig` bottom-up, applying [`Self::transformation`] to every
    /// node.
    fn map(&mut self, arena: &mut TreeArena, sig: SigId) -> Result<SigId, NormalFormError> {
        if let Some(cached) = self.cache.get(&sig) {
            return Ok(cached.unwrap_or(sig));
        }

        // Rec node: mark sentinel before descending to avoid infinite loops.
        let rec_body = match match_sig(arena, sig) {
            SigMatch::Rec(body) => Some(body),
            _ => None,
        };
        if let Some(body) = rec_body {
            self.cache.insert(sig, None);
            let new_body = self.map(arena, body)?;
            return Ok(SigBuilder::new(arena).rec(new_body));
        }

        let (kind, children) = {
            let node = arena.node(sig).expect("table_promote: invalid SigId");
            (node.kind.clone(), node.children.as_slice().to_vec())
        };

        let mut new_children: Vec<SigId> = Vec::with_capacity(children.len());
        for &c in &children {
            new_children.push(self.map(arena, c)?);
        }
        let rebuilt = arena.intern(kind, &new_children);

        let result = self.transformation(arena, rebuilt)?;

        if result == sig {
            self.cache.insert(sig, None);
        } else {
            self.cache.insert(sig, Some(result));
        }
        Ok(result)
    }

    /// Applies the table-clamping rule to one rebuilt node.
    ///
    /// C++: `SignalTablePromotion::transformation`
    /// (`sigPromotion.cpp:640-655`) dispatching to `safeSigRDTbl` /
    /// `safeSigWRTbl`.
    fn transformation(
        &mut self,
        arena: &mut TreeArena,
        sig: SigId,
    ) -> Result<SigId, NormalFormError> {
        match match_sig(arena, sig) {
            SigMatch::RdTbl(tbl, ri) => {
                let Some(size) = table_size(arena, tbl)? else {
                    return Ok(sig);
                };
                check_size("RDTbl", size)?;
                match self.checked_index(arena, ri, size) {
                    None => Ok(sig),
                    Some((clamped, lo, hi)) => {
                        let clamped_sig = SigBuilder::new(arena).rdtbl(tbl, clamped);
                        self.warnings.push(TableRangeWarning {
                            lo,
                            hi,
                            size,
                            sig: clamped_sig,
                            write: false,
                        });
                        Ok(clamped_sig)
                    }
                }
            }
            SigMatch::WrTbl(size_sig, generator, widx, wsig) => {
                if arena.is_nil(widx) {
                    // Read-only generated table: no runtime index to check.
                    return Ok(sig);
                }
                let Some(size) = int_const(arena, size_sig) else {
                    return Err(NormalFormError::TableAccess(format!(
                        "ERROR : WRTbl size {} should be a constant integer\n",
                        signals::dump_sig(arena, size_sig)
                    )));
                };
                check_size("WRTbl", size)?;
                match self.checked_index(arena, widx, size) {
                    None => Ok(sig),
                    Some((clamped, lo, hi)) => {
                        let clamped_sig =
                            SigBuilder::new(arena).wrtbl(size_sig, generator, clamped, wsig);
                        self.warnings.push(TableRangeWarning {
                            lo,
                            hi,
                            size,
                            sig: clamped_sig,
                            write: true,
                        });
                        Ok(clamped_sig)
                    }
                }
            }
            _ => Ok(sig),
        }
    }

    /// Decides whether `index` needs clamping against `[0, size-1]`.
    ///
    /// Returns `None` when the interval proves the access in-bounds, and
    /// `Some((max(0, min(index, size-1)), lo, hi))` otherwise, where
    /// `[lo, hi]` is the interval the decision was based on
    /// (`[INT32_MIN, INT32_MAX]` when `index` has no recorded type, matching
    /// C++ `safeSigRDTbl`'s untyped fallback).
    fn checked_index(
        &self,
        arena: &mut TreeArena,
        index: SigId,
        size: i32,
    ) -> Option<(SigId, f64, f64)> {
        let (lo, hi) = match self.types.get(&index) {
            Some(ty) => {
                let iv = ty.interval();
                (iv.lo(), iv.hi())
            }
            None => (f64::from(i32::MIN), f64::from(i32::MAX)),
        };
        let in_bounds = lo.is_finite() && hi.is_finite() && lo >= 0.0 && hi <= f64::from(size - 1);
        if in_bounds {
            return None;
        }
        let mut b = SigBuilder::new(arena);
        let zero = b.int(0);
        let upper = b.int(size - 1);
        // Same operand order as C++: sigMax(sigInt(0), sigMin(ri, sigInt(size-1))).
        let inner = b.min(index, upper);
        let clamped = b.max(zero, inner);
        Some((clamped, lo, hi))
    }
}

/// Extracts the compile-time size of a table-producing signal.
///
/// Handled forms are the two the FIR lowerer accepts (`resolve_table`):
/// `SIGWRTBL(size, ...)` with a constant size, and `SIGWAVEFORM` (size =
/// number of samples). Returns `Ok(None)` for any other producer, in which
/// case the read is left untouched — the FIR lowerer will reject unsupported
/// producers with its own diagnostic.
///
/// # Errors
///
/// A `SIGWRTBL` whose size did not fold to an integer constant is an error:
/// the pass runs after simplification, so this only happens on a pipeline
/// bug (C++ fails the same way through `tree2int`).
fn table_size(arena: &TreeArena, tbl: SigId) -> Result<Option<i32>, NormalFormError> {
    match match_sig(arena, tbl) {
        SigMatch::WrTbl(size_sig, ..) => match int_const(arena, size_sig) {
            Some(size) => Ok(Some(size)),
            None => Err(NormalFormError::TableAccess(format!(
                "ERROR : RDTbl size {} should be a constant integer\n",
                signals::dump_sig(arena, size_sig)
            ))),
        },
        SigMatch::Waveform(values) => Ok(Some(i32::try_from(values.len()).unwrap_or(i32::MAX))),
        _ => Ok(None),
    }
}

/// Rejects non-positive table sizes with the C++ message shape
/// (`ERROR : RDTbl size = N should be > 0`).
fn check_size(node: &str, size: i32) -> Result<(), NormalFormError> {
    if size <= 0 {
        return Err(NormalFormError::TableAccess(format!(
            "ERROR : {node} size = {size} should be > 0\n"
        )));
    }
    Ok(())
}

/// Returns `Some(n)` when `sig` is the integer constant `n`.
fn int_const(arena: &TreeArena, sig: SigId) -> Option<i32> {
    match match_sig(arena, sig) {
        SigMatch::Int(n) => Some(n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interval::Interval;
    use sigtype::{Boolean, Computability, Nature, Variability, Vectorability, make_simple};

    /// Builds the `SigType` of an integer signal with interval `[lo, hi]`.
    fn int_type(lo: f64, hi: f64) -> SigType {
        make_simple(
            Nature::Int,
            Variability::Samp,
            Computability::Exec,
            Vectorability::Vect,
            Boolean::Num,
            Interval::new(lo, hi, 0),
        )
    }

    /// Builds a read-only generated table of `size` samples and returns
    /// `(table, read_index, rdtbl_access)` where the index is a free input.
    fn rdtbl_fixture(arena: &mut TreeArena, size: i32) -> (SigId, SigId, SigId) {
        let mut b = SigBuilder::new(arena);
        let size_sig = b.int(size);
        let content = b.real(1.0);
        let generator = b.generate(content);
        let tbl = b.wrtbl_readonly(size_sig, generator);
        let ri = b.input(0);
        let access = b.rdtbl(tbl, ri);
        (tbl, ri, access)
    }

    fn run(
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
        sig: SigId,
    ) -> (SigId, Vec<TableRangeWarning>) {
        let mut warnings = Vec::new();
        let out = promote_table_signals(arena, types, &[sig], &mut warnings)
            .expect("promotion should succeed");
        (out[0], warnings)
    }

    /// Asserts that `sig` is `rdtbl(_, max(0, min(orig_ri, size-1)))`.
    fn assert_clamped_read(arena: &TreeArena, sig: SigId, orig_ri: SigId, size: i32) {
        let SigMatch::RdTbl(_, idx) = match_sig(arena, sig) else {
            panic!("expected RdTbl, got {:?}", match_sig(arena, sig));
        };
        let SigMatch::Max(zero, inner) = match_sig(arena, idx) else {
            panic!("expected Max clamp, got {:?}", match_sig(arena, idx));
        };
        assert!(matches!(match_sig(arena, zero), SigMatch::Int(0)));
        let SigMatch::Min(ri, upper) = match_sig(arena, inner) else {
            panic!("expected Min clamp, got {:?}", match_sig(arena, inner));
        };
        assert_eq!(ri, orig_ri, "clamp must wrap the original index");
        match match_sig(arena, upper) {
            SigMatch::Int(n) => assert_eq!(n, size - 1),
            other => panic!("expected Int upper bound, got {other:?}"),
        }
    }

    #[test]
    fn in_bounds_interval_is_identity() {
        let mut arena = TreeArena::default();
        let (_, ri, access) = rdtbl_fixture(&mut arena, 16);
        let mut types = HashMap::new();
        types.insert(ri, int_type(0.0, 15.0));

        let (out, warnings) = run(&mut arena, &types, access);
        assert_eq!(out, access, "provably safe access must be untouched");
        assert!(warnings.is_empty());
    }

    #[test]
    fn upper_overflow_gets_full_clamp() {
        let mut arena = TreeArena::default();
        let (_, ri, access) = rdtbl_fixture(&mut arena, 16);
        let mut types = HashMap::new();
        // Non-negative but may exceed 15 — C++ still inserts the full pair.
        types.insert(ri, int_type(0.0, 100.0));

        let (out, warnings) = run(&mut arena, &types, access);
        assert_ne!(out, access);
        assert_clamped_read(&arena, out, ri, 16);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].size, 16);
        assert!(!warnings[0].write);
        assert_eq!(
            warnings[0].to_string(),
            "RDTbl read index [0:100] is outside of table size (16)"
        );
    }

    #[test]
    fn signed_interval_gets_full_clamp() {
        let mut arena = TreeArena::default();
        let (_, ri, access) = rdtbl_fixture(&mut arena, 16);
        let mut types = HashMap::new();
        types.insert(ri, int_type(-4.0, 4.0));

        let (out, warnings) = run(&mut arena, &types, access);
        assert_clamped_read(&arena, out, ri, 16);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn untyped_index_is_clamped_with_int32_bounds() {
        let mut arena = TreeArena::default();
        let (_, ri, access) = rdtbl_fixture(&mut arena, 16);
        let types = HashMap::new();

        let (out, warnings) = run(&mut arena, &types, access);
        assert_clamped_read(&arena, out, ri, 16);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].lo, f64::from(i32::MIN));
        assert_eq!(warnings[0].hi, f64::from(i32::MAX));
    }

    #[test]
    fn infinite_interval_is_clamped() {
        let mut arena = TreeArena::default();
        let (_, ri, access) = rdtbl_fixture(&mut arena, 16);
        let mut types = HashMap::new();
        types.insert(ri, int_type(0.0, f64::INFINITY));

        let (out, warnings) = run(&mut arena, &types, access);
        assert_clamped_read(&arena, out, ri, 16);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn write_index_is_clamped() {
        let mut arena = TreeArena::default();
        let mut b = SigBuilder::new(&mut arena);
        let size_sig = b.int(8);
        let content = b.real(0.0);
        let generator = b.generate(content);
        let widx = b.input(0);
        let wsig = b.input(1);
        let tbl = b.wrtbl(size_sig, generator, widx, wsig);
        let types = HashMap::new();

        let (out, warnings) = run(&mut arena, &types, tbl);
        let SigMatch::WrTbl(_, _, idx, ws) = match_sig(&arena, out) else {
            panic!("expected WrTbl");
        };
        assert_eq!(ws, wsig);
        assert!(matches!(match_sig(&arena, idx), SigMatch::Max(..)));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].write);
        assert_eq!(warnings[0].size, 8);
    }

    #[test]
    fn readonly_wrtbl_is_identity() {
        let mut arena = TreeArena::default();
        let (tbl, _, _) = rdtbl_fixture(&mut arena, 16);
        let types = HashMap::new();

        let (out, warnings) = run(&mut arena, &types, tbl);
        assert_eq!(out, tbl);
        assert!(warnings.is_empty());
    }

    #[test]
    fn nested_write_and_read_are_both_clamped() {
        let mut arena = TreeArena::default();
        let mut b = SigBuilder::new(&mut arena);
        let size_sig = b.int(8);
        let content = b.real(0.0);
        let generator = b.generate(content);
        let widx = b.input(0);
        let wsig = b.input(1);
        let tbl = b.wrtbl(size_sig, generator, widx, wsig);
        let ri = b.input(2);
        let access = b.rdtbl(tbl, ri);
        let types = HashMap::new();

        let (out, warnings) = run(&mut arena, &types, access);
        assert_eq!(warnings.len(), 2, "write index and read index");
        assert!(warnings.iter().any(|w| w.write));
        assert!(warnings.iter().any(|w| !w.write));
        // The read clamp survives on top of the rewritten table.
        assert_clamped_read(&arena, out, ri, 8);
    }

    #[test]
    fn non_positive_size_is_an_error() {
        let mut arena = TreeArena::default();
        let (_, _, access) = rdtbl_fixture(&mut arena, 0);
        let types = HashMap::new();
        let mut warnings = Vec::new();

        let err = promote_table_signals(&mut arena, &types, &[access], &mut warnings)
            .expect_err("size 0 must be rejected");
        assert!(
            err.to_string().contains("RDTbl size = 0 should be > 0"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn non_constant_size_is_an_error() {
        let mut arena = TreeArena::default();
        let mut b = SigBuilder::new(&mut arena);
        let size_sig = b.input(0);
        let content = b.real(1.0);
        let generator = b.generate(content);
        let tbl = b.wrtbl_readonly(size_sig, generator);
        let ri = b.input(1);
        let access = b.rdtbl(tbl, ri);
        let types = HashMap::new();
        let mut warnings = Vec::new();

        let err = promote_table_signals(&mut arena, &types, &[access], &mut warnings)
            .expect_err("non-constant size must be rejected");
        assert!(
            err.to_string().contains("should be a constant integer"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn shared_access_is_rewritten_once() {
        let mut arena = TreeArena::default();
        let (_, _, access) = rdtbl_fixture(&mut arena, 4);
        let sum = SigBuilder::new(&mut arena).add(access, access);
        let types = HashMap::new();

        let (out, warnings) = run(&mut arena, &types, sum);
        // The access is shared: one rewrite, one warning.
        assert_eq!(warnings.len(), 1);
        let SigMatch::BinOp(signals::BinOp::Add, l, r) = match_sig(&arena, out) else {
            panic!("expected Add");
        };
        assert_eq!(l, r, "hash-consing must keep the rewritten access shared");
    }
}
