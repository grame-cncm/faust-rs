//! A definition is located at its name, even when the name is used earlier.
//!
//! Identifiers are hash-consed, so every occurrence of `b` is one node; the
//! parser tells them apart by the origins it records. The definition used to
//! be located at the first use of the name recorded since its previous
//! definition: in `a = b + 1; b = c * 2;` that is the `b` of `a`'s body, and
//! every "definition site" label for `b` pointed there.

use boxes::BoxBuilder;
use parser::{BoxOriginRole, parse_program};

/// `(line, col)` of the definition of each name in `source`.
fn definition_sites(source: &str, names: &[&str]) -> Vec<Option<(u32, u32)>> {
    let mut parsed = parse_program(source, "definition_locations.dsp");
    names
        .iter()
        .map(|name| {
            let ident = BoxBuilder::new(&mut parsed.state.arena).ident(name);
            let provenance = parsed.state.ctx.box_provenance();
            let definitions: Vec<_> = provenance
                .origins_for(ident)
                .iter()
                .filter_map(|id| provenance.get(*id))
                .filter(|origin| origin.role == BoxOriginRole::Definition)
                .map(|origin| (origin.location.line(), origin.location.col()))
                .collect();
            let def_prop = parsed
                .state
                .ctx
                .def_prop(ident)
                .map(|location| (location.line(), location.col()));
            assert_eq!(definitions.last().copied(), def_prop, "{name}");
            def_prop
        })
        .collect()
}

#[test]
fn a_name_used_before_its_definition_is_located_at_the_definition() {
    let sites = definition_sites(
        "a = b + 1;\nb = c * 2;\nc = a;\nprocess = a;\n",
        &["a", "b", "c", "process"],
    );
    assert_eq!(
        sites,
        [Some((1, 1)), Some((2, 1)), Some((3, 1)), Some((4, 1))]
    );
}

#[test]
fn definitions_of_a_with_block_are_located_at_their_names() {
    let sites = definition_sites(
        "process = f;\nf = g with {\n  g = h + 1;\n  h = 2;\n};\n",
        &["f", "g", "h"],
    );
    assert_eq!(sites, [Some((2, 1)), Some((3, 3)), Some((4, 3))]);
}

#[test]
fn a_definition_after_a_syntax_error_is_still_located_at_its_name() {
    // `x = ;` is a recovery statement: its name was read, its definition never
    // completed
    let sites = definition_sites("y = a;\nx = ;\na = 1;\nprocess = y;\n", &["a"]);
    assert_eq!(sites, [Some((3, 1))]);
}
