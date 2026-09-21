//! A seed of `fad` or `rad` computed from another seed of the same call.
//!
//! A seed is differentiated as an independent variable: the forward transform
//! returns at a seed without visiting its operands, the reverse sweeps stop
//! at it, and whatever computes it is a constant for every lane. So
//! `fad(x + y, (x + y, x, y))` gives `1, 0, 0`: the lanes of `x` and `y` do
//! not pass through the seed `x + y`. In every program seen that shape was a
//! mistake whose symptom was a silent zero, and the analysis in
//! `porting/fad-rad-seed-semantics-analysis-2026-09-21-en.md` settled on
//! refusing it with the two spellings that mean something.
//!
//! The walk over a seed's computation is structural, stays in the seed's
//! own scope and at the current sample: it does not enter a `DEBRUIJNREC`
//! body, cross a `DEBRUIJNREF`, a delay or a clock-domain wrapper. So a seed
//! that reads another seed only through a recursion (the parameters of
//! `optimizers.lib`'s descents, each a clamp of a projection of the
//! optimizer's own loop) is not reported, and neither is `y'` seeded next to
//! `y` (`ddsp_fad_diode_clipper_newton.dsp`: the two variables of the
//! implicit equation). A seed found inside another seed's computation is
//! recorded and not entered: what it is computed from is its own check.
//! Duplicated seeds (`(s, s)`) are legal.

use ahash::{AHashMap, AHashSet};
use signals::{SigId, SigMatch, match_sig};
use tlib::{TreeArena, TreeId, match_de_bruijn_rec, match_de_bruijn_ref};

use crate::PropagateError;
use crate::error::DependentSeed;

/// Refuses a seed list in which a seed is computed from a different seed.
///
/// `seeds` are the lowered seed signals in lane order, `node` the `fad`/`rad`
/// box and `mode` its name for the message. The first dependent seed, in lane
/// order, is reported with every seed it depends on, in lane order.
pub(crate) fn check_dependent_seeds(
    arena: &TreeArena,
    seeds: &[SigId],
    node: TreeId,
    mode: &'static str,
) -> Result<(), PropagateError> {
    if seeds.len() < 2 {
        return Ok(());
    }
    let mut lanes_of: AHashMap<SigId, Vec<usize>> = AHashMap::new();
    for (index, &seed) in seeds.iter().enumerate() {
        lanes_of.entry(seed).or_default().push(index + 1);
    }
    for (index, &seed) in seeds.iter().enumerate() {
        let mut found: Vec<usize> = Vec::new();
        let mut visited: AHashSet<SigId> = AHashSet::new();
        visited.insert(seed);
        // The seed's own node obeys the same boundary as any node below it:
        // what `y'` is computed from is `y` at another sample.
        let mut stack: Vec<SigId> = if is_temporal_boundary(arena, seed) {
            Vec::new()
        } else {
            children_of(arena, seed)
        };
        while let Some(sig) = stack.pop() {
            if !visited.insert(sig) {
                continue;
            }
            if sig != seed
                && let Some(lanes) = lanes_of.get(&sig)
            {
                found.extend(lanes.iter().copied());
                continue;
            }
            if is_temporal_boundary(arena, sig) {
                continue;
            }
            stack.extend(children_of(arena, sig));
        }
        if !found.is_empty() {
            found.sort_unstable();
            found.dedup();
            return Err(PropagateError::AdDependentSeed(Box::new(DependentSeed {
                node,
                mode,
                seed: index + 1,
                seed_text: render(arena, seed, RENDER_DEPTH),
                depends_on: found
                    .into_iter()
                    .map(|lane| (lane, render(arena, seeds[lane - 1], RENDER_DEPTH)))
                    .collect(),
            })));
        }
    }
    Ok(())
}

/// The nodes the walk does not enter: what lies below them is another
/// sample, or another clock, of the signals it holds, and a seed there is
/// not the seed at the current sample. `y` and `y'` are two variables of the
/// implicit equation a Newton solver differentiates, and a program that
/// seeds both means exactly that.
fn is_temporal_boundary(arena: &TreeArena, sig: SigId) -> bool {
    if match_de_bruijn_rec(arena, sig).is_some() || match_de_bruijn_ref(arena, sig).is_some() {
        return true;
    }
    matches!(
        match_sig(arena, sig),
        SigMatch::Delay1(_)
            | SigMatch::Delay(..)
            | SigMatch::Prefix(..)
            | SigMatch::Rec(_)
            | SigMatch::ReverseTimeRec(_)
            | SigMatch::Seq(..)
            | SigMatch::ZeroPad(..)
            | SigMatch::OnDemand(_)
            | SigMatch::Upsampling(_)
            | SigMatch::Downsampling(_)
            | SigMatch::Clocked(..)
    )
}

fn children_of(arena: &TreeArena, sig: SigId) -> Vec<SigId> {
    arena
        .children(sig)
        .map(|children| children.to_vec())
        .unwrap_or_default()
}

/// How deep the rendering of a seed goes before it writes `…`.
const RENDER_DEPTH: usize = 4;

/// A short Faust-like rendering of a signal for the message: `input 0 +
/// input 1`, `hslider #3 * 2`, `sin(…)`. Inputs are numbered because the
/// names of `process`'s arguments are gone after lowering; controls are named
/// by their identifier because the UI program is built after propagation.
fn render(arena: &TreeArena, sig: SigId, depth: usize) -> String {
    if depth == 0 {
        return String::from("…");
    }
    let sub = |s: SigId| render(arena, s, depth - 1);
    let operand = |s: SigId| match match_sig(arena, s) {
        SigMatch::BinOp(..) | SigMatch::Pow(..) => format!("({})", render(arena, s, depth - 1)),
        _ => render(arena, s, depth - 1),
    };
    match match_sig(arena, sig) {
        SigMatch::Int(value) => value.to_string(),
        SigMatch::Real(value) => format!("{value}"),
        SigMatch::Input(index) => format!("input {index}"),
        SigMatch::BinOp(op, left, right) => {
            format!("{} {} {}", operand(left), op.symbol(), operand(right))
        }
        SigMatch::Pow(base, exponent) => format!("{} ^ {}", operand(base), operand(exponent)),
        SigMatch::Min(a, b) => format!("min({}, {})", sub(a), sub(b)),
        SigMatch::Max(a, b) => format!("max({}, {})", sub(a), sub(b)),
        SigMatch::Atan2(a, b) => format!("atan2({}, {})", sub(a), sub(b)),
        SigMatch::Fmod(a, b) => format!("fmod({}, {})", sub(a), sub(b)),
        SigMatch::Remainder(a, b) => format!("remainder({}, {})", sub(a), sub(b)),
        SigMatch::Acos(x) => format!("acos({})", sub(x)),
        SigMatch::Asin(x) => format!("asin({})", sub(x)),
        SigMatch::Atan(x) => format!("atan({})", sub(x)),
        SigMatch::Cos(x) => format!("cos({})", sub(x)),
        SigMatch::Sin(x) => format!("sin({})", sub(x)),
        SigMatch::Tan(x) => format!("tan({})", sub(x)),
        SigMatch::Exp(x) => format!("exp({})", sub(x)),
        SigMatch::Exp10(x) => format!("exp10({})", sub(x)),
        SigMatch::Log(x) => format!("log({})", sub(x)),
        SigMatch::Log10(x) => format!("log10({})", sub(x)),
        SigMatch::Sqrt(x) => format!("sqrt({})", sub(x)),
        SigMatch::Abs(x) => format!("abs({})", sub(x)),
        SigMatch::Floor(x) => format!("floor({})", sub(x)),
        SigMatch::Ceil(x) => format!("ceil({})", sub(x)),
        SigMatch::Rint(x) => format!("rint({})", sub(x)),
        SigMatch::Round(x) => format!("round({})", sub(x)),
        SigMatch::IntCast(x) => format!("int({})", sub(x)),
        SigMatch::FloatCast(x) => format!("float({})", sub(x)),
        SigMatch::Delay1(x) => format!("{}'", operand(x)),
        SigMatch::Delay(x, d) => format!("{} @ {}", operand(x), operand(d)),
        SigMatch::Select2(c, a, b) => format!("select2({}, {}, {})", sub(c), sub(a), sub(b)),
        SigMatch::Proj(index, _) => format!("proj{index}(~)"),
        SigMatch::HSlider(id) => format!("hslider #{id}"),
        SigMatch::VSlider(id) => format!("vslider #{id}"),
        SigMatch::NumEntry(id) => format!("nentry #{id}"),
        SigMatch::Button(id) => format!("button #{id}"),
        SigMatch::Checkbox(id) => format!("checkbox #{id}"),
        SigMatch::Attach(x, _) | SigMatch::Enable(x, _) | SigMatch::Control(x, _) => sub(x),
        SigMatch::VBargraph(_, x) | SigMatch::HBargraph(_, x) => sub(x),
        _ => String::from("…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signals::SigBuilder;

    fn arena_with_inputs() -> (TreeArena, SigId, SigId) {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let y = b.input(1);
        (arena, x, y)
    }

    #[test]
    fn a_seed_computed_from_two_other_seeds_is_reported_with_both() {
        let (mut arena, x, y) = arena_with_inputs();
        let sum = SigBuilder::new(&mut arena).add(x, y);
        let node = sum;
        let err = check_dependent_seeds(&arena, &[sum, x, y], node, "fad")
            .expect_err("x + y is computed from x and y");
        match &err {
            PropagateError::AdDependentSeed(details) => {
                assert_eq!(details.mode, "fad");
                assert_eq!(details.seed, 1);
                assert_eq!(details.seed_text, "input 0 + input 1");
                assert_eq!(
                    details.depends_on,
                    vec![(2, "input 0".to_owned()), (3, "input 1".to_owned())]
                );
            }
            other => panic!("unexpected error {other:?}"),
        }
        assert_eq!(
            err_text(&arena, &[x, y, sum]),
            "rad seed 3 `input 0 + input 1` is computed from seeds 1 `input 0` and 2 `input 1`"
        );
    }

    fn err_text(arena: &TreeArena, seeds: &[SigId]) -> String {
        check_dependent_seeds(arena, seeds, seeds[0], "rad")
            .expect_err("dependent")
            .to_string()
    }

    #[test]
    fn independent_and_duplicated_seeds_pass() {
        let (mut arena, x, y) = arena_with_inputs();
        let sum = SigBuilder::new(&mut arena).add(x, y);
        check_dependent_seeds(&arena, &[x, y], sum, "fad").expect("independent");
        check_dependent_seeds(&arena, &[x, x], sum, "fad").expect("duplicated");
        check_dependent_seeds(&arena, &[sum], sum, "fad").expect("alone");
    }

    #[test]
    fn a_delayed_copy_of_a_seed_is_another_variable() {
        let (mut arena, x, y) = arena_with_inputs();
        let mut b = SigBuilder::new(&mut arena);
        let y1 = b.delay1(y);
        let two = b.int(2);
        let y2 = b.delay(y, two);
        check_dependent_seeds(&arena, &[x, y1, y], y1, "fad").expect("y' next to y");
        check_dependent_seeds(&arena, &[y2, y], y2, "rad").expect("y @ 2 next to y");
    }

    #[test]
    fn a_seed_read_through_a_recursion_is_not_reported() {
        // s = REC([ proj0(ref) * g ]) : a recursion whose body reads the seed
        // g; the walk from s stops at the DEBRUIJNREC and reports nothing.
        let (mut arena, _x, g) = arena_with_inputs();
        let reference = tlib::de_bruijn_ref(&mut arena, 1);
        let body = {
            let mut b = SigBuilder::new(&mut arena);
            let state = b.proj(0, reference);
            b.mul(state, g)
        };
        let body_list = tlib::vec_to_list(&mut arena, &[body]);
        let rec = tlib::de_bruijn_rec(&mut arena, body_list);
        let s = SigBuilder::new(&mut arena).proj(0, rec);
        check_dependent_seeds(&arena, &[s, g], s, "fad").expect("stops at the recursion");
    }
}
