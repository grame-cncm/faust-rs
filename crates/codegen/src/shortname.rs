//! Short, unique identifiers derived from UI widget paths.
//!
//! A widget's full path (`/dsp/group/label`) is unambiguous but unusable as an
//! identifier. Several consumers need a short one instead:
//!
//! - the JSON description exposes it as the `shortname` field;
//! - the codebox backend uses it as the `@param` name and as the `update()`
//!   argument name, because codebox parameters *are* identifiers rather than
//!   pointers to zones.
//!
//! The rule is "shortest suffix of the path that is still unique". Two widgets
//! both labelled `gain` in groups `a` and `b` become `a_gain` and `b_gain`,
//! while a `gain` that collides with nothing stays `gain`.
//!
//! # Source provenance (C++)
//! `PathBuilder::computeShortNames` in `architecture/faust/gui/PathBuilder.h`,
//! driven by `ShortnameInstVisitor` in `compiler/generator/json_instructions.hh`.
//! Mapping status: `preserved` — the collision-resolution result must match the
//! C++ compiler's, since the JSON `shortname` field and codebox parameter names
//! are both externally observable.

use std::collections::{BTreeMap, BTreeSet};

/// Computes the short name of every path, keyed by the path itself.
///
/// Keyed by address, so two identical addresses collapse to one entry. That is
/// sound because Faust rejects duplicate addresses before codegen; it is not a
/// case this function has to disambiguate.
///
/// `paths` are widget **addresses** (`/dsp/group/label`), not raw labels: by the
/// time they reach here Faust has already trimmed the labels and replaced
/// space, `#`, `*`, `,`, `?`, brackets and parens with `_` (C++
/// `PathBuilder::buildPath`). So `hslider("my  gain!")` arrives as
/// `/dsp/my__gain!` and comes back as `my_gain`.
///
/// Addresses must be given in UI declaration order: the disambiguation prefix is
/// derived from each path's position, so reordering the input can change which
/// of two colliding names grows first (never the final set of names).
///
/// # Algorithm
///
/// 1. Each path is turned into a *unique* path `"/P<index>" + str2id(path)`.
///    The `P<index>` prefix guarantees uniqueness even for identical paths, so
///    the loop below always terminates.
/// 2. Every unique path starts at level 1, meaning "keep the last slash-part".
/// 3. While two paths cut to the same name, both get their level raised — so
///    they keep one more slash-part — until no two agree.
/// 4. The surviving cut has its remaining `/` turned into `_`.
#[must_use]
pub fn compute_short_names(paths: &[String]) -> BTreeMap<String, String> {
    let unique: Vec<String> = paths
        .iter()
        .enumerate()
        .map(|(index, path)| format!("/P{index}{}", str2id(&remove_0x00(path))))
        .collect();

    // BTreeMap so the iteration order below is deterministic. It does not affect
    // the result — both sides of a collision are always raised — but it keeps a
    // failure reproducible.
    let mut level: BTreeMap<&str, usize> = unique.iter().map(|u| (u.as_str(), 1)).collect();

    loop {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        // A set, not a list: with three or more colliding paths the middle one
        // is reported twice, and raising it twice skips a level.
        let mut colliding: BTreeSet<&str> = BTreeSet::new();
        for (unique_path, &n) in &level {
            let short = cut(unique_path, n);
            if let Some(previous) = seen.insert(short, unique_path) {
                colliding.insert(unique_path);
                colliding.insert(previous);
            }
        }
        if colliding.is_empty() {
            break;
        }
        for unique_path in colliding {
            *level.get_mut(unique_path).expect("level entry exists") += 1;
        }
    }

    paths
        .iter()
        .zip(unique.iter())
        .map(|(path, unique_path)| {
            let n = level[unique_path.as_str()];
            (path.clone(), cut(unique_path, n).replace('/', "_"))
        })
        .collect()
}

/// Whether a character may appear in an identifier as-is.
fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// Removes every `/0x00` segment, which the label encoder inserts for unnamed
/// groups.
fn remove_0x00(src: &str) -> String {
    src.replace("/0x00", "")
}

/// Replaces every run of non-identifier characters with a single `_`.
///
/// The underscore is emitted *before the next kept character*, so a run at the
/// end of the string produces nothing — `"gain "` and `"gain"` both give
/// `"gain"`. `/` survives, because the path structure is still needed for
/// [`cut`].
fn str2id(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut need_underscore = false;
    for c in src.chars() {
        if is_id_char(c) || c == '/' {
            if need_underscore {
                out.push('_');
                need_underscore = false;
            }
            out.push(c);
        } else {
            need_underscore = true;
        }
    }
    out
}

/// Keeps the last `n` slash-separated parts of `src`.
///
/// `n == 1` is the last part alone. When `src` has fewer than `n` parts it is
/// returned whole.
fn cut(src: &str, n: usize) -> String {
    let mut remaining = n;
    let mut tail = String::new();
    for c in src.chars().rev() {
        if c != '/' {
            tail.push(c);
        } else if remaining == 1 {
            return tail.chars().rev().collect();
        } else {
            remaining -= 1;
            tail.push(c);
        }
    }
    src.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(paths: &[&str]) -> Vec<String> {
        let owned: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
        let map = compute_short_names(&owned);
        owned.iter().map(|p| map[p].clone()).collect()
    }

    /// The measured C++ result for
    /// `vgroup("a", hslider("gain",…)) + vgroup("b", hslider("gain",…)) + hslider("0freq",…)`.
    #[test]
    fn colliding_labels_grow_by_one_group_each() {
        assert_eq!(
            short(&["/cbs2/a/gain", "/cbs2/b/gain", "/cbs2/0freq"]),
            ["a_gain", "b_gain", "0freq"]
        );
    }

    #[test]
    fn a_lone_label_keeps_its_last_part() {
        assert_eq!(short(&["/dsp/group/gain"]), ["gain"]);
    }

    /// Three-way collisions must raise every participant, not just the last two.
    #[test]
    fn three_way_collision_raises_all_three() {
        assert_eq!(
            short(&["/d/a/g", "/d/b/g", "/d/c/g"]),
            ["a_g", "b_g", "c_g"]
        );
    }

    /// When the group names collide too, the level keeps rising.
    #[test]
    fn nested_collisions_keep_growing() {
        assert_eq!(short(&["/d/x/a/g", "/d/y/a/g"]), ["x_a_g", "y_a_g"]);
    }

    /// Duplicate addresses must not hang the collision loop.
    ///
    /// They collapse to one entry, because the result is keyed by address — and
    /// that is acceptable precisely because Faust rejects duplicate addresses
    /// earlier (`ERROR : path '/dsp/g' is already used`), so a valid program
    /// cannot reach here with two identical ones. What matters is termination:
    /// the `P<index>` prefix is what makes the two unique paths differ, so the
    /// loop settles instead of raising levels forever.
    #[test]
    fn duplicate_addresses_terminate_instead_of_looping() {
        let map = compute_short_names(&["/d/g".to_owned(), "/d/g".to_owned()]);
        assert_eq!(map.len(), 1, "keyed by address, so duplicates collapse");
        // Level rose past the shared parts and reached the P-prefix.
        assert_eq!(map["/d/g"], "P1_d_g");
    }

    #[test]
    fn non_id_characters_collapse_to_one_underscore() {
        assert_eq!(str2id("my__gain!"), "my_gain");
        assert_eq!(str2id("gain "), "gain", "a trailing run emits nothing");
        assert_eq!(str2id("a/b c"), "a/b_c", "slashes survive");
        // A leading run DOES emit an underscore, because the `_` is pushed
        // before the next kept character. It never shows up in practice: Faust
        // trims labels before building the address, so `hslider(" gain")`
        // arrives as `/dsp/gain`. Asserted so the function's real behaviour is
        // on record rather than assumed away.
        assert_eq!(str2id(" gain"), "_gain");
    }

    /// End-to-end against the measured C++ output for
    /// `hslider(" gain") + hslider("my  gain!")`, whose addresses are
    /// `/lead/gain` and `/lead/my__gain!`.
    #[test]
    fn address_normalisation_matches_the_reference() {
        assert_eq!(
            short(&["/lead/gain", "/lead/my__gain!"]),
            ["gain", "my_gain"]
        );
    }

    #[test]
    fn cut_keeps_the_requested_number_of_parts() {
        assert_eq!(cut("/a/b/c", 1), "c");
        assert_eq!(cut("/a/b/c", 2), "b/c");
        assert_eq!(cut("/a/b/c", 3), "a/b/c");
        assert_eq!(
            cut("/a/b/c", 9),
            "/a/b/c",
            "asking for more returns the whole path"
        );
    }

    #[test]
    fn unnamed_group_segments_are_removed() {
        assert_eq!(remove_0x00("/dsp/0x00/gain"), "/dsp/gain");
    }
}
