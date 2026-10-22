//! Box-complexity scoring for SVG fold decisions.
//!
//! C++ reference: `compiler/boxes/boxcomplexity.cpp` — `boxComplexity` /
//! `computeBoxComplexity`.

use std::collections::HashMap;

use tlib::TreeArena;

use crate::{BoxId, BoxMatch, match_box};

/// Where a complexity saturates. The complexity counts the boxes of the
/// *expanded* tree, so a shared DAG reaches it quickly (`ma.chebychev(300)`);
/// a saturated value still reads as "very complex" to the folding tests.
///
/// C++ reference: `boxcomplexity.cpp`, `MAX_BOX_COMPLEXITY` (#1290).
pub const MAX_BOX_COMPLEXITY: usize = 1_000_000_000;

/// `a + b`, saturating at [`MAX_BOX_COMPLEXITY`] (C++ `addBoxComplexity`).
fn add(a: usize, b: usize) -> usize {
    a.saturating_add(b).min(MAX_BOX_COMPLEXITY)
}

/// The complexity of every box met so far, for a caller that asks about many
/// boxes of one DAG (the drawing asks about every sub-diagram).
///
/// C++ keeps the value as a property of the tree (`BCOMPLEXITY`); a memo per
/// caller does the same without writing to the arena.
#[derive(Debug, Default)]
pub struct BoxComplexity {
    memo: HashMap<BoxId, usize>,
}

impl BoxComplexity {
    /// An empty memo.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The complexity of `b`: each distinct box is scored once.
    pub fn of(&mut self, arena: &TreeArena, b: BoxId) -> usize {
        if let Some(&c) = self.memo.get(&b) {
            return c;
        }
        let c = node_complexity(arena, b, &mut |child| self.of(arena, child));
        self.memo.insert(b, c);
        c
    }
}

/// Recursive complexity score of a box expression.
///
/// Used by the SVG draw module to decide whether to fold a sub-diagram into a
/// separate file (`complexity > fold_threshold`) or decorate it in-place.
///
/// Scoring rules (matching C++ `computeBoxComplexity`):
/// - Leaves (all primitives, UI widgets, tables, foreign items) → **1**
/// - Pass-throughs (wire, cut, route, environment) → **0**
/// - Compositions (seq, par, split, merge, rec) → **sum of children**
/// - Groups (vgroup, hgroup, tgroup) → **complexity of body** (transparent)
/// - Symbolic / ondemand / up/downsampling → **1 + child**
/// - Metadata → **complexity of payload** (transparent)
///
/// Sums saturate at [`MAX_BOX_COMPLEXITY`]. Each distinct box of the DAG is
/// scored once; [`BoxComplexity`] keeps the scores across calls.
///
/// C++ reference: `boxcomplexity.cpp:77` — `computeBoxComplexity`.
pub fn box_complexity(arena: &TreeArena, b: BoxId) -> usize {
    BoxComplexity::new().of(arena, b)
}

/// The score of `b` alone, `child` scoring its children.
fn node_complexity(arena: &TreeArena, b: BoxId, child: &mut dyn FnMut(BoxId) -> usize) -> usize {
    match match_box(arena, b) {
        // ── Zero-complexity structural nodes ─────────────────────────
        BoxMatch::Cut | BoxMatch::Wire | BoxMatch::Route(..) | BoxMatch::Environment => 0,
        BoxMatch::Slot(_) => 1,

        // ── Leaf nodes: complexity 1 ──────────────────────────────────
        BoxMatch::Int(_) | BoxMatch::Real(_) | BoxMatch::Waveform(_) => 1,

        // binary primitives
        BoxMatch::Add
        | BoxMatch::Sub
        | BoxMatch::Mul
        | BoxMatch::Div
        | BoxMatch::Rem
        | BoxMatch::And
        | BoxMatch::Or
        | BoxMatch::Xor
        | BoxMatch::Lsh
        | BoxMatch::Rsh
        | BoxMatch::LRsh
        | BoxMatch::Lt
        | BoxMatch::Le
        | BoxMatch::Gt
        | BoxMatch::Ge
        | BoxMatch::Eq
        | BoxMatch::Ne
        | BoxMatch::Pow
        | BoxMatch::Atan2
        | BoxMatch::Fmod
        | BoxMatch::Remainder
        | BoxMatch::Min
        | BoxMatch::Max
        | BoxMatch::Delay => 1,

        // unary math
        BoxMatch::Acos
        | BoxMatch::Asin
        | BoxMatch::Atan
        | BoxMatch::Cos
        | BoxMatch::Sin
        | BoxMatch::Tan
        | BoxMatch::Exp
        | BoxMatch::Exp10
        | BoxMatch::Log
        | BoxMatch::Log10
        | BoxMatch::Sqrt
        | BoxMatch::Abs
        | BoxMatch::Floor
        | BoxMatch::Ceil
        | BoxMatch::Rint
        | BoxMatch::Round
        | BoxMatch::IntCast
        | BoxMatch::FloatCast
        | BoxMatch::Delay1
        | BoxMatch::Prefix => 1,

        // selects + tables
        BoxMatch::Select2 | BoxMatch::Select3 => 1,
        BoxMatch::ReadOnlyTable | BoxMatch::WriteReadTable => 1,

        // misc primitives
        BoxMatch::AssertBounds
        | BoxMatch::Lowest
        | BoxMatch::Highest
        | BoxMatch::Attach
        | BoxMatch::Enable
        | BoxMatch::Control => 1,

        // UI widgets
        BoxMatch::Button(_)
        | BoxMatch::Checkbox(_)
        | BoxMatch::VSlider(..)
        | BoxMatch::HSlider(..)
        | BoxMatch::NumEntry(..)
        | BoxMatch::VBargraph(..)
        | BoxMatch::HBargraph(..)
        | BoxMatch::Soundfile(..) => 1,

        // foreign
        BoxMatch::FFun(_) | BoxMatch::Ffunction(..) | BoxMatch::FConst(..) | BoxMatch::FVar(..) => {
            1
        }

        // named ident (unresolved reference)
        BoxMatch::Ident(_) => 1,

        // ── Composition: sum of children ─────────────────────────────
        BoxMatch::Seq(a, b)
        | BoxMatch::Par(a, b)
        | BoxMatch::Split(a, b)
        | BoxMatch::Merge(a, b)
        | BoxMatch::Rec(a, b) => {
            let ca = child(a);
            add(ca, child(b))
        }

        // ── Groups: transparent (complexity of body) ──────────────────
        BoxMatch::VGroup(_, body) | BoxMatch::HGroup(_, body) | BoxMatch::TGroup(_, body) => {
            child(body)
        }

        // ── Wrappers: 1 + child ───────────────────────────────────────
        BoxMatch::Symbolic(_, body) => add(1, child(body)),
        BoxMatch::Ondemand(inner) | BoxMatch::Upsampling(inner) | BoxMatch::Downsampling(inner) => {
            add(1, child(inner))
        }

        // ── Metadata: transparent ─────────────────────────────────────
        BoxMatch::Metadata(a, _) => child(a),

        // ── Unknown / anything else: treat as 1 ──────────────────────
        _ => 1,
    }
}
