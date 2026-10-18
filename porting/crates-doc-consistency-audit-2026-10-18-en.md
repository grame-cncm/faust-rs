# Documentation consistency audit of `crates/` — faust-rs

- Repository: `faust-rs`, branch `main-dev`, HEAD `a66aebfc`.
- C++ reference: sibling checkout `../faust` @ `8eebea429`.
- Scope: every crate README, `//!` module docs, `///` docs of public items and of the main modules, `.md` files under `crates/`.
- Method:
  - `cargo doc` (public items, then `--document-private-items`);
  - automatic scan of every cited path;
  - search for files deleted or renamed since 2026-05-01;
  - reading by seven agents, one per crate group;
  - manual cross-check of a sample (about 15 findings, all confirmed).
- Read-only: no repository file was changed by the audit.

Severity:
- **wrong**: the doc states something false or names something that does not exist;
- **stale**: the doc describes a former state;
- **imprecise**: the doc is incomplete or misleading;
- **minor**: link, typo, rustdoc warning.

Format of each item: `file:line` — *Doc* — *Code* — **Fix**.

---

## 0. Summary

| Group | wrong | stale | imprecise | minor |
|---|---|---|---|---|
| tlib, boxes, signals, ui, normalize, sigtype, interval, algebra, graph, doc, draw, diagnostics, foreign-call | 8 | 5 | 17 | 8 |
| parser, eval, propagate | 16 | 19 | 15 | 13 |
| transform (outside vector/module) | 8 | 7 | 12 | 8 |
| transform `signal_fir/vector` + `signal_fir/module` | 11 | 6 | 12 | 4 |
| fir, codegen | 13 | 5 | 11 | 10 |
| compiler, faust, C/C++ FFI crates | 6 | 4 | 12 | 8 |
| cranelift-ffi, interp-ffi, wasm-ffi, impulse-runner, xtask | 7 | 7 | 16 | 9 |
| **Total (≈ 300)** | **69** | **53** | **95** | **60** |

The most frequent defect families:

1. **Public API renamed on 2026-08-18, READMEs not updated.**
   - `eval`: `eval_entrypoint*` and `eval_process_with_*` → `eval(&EvalRequest)`;
   - `parser`: `parse_*_with_*` → `ParseOptions`;
   - `transform`: `compile_signals_to_fir_fastlane_with_ui/_clocked*` → `SignalFirRequest`;
   - `propagate`: wrappers gone.
2. **Stale "scaffold", "planned", "fail-closed", "Step N" statuses.**
   - `codegen/backends/mod.rs` calls 10 complete backends scaffolds;
   - the vector UI is described as a scalar fallback;
   - BRA is described as dormant;
   - `cranelift` is called "Planned".
3. **Dead names after the module splits**: `delay/`, `module/` (`BraState`), `vector_module`, `ensure_delay_line`, `promote_signals_for_fir`, `lower_readonly_table_definition`, `sample_statements`/`lower_sig`, `cli/human.rs`.
4. **Invented or misplaced C++ provenance**, about fifteen. Examples: `boxForwardAD`, `getBoxType` for `fad`, `checkRange`, `signalPromotion`, `recursivness.cpp`, `conditionAnnotation.cpp`, `sound_player.h`, `CmajorTableVisitor`, `compiler/generator/asc/`, `evalBoxWidget`.
5. **"Position in the pipeline" diagrams that contradict each other** from one README to the next, and differ from AGENTS.md (see §T1).

### **Code** defects found in passing (not doc, to be handled separately)

1. **`freeCMemory` in `libfaust-rs`** (likely undefined behaviour).
   - The exported symbol is the `interp-ffi` one (`crates/interp-ffi/src/factory.rs:488-491`, unconditional `no_mangle`). It only does `CString::from_raw`.
   - Yet `libfaust-box-c.h`/`libfaust-signal-c.h` (and the `box-ffi`:37 and `signal-ffi`:26 READMEs) say to free the `Signal*` arrays of `CboxesToSignals*`/`CsimplifyToNormalForm2` with `freeCMemory`.
   - The version that recognises handle arrays (`box-ffi/src/lib.rs:541-555`) is behind the `standalone-capi-globals` feature, which nothing enables. `tree_ffi::free_context_or_c_string` has no caller.
   - Confirmed by reading, not yet by a C client.
2. **`xtask diagnostics-quality-check` fails on HEAD.**
   - The allowlist at `crates/xtask/src/diagnostics_quality_check.rs:62` points to `crates/compiler/src/cli/human.rs`, renamed `diagnostics_human.rs` (`d5b745df`, 2026-09-17).
   - Other failures: FRS-EVAL-0008..0011 missing from `docs/faust-error-model-en.md`.
   - This check is not one of the five CI gates.
3. **`EvalStats`** (`eval/src/error.rs`, `lib.rs:395-416`).
   - `nodes_evaluated` and `env_lookup_total_depth` are never written.
   - `env_layers_pushed` and `env_lookups` are always 1.
   - `loop_detector_max_depth` receives the final stack length, not its maximum.
4. **FAD/RAD divergence on bargraphs.**
   - Since 2026-09-19, FAD passes the tangent through `vbargraph`/`hbargraph` (`forward_ad.rs:349-355`, `:1309-1320`).
   - RAD still gives a zero adjoint (`reverse_ad.rs:243-248`, `:480-482`).
   - Consequence: a loss that goes through a bargraph has a zero gradient under `rad`.
5. **`porting/MEMOIZATION.md` incomplete.**
   - §2.17 does not record `env_tokens_memo` (`propagate/src/forward_ad.rs:507-511`, added by `9e5d9320`).
   - §2.22 cites a nonexistent `has_free_slot` (the real name is `free_slots`).
   - This registry is mandatory under AGENTS.md §9bis.
6. **Difference registry incomplete** (AGENTS.md §5).
   - Not recorded:
     - the C++ headers `libfaust-box.h`/`libfaust-signal.h` cover only part of the reference: no `isBox*`, no `DSPToBoxes`, no `boxesToSignals`…;
     - the overloads are named `boxAddAux` instead of `boxAdd`;
     - the `CboxFad`/`CboxRad` additions;
     - the absence of `freeCMemory` from `libfaust-c.h`;
     - `-mdoc` (`compiler/documentator/`) not being ported.
   - DIFF-API-004 only covers `libfaust.h`.
7. **AGENTS.md §9bis does not match the code.**
   - AGENTS.md says "tolerance 25%", "regression of 25% or more", "spread ±8%".
   - The code and the baseline say otherwise:
     - `tests/compile-budget/release-baseline.json:11` holds `"frontend_tolerance_percent": 40`, applied to both baskets;
     - codegen also gets `CODEGEN_TIMER_NOISE_MS` = 5 ms;
     - `compile_budget.rs:50` says about 10%.
   - The `xtask` README (l. 209, 40%) and the 2026-10-18 journal entry ("under a 40% tolerance") are right. AGENTS.md must be fixed, or the value 40 changed.
8. **Absolute local paths in code constants**, outside the docs. Constants `DEFAULT_FAUSTLIBRARIES_ROOT` (in `cranelift-ffi/src/diff.rs`, `probe/train.rs`, `faust/tests/ddsp.rs`, `compiler/tests/import_loading_cpp_gap.rs`…), `xtask/src/main.rs:66`, `examples_compare.rs:60`. These values are probably overridable by environment variables: check against AGENTS.md §4.

---

## 1. `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"`

**Result: fails**, with 5 errors in 3 crates. The other crates document cleanly with `--keep-going`.

| File:line | Error | Fix |
|---|---|---|
| `crates/diagnostics/src/codes.rs:94` | `unclosed HTML tag <k>` | write `` `<k>` `` or `&lt;k&gt;` |
| `crates/fir/src/types.rs:22` | unresolved link `FirMatch::DeclareDependency` (the variant exists, `matcher.rs:149`) | `[`FirMatch::DeclareDependency`](crate::FirMatch::DeclareDependency)` |
| `crates/codegen/src/backends/wasm/layout.rs:131` | public doc → private item `FieldOrder::Declaration` | plain backticks, no link (the item is `pub(crate)`) |
| `crates/codegen/src/backends/wasm/layout.rs:133` (×2) | → `FieldOrder::SmallFirst`, `Self::from_module_with_order` (private) | same |

For information, a `--document-private-items` pass gives 36 warnings. They do not break CI, but several point at real dead names; they are triaged crate by crate below.

---

## T. Cross-cutting points

### T1 — Contradictory pipeline diagrams (imprecise, several crates)

Each README has its own "Position in the pipeline" line:

| File | Diagram |
|---|---|
| `boxes/README.md:37` | `parser → [boxes] → eval → propagate → signals → transform → codegen` |
| `signals/README.md:42` | `propagate → [signals] → transform → codegen` |
| `normalize/README.md:46` | `propagate → [normalize] → transform → codegen` |
| `sigtype/README.md:67` | `signals → normalize → [sigtype inference] → transform` |
| `interval/README.md:60` | `signals → normalize → [interval inference] → transform` |
| `ui/README.md:71` | `propagate → [ui] → transform::signal_fir → codegen` |
| `eval/README.md:76-80` | `parser → boxes → [eval] → propagate → signals` |
| `propagate/README.md:97-99` | `eval → [propagate] → signals → transform → codegen` |
| `codegen/README.md:13` | `parser → boxes → eval → propagate → signals → transform → fir` |
| `compiler/README.md:170-172`, `compiler/src/lib.rs:14` | no `boxes`; "[optional signal→FIR]" |
| `transform/README.md:11`, `transform/src/lib.rs:14-15` | no normalize/sigtype |

What the code does:
- `normalize` and `sigtype` are **libraries** called alternately by `transform::signal_prepare` (`signal_prepare/mod.rs:13-35`, 5 interleaved `TypeAnnotator` re-typings).
- `normalize` depends on `sigtype`.
- `eval` depends on `propagate` and `normalize` for constant folding (`eval/src/simplify.rs`).
- `interval` has no dependencies; interval inference is done in `sigtype::rules`.
- `fir` appears almost nowhere.
- Every backend goes through signal→FIR; only the dump modes skip it.

**Fix**: one shared sentence restating AGENTS.md (`parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend`), followed by a crate-specific clarification. For example:
- `normalize`: "library called by `transform::signal_prepare` and by `eval`, not a standalone pass";
- `interval`: "pure arithmetic, consumed by `sigtype::rules`";
- `compiler`: `parse → boxes → eval → propagate → signal_prepare (normalize/type/interval) → signal_fir → fir → codegen`.

### T2 — DirectedGraph: `graph` or `transform::schedule`? (imprecise)

- What the docs say:
  - plan §2 assigns `compiler/DirectedGraph/` to `graph`;
  - `crates/graph/README.md:5-14` and `lib.rs:5-10` present `graph` as the future home of "topological order… scheduling helpers";
  - the `graph` README also says these helpers must stay **out** of `transform`.
- What the code does: the port lives in `transform::schedule` (`schedule/mod.rs:4-6`, `dag.rs`, `depth_first.rs`…), and no crate depends on `graph`.
- **Fix**: the `graph` README must say that the DirectedGraph port lives in `transform::schedule` and that `graph` is an unused scaffold. Add `compiler/DirectedGraph/*` to the provenance in the `transform` README.

### T3 — The plan (`porting/faust-rust-porting-plan-en.md`) no longer tracks the workspace (reported once)

- §4/§5 name `errors/`, `utils/` and a single `cffi/` crate ("cbindgen/cxx"); they omit `sigtype`, `ui`, `foreign-call`, `diagnostics`, `xtask`, `impulse-runner`, `faust` and the eight `*-ffi` crates.
- Wrong dependencies:
  - "propagate depends on eval": it is the other way round;
  - "transform depends on graph": false;
  - "signals depends on interval": actually `tlib` + `ui`;
  - "normalize depends on algebra": false.
- §2 puts typing in `signals` (actually `sigtype`) and does not mention `asc` or `cranelift`.
- §3.2/§3.3 describe files that do not exist (`eval.rs`/`pattern.rs`/`lambda.rs`, `signals/src/sigtype.rs`, `signals/src/xtended/`).
- §4.6 of the vector cleanup plan still places `fir_children` "in `lower.rs`". It is in `vector/lower/check.rs:388`, delegating to `fir::fir_match_children`.

The crate READMEs are, on the whole, closer to reality than the plan.

### T4 — AGENTS.md

- §9bis: see code defect no. 7 (25% / ±8% versus 40% / ~10%).
- The rest is consistent:
  - patternmatcher is indeed in `eval`, but the `eval` README does not say so (see `eval`);
  - extended math is indeed first-class `signals` nodes, but the `signals` README does not say so;
  - parallelize is indeed in `transform`;
  - no doc reintroduces public `box_*`/`is_box_*`.

---

## 2. Foundation crates

### tlib

**wrong**
- `tlib/README.md:55` — *Doc*: "dependency of every other crate in the workspace". *Code*: 15 crates do not depend on it, including `codegen`, `diagnostics`, `interval`, `foreign-call`, `algebra`, `graph`, `doc`, `faust` and the `*-ffi` crates. **Fix**: "foundation of the tree-encoded IR crates (`boxes`, `signals`, `ui`, `fir`…) and of the passes over them".

**imprecise**
- `tlib/src/lib.rs:33-37` — *Doc*: "the fast-lane now clones… applies `de_bruijn_to_sym`". *Code*: done in `transform::signal_prepare` (`signal_prepare/mod.rs:598-601`). **Fix**: name `transform::signal_prepare`.
- `tlib/README.md` (Public API) — missing: the `stack` module (`on_compile_stack`, `on_deep_stack`, `COMPILE_STACK_SIZE`), `vec_to_list`/`list_to_vec`, `tree_to_str`, `de_bruijn_to_sym_many` and `PropertyKey`; the provenance omits `compiler/tlib/recursive-tree.cpp` (cited by `recursion.rs:4`). **Fix**: complete.

### boxes

**wrong**
- `boxes/src/builder.rs:677-679`, `matcher.rs:157-159`, `internals.rs:884,890` — *Doc*: C++ provenance `boxForwardAD(Tree exp, Tree seed)`, `isBoxForwardAD`, `boxReverseAD`. *C++ code*: nothing (`grep -rn ForwardAD ../faust/compiler` → 0); `fad`/`rad` are faust-rs extensions (registry l. 60-83). **Fix**: "faust-rs extension, no C++ equivalent" (as for `CInputs`, `matcher.rs:143`). Adjust `README.md:20` ("1:1 mapping with C++ `box*`") to mention these extensions.

**stale**
- `boxes/src/lib.rs:13` — *Doc*: "Legacy `node_*` / `is_node_*` helpers are kept internal". *Code*: no `is_node_*` (grep → 0); only the `node_*` remain, as `pub(crate)`. **Fix**: "the `node_*` constructors are `pub(crate)`; the public `box_*`/`is_box_*` helpers were removed in favour of `BoxBuilder` + `match_box`".

**imprecise**
- `boxes/README.md` (Public API) — missing `box_pp`, `box_pp_shared` (option `-e`), `format_real`, `FloatSize`, `SharedBoxProgram`, `BoxPrintError`, `box_complexity`; the provenance omits `ppbox.cpp` and `boxcomplexity.cpp`. **Fix**: complete.

**minor**
- `boxes/README.md:29` — *Doc*: `list4` payload for "slider/button". *Code*: button and checkbox carry only the label (`internals.rs:898-905`). **Fix**: "slider/numentry".

### signals

**stale**
- `signals/README.md:30` — *Doc*: "Slider parameter payload keeps Faust list encoding `list4(init,min,max,step)`". *Code*: UI nodes carry only a `ControlId` (`SigBuilder::vslider(control: ControlId)` `lib.rs:938`, `SigMatch::VSlider(ControlId)` `:1197`); `lib.rs:18-19` actually says the opposite. **Fix**: "control signals carry a stable `ui::ControlId` only; label, range and layout live in `ui::UiProgram`".

**imprecise**
- `signals/README.md` (whole file) — does not say that extended math (`compiler/extended/*prim.hh`) is integrated here as `SigMatch` variants (`Acos`…`Round`, `Pow`, `Min`, `Max`, `Fmod`, `Remainder`, `lib.rs:1130-1172`), although AGENTS.md §2 requires it. It does not say that typing lives in `sigtype` either, and does not mention `filter_algebra` (17 public functions). **Fix**: add these three points.
- `signals/src/lib.rs:41-43` — *Doc*: "algebraic helpers… live above this representation layer". *Code*: they are in `signals::filter_algebra`; only the reveal passes, not ported, would sit above. **Fix**: say so.

### ui

**wrong**
- `ui/README.md:41` — *Doc*: `UiGroupKind` = `HGroup`/`VGroup`/`TGroup`. *Code*: `Vertical`/`Horizontal`/`Tab` (`lib.rs:63-67`).
- `ui/README.md:45` — *Doc*: `ControlKind` = `Button`, `CheckBox`, `Slider`, `Bargraph`, `Soundfile`. *Code*: `Button, Checkbox, VSlider, HSlider, NumEntry, VBargraph, HBargraph, Soundfile` (`lib.rs:151-160`).

**imprecise**
- `ui/README.md:20` — *Doc*: "Builds a hierarchical `UiProgram` from box-tree traversal". *Code*: `ui` depends only on `tlib`; the box traversal is done by `propagate/src/ui_build.rs`, which feeds `UiProgramBuilder`.
- `ui/README.md:43,47,48` — three incomplete or wrong descriptions:
  - `UiGroupSpec` "(kind + path)": real fields `kind`, `label`, `metadata`;
  - `ControlSpec`: also carries `id`, `metadata`, `source_node`, and `range` is an `Option`;
  - `UiMetadata`: holds the `[key:value]` metadata extracted from labels (`split_label_metadata`), not `declare` statements.
- `ui/README.md` — missing `find_duplicate_control_paths`/`DuplicateControlPath` (source of FRS-UI-0001), `group_ui_label`, `AnonymousWidgetNames`, `ordering_key_from_label` and `UiNormalized*Path`; the provenance omits `propagate/labels.cpp` and `generator/uitree.cpp`.

### normalize

**wrong**
- `normalize/src/normalform.rs:199` — *Doc*: "C++: `signalPromotion(sig)` in `normalform.cpp`". *C++ code*: the pass is `signalPromote(Tree,bool)` (`compiler/transform/sigPromotion.cpp:1165`, class `SignalPromotion`), called from `normalform.cpp:107`.

**stale**
- `normalize/README.md:37` — *Doc*: "`promote_signals_fastlane` … used by `transform::signal_fir`". *Code*: the only caller is `transform::signal_prepare`. **Fix**: say so, and add `simplify_signals_fastlane` (`normalform.rs:274`).
- `normalize/src/table_promote.rs:4-5` — *Doc*: `sigPromotion.cpp:577-640`, `normalform.cpp:115`. *C++ code*: `SignalTablePromotion` spans lines 702-782 and the call is at `normalform.cpp:118`.

**imprecise**
- `normalize/README.md:26-28`, `src/lib.rs:19-21` — *Doc*: "`aterm`, `normalize`, `simplify`, `normalform`: in progress". *Code*: `simplify`, `rec_merge` and `table_promote` run on every compilation (`signal_prepare` 2.7-2.10b); `prepare_signals*` has a single caller, `signal-ffi`.
- `normalize/README.md` — missing `rec_merge` (no C++ equivalent), `table_promote`, `try_simplify_const`, `try_simplify_const_cached`, `ConstSimplifyCache` (used by `eval`), `DivisionByZero` and `TableRangeWarning`.
- `normalize/src/normalform.rs:7-18` — *Doc*: "three preparation steps that `crates/transform/signal_prepare` currently performs", then lists four steps. The call direction is also reversed: `signal_prepare` calls `normalize`.

**minor**
- `normalize/src/normalform.rs:8` — path `crates/transform/signal_prepare` → `transform::signal_prepare`, or `crates/transform/src/signal_prepare/`.
- `normalize/src/rec_merge.rs:5-6` — "macroeconomic" → "macro-expanded (library)".

### sigtype

**imprecise**
- `sigtype/README.md` — missing `InferenceError`, `InferenceRule`, `InferenceWarning` and the public `api` module. The split with `signals` and `interval` is not explained. **Fix**: add these lines and "type lattice/inference for the IR defined in `signals`; interval bounds from `interval`".

**minor**
- `sigtype/src/ops.rs:7` — `mergenature`/`mergevariability` are `inline` functions of `sigtype.hh:162,174`, not of `sigtype.cpp`.
- `sigtype/src/rules.rs:1370` (private rustdoc warning) — `Self::solve_recursive_groups_jacobi` exists (`:1433`) but under `#[cfg(test)]`; it is a scoping issue. **Fix**: plain backticks.

### interval

**wrong**
- `interval/README.md:39` — *Doc*: `floor`/`ceil` in `ops::arithmetic`. *Code*: they are in `ops::math` (`math.rs:22,38`), with `round rint exp exp10 log log10 sqrt pow remainder`.

**imprecise**
- `interval/README.md:57-61` — the "[interval inference]" stage does not exist: the crate has no dependencies and inference lives in `sigtype::rules::TypeAnnotator`.

**minor**
- `interval/README.md:43` — `ops::trig` omits `acosh asinh atanh cosh sinh tanh`, which are re-exported (`lib.rs:373`).

### algebra / graph / doc

**imprecise**
- `algebra/README.md:16-25`, `src/lib.rs:3-18` — the provenance speaks of an algebra "historically spread across… passes", whereas `compiler/FaustAlgebra/FaustAlgebra.hh` exists: a dispatch-table interface derived by `interval_algebra.hh` (plan §2). It also says "so upper crates can declare dependencies", but no crate depends on it. **Fix**: cite `FaustAlgebra.hh` and say that the Rust interval operations are free functions called by `sigtype::rules`.
- `graph/README.md:5-14`, `src/lib.rs:5-10` — see §T2.
- `doc/README.md:1-12`, `src/lib.rs:1-11` — *Doc*: "documentation builders shared by developer tooling (xtask, phase reports, parity dashboards)". *Plan §2*: home of `compiler/documentator/` (`-mdoc`). *Code*: not ported, no `-mdoc` option, absent from the registry. **Fix**: "planned home of the `compiler/documentator/` port (`-mdoc`); not ported, empty scaffold", and add a registry entry.

### draw

No discrepancy. C++ module mapping, sampled line numbers, options `-svg`, `--scaled-svg`, `--fold`, `--fold-complexity`: all exact.

### diagnostics

**wrong**
- `diagnostics/src/codes.rs:46` — *Doc*: "the `checkRange` check in `eval.cpp`". *C++ code*: absent from the pinned reference; the message comes from `SignalChecker::isRange` (`compiler/transform/sigPromotion.cpp:70-82`). (`checkRange` only exists upstream, ≥ 2.90; see `eval`.) **Fix**: cite `SignalChecker::isRange` and say that Rust reports it at evaluation time.

**imprecise**
- `diagnostics/src/lib.rs:10` — *Doc*: "envelope used by all compiler stages". *Code*: only `parser`, `eval`, `propagate` and `compiler` depend on it; the README (l. 31-34) says so correctly. **Fix**: reuse the README wording.
- `diagnostics/README.md:14-24` — the v2 model is missing: `SourceMap`, `SourceRange`, `ContentHash`, `LspPosition`, `SuggestedFix`, `TextEdit`, `Applicability`, `DiagnosticTrace`, `IrReference`, `all_codes`.

**minor**
- `diagnostics/src/codes.rs:94` — `cargo doc` error `<k>` (see §1).

### foreign-call

**wrong**
- `foreign-call/README.md:34` — *Doc*: "the workspace exception to the global `unsafe_code = "forbid"` lint". *Code*: 10 crates declare `unsafe_code = "allow"` (`box-ffi`, `cranelift-ffi`, `faust`, `ffi-common`, `foreign-call`, `interp-ffi`, `libfaust-ffi`, `signal-ffi`, `tree-ffi`, `wasm-ffi`). **Fix**: "one of the exempted crates (the only one outside the FFI crates and `faust`)".

---

## 3. Front end

### parser

**stale**
- `parser/README.md:25-36` (entry-point table):
  - `parse_program(arena, ctx, src)`: real signature `(input, source_file)` (`lib.rs:2021`);
  - `parse_program_with_metadata`, `parse_file_with_imports`, `parse_file_with_imports_and_metadata` and `parse_program_with_remote_imports_and_precision_and_metadata` no longer exist (replaced on 2026-08-18, `lib.rs:1900-1914`);
  - replacements: `ParseOptions` (`:1916`, `with_*`), `parse_program_with_options` (`:2031`), `parse_program_with_imports` (`:2060`), `parse_file` (`:2100`), `parse_url` (`:2133`);
  - `parse_minimal(input) -> bool`;
  - `with_state(&RefCell<ParseState>, f)` borrows an existing state, it does not create one;
  - `set_use_prop_from_token(ctx, sym, file, token)`;
  - missing: `lex_stream`, `LexerImpl`, `LexOutcome`, `RawLexeme` and `import_candidates`.
- `parser/src/lib.rs:1474` (doc of `ParseOutput::used_files`) — `parse_file_with_imports(...)` → `parse_file` / `parse_program_with_imports`.
- `parser/src/lib.rs:1520-1523` — "before either becomes the default": the parse path already uses the combined DFA (`combined_lexer`, `:2826`, `:1819`); `PerRule` only serves `lex_tokens` and the differential. Same imprecision in `lexerdef()` (`:1489-1490`).
- `parser/src/context.rs:1` ("migration prototype"), `lib.rs:12` ("migration is active through slices 1..12") — this is now the production parser.
- `parser/tests/production_corpus.rs:5` — "parser-proto internals": that crate no longer exists.

**imprecise**
- `parser/README.md:38-74` — missing `ParseOptions`, `VirtualSourceMap`, `ImportSite`/`ImportCycleEdge`, `RemoteFetchPolicy`/`RemoteFetchRequest`/`FetchedSource`/`SourceFetchError(Kind)`, `PrefetchedRemoteSourceBundleError`, `BoxProvenance`/`BoxOrigin*`/`LocatedBox` and `WidgetDeclaration`.
- `parser/fuzz/README.md:3-5` — "any input": the target skips invalid UTF-8 (`fuzz_targets/parse_program.rs:9`).

**minor**
- `parser/tests/api_bridge.rs:1` — header "Integration tests for `api_bridge`": no module by that name.

### eval

**wrong**
- `eval/README.md:55-63` — *Doc*: "Public API / Pattern matcher" presents `make_pattern_matcher(rules)`, `apply_pattern_matcher(matcher,args,env)`, `Rule`, `Automaton`… as public. *Code*: `mod pattern_matcher;` is private (`lib.rs:179`); `apply_pattern_matcher` is `pub(crate)`. Real signatures: `make_pattern_matcher(arena, rules) -> Automaton` and `apply_pattern_matcher(arena, automaton, s, x, env_out, fold) -> (Option<usize>, Option<TreeId>)`. **Fix**: "Pattern matcher (internal, `pattern_matcher.rs`): port of `compiler/patternmatcher/patternmatcher.cpp` merged into `eval` (AGENTS §2), no public API". Note: `public-api-baseline.txt` lists these items because the tool ignores reachability.
- `eval/src/lib.rs:12`, `:262-273`, `:278` — *Doc*: `eval_process` ↔ a `Tree eval(Tree ldef, int& numInputs, int& numOutputs) { gGlobal->gCurrentEnv = …; initRecursion(); … }`. *C++ code*: neither this signature, nor `gCurrentEnv`, nor `initRecursion` exist. The real entry point is `Tree evalprocess(Tree eqlist)` (`eval.cpp:105`), with `boxSimplification` when `gSimplifyDiagrams`. **Fix**: cite `evalprocess()` and replace the excerpt.
- `eval/src/lib.rs:119` — *Doc*: "Fully `Send`/`Sync`". *Code*: `Environment.store` is an `Rc<RefCell<…>>` (`environment.rs:224`) and `PatternMatcherValue` holds an `Rc<Automaton>`. **Fix**: "no global state; single-threaded values (Rc/RefCell)".
- `eval/src/loop_detector.rs:24-41`, `README.md:50,74` — *Doc*: the C++ `LoopDetector` would be a `set<Tree>` plus a counter; on the Rust side, "`Vec<TreeId>`" and "8 bytes per frame". *C++ code*: a 1024-entry ring buffer checked every 400 steps (`loopDetector.cpp:30-55`, `global.cpp:139`), fed by `detect(cons(exp, localValEnv))`, completed by `stackOverflowDetector` and the `visited` list. *Rust code*: `Vec<LoopFrame{id|sym, env_key}>`. **Fix**: describe the real C++ and call it an adaptation.
- `eval/src/environment.rs:204` — *Doc*: "Env copy/rewire: Deferred". *Code*: implemented by `copy_env_replace_defs` (`definitions.rs:192`) and `rewrite_captured_env` (`:156`). `lib.rs:106` actually says "Present".
- `eval/src/error.rs:12-42`, `:64-81` — three problems:
  - the `EvalStats` doc block sits above `EvalWarning`, so rustdoc attaches it to `EvalWarning`;
  - "opt-in entry points": `eval()` always returns the statistics;
  - the field docs are wrong (see code defect no. 3).
  **Fix**: move the block, drop "opt-in", document or remove the unfed counters.
- `eval/src/ui_widgets.rs:5` — *Doc*: "C++ `evalBoxWidget`/`evalBoxGroup`". *C++ code*: these functions do not exist; they are the `isBoxVSlider`… branches of `realeval` (`eval.cpp:356`, `:485+`).
- `eval/src/pattern_matcher.rs:44-46` — *Doc*: "`(-1, None)` on failure". *Code*: `(None, None)` (`:1022-1029`).

**stale**
- `eval/README.md:26-40` — the nine `eval_entrypoint*` and `eval_process_with_*` entry points: only `eval_process` still exists. *Code*: `eval(arena, defs, &EvalRequest) -> Result<(TreeId, EvalStats), EvalError>` (`lib.rs:363`), `EvalRequest::default().with_entrypoint/with_source_context/with_cancel`, `eval_process` (`:283`), `eval_box` (`:705`). Also add `EvalRequest`, `EvalWarning`, `DepthLimit`/`DepthGuard`/`DepthLimitSource`/`DepthContext` and `suggestions`.
- `eval/src/lib.rs:16`, `pattern_matcher.rs:34,231,976` — `apply_case_rules` does not exist → `eval_case_value` (`lib.rs:1581`) and `apply_pattern_matcher_value` (`apply.rs:194`).
- `eval/src/pattern_matcher.rs:29-30,176` — *Doc*: `match_num` "reserved for future optimisation". *Code*: used by `simplify_dispatch_argument`, `dispatches_on_number` and `unreachable_numeric_patterns`.
- `eval/src/lib.rs:116,121-126` — *Doc*: lookup by "Reverse u32 scan per layer". *Code*: `binding_index` hash index (`environment.rs:152,380`).
- `eval/src/lib.rs:213-215,227` (doc of `SymId`) — *Doc*: `Vec<(SymId, TreeId)>`, SIMD, and `match_pattern`. *Code*: `Vec<(SymId, EvalValue)>` + `HashMap`; `match_pattern` does not exist → `eval_ident_value`.
- `eval/src/environment.rs:177,197,201` — same stale storage description; `EvalValue::PatternMatcher` is missing from the variant list.
- `eval/src/lib.rs:377` — "behind every `eval_entrypoint_*`" → "implementation of [`eval`]".
- `eval/tests/core_eval.rs:1944,1955` — the comment and the `expect` message mention `eval_process_with_stats`.

**imprecise**
- `eval/README.md:8-14` — the provenance omits `compiler/patternmatcher/patternmatcher.{hh,cpp}` (integration required by AGENTS.md §2), as well as `environment.cpp` and `loopDetector.cpp`.
- `eval/README.md:3` ("Phase 4, section 2.2": the plan numbers it 4.2), `:76-80` (pipeline, see T1).
- `eval/src/apply.rs:142`, `environment.rs:129`, `error.rs:504`, `error.rs:26` — `gApplyMemo`, `realApplyList`, `checkRange` and `gRecursionLimit` are absent from the `8eebea429` reference; they exist upstream (≥ 2.90). **Fix**: suffix "(upstream ≥ 2.90, absent from pinned ref)".
- `eval/src/lib.rs:41` — `evalClosure()` does not exist in C++; closures are opened by `isClosure` in `real_a2sb`, `evalIdDef` and `applyList`.
- `eval/src/loop_detector.rs:47-53` — *Doc*: "Currently it holds: automaton_cache". *Code*: also `eval_cache`, `apply_memo`, `symbolic_box_cache`, `evaluated_boxes`, `box_arity_cache`, the closure store and `pm_store`. **Fix**: point to MEMOIZATION §2.3-2.5b.

**minor**
- `eval/src/loop_detector.rs:846` (private rustdoc) — `[`enter`]` → `[`enter`](Self::enter)`.
- `eval/src/case.rs:125` — `isBoxPatternOp` → `isBoxPatternOpBinary`/`isBoxPatternOpTernary`.
- `eval/src/lib.rs:938` (`//` comment) — absolute path `/Users/letz/faust/compiler/evaluate/eval.cpp` → `compiler/evaluate/eval.cpp` (AGENTS.md §4).

### propagate

**wrong**
- `propagate/src/api.rs:17-23` (doc of `propagate_typed_with_ui`) — *Doc*: "one tangent bundle per enabled control… honor `[autodiff:false]`"; `rad` → `RadUnsupportedNode`. *Code and other docs*: explicit seeds, metadata ignored (`forward_ad.rs:92-95,383`, `lib.rs:59-61`); in RAD, temporal and recursive nodes go to `SigBlockReverseAD` (`reverse_ad.rs:63-69`). **Fix**: "one tangent per seed; `[autodiff:false]` not consulted; temporal/recursive rad → BlockReverseAD, other unsupported nodes → RadUnsupportedNode".
- `propagate/src/lib.rs:56-57`, `arity.rs:32-33` — *Doc*: "matches C++ getBoxType (boxtype.cpp:371)" for `fad`. *C++ code*: no `fad` box; line 371 is `isBoxRoute` (checked). **Fix**: "faust-rs extension, no C++ rule".
- `propagate/src/profile.rs:3-4` — *Doc*: "C++ … FAUST_PROPAGATE_PROFILE table". *C++ code*: absent; only `FAUST_STATS_DO(gStats.fPropagate*)` exists. **Fix**: "Rust-only profiler".
- `propagate/src/engine.rs:331` — `sigattributes.cpp` does not exist → `compiler/signals/sigtyperules.cpp:802` (`isSigControl` branch).
- `propagate/src/forward_ad.rs:5` — `compiler/transform/sigtyperules.hh` → `compiler/signals/sigtyperules.hh`.

**stale**
- `propagate/README.md:35-48` — *Doc*: `propagate_typed_with_ui(arena, flat, inputs, cache)`; `propagate_typed_with_ui_options` and the `box_arity`/`propagate_with_ui`/`propagate` wrappers. *Code*: the function takes five parameters, including `ui_options`; the others do not exist. Real exports (`lib.rs:525-529`): `propagate_typed`, `propagate_typed_with_ui`, `propagate_fold`, `FoldSession`, `box_arity_typed`, `make_sig_input_list`, `control_widgets`, `ControlWidget(s)`, `UiGroupContext`, `DependentSeed`, `PropagateError`, `FlatBox*`, `try_build_flat_box`, plus the `clock_domain`, `stateful_rad` and `transpose_ad` modules.
- `propagate/src/lib.rs:76-77` — *Doc*: "One internal ForwardADTransform instance per seed". *Code*: a single multi-lane transformer (`generate_fad_signals_multi`, `forward_ad.rs:1945`).
- `propagate/src/stateful_rad.rs:14-16` — *Doc*: "phase-1 rad lowering still rejects temporal and recursive families". *Code*: these families go to BlockReverseAD; the classifier now only serves diagnostics and the dormant LTI path.
- `propagate/src/forward_ad.rs:105-106` — *Doc*: "bargraphs are metering sinks, not differentiated". *Code*: the tangent passes since 2026-09-19 (same file, `:349-355`). **Fix**: delete the line.
- `porting/MEMOIZATION.md` §2.17 and §2.22 — see code defect no. 5.

**imprecise**
- `propagate/README.md:63,66` — `PropagateOutput` omits `signal_origins` and `clock_domains`; `ArityCache` has four tables (`typed`, `wiring`, `forward_ad`, `fad_consumed_locally`).
- `propagate/README.md:68-75` — `forward_ad` and `reverse_ad` are presented as public API, but they are private `mod`s.
- `propagate/README.md:3`, `lib.rs:3` — "Phase 4, section 2.4": the plan numbers it 4.3. Pipeline: see T1.
- `propagate/src/result_memo.rs:4-6` — the C++ key is `tree(PROPAGATEPROPERTY, clockenv, slotenv, path, box, lsig)` (`propagate.cpp:921`), so `clockenv` is part of it.
- `propagate/src/result_memo.rs:50` — "1,024-entry warm-up" → 1,024 eligible calls (`RESULT_MEMO_WARMUP_CALLS`).
- `propagate/src/reverse_ad.rs:40-47` — the overlap with FAD on bargraphs no longer holds (see code defect no. 4).
- `propagate/src/result_memo.rs:60` — `examples/SAM/virtualAnalog/virtualAnalog.dsp` is in the C++ checkout; say so.

**minor**
- `control_widgets.rs:67` — ambiguous function/module link → `[`control_widgets()`]`.
- `forward_ad.rs:130` — the `|x|` split the table cell → `\|x\|`.
- `result_memo.rs:10-11` — redundant targets → `[`SlotEnvId`]`.
- `reverse_ad.rs:32` — `[`add_adjoint`]` → `[`ReverseADTransform::add_adjoint`]`.
- `reverse_ad.rs:116` — `signalPromotion` → `signalPromote`.

---

## 4. `transform`

### Outside `signal_fir/vector` and `signal_fir/module`

**wrong**
- `transform/README.md:91,99-100` — *Doc*: "Stable compiler contract" tier with `compile_signals_to_fir_fastlane_with_ui`, `_clocked` and `_clocked_with_timing`. *Code*: only `compile_signals_to_fir_fastlane(&SignalFirRequest)` exists (`signal_fir/mod.rs:639`); `mod.rs:505-516` documents the 2026-08-18 replacement. **Fix**:
  - write `compile_signals_to_fir_fastlane(&SignalFirRequest)`, built by `SignalFirRequest::new(arena, sigs, n_in, n_out, ui, opts).with_clock_domains/.with_timing_sink/.with_signal_origins/.with_forced_shadow_report()`;
  - add to the tier `SignalFirRequest`, `ControlRateMode`, `ProcessingApi`, `SignalFirTimingSink`, `DEFAULT_BRA_TAPE_BLOCK_SIZE`, `FirOrigins`/`FirSignalOrigin`;
  - on the `signal_prepare` side, add `PrepareOptions` and `prepare_signals_for_fir_{with_options, verified_with_options, verified_with_origins}`.
- `signal_fir/mod.rs:53-54` — *Doc*: "UI-program and reverse-AD graphs under -vec fall back to scalar". *Code*: UI programs are certified (`compiler/tests/vector_mode.rs:651-656`); `UiProgram` only appears when `build_vector_ui_fir` fails. **Fix**: "reverse-AD graphs fall back to scalar (FRS-VEC-RAD-SCALAR); a UI program falls back only if the vector UI FIR cannot be built (FRS-VEC-FALLBACK-UI)".
- `signal_fir/delay/options.rs:15-20,24-27`, `delay/mod.rs:45,70` — *Doc*: "≤ mcd → shift", "> dlt → if-wrapping", range "(mcd, dlt]"; mcd "upper bound (inclusive)", dlt "lower bound (exclusive)". *Code*: `select_delay_kind` (`options.rs:47-61`) does `< mcd` → Shift, `< dlt` → CircularPow2, otherwise IfWrapping (checked). The same file contradicts itself: `:43-45` is right. **Fix**: "< N", "≥ N", "[mcd, dlt)", "exclusive", "inclusive".
- `signal_fir/decoration_verify.rs:18` — `compiler/signals/conditionAnnotation.cpp` does not exist → `compiler/generator/instructions_compiler.cpp::InstructionsCompiler::conditionAnnotation` (l. 391).
- `signal_fir/recursion.rs:74-75` — `generateRecProj`/`generateRec` are not in `signalFIRCompiler.cpp` → `instructions_compiler.cpp:2162/2179/2360`.
- `signal_fir/delay/sizing.rs:90`, `delay/mod.rs:126-127` — `checkDelayInterval` is in `compiler/signals/sigtype.cpp:336`, not in `signalFIRCompiler.cpp`; the Rust equivalent is `sigtype::check_delay_interval`.
- `hgraph/mod.rs:92-94` — the test `barrier_orientation_equals_the_pairwise_reference` does not exist. The real tests are `barrier_orientation_keeps_the_pairwise_orders_with_linear_edges` and `a_chain_of_foreign_recursions_is_ordered_with_linear_edges`; they compare **orders**, not edges.
- `signal_fir/loop_graph.rs:15-17` — *Doc*: `needs_separate_loop` would be consumed only by `pv_slice` and the tests. *Code*: also by the vector planner, in production (`vector/plan/build.rs:11,202`).

**stale**
- `delay/mod.rs:94`, `delay/options.rs:4` — `DelayManager::ensure_delay_line` → `ensure_delay_line_in_context` (`manager.rs:103`).
- `delay/plan.rs:103,162-164,294-295,323-324,358` — `DelayManager::is_recursion_delay_chain`, `analyze_node`, `scan_node`, `analyze_child`, `scan_child`, the `max_delays` map and `record_rec_output_delay_analysis` no longer exist. They were merged into `DelayPlanner` (`node`, `child`, `scan_once`, `record_rec_output`, `is_recursion_delay_chain_static`).
- `placement.rs:16,26,73,76-82`, `cse.rs:6-7` — the `sample_statements` section and the "gate inside `lower_sig`" no longer exist. *Code*: the gate is `place_by_variability` (`module/core_lowering.rs:162`), called by `lower_signal`; per-sample code is in `sample_loops[i]` (`run_bucket_cse`, `module/build.rs:396-464`); the signature is `materialize_in_bucket(sig, lowered, Bucket)`; the Block branch goes through `lazy_block_slot`/`materialize_lazy_block`.
- `block_reverse_ad.rs:22` — `MAX_BRA_TAPE_BLOCK_SIZE` does not exist → `Array(v_ty, self.bra_tape_block_size)`, set by `-bra-tape`, default `DEFAULT_BRA_TAPE_BLOCK_SIZE` = 8192.
- `signal_fir/mod.rs:625-637` (doc of `compile_signals_to_fir_fastlane`) — *Doc*: "lowers one executable bootstrap signal slice"; the "Errors" section is incomplete. *Code*: the function lowers the whole program, vector first in Vector mode. It also fails on a non-power-of-two `-bra-tape`, on FRS-SFIR-0004/0008/0009/0010 and on FRS-UI-0001.
- `error.rs:1-4`, `planner.rs:3,10,26-27` — "experimental fast-lane", "Step 1A… Step 2+": this is the single production route. The runtime error message in `planner.rs` still contains "in Step 1A".
- `siggen.rs:92`, `signal_prepare/mod.rs:99` — "so `tests.rs` can" → "the `tests/` suite".

**imprecise**
- `README.md:11`, `lib.rs:14-15` — pipeline (see T1). The provenance table omits `compiler/DirectedGraph/*`, `signals/clkEnvInference.cpp`, `transform/signalFIRCompiler.*` and `parallelize/graphSorting.cpp`.
- `README.md:101` — the `SignalFirOptions` field list omits `bra_tape_block_size`, `control_rate_mode` (-ec), `processing_api` (-os), `table_init_mode`, `table_init_sample_rate` and `check_table` (-ct).
- `README.md:55`, `mod.rs:30` — "FRS-VEC-FALLBACK-*": the RAD fallback has code `FRS-VEC-RAD-SCALAR`.
- `README.md:103` — "FRS-SFIR-*": `UiDuplicatePath` has code `FRS-UI-0001`.
- `siggen.rs:3-5,12-15` — *Doc*: "Mirrors C++ signal2Container()… instead of emitting runtime init code". *C++ code*: `signal2Container` compiles a sub-container run at init, i.e. the equivalent of `TableInitMode::Runtime`. *Rust code*: this interpreter is the Rust-only `Const` path. "No runtime context" is also inaccurate: `ma.SR` passes through `table_init_sample_rate`.
- `mod.rs:50-53` — the SIGGEN and foreign restrictions only apply under `TableInitMode::Const`.
- `signal_prepare/mod.rs:13-34,65-68`, `rewrites.rs:3-4` — step 2.1b `drop_clock_annotations` is missing; there are three rewrites, not two.
- `delay/mod.rs:28-41` — the "Module layout" omits `domain_counters.rs`.
- `signal_fir/mod.rs:37-49` — the "Module map" omits `leaf_emit`, `one_sample` and `origins`.
- `lib.rs:50-51,54-57` — "model → build → check" only holds for some stages. The codename check (`structure_check`, check 6) only recognises P/R/V/S; B3–B6, "Step 1A" and D2 escape it.
- `leaf_emit.rs:373-383` — `wrapChannel` is not in the pinned reference: it comes from the C++ `rework-soundfile` branch (see DIFF-BEH-016). Name the branch.
- `README.md:68` — "plan §3.2": the quoted rule is in §4.6.

**minor**
- Private rustdoc warnings (scoping issues, existing items): `clk_env/mod.rs:741` (`find_fixpoint_per_group`, under `cfg(test)`), `delay/mod.rs:36,92,107-109,118` (`plan::DelayPlan`, `sizing::…`), `block_reverse_ad.rs:168`, `placement.rs:25` (`SignalToFirLower` is private).
- `mod.rs:123` — `gFLoatSize` → `gFloatSize`.
- `recursion.rs:50` — `PostOutput` → phase `post_output`.
- `schedule/mod.rs:47` — `signal_fir::LoopGraph` → `signal_fir::loop_graph::LoopGraph`.
- `README.md:92` — `signal_fir::{shadow, pv_slice}` are re-exports of `signal_fir::diagnostics::*`.
- `mod.rs:4` — "Entry points" in the plural: only one remains.
- `one_sample.rs:1-2`, `tests/execution.rs:2`, `error.rs:57` — cite `porting/external-control-one-sample-port-plan-2026-07-23-en.md`.

### `signal_fir/vector/`

**wrong**
- `vector/mod.rs:25` (`ui` row of the stage table) — *Doc*: "UI programs currently fall back". *Code*: `build_vector_ui_fir` (`vector/module/build.rs:169`), lowered widgets (`lower/signal.rs:825-860`), `verify_ui_write_attribution` (`module/check.rs:249`). **Fix**: "`VectorUiFir`: zone declarations, `instanceResetUserInterface`, `buildUserInterface` (shared with scalar) | exact coverage checked in the final-module check". Same stale claim at `vector/mod.rs:27`.
- `vector/lower/mod.rs:13` — *Doc*: "UI programs, foreign calls, and reverse AD remain fail-closed". *Code*: UI is lowered; only `FFun` and RAD stay closed.
- `vector/module/check.rs:308-310` — `lower_readonly_table_definition` does not exist. Mutable tables have been lowered since `d1292292` (`lower/signal.rs:1414`, `lower_table_definition`) and checked by `verify_mutable_table_attribution` (`check.rs:64`).
- `vector/verify/error.rs:11` (`UnsupportedSchema`, public) — *Doc*: "exact v2 schema". *Code*: `VECTOR_PLAN_SCHEMA_VERSION = 4` (checked).
- `vector/assemble/mod.rs:13` — `SignalFIRLowerer` does not exist → `SignalToFirLower` (`module/mod.rs:368`).
- `vector/lockstep/mod.rs:5` — `recursivness.cpp` → `compiler/signals/recursiveness.cpp`.

**stale**
- `vector/assemble/mod.rs:19-20` — `vector_module` (former flat file) → `vector::module` (`check::verify_final_module`, `module/check.rs:356`).
- `vector/verify/mod.rs:9-13` — cites `…-v2.schema.json` and "v1 plans are not accepted by the v2 verifier". *Code*: v4, `porting/schemas/vector-verification-certificate-v4.schema.json`. **Fix**: cite v4 and trace v2 (lockstep), v3 (fused-group carriers), v4 (per-signal effects).
- `vector/analysis/mod.rs:13-15` — *Doc*: "without activating either scalar or vector scheduling". *Code*: used in production by the scalar path (`signal_fir/mod.rs:714`, `module/subcontainer_compile.rs:133`, `hgraph/mod.rs:366-379`) and by the vector plan.

**imprecise**
- `vector/mod.rs:55-58` — "R5–R7 split each stage into model/build/check/tests": `lower/` is split into program/signal/tables/check; `schedule/` and `lockstep/` remain single files.
- "§N" references without a plan name: `verify/check.rs:128` (§5.5/§5.10 of the vector-mode plan), `verify/error.rs:8` (§8). **Fix**: prefix the plan name.
- `vector/lower/signal.rs:139-143` (`pub fn lower_pure_vector_program`) — "not yet connected to backend module assembly": production goes through `lower_vector_program` (`module/build.rs:322`); this function is now only the unit-test entry point.
- `vector/plan/mod.rs:5-7` — `DAGInstructionsCompiler::needSeparateLoop`: also cite `VectorCompiler::needSeparateLoop` (`compile_vect.cpp:304`).

**minor**
- `vector/clock_ad/check.rs:9`, `vector/schedule/mod.rs:4` — plan paths without the `porting/` prefix.

### `signal_fir/module/`

**wrong**
- `module/bra.rs:12-14` — *Doc*: "primary RAD dispatcher uses the forward-mode algebraic path; the BRA lowering here is preserved for the LTI adjoint fast-path revival". *Code*: the reverse. Every temporal or recursive `rad` arrives as `SigBlockReverseAD`, and it is `ReverseTimeRec` (LTI) that is dormant (`propagate/src/reverse_ad.rs:63-69`, checked). **Fix**: "since 2026-05-10 the symbolic RAD sweep is feed-forward only; every temporal or recursive `rad` body arrives as `SigBlockReverseAD`, so this is the production path; the `ReverseTimeRec` LTI path is dormant".
- `module/build.rs:1070-1086` (doc of `build_module`, "Promotion invariant"), `module/mod.rs:350` — `signal_prepare::promote_signals_for_fir` is gone (`8b217616`); promotion is step 2.5 of `signal_prepare` (`normalize::promote_signals_fastlane`). `normalized_table_index` was removed (`35fd8a66`); clamping is now done at signal level, step 2.10b.
- `module/core_lowering.rs:880-888` (doc of `lower_delay_state`) — *Doc*: `recursion_feedback_info` and `rec_array[(fIOTA-1)&1]`. *Code*: the call is `resolve_recursion_delay_ref` (l. 906), and the read depends on `RecursionStorageStrategy` (`SingleScalar`/`ExactShift`/`Circular`).
- `module/bra.rs:360,379,1371,1380,1385,1389` — names predating the `BraState` extraction (`df1786a3`): `bra_state_scheduled` → `bra.scheduled`; `bra_grad_cache` → `bra.grad_cache`; `bra_tape_store_var` → `bra.tape_store_var`; `sample_phases.immediate` → `regions.current_phases_mut().immediate`.
- `module/tables.rs:9-10` — `table_size_for_signal` → `table_size_from_sig` (`:420`). Index normalisation moved to `signal_prepare` 2.10b; only `debug_assert_index_checked` remains here.

**stale**
- `module/mod.rs:3-49` — the "executable fast-lane slice" list (sin/cos/…, fixed SIGDELAY, "other families return FRS-SFIR-*") is far behind: acos…remainder, FFun/FConst/FVar, UI, soundfiles, Gen/WrTbl, Attach/Enable/Control, Seq/Clocked/OnDemand/US/DS/TempVar/PermVar/ZeroPad, BRA and variable delays are all handled. **Fix**: rewrite or delete the list.
- `module/region.rs:15-22,30-37` — *Doc*: "exactly one region per sample loop". *Code*: guarded child frames exist in production (`open_child`/`close_child`/`open_child_under`, `set_redirect`, called from `clocked.rs:771-826`).
- `module/ui_lowering.rs:9-14` — *Doc*: `emit_ui_program` would be "the recursive walk", with `addMetaDeclare` emission. *Code*: it delegates the walk and the metadata to `vector::ui::build_ui_statements`.

**imprecise**
- `module/core_lowering.rs:729-743` (doc of `lower_delay`) — "constant integer amount only": contradicted by the rest of the same comment (bounded variable delays), and the strategies are delegated to `delay/`.
- `module/core_lowering.rs:978-987` — the doc of `lower_forward_output_delay1_for_reverse_loop` sits in front of `forward_output_by_structure`. Move it.
- `module/mod.rs:81-87` — `ReverseTimeRec` is presented as active; it is dormant.
- `module/mod.rs:96-99` — "forced… by `PlacementInfo` (see `setup.rs`)": the analysis is in `placement.rs:39-61,181`.
- `module/mod.rs:359-367` — the sub-structure table omits `regions`, `clocked`, `recursion` and `delay`.
- `module/build.rs:6-11` — the lifecycle omits `staticInit` and the `frame`/`control` entries.
- `module/setup.rs:3-5`, `module/state.rs:3-7` — omit `guard_exclusive` and `static_init_statements`.
- `module/tables.rs:6-7` — *Doc*: "`SIGGEN` — write-driven tables". *Code*: `SIGGEN` is the content generator (`fill` sub-module), not a write-driven table.

**minor**
- `module/setup.rs:71` — `instructions_compiler.cpp:1040` points at `generateHSlider`; the `getTypedNames(…,"tbl")` calls are at lines 1947 and 2019.
- `module/clocked.rs:4-5` — "ondemand port §3.8" → `porting/ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md` §3.8; the `sigSeq`/`sigClocked` cases are at lines 844-853.
- `module/build.rs:13-14` — "All other submodules provide `impl SignalToFirLower`" → "Most" (`region.rs`, `subcontainer_compile.rs` and `rad_formula_builder.rs` are exceptions).
- Private rustdoc warnings: `module/mod.rs:19,23` (redundant targets), `:345` (`super::SignalFirOutput`), `setup.rs:130` (`super::build_module`), `ui_lowering.rs:11` (`ui::UiProgram`), `bra.rs:1519` (put `tape[i0]` in backticks).

---

## 5. `fir`

**wrong**
- `fir/src/inliner.rs:2101-2112` — the **public** doc of `sweep_scaffolding_drop_roots` starts with a paragraph about `HygienicCloner`, a private struct (`:2198`) that has no doc. **Fix**: move lines 2101-2105 above `HygienicCloner`.
- `fir/src/checker.rs:14-60` — the code table omits `FIR-M08`, `FIR-M09` (`checker/module_shape.rs:511-569`) and `FIR-SM01`…`SM06` (`table_fill.rs`, `module_shape.rs`), which are emitted.

**stale**
- `fir/src/inliner.rs:1-31` — *Doc*: "inliner scaffolding… before implementing rewriting". *Code*: rewriting exists (`inline_fir_module`, `inline_fir_module_once`) but has no caller outside `fir`. The module also carries three undocumented roles: `sweep_scaffolding_drop_roots[_with_mapping]` (mandatory canonicalisation boundary), the hygienic cloning used by `subcontainer`, and `is_obviously_side_effect_free_value`.

**imprecise**
- `fir/src/schedule/mod.rs:18,108` — *Doc*: "Both keep…", "W3 forwarding, then W2 sinking". *Code*: `schedule_body` applies W3, W2 **and W7** (`coalesce`/`check_reuse`). Same gap in the `keep_fir_order` docs: `interp/mod.rs:92-93`, `wasm/mod.rs:95-96`, `cranelift/core.rs:95-96`. **Fix**: "W3/W2/W7".
- `fir/README.md`, `fir/src/lib.rs:3` — *Doc*: "construction and matching helpers". *Code*: the crate also exposes `schedule`, `inliner`, `subcontainer`, `dump_fir`/`canonical_fir_fingerprint` and `helpers`. The latter lives **outside `src/`** (`crates/fir/helpers.rs`, `#[path = "../helpers.rs"]`, `lib.rs:36-37`) and serves `transform` (`recursion.rs`, `delay/{context,shift}.rs`). **Fix**: add a "Modules" section. Independently of the docs, consider moving the file into `src/`.
- `fir/src/matcher.rs:249,254,264` — "C++ parity: `LoadSoundfileInst`": this instruction does not exist in C++ → `InstructionsCompiler::generateSoundfileLength/Rate/Buffer`.

**minor**
- `fir/src/types.rs:22` — `cargo doc` error (see §1).
- `fir/src/builder.rs:404` — redundant link target.
- `fir/src/matcher.rs:259`, `builder.rs:838`, `codegen/.../interp/compiler/ui_soundfile.rs:217` — `Soundfile::kChannels` → `fChannels` (`Soundfile.h:71`).

---

## 6. `codegen`

**wrong**
- `codegen/src/backends/mod.rs:4-14` — *Doc*: "Implemented: `c`, `codebox`, `cpp`; Scaffolded: `cranelift`, `cmajor`, …, `interp`, `julia`, `rust`, `wasm`"; `asc` missing. *Code*: asc, c, cmajor, codebox, cpp, cranelift, interp, julia, rust and wasm are complete (2,000 to 10,000 lines each); only csharp, dlang, jax, jsfx, llvm, sdf3 and vhdl are 17-line scaffolds (checked). **Fix**: rewrite both lists; add the `codegen_error` and `host_function` modules. The `readme_consistency_tests` test could cover this doc too.
- `codegen/src/lib.rs:15-23` — same stale status. "Public surface" omits `backend_error`, `compute_cost`, `json`, `memory_layout` and `shortname`. "Emits target-language source text": backends also produce FBC bytecode, wasm binaries and JIT code.
- `codegen/src/backends/mod.rs:70` — `backends::emits_no_lifecycle_leak` does not exist → `crates/compiler/tests/lifecycle_leak_guard.rs::no_backend_emits_static_init_as_an_ordinary_function`.
- `codegen/README.md:267,397-400,441-444` — the literals `InterpOptions { opt_level: 4, module_name: None }`, `JuliaOptions {…}` and `RustOptions {…}` do not compile: fields are missing (`compile_options`, `keep_fir_order`, `metadata_*`, `dsp_json`). **Fix**: add `..Default::default()` and complete the tables (l. 287, 418, 450).
- `codegen/README.md:312` — *Doc*: "All 298 FBC opcodes are covered". *Code*: `ForeignCall{Real,Int,Void}` and `LoadSoundField{Int,Real}` return `FbcCppError::Unsupported`; `AddSoundfile` only emits a comment.
- `codegen/README.md:520-526` — *Doc*: WASM layout "static tables first, mutable fields next…". *Code*: since W5 (`e2d758b9`), `FieldOrder::SmallFirst` places the `Soundfile*` fields, then mutable fields by increasing size, then static tables, then local arrays (`layout.rs:19-21`). The "tables first" order is `FieldOrder::Declaration`, reserved for `-json`.
- `codegen/README.md:168,198` — codes `FRS-CGEN-C/CPP-0001..0003` → `0001..0004` (`MemoryLayout`).
- `codegen/README.md:32` — `compiler/generator/asc/` does not exist → `compiler/generator/assemblyscript/`, absent from the pinned reference (only on `master-dev`). Add the same note in `asc/mod.rs:4-6`.
- `codegen/src/backends/cmajor/mod.rs:43-44` — `CmajorTableTypeVisitor`/`CmajorTableVisitor` do not exist → `TableSizeVisitor`/`TableSizeCloneVisitor` (`cmajor_code_container.hh:35,53`).
- `codegen/src/backends/interp/soundfile.rs:4,19` — `architecture/faust/sound_player.h` does not exist → `architecture/faust/gui/Soundfile.h:66`.
- `codegen/README.md:259` — `FbcDspFactory::optimize(level)` → `optimize()`, no argument (`factory.rs:144`).

**stale**
- `codegen/src/json.rs:11-12` — "a future global `-json` CLI path": it exists (`--json`/`-json` → `build_json_description_from_fir`, `cli/runner.rs:580`), also used by the FFI crates.
- `codegen/src/backends/cranelift/mod.rs:4-5` — "Planned native-code backend": it is in production.
- `codegen/src/backends/interp/mod.rs:13-30` — "Step 5 complete… Steps 1–4"; the module layout omits `foreign` and `soundfile`.
- `codegen/src/backend_error.rs:3-5` — "five of them expose `code()`/`message()`" → eight.

**imprecise**
- `codegen/README.md:13` — pipeline (see T1).
- `codegen/README.md:288` — `Result<FbcDspFactory<f32>,…>`: the function is generic, `FbcDspFactory<R: FbcReal>`.
- `codegen/README.md:225,287,533` — the Cranelift, Interp and Wasm option tables omit `keep_fir_order`, the option that disables `fir::schedule`.
- `asc/mod.rs:18-19` — "UI / soundfile nodes are emitted as comments": only `AddSoundfile` is; reads call the `_soundfile*` host imports.
- `codegen/README.md:635-636` — "All backends" would use the 8 fixtures; asc, codebox and cmajor do not.
- `interp/fbc_to_cpp.rs:1-52` — missing C++ provenance: `FBCCPPCompiler`, in `compiler/generator/interpreter/fbc_cpp_compiler.hh:107`.
- `codegen/README.md:316-322` — the `If(b1,b2) → if/else` rule omits the two special cases documented in the module (branches reduced to `Return` omitted, inverted condition).
- `c/mod.rs:967` — `CodeContainer::generateMemoryMethods` does not exist in the reference: the C surface is a Rust extension (see `cpp_code_container.cpp:754-830`).

**minor**
- `wasm/layout.rs:131-133` — `cargo doc` errors (see §1).
- `cranelift/math.rs:5` — `[HOST_MATH]` → `(super::host::HOST_MATH)`; `cranelift/lowering.rs:1538`: redundant target.
- `interp/soundfile.rs:11-14` — a line starting with `>= 1` is read as a Markdown blockquote, which breaks the code span.
- `c_family.rs:27` — absolute path `/Users/letz/faust/compiler/generator/Text.hh` → `compiler/generator/Text.hh` (AGENTS.md §4).
- `interp/compiler.rs:17` — `math_lib_lookup` is not a `const fn`.
- `rust/mod.rs:252`, `julia/mod.rs:232` — "transform fast lane" → "transform's signal→FIR lowering".
- `rust/mod.rs:14-20` — the contract header matches AGENTS.md; validation through `faust2jackrust -source` could be mentioned.

---

## 7. `compiler`

**wrong**
- `compiler/tests/closed_box_memo.rs:3` — `propagate::flat::mentions_slot` never existed → `flat::free_slots` + `SlotEnv::restricted_id` (`6f06e0ab`).
- `compiler/src/error_mapping.rs:3-6` — *Doc*: "three-variant `LowerError<E>` (Transform/Verify/Codegen)" for C++, C, Julia, interp and FIR. *Code*: four variants, including `ExecutionOptions`; ten backend converters plus `wasm_error_to_compiler`.
- `compiler/src/signal_lowering.rs:6-10,26` — *Doc*: "three-stage"; closed list of `lower_signals_to_*`; "Specialised as LowerToCppError and LowerToCError". *Code*: four stages; `asc`, `cmajor`, `codebox`, `rust` and `cranelift_report` also exist; there are 7 type aliases.

**stale**
- `compiler/src/enrobage.rs:7-9` — "Step B… added in subsequent steps": `open_arch_stream`, `fopen_search`, `stream_copy_*` and `wrap_cpp_with_(remote_)architecture` are implemented.
- `compiler/src/lib.rs:9-11` — "Exposes minimal compile-session APIs": the crate exposes more than 200 public functions.
- `compiler/README.md:150-166` — ten bullets "now default to TransformFastLane": `SignalFirLane` has a single variant. **Fix**: one sentence.

**imprecise**
- `compiler/src/lib.rs:20-25` — the emitter list omits Codebox; "file-based" only, whereas the crate also handles sources.
- `compiler/src/lib.rs:14`, `README.md:170-172` — pipeline (see T1).
- `compiler/src/paths.rs:4-5` — *Doc*: order "(current dir, FAUST_LIB_PATH, exe-relative, system)". *Code* (function doc, l. 169-178): `FAUST_LIB_PATH`, executable-relative `../share/faust`, `/usr/local/share/faust`, `/usr/share/faust`, then the source directory; the current directory is handled earlier, by `parser::import_candidates`.
- `compiler/README.md:13-21` — the table omits the public modules `diagnostics_human`, `diagnostics_json`, `expand` and `execution`.
- `compiler/README.md:71-72` — *Doc*: "`-it` … fail closed". *Code*: `-it` is not a faust-rs option (`faust-rs -it …` → `unexpected argument '-t'`); only the FFI argv parser rejects `-mem0`+`-it`.
- `compiler/tests/fixtures/enrobage/corpus/README.md:3-10` — the `<<includeIntrinsic>>`/`<<includeclass>>` sentinels come from `libcode.cpp:196-197/910-911`, not `enrobage.cpp`. `wrapper_wrapped.expected.jl` is not mentioned; there is no Cmajor expected file (the assertion is inline).

**minor**
- `tests/check_table.rs:14` — `tests/lean/table_unclamped.dsp` was never committed → point to `porting/table-clamp-signal-promotion-plan-2026-08-28-en.md`.
- `tests/clocked_waveform_regression.rs:4` — `examples/ondemand/gamme.dsp` is absent → "local example, not committed".
- `tests/rad_runtime.rs:1273` — `docs/fad-...md` → `docs/fad-note-en.md`.
- `README.md:52-65` — methods missing from the table: `compile_file_to_boxes`, `compile_parsed_to_signals`, `compile_source_to_signals_with_search_paths`, `…_with_lane_and_search_paths`, `compile_fixture_to_*`. Also `diagnostic_enrichment.rs:6`: `eval_source_labels` → `maybe_add_eval_source_labels`.

## 8. `faust`

**minor**
- `faust/src/factory.rs:45,67,121` — redundant targets `(crate::ErrorKind::…)` → `[`ErrorKind::Compile`]`.
- `crates/faust/` has no README, although it is the supported Rust API (root README, l. 217). The `lib.rs` `//!` is accurate.

## 9. C/C++ FFI crates

### ffi-common

**wrong**
- `ffi-common/README.md:74` — *Doc*: `FfiCompileArgs` reads `-I -cn -double -vec -vs -lv -ss`. *Code*: only `-I`, `-cn`, the four `-mem0` aliases and `--warn` (`args.rs:5-36`); program options are read by `compiler::CompileOptionArgs::from_argv`.

**imprecise**
- `ffi-common/README.md:5-7`, `lib.rs:3-5` — the consumers omit `faust` and `libfaust-ffi`; the table omits `control_range` and `sha1`; the `abi` row omits `FaustMemoryManager*`/`FAUST_MEMORY_MANAGER_ABI_VERSION`/`faust-memory-manager.h`.
- `ffi-common/src/complete_error.rs:9-10,37` — the `getCComplete<Backend>DSPFactoryError` pattern only holds for the interpreter and Cranelift. **Fix**: point to the table in `faust-ffi/README.md:29-35`, which is accurate.

### tree-ffi

**imprecise**
- `tree-ffi/README.md:5-12` — the scope omits the Signal control registry (`FfiSignalControl[Kind]`, `register_signal_control`) and the global context (`global_context`, `with_global_context`, `reset_global_context`).

### box-ffi

**imprecise**
- `box-ffi/README.md:21,67-68` — the headers "mirror" `libfaust-box.h`: partial coverage and `boxAddAux` names (see code defect no. 6). The shared symbols exported here (`createLibContext`, `destroyLibContext`, `CprintBox`, `CprintSignal`, `Ctree2str`, `CisNil`, `CgetUserData`) are not listed.
- `box-ffi/README.md:37-38` — `freeCMemory` contract on arrays (see code defect no. 1).

**minor**
- `box-ffi/README.md:78-79` — "unified `libfaust` distribution" → `libfaust-rs`.

### signal-ffi

**wrong**
- `signal-ffi/README.md:24-26` — *Doc*: `createLibContext`/`destroyLibContext` would be "exported from `tree-ffi`". *Code*: `tree-ffi` exports no C symbol (its README and `lib.rs:6` say so); these symbols are `no_mangle` items of `box-ffi/src/lib.rs:530-640`, and `signal-ffi` depends on `box-ffi`. **Fix**: "the context state lives in `tree_ffi::FfiTreeContext`; the shared C entry points are exported by `box-ffi`, a dependency of this crate".
- `signal-ffi/README.md:26-28` — `freeCMemory` for `Signal*` arrays (see code defect no. 1).

**minor**
- `signal-ffi/README.md:65-66` — "`libfaust`" → `libfaust-rs`.

### libfaust-ffi

**imprecise**
- `libfaust-ffi/README.md:3-4`, `lib.rs:1-2` — the quoted names are those of the C++ wrappers. The C symbols are `generateCSHA1`, `expandCDSPFrom{File,String}` and `generateCAuxFilesFrom{File,String}[2]`. There are two port-specific additions, not one: `getCCompleteDSPError` and `getCDSPErrorDiagnostics`.
- `libfaust-ffi/README.md:23-24` — our `libfaust-c.h` does not declare `freeCMemory`, while the reference does (`libfaust-c.h:145`). Unrecorded difference.

**minor**
- `libfaust-ffi/src/lib.rs:17-18` — "faust-ffi… with no Rust of its own" → "no API of its own" (the crate holds the mimalloc allocator and re-exports).

### faust-ffi

**stale**
- `faust-ffi/src/lib.rs:4-6` — the list of linked crates omits `libfaust-ffi`, which is linked (`pub use libfaust_ffi as libfaust`, l. 34).

---

## 10. FFI runtimes and tooling

### interp-ffi

**wrong**
- `interp-ffi/src/lib.rs:6` — *Doc*: `interpreter-dsp-c.h` "generated by cbindgen + `build.rs`". *Code*: generation is disabled and the header is maintained by hand (`build.rs:3-7`, dependency commented out, README l. 44-47).
- `interp-ffi/src/factory.rs:12` — *Doc*: `createCInterpreterDSPFactoryFromSignals/Boxes` "return `null`". *Code*: these functions are neither defined nor declared (the C++ reference declares them, `interpreter-dsp-c.h:121,138`). **Fix**: "not exported (deferred)", and add a registry entry if there is none yet.

**stale**
- `interp-ffi/README.md:76-77` — wrong `ffi-common` option list (same finding as `ffi-common`).

**imprecise**
- README ("Internal structure") — omits `src/json.rs`.
- `cbindgen.toml` — inert configuration whose `autogen_warning` announces "Auto-generated by cbindgen". Add a note or delete the file.

**minor**
- `src/types.rs:573` — `[`free_c_string`]` → `[`ffi_common::free_c_string`]`.

### cranelift-ffi

**stale**
- `README.md:101-102` — same wrong `ffi-common` option list.
- `README.md:82,245-246` — *Doc*: "temporary bitcode family", `CRANELIFT_FFI_V2_SOURCE` payload. *Code*: writing produces a `FAUST_CLIF_V1` container (`clif.rs:10`, `factory.rs:717,781`); V2 is only accepted on read.
- `src/lib.rs:18,26`, `Cargo.toml:30` — "# Planned modules" although all exist; `probe` is missing; "Future FFI implementation will require raw pointers" although `unsafe` is already in use.

**imprecise**
- `README.md:42-55,72-88` — omits `src/probe/`, `src/bin/` (`faustprobe`, `impulse-cranelift`, `mem0-json-check`) and `src/diff.rs`; the "What `faustprobe` prints" section comes without any introduction of the tool.
- `src/probe/mod.rs:34-50` ("# Layout") — omits `audio_file` and `train`.
- `src/probe/engine/mod.rs:3` — "the only module in the crate that touches FFI": false, see `probe/params.rs:365-430`. **Fix**: "the only `probe` module that owns FFI handles".
- `src/bin/faustprobe/main.rs:3` — "See the crate documentation": the rationale is actually in the `cranelift_ffi::probe` module doc (`probe/mod.rs:13-28`).
- `README.md:57-62` — the "Headers" section omits `include/faust-memory-manager.h`.

**minor**
- `README.md:26` — `codegen::cranelift` → `codegen::backends::cranelift`.
- `Cargo.toml:37` — "Both binaries": there are three `[[bin]]`.
- `src/types.rs:241` — redundant link target.
- `src/bin/impulse_cranelift.rs:8` — the Usage line omits `--json-output`, `--opt-level` and the flattened options.

### wasm-ffi

**stale**
- `README.md:203` — *Doc*: `expandDSP` = "validation + source passthrough". *Code*: real expansion via `Compiler::expand_dsp`; passthrough only applies to sources that start with `declare compile_options`.

**imprecise**
- `lib.rs:35` — "future `faustwasm` loader": the README cites existing consumers.
- README ("Raw ABI usage") — omits `faust_wasm_version_ptr/len` and `faust_wasm_generate_aux_files` (27 exports in total).

**minor**
- `README.md:202` — `\\|` in a table cell → `\|`.

### impulse-runner

**wrong**
- `README.md:54` — *Doc*: "64 MiB" stack. *Code*: `stack_size(512 * 1024 * 1024)` (`src/main.rs:74`, checked).

**imprecise**
- `README.md:24-35` — the option table omits the flattened compile options (`-pn`, `-mcd`, `-dlt`, `-check-table`, `-bra-tape`…) and the explicit rejection of `-mem0`.

### xtask

**wrong**
- `xtask/README.md:254`, `src/main.rs:35` — *Doc*: "Fail-fast golden checks". *Code*: `golden_check` (`runtime_trace.rs:1794-1836`) prints every failing case and fails at the end; only `interp-trace-check` stops at the first mismatch.
- `xtask/src/diagnostics_quality_check.rs:53,62` — `human.rs` / `crates/compiler/src/cli/human.rs` → `diagnostics_human.rs` (see code defect no. 2).
- AGENTS.md §9bis and the code disagree on tolerance and spread (see code defect no. 7).

**stale**
- `xtask/README.md:232`, `cli.rs` (`CompileBudgetArgs::update`), `compile_budget.rs:551` — *Doc*: "`--update` re-records the front-end units". *Code*: `write_updated_baseline` also rewrites `scalar_units_milli`/`vector_units_milli` of the `codegen_cases`.
- `xtask/README.md:258-260` — "the compiler and two impulse runners": `faustprobe` is the fourth normalized entry point.

**imprecise**
- `README.md:47-91` — the "Commands" table omits `diagnostics-quality-check`, `lexer-differential`, `expand-oracle` and the `vector-compile-budget-check` alias.
- `README.md:557-592` ("Code Graphs") — mentions neither `--check` nor `public-api-baseline.txt` (12 generated artifacts, not 11), nor the `--check` exemptions (`public-api-index.md`, `.svg`).
- `README.md:241-248` — `FAUST_CPP_BIN` is read by other commands too; `FAUST_ARCH_DIR` and `GITHUB_ACTIONS` are not listed.
- `README.md:81` — "normalized front-end cost": codegen is normalized too (schema 3).
- `structure_check.rs:24-25` — rule 4 omits `signal` from `PRODUCER_SIBLING_MODULES`.
- `cli.rs` (`ExamplesCompareArgs::repeats`) — "default 3": 1 under `--per-symbol`.

**minor**
- `README.md:178-180` — the example writes the file to `/tmp/` and then merges `/tmp/vector-reports`: it should write to `/tmp/vector-reports/`.

---

## 11. Verified consistent (summary)

- The five structure gates (`cli-parser-check`, `error-model-check`, `ffi-boundary-check`, `structure-check`, `code-graphs --check`) pass on HEAD.
- Files deleted or renamed since May are no longer cited by path in the docs, with two exceptions: `vector_module` (assemble) and `cli/human.rs` (xtask). The `pv_slice`/`shadow` mentions stay valid thanks to the re-exports.
- Every cited `faustprobe` option exists, as do the `xtask` subcommands and options cited in the README and in AGENTS. No trace of the vector/scalar ratio check remains.
- The Rust backend contract (`F32`/`F64`/`FaustFloat`, `ParamIndex`, `FaustDsp`) matches AGENTS.md.
- No doc reintroduces public `box_*`/`is_box_*`. `BoxBuilder`/`match_box` and `SigBuilder`/`match_sig` are presented as canonical.
- The C++ provenance of `draw`, `sigtype`, `tlib`, `schedule`, `clk_env`, `parser`, most of `eval`/`propagate` and `libfaust-ffi` is accurate. Every cited `porting/*.md` and `docs/*.md` document exists.
- False positives of the automatic scan: example names (`my.dsp`, `file.h`, `mydsp.wat`…), system headers, files from sibling repositories (faust-diff-fdn, faust-coopiano), `foo.hh/.cpp` notation.

---

## 12. Proposed fixes (nothing applied, pending approval)

Proposed split into local commits, no push:

1. **Doc CI**: the 5 `cargo doc -D warnings` errors. Five lines, which would then allow adding `cargo doc` to CI.
2. **Wrong, crate by crate**: about 70 items. One commit per group: foundation, front end, transform, fir/codegen, compiler/FFI, tooling. C++ provenance included.
3. **Stale and imprecise**: API READMEs (`eval`, `parser`, `propagate`, `transform`, `ui`), backend statuses, pipeline diagrams aligned on AGENTS.md (T1), `graph`/`doc`/`algebra`.
4. **Minor**: private rustdoc links, typos, absolute paths.
5. **Outside the docs, to be decided separately**:
   - `freeCMemory` (no. 1): fix plus a C client test;
   - `diagnostics-quality-check` allowlist (no. 2);
   - `EvalStats` counters (no. 3);
   - FAD/RAD bargraph divergence (no. 4);
   - MEMOIZATION.md and difference-registry entries (no. 5, 6);
   - AGENTS.md §9bis, 25% versus 40% (no. 7): decide which value is authoritative;
   - plan update §2/§4/§5 (T3).
