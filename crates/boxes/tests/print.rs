//! Integration tests for the Faust-syntax box printers.
//!
//! Scope:
//! - Real-literal formatting against values captured from the C++ compiler.
//! - Sharing, `ID_` ordering, and parenthesization invariants of
//!   `box_pp_shared`.
//! - Structural checks that hold for every emitted program, independently of
//!   the expected text.
//!
//! The reference values in `format_real_matches_cpp_*` were produced by
//! running `faust -e` on programs containing the literals and reading the
//! expansion, not by reimplementing `%g` a second time.

use std::collections::HashSet;

use boxes::{BoxBuilder, BoxId, BoxPrintError, FloatSize, box_pp, box_pp_shared, format_real};
use tlib::TreeArena;

// ── Real-literal formatting ───────────────────────────────────────────────────

#[test]
fn format_real_matches_cpp_single_precision() {
    for (value, expected) in [
        (0.0, "0.0f"),
        (-0.0, "-0.0f"),
        (1.0, "1.0f"),
        (-1.0, "-1.0f"),
        (0.5, "0.5f"),
        (0.99, "0.99f"),
        (0.1, "0.1f"),
        (0.01, "0.01f"),
        (2.0, "2.0f"),
        (3.0, "3.0f"),
        (16.0, "16.0f"),
        // `%g` switches to exponent form as soon as the decimal exponent
        // reaches the precision it settled on, which is why round numbers get
        // an exponent while 16 does not.
        (10.0, "1e+01f"),
        (20.0, "2e+01f"),
        (-70.0, "-7e+01f"),
        (440.0, "4.4e+02f"),
        (20000.0, "2e+04f"),
        (1e20, "1e+20f"),
        (1e-5, "1e-05f"),
        (0.000123, "0.000123f"),
        (1234567.0, "1234567.0f"),
        (123456789.0, "1.2345679e+08f"),
        (0.1234567890123, "0.12345679f"),
        // Narrowing happens before formatting, so the digits describe the
        // `f32` the program will hold.
        (std::f64::consts::PI, "3.1415927f"),
        (std::f64::consts::TAU, "6.2831855f"),
    ] {
        assert_eq!(
            format_real(value, FloatSize::Single),
            expected,
            "single-precision rendering of {value}"
        );
    }
}

#[test]
fn format_real_matches_cpp_double_precision() {
    for (value, expected) in [
        (1.0, "1.0"),
        (1e-5, "1e-05"),
        (1e20, "1e+20"),
        (0.000123, "0.000123"),
        (1234567.0, "1234567.0"),
        (123456789.0, "123456789.0"),
        (0.1234567890123, "0.1234567890123"),
        (std::f64::consts::PI, "3.141592653589793"),
    ] {
        assert_eq!(
            format_real(value, FloatSize::Double),
            expected,
            "double-precision rendering of {value}"
        );
    }
}

#[test]
fn format_real_round_trips_at_the_target_precision() {
    // The formatter's contract is that its text parses back to the value the
    // program holds. Checking the property directly catches a precision-ladder
    // regression that a fixed expectation table would miss for other values.
    for value in [
        0.1,
        1.0 / 3.0,
        std::f64::consts::E,
        1.234_567_890_123_456_7e-7,
        9.876_543_21e11,
    ] {
        let single = format_real(value, FloatSize::Single);
        let single = single.trim_end_matches('f');
        #[expect(clippy::cast_possible_truncation, reason = "checking the f32 contract")]
        let narrowed = value as f32;
        assert_eq!(
            single.parse::<f32>().expect("single literal must parse"),
            narrowed,
            "single-precision round trip of {value}"
        );

        let double = format_real(value, FloatSize::Double);
        assert_eq!(
            double.parse::<f64>().expect("double literal must parse"),
            value,
            "double-precision round trip of {value}"
        );
    }
}

// ── Sharing behavior ──────────────────────────────────────────────────────────

/// Builds `_ * 2 : (+ ~ *(0.5))` and returns its root.
///
/// The shape mirrors fixture `019_shared_dag`: a sub-diagram reused three
/// times, with a `~` whose operand priority forces parentheses.
fn shared_dag(arena: &mut TreeArena) -> BoxId {
    let mut builder = BoxBuilder::new(arena);
    let wire = builder.wire();
    let two = builder.int(2);
    let pair = builder.par(wire, two);
    let mul = builder.mul();
    let scaled = builder.seq(pair, mul);

    let half = builder.real(0.5);
    let half_pair = builder.par(wire, half);
    let half_mul = builder.mul();
    let attenuate = builder.seq(half_pair, half_mul);
    let add = builder.add();
    let feedback = builder.rec(add, attenuate);

    builder.seq(scaled, feedback)
}

#[test]
fn shared_subdiagram_is_defined_once() {
    let mut arena = TreeArena::new();
    let shared = shared_dag(&mut arena);
    let mut builder = BoxBuilder::new(&mut arena);
    let pair = builder.par(shared, shared);
    let root = builder.par(shared, pair);

    let program = box_pp_shared(&arena, root, FloatSize::Single).expect("printable program");
    let text = program.render("process");

    // Three occurrences of one sub-diagram must not produce three copies of
    // its body; that is the property the memo table exists for.
    assert_eq!(
        text.matches("~").count(),
        1,
        "the recursive operand must be emitted once:\n{text}"
    );
}

#[test]
fn definitions_precede_their_first_use() {
    let mut arena = TreeArena::new();
    let root = shared_dag(&mut arena);
    let program = box_pp_shared(&arena, root, FloatSize::Single).expect("printable program");

    let mut defined: HashSet<String> = HashSet::new();
    for (index, definition) in program.definitions.iter().enumerate() {
        let (name, body) = definition
            .split_once(" = ")
            .expect("every definition binds a name");
        for used in referenced_identifiers(body) {
            assert!(
                defined.contains(&used),
                "definition {index} uses {used} before it is defined:\n{definition}"
            );
        }
        defined.insert(name.to_owned());
    }
    for used in referenced_identifiers(&program.root) {
        assert!(defined.contains(&used), "root uses undefined {used}");
    }
}

#[test]
fn every_definition_is_used_exactly_once_by_name() {
    let mut arena = TreeArena::new();
    let root = shared_dag(&mut arena);
    let program = box_pp_shared(&arena, root, FloatSize::Single).expect("printable program");

    let mut names = HashSet::new();
    for definition in &program.definitions {
        let (name, _) = definition
            .split_once(" = ")
            .expect("every definition binds a name");
        assert!(
            names.insert(name.to_owned()),
            "{name} is defined twice:\n{definition}"
        );
    }

    // An unused definition means the printer emitted a body it then rebuilt
    // somewhere else, which is the signature of a memo-table miss.
    let mut used = HashSet::new();
    for definition in &program.definitions {
        let (_, body) = definition.split_once(" = ").expect("named definition");
        used.extend(referenced_identifiers(body));
    }
    used.extend(referenced_identifiers(&program.root));
    for name in &names {
        assert!(used.contains(name), "{name} is defined but never used");
    }
}

#[test]
fn sharing_keeps_output_linear_in_dag_size() {
    // A chain of shared nodes: each level references the previous one twice.
    // Without memoization the expansion doubles at every level, so 24 levels
    // would be roughly 16 million occurrences.
    let mut arena = TreeArena::new();
    let mut node = {
        let mut builder = BoxBuilder::new(&mut arena);
        builder.wire()
    };
    const LEVELS: usize = 24;
    for _ in 0..LEVELS {
        let mut builder = BoxBuilder::new(&mut arena);
        node = builder.par(node, node);
    }

    let program = box_pp_shared(&arena, node, FloatSize::Single).expect("printable program");
    assert_eq!(
        program.definitions.len(),
        LEVELS,
        "one definition per distinct node"
    );
}

#[test]
fn deep_chains_do_not_exhaust_the_stack() {
    // The C++ printer recurses once per level; this depth would overflow a
    // default 8 MiB stack there. The explicit worklist must handle it.
    let mut arena = TreeArena::new();
    let mut node = {
        let mut builder = BoxBuilder::new(&mut arena);
        builder.wire()
    };
    for _ in 0..50_000 {
        let mut builder = BoxBuilder::new(&mut arena);
        let one = builder.int(1);
        let pair = builder.par(node, one);
        let add = builder.add();
        node = builder.seq(pair, add);
    }

    let program = box_pp_shared(&arena, node, FloatSize::Single).expect("printable program");
    assert!(program.definitions.len() > 50_000);
}

// ── Parenthesization ──────────────────────────────────────────────────────────

#[test]
fn operand_priority_drives_parentheses() {
    let mut arena = TreeArena::new();
    let (loose, tight) = {
        let mut builder = BoxBuilder::new(&mut arena);
        let wire = builder.wire();
        let two = builder.int(2);
        let pair = builder.par(wire, two);
        let mul = builder.mul();
        let scaled = builder.seq(pair, mul);
        let add = builder.add();
        let under_rec = builder.rec(add, scaled);
        (scaled, under_rec)
    };

    // The unshared printer writes `,` and `~` unpadded, unlike the shared one.
    // At the top level a `:` node needs no parentheses.
    let plain = box_pp(&arena, loose, 0, FloatSize::Single).expect("printable");
    assert_eq!(plain, "_,2 : *");

    // As the operand of `~` (priority 4) the same node is parenthesized.
    let nested = box_pp(&arena, tight, 0, FloatSize::Single).expect("printable");
    assert_eq!(nested, "+~(_,2 : *)");
}

// ── Binders ───────────────────────────────────────────────────────────────────

#[test]
fn abstraction_bodies_are_never_hoisted() {
    // `\(x0).(x0, x0 : +)` shares `x0`, but hoisting it into a top-level
    // definition would move the bound variable out of its binder's scope.
    let mut arena = TreeArena::new();
    let root = {
        let mut builder = BoxBuilder::new(&mut arena);
        let slot = builder.slot(0);
        let pair = builder.par(slot, slot);
        let add = builder.add();
        let body = builder.seq(pair, add);
        builder.symbolic(slot, body)
    };

    let program = box_pp_shared(&arena, root, FloatSize::Single).expect("printable program");
    assert!(
        program.definitions.is_empty(),
        "nothing inside a binder may be hoisted: {:?}",
        program.definitions
    );
    assert_eq!(program.root, "\\(x0).(x0,x0 : +)");
}

// ── Rejection ─────────────────────────────────────────────────────────────────

#[test]
fn unevaluated_shapes_are_rejected_rather_than_approximated() {
    // `with { ... }` carries an environment tree with no source syntax. C++
    // prints a raw tree dump inside the braces, which does not re-parse; a
    // printer whose output is meant to compile must refuse instead.
    let mut arena = TreeArena::new();
    let root = {
        let mut builder = BoxBuilder::new(&mut arena);
        let wire = builder.wire();
        let environment = builder.environment();
        builder.with_local_def(wire, environment)
    };

    let error = box_pp_shared(&arena, root, FloatSize::Single)
        .expect_err("an unevaluated `with` must not be serialized");
    assert!(matches!(error, BoxPrintError::NotAValidBox { .. }));
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extracts the `ID_<n>` identifiers referenced by one expression.
fn referenced_identifiers(expression: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = expression.as_bytes();
    let mut index = 0;
    while let Some(offset) = expression[index..].find("ID_") {
        let start = index + offset;
        let mut end = start + 3;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        out.push(expression[start..end].to_owned());
        index = end;
    }
    out
}
