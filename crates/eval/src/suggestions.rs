//! Conservative near-name suggestions for unresolved identifiers.
//!
//! # Role in pipeline
//! - Rank candidate names for [`crate::EvalError::UndefinedSymbol`] and
//!   [`crate::EvalError::MissingProcessDefinition`].
//! - Feed typed diagnostic facts, never free-form guessing.
//!
//! # Design invariants
//! - Candidates come only from names the evaluator actually recorded as
//!   visible; this module never invents or widens a scope.
//! - Ranking is deterministic: `(distance, name)` ordering with no reliance on
//!   hash iteration order.
//! - The distance budget is length-relative and capped, so a short identifier
//!   cannot match an unrelated long one.
//! - [`unambiguous_suggestion`] is intentionally strict: a rename edit is only
//!   proposed when exactly one candidate wins by a strict margin.

/// Maximum number of ranked suggestions exposed to a consumer.
const MAX_SUGGESTIONS: usize = 3;

/// One ranked near-name candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolSuggestion {
    /// Candidate identifier taken verbatim from a recorded scope.
    pub name: String,
    /// Damerau-Levenshtein distance to the unresolved identifier.
    pub distance: usize,
}

/// Ranks visible candidates by similarity to `target`.
///
/// `candidates` is consumed in iteration order and deduplicated, so callers may
/// pass overlapping scope lists (local, visible, top level) without
/// pre-merging. The result is ordered by increasing distance, then by name, and
/// truncated to at most three entries.
///
/// Exact matches are excluded: if the identifier were already present under the
/// same spelling, resolution would not have failed.
#[must_use]
pub fn rank_similar_names<'a>(
    target: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<SymbolSuggestion> {
    let budget = distance_budget(target);
    if budget == 0 {
        return Vec::new();
    }

    let mut ranked: Vec<SymbolSuggestion> = Vec::new();
    for candidate in candidates {
        if candidate == target || ranked.iter().any(|entry| entry.name == candidate) {
            continue;
        }
        let Some(distance) = bounded_edit_distance(target, candidate, budget) else {
            continue;
        };
        ranked.push(SymbolSuggestion {
            name: candidate.to_owned(),
            distance,
        });
    }

    ranked.sort_by(|left, right| {
        left.distance
            .cmp(&right.distance)
            .then_with(|| left.name.cmp(&right.name))
    });
    ranked.truncate(MAX_SUGGESTIONS);
    ranked
}

/// Returns the single candidate a rename edit may safely propose.
///
/// A suggestion is unambiguous only when the best candidate is strictly closer
/// than every other candidate. Two equally close names mean the compiler cannot
/// know which one the programmer meant, so no edit is offered.
#[must_use]
pub fn unambiguous_suggestion(ranked: &[SymbolSuggestion]) -> Option<&SymbolSuggestion> {
    let best = ranked.first()?;
    match ranked.get(1) {
        Some(runner_up) if runner_up.distance == best.distance => None,
        _ => Some(best),
    }
}

/// Length-relative edit budget.
///
/// One-character identifiers get no budget at all: every other one-character
/// name would be within distance one, which is noise rather than guidance.
fn distance_budget(target: &str) -> usize {
    match target.chars().count() {
        0 | 1 => 0,
        2..=4 => 1,
        _ => 2,
    }
}

/// Damerau-Levenshtein distance, returning `None` past `budget`.
///
/// Operates on Unicode scalar values so non-ASCII identifiers rank the same way
/// ASCII ones do. The length pre-check makes the common "no candidate is close"
/// case cheap.
fn bounded_edit_distance(left: &str, right: &str, budget: usize) -> Option<usize> {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > budget {
        return None;
    }

    // Three rolling rows are enough for the transposition rule, which only ever
    // looks two rows back.
    let width = right.len() + 1;
    let mut two_back = vec![0usize; width];
    let mut one_back: Vec<usize> = (0..width).collect();
    let mut current = vec![0usize; width];

    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        let mut row_best = current[0];
        for (j, right_char) in right.iter().enumerate() {
            let substitution_cost = usize::from(left_char != right_char);
            let mut best = (current[j] + 1)
                .min(one_back[j + 1] + 1)
                .min(one_back[j] + substitution_cost);
            if i > 0
                && j > 0
                && *left_char == right[j - 1]
                && left[i - 1] == *right_char
                && let Some(transposed) = two_back.get(j - 1)
            {
                best = best.min(transposed + 1);
            }
            current[j + 1] = best;
            row_best = row_best.min(best);
        }
        if row_best > budget {
            return None;
        }
        std::mem::swap(&mut two_back, &mut one_back);
        std::mem::swap(&mut one_back, &mut current);
    }

    let distance = one_back[right.len()];
    (distance <= budget).then_some(distance)
}

#[cfg(test)]
mod tests {
    use super::{rank_similar_names, unambiguous_suggestion};

    #[test]
    fn ranks_closest_candidate_first() {
        let ranked = rank_similar_names("filtr", ["filter", "flanger", "reverb"]);
        assert_eq!(ranked[0].name, "filter");
        assert_eq!(ranked[0].distance, 1);
    }

    #[test]
    fn excludes_candidates_beyond_the_length_relative_budget() {
        // "gain" gets a budget of one, so a two-edit name must not appear.
        let ranked = rank_similar_names("gain", ["grain", "gate", "oscillator"]);
        let names: Vec<&str> = ranked.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["grain"]);
    }

    #[test]
    fn one_character_identifiers_get_no_suggestion() {
        assert!(rank_similar_names("x", ["y", "z"]).is_empty());
    }

    #[test]
    fn transposition_counts_as_one_edit() {
        let ranked = rank_similar_names("fitler", ["filter"]);
        assert_eq!(ranked[0].distance, 1);
    }

    #[test]
    fn deduplicates_overlapping_scope_lists() {
        let ranked = rank_similar_names("filtr", ["filter", "filter", "filte"]);
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn ordering_is_deterministic_for_equal_distances() {
        let ranked = rank_similar_names("gainx", ["gainb", "gaina"]);
        let names: Vec<&str> = ranked.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["gaina", "gainb"]);
    }

    #[test]
    fn equal_best_distances_are_ambiguous() {
        let ranked = rank_similar_names("gainx", ["gainb", "gaina"]);
        assert!(unambiguous_suggestion(&ranked).is_none());
    }

    #[test]
    fn a_strictly_closer_candidate_is_unambiguous() {
        let ranked = rank_similar_names("filtr", ["filter", "fltrs"]);
        assert_eq!(
            unambiguous_suggestion(&ranked).map(|s| s.name.as_str()),
            Some("filter")
        );
    }

    #[test]
    fn an_exact_match_is_never_suggested() {
        assert!(rank_similar_names("filter", ["filter"]).is_empty());
    }

    #[test]
    fn truncates_to_three_suggestions() {
        let ranked =
            rank_similar_names("filter", ["filten", "filtel", "filtem", "filtek", "filtej"]);
        assert_eq!(ranked.len(), 3);
    }
}
