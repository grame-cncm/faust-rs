# Faust C++ Difference Audit Over `tests/corpus`

Date: 2026-08-11

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`

Corpus: all 219 `tests/corpus/*.dsp` files present on the audit date

Status update: the root-sibling ordering gap found by this audit was fixed on
2026-08-11; `DIFF-GAP-012` and `DIFF-GAP-013` remain open.

## 1. Purpose

This audit checks the complete local corpus for observable differences between
faust-rs and the pinned Faust C++ compiler. It is broader than the UI pathname
review that triggered it: it covers acceptance, diagnostics classification,
generated C/C++ module contracts, I/O arity, metadata, and UI event structure.

The durable classification of each confirmed difference lives in
[`faust-rs-vs-faust-cpp-differences-en.md`](faust-rs-vs-faust-cpp-differences-en.md).
This file is the dated evidence snapshot.

## 2. Acceptance and rejection matrix

`cargo run -p xtask -- corpus-status-query --all` produced:

| Classification | Cases |
|---|---:|
| accepted by C++ and Rust | 102 |
| rejected by C++ and Rust | 39 |
| expected C++ rejection / Rust acceptance | 78 |
| real acceptance divergence | 0 |

All 78 expected divergences are Rust source extensions already present in the
compatibility registry:

- 51 cases fail in C++ because `fad` is undefined;
- 27 cases fail in C++ because `rad` is undefined.

The 39 common rejections include intentional error fixtures, the soundfile
part-range rejection, and `ondemand`/FFT corpus files whose local
`interleave.lib` is deliberately outside the default golden import path. Rust
and C++ diagnostic wording and stages differ as described by `DIFF-BEH-005`.

The regenerated detailed matrix is
[`phase-4-corpus-status-diff-report-en.md`](phases/phase-4-corpus-status-diff-report-en.md).

## 3. Common generated-backend surface

The full-corpus backend report found the expected C and C++ module-shell
signature for all 102 cases accepted by both compilers:

| Backend | Matching shell | Signature diff | Unsupported/rejected |
|---|---:|---:|---:|
| C++ | 102 | 0 | 117 |
| C | 102 | 0 | 117 |

Here, "matching shell" means the reduced public envelope checked by the report:
class/struct identity and required platform macros. It is not a whole-source or
numerical-equivalence claim. See
[`phase-6-backend-full-corpus-diff-report-en.md`](phases/phase-6-backend-full-corpus-diff-report-en.md)
and `DIFF-BACK-004`.

An additional extraction of `getNumInputs()` and `getNumOutputs()` from both
generated C++ outputs found zero arity differences across the 102 common cases.

## 4. Confirmed general metadata gaps

### 4.1 Source identity (`DIFF-GAP-012`)

All 102 common generated C++ outputs differ in source identity metadata:

- C++ derives `filename` from the DSP basename and uses that basename as the
  default `name`;
- C++ also honors an explicit top-level `declare name`, as in
  `rep_40_metadata_master.dsp`;
- Rust currently emits `filename = "mydsp.dsp"` and `name = "mydsp"` in both
  the banner and `metadata()` callback.

This is observable to a host through `Meta::declare`; it is not merely a banner
formatting difference.

### 4.2 Global and imported metadata (`DIFF-GAP-013`)

After excluding `compile_options`, `name`, and `filename`, eleven common cases
still have different metadata sets. Rust C/C++ output omits declarations that
C++ emits:

- master declaration: `rep_11_declare_metadata`;
- import/component/library provenance: `rep_41_metadata_import`,
  `rep_42_component_metadata`, and `rep_43_library_metadata`;
- standard-library metadata reached by `rep_61_fmin_sr`,
  `rep_64_dynamic_rem`, `rep_69_variable_delay_sr_millisec`,
  `rep_71_degenerate_unary_recursion`, `rep_79_multi_output_recursion`,
  `rep_80_mutual_recursion_crossed`, and
  `vector_recursive_delay_fusion_pulse_countup_loop`.

The parser/eval global metadata store is implemented; the remaining defect is
transport into generated C/C++ `metadata()`. Widget and group metadata emitted
as UI `declare` calls use a different path and are not included in this gap.

## 5. UI event audit

Ninety-seven corpus files contain a direct UI primitive. Twenty-five of those
are accepted by both compilers under the default self-contained corpus setup.
Comparing the complete ordered stream of box, widget, metadata-declare, and
close-box events produced:

| Result | Cases |
|---|---:|
| exact event parity after the root-order fix | 23 |
| deliberate relative-group extension | 2 |
| remaining unintended UI difference | 0 |

The two deliberate extensions are `rep_63_ui_relative_group_rebase` and
`rep_64_ui_relative_group_root_clamp` (`DIFF-BEH-008`).

The initial audit found five root-ordering mismatches:

- `rep_38_sine_phasor`;
- `rep_55_sine_phasor_echo_feedback`;
- `rep_57_additive_synth`;
- `rep_66_variable_delay_feedback`;
- `rep_75_ui_widget_family_breadth`.

C++ sorts controls below its implicit root by the raw label/order key. Rust was
retaining dataflow-discovery order in the root forest. `UiProgramBuilder` now
sorts root siblings with the same stable raw-label key used inside explicit
groups, and all five cases are included in the maintained C++ UI-event
differential. Re-running the 25-case comparison leaves only the two deliberate
relative-group extensions.

## 6. Whole-source comparison and limits

`cargo run -p xtask -- golden-check-cpp` currently checks 34 stored C++ source
snapshots. All 34 differ, with the first reported difference being the source
identity described by `DIFF-GAP-012`. Further differences include the expected
Rust compiler banner, compilation-option text, formatting, declaration layout,
and internal emitter choices. Raw byte identity is therefore not the active
compatibility contract (`DIFF-BACK-004`).

This audit does not claim corpus-wide numerical equivalence. The generated
backend report intentionally compares only a reduced shell signature, and
`tests/corpus` does not currently provide a general C++-versus-Rust executable
sample harness for every accepted DSP. Numerical parity remains covered by the
dedicated impulse, runtime-trace, optimized/unoptimized, table, vector, and
backend-specific suites. A future corpus-wide execution harness would increase
assurance beyond this structural audit.

## 7. Reproduction

Primary maintained commands:

```sh
cargo run -p xtask -- corpus-status-query --all --format human
cargo run -p xtask -- corpus-status-report
cargo run -p xtask -- backend-full-corpus-diff-report
FAUST_CPP_BIN=../faust/build/bin/faust cargo run -p xtask -- golden-check-cpp
```

The arity, metadata, and complete UI-event comparisons were audit scripts over
the 102 mutually accepted generated C++ files. Their durable findings are now
represented by corpus fixtures and stable `DIFF-*` entries; future automation
should promote these comparisons into maintained differential tests when the
corresponding gaps are fixed.
