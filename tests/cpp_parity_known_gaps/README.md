# C++ Parity Known Gaps Corpus

This folder contains small `.dsp` fixtures that were introduced as focused
parity reproducers before promotion into the default `tests/corpus/*.dsp`
scans.

Purpose:

- preserve minimal reproducers for known Rust vs C++ front-end parity gaps,
- keep the original focused inputs available for manual differential runs even
  after equivalent fixtures are promoted into `tests/corpus/`,
- provide auditable provenance for parity fixes and future gate promotions.

These files were added as focused C++ parity reproducers. The parser/eval
pipeline now accepts all current entries. Equivalent fixtures have now been
promoted into the regular parity guardrails under `tests/corpus/`, while the
original reproducers remain here as historical/parity references.

## Current entries

- `gap_01_pattern_def_constant_clause.dsp`
  - purpose: patterned definition with a constant clause followed by a variable
    clause
  - current status:
    - C++: accepted
    - Rust: fixed through parser `prepare_pattern` + eval `a2sb`
  - promoted corpus fixture:
    - `tests/corpus/rep_44_pattern_def_constant_clause.dsp`

- `gap_02_pattern_def_clause_grouping.dsp`
  - purpose: repeated same-name definition clauses that should be grouped into a
    single pattern-based definition family
  - current status:
    - C++: accepted
    - Rust: fixed through parser `prepare_pattern` + eval `a2sb`
  - promoted corpus fixture:
    - `tests/corpus/rep_45_pattern_def_clause_grouping.dsp`

- `gap_03_case_pattern_constant_folding.dsp`
  - purpose: `case` pattern requiring compile-time pattern evaluation
  - current status:
    - C++: accepted
    - Rust: fixed in the eval phase; the folded pattern now matches correctly
  - promoted corpus fixture:
    - `tests/corpus/rep_46_case_pattern_constant_folding.dsp`

- `gap_04_case_pattern_scope_barrier.dsp`
  - purpose: rule-local pattern variable that must not capture an outer binding
  - current status:
    - C++: accepted
    - Rust: fixed in the eval phase; pattern-variable lookup now stops at the
      barrier for nonlinearity checks while RHS evaluation still sees outer scope
  - promoted corpus fixture:
    - `tests/corpus/rep_47_case_pattern_scope_barrier.dsp`

## Suggested manual differential commands

Reference C++:

```bash
FAUST_CPP_BIN=/path/to/faust
"$FAUST_CPP_BIN" tests/cpp_parity_known_gaps/gap_01_pattern_def_constant_clause.dsp -norm
"$FAUST_CPP_BIN" tests/cpp_parity_known_gaps/gap_02_pattern_def_clause_grouping.dsp -norm
"$FAUST_CPP_BIN" tests/cpp_parity_known_gaps/gap_03_case_pattern_constant_folding.dsp -norm
"$FAUST_CPP_BIN" tests/cpp_parity_known_gaps/gap_04_case_pattern_scope_barrier.dsp -norm
```

Rust compiler:

```bash
cargo run -p compiler -- --dump-sig tests/cpp_parity_known_gaps/gap_01_pattern_def_constant_clause.dsp
cargo run -p compiler -- --dump-sig tests/cpp_parity_known_gaps/gap_02_pattern_def_clause_grouping.dsp
cargo run -p compiler -- --dump-sig tests/cpp_parity_known_gaps/gap_03_case_pattern_constant_folding.dsp
cargo run -p compiler -- --dump-sig tests/cpp_parity_known_gaps/gap_04_case_pattern_scope_barrier.dsp
```
