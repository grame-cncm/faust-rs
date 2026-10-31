# `faust-rs` Differences From Faust C++

Status: living compatibility registry

Last reviewed: 2026-10-30 (`--color` for human diagnostics, `faust-rs` and `faustprobe`: DIFF-CLI-015 recorded); 2026-10-26 (C/C++ helpers guarded as in C++, `faust_wrap_*` under `FAUST_INT_WRAP` with the C++ ones as upstream's, C `faustmaxi`/`faustmini` under `FAUSTMAXI`: DIFF-BEH-028 recorded; one bargraph per bargraph signal: DIFF-BEH-027 recorded, the merged meters of `vumeter.dsp` an unrecorded difference closed; control characters of string literals: C++ `Symbol::get` turns every newline, tab and carriage return of a label, tooltip, `declare` value or foreign-function string into a space, and the parser now does the same, an unrecorded difference closed, test `crates/compiler/tests/string_literal_control_chars.rs`; `-uim` user-interface macros ported for C and C++: DIFF-CLI-014 recorded); 2026-10-18 (codegen factorization, [analysis §9](codegen-factorization-analysis-2026-10-18-en.md#9-outcome-2026-10-18): JSON widget `address` built as C++ `PathBuilder::buildPath` builds it, an unrecorded difference closed; codebox `min`/`max` and cmajor/codebox `>>>` now as upstream; DIFF-BACK-006 extended to rust, julia and asc; DIFF-BEH-005 backend module-shape diagnostics unified; the c, cpp, rust and cmajor banners copy upstream's `author`/`copyright`/`license`/`name`/`version` keys and the rust `metadata()` replays the compilation metadata, unrecorded differences closed); 2026-10-18 (reference spelling of the applied Box constructors: DIFF-API-009 narrowed; RAD through bargraphs: DIFF-SRC-002 gap closed; partial C++ Box/Signal headers: DIFF-API-009 recorded; `freeCMemory` missing from `libfaust-c.h`: DIFF-API-010 recorded, then closed (the header now declares it); `-mdoc` not ported: DIFF-GAP-020 recorded); 2026-10-17 (block-rate work in a boolean `ondemand`: DIFF-BEH-024 recorded; `pow(x, 1)` of an init-local constant: DIFF-BEH-022 updated); 2026-10-16 (shared pattern matchers: DIFF-GAP-018 recorded, then closed; isomorphic recursion merging: DIFF-BEH-021 recorded; small real powers: DIFF-BEH-022 recorded; late state join in sums: DIFF-BEH-023 recorded; reserved-word, unary-minus and arity-location diagnostics added to DIFF-SRC-001/002 and DIFF-BEH-005); full review 2026-08-13

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`

## 1. Purpose and scope

Semantic parity with Faust C++ remains the default objective of `faust-rs`.
This file records the places where the port deliberately does something else,
adds a Rust-only surface, exposes an adapted contract, or has a known narrower
scope. It is the first document to consult before assuming that a difference is
a regression.

The registry covers externally observable and compatibility-relevant
differences:

- Faust source accepted only by one compiler;
- command-line options, modes, defaults, and validation;
- generated-code, initialization, scheduling, and diagnostic behavior;
- backends and delivery forms that exist only on one side;
- public Rust/C/C++ API adaptations;
- durable exclusions and known coverage gaps.

It does not enumerate ordinary implementation differences that preserve the
same contract, such as Rust ownership, arena indices in place of pointers, or
private helper decomposition. Those belong in the relevant phase documents.
It also does not turn a prototype or an unmerged plan into a supported feature:
an item is listed as implemented only when it is reachable in the current
tree and covered by tests or an explicit contract document.

Detailed support breadth remains in
[`faust-rs-supported-faust-subset-en.md`](faust-rs-supported-faust-subset-en.md).
Backend-specific ABI detail remains in the corresponding parity matrices. This
file is the consolidated index of differences, not a replacement for those
documents.

## 2. Classification

| Status | Meaning |
|---|---|
| `extension` | `faust-rs` intentionally adds a source, CLI, backend, diagnostic, or API capability absent from the pinned C++ reference. |
| `adapted` | Both sides expose the same broad capability, but the accepted inputs, output shape, ownership, validation, or observability contract differs. |
| `narrower` | Faust C++ supports a broader production surface; Rust rejects or defers part of it. |
| `excluded` | The difference is frozen as a non-goal unless a planning decision explicitly reopens it. |
| `reference-fix` | Rust intentionally avoids a known behavior or defect observed in the pinned C++ implementation. |
| `parity-gap` | The current Rust behavior differs observably from C++ without being an intended extension; it remains a defect to close. |

These labels are compatibility statements, not quality rankings. In
particular, `adapted` must not be presented as `1:1`, and an independently
verified Rust extension must not be presented as a proof of C++ parity.

## 3. Faust source-language extensions

### DIFF-SRC-001 — explicit-seed forward automatic differentiation

- Status: `extension`.
- Rust surface: `fad(expr, seeds)`.
- Difference: the pinned C++ reference does not provide this explicit-seed
  source contract. Rust accepts one or more seed outputs and returns primal and
  tangent lanes. Multi-seed recursion is carried through one augmented
  recursive group rather than rebuilding a primal shadow per seed.
- Additional adapted behavior: differentiating a read-only table index uses a
  symmetric finite-difference slope. This is a documented derivative model,
  not C++ semantic parity.
- Implemented correction (2026-10-24, shared with `rad`): the derivative of
  `abs(x)` is `sign(x)` with `sign(0) = 0`, the value of a central difference.
  It was `x / abs(x)`, `0 / 0` at a value that is 0 at run time, which made
  every tangent `NaN` whatever the seed (the faust-elements string models, a
  silent delay line). Pinned by `abs_at_zero_has_a_zero_derivative_in_fad_and_rad`
  in `crates/compiler/tests/rad_runtime.rs`.
- Implemented correction at `f18a50c8`: bargraphs forward the wrapped signal's
  tangents while retaining the meter on the rebuilt primal; see the
  [2026-09-19 review](fad-clock-domains-explanation-and-review-2026-09-19-en.md).
- Additional adapted behavior (shared with `rad`): a widget referenced from a
  seed is one control in every group context (C++ makes the group path part
  of a widget's identity, and so does `faust-rs` outside seeds, see
  `replicated_widgets_wire_distinct_dsp_fields`), and that control owns one
  UI leaf, at the body's group path; a seed the body never reads is placed at
  the root. Guarded by `ad_seed_references_unify_to_one_control` and
  `ad_seed_reference_is_placed_once_at_its_body_group_path` in
  `crates/compiler/src/tests.rs`, described in
  [`docs/fad-note-en.md`](../docs/fad-note-en.md) §2.1.
- Compatibility impact: DSPs using this form are `faust-rs` programs and must
  not be expected to compile with the pinned C++ compiler. `fad` is a
  reserved word of the faust-rs grammar, where both C++ compilers accept it
  as an identifier: a C++ program naming something `fad` fails to parse, and
  since 2026-10-16 the parse error says so and proposes a rename
  (`crates/compiler/tests/diagnostic_hints.rs`).
- Evidence:
  [`docs/fad-rad-synthesis-en.md`](../docs/fad-rad-synthesis-en.md),
  [`faust-rs-supported-faust-subset-en.md`](faust-rs-supported-faust-subset-en.md),
  and the `tests/corpus/fad_*.dsp` fixtures.

### DIFF-SRC-002 — explicit-seed reverse automatic differentiation

- Status: `extension`.
- Rust surface: `rad(expr, seeds)`.
- Difference: Rust returns the primal outputs followed by one gradient lane per
  seed, using an implicit all-ones cotangent for a multi-output body. Temporal
  and recursive bodies use the finite-horizon `BlockReverseAD` fallback; the
  specialized reverse-time recursion route remains disabled.
- Implemented correction (2026-10-18): a bargraph is the identity on its
  signal under `rad` too. The symbolic sweep gave it a zero adjoint, and the
  `BlockReverseAD` lowering of temporal and recursive bodies rejected it
  (`FRS-SFIR-0004`); both now pass the adjoint to the metered signal and do
  not replay the meter, as FAD has done since `f18a50c8`. Pinned by
  `rad_adjoint_passes_through_a_bargraph` (finite difference, symbolic) and
  `rad_adjoint_passes_through_a_bargraph_in_a_recursive_body` (block-total
  finite difference, `BlockReverseAD`) in `crates/compiler/tests/rad_runtime.rs`.
  The gap was first recorded in the
  [2026-09-19 review, §10](fad-clock-domains-explanation-and-review-2026-09-19-en.md#10-what-these-results-do-not-establish).
- Implemented correction (2026-10-24): the `abs` rule is the sign, 0 at 0, as
  under `fad` (DIFF-SRC-001), in the symbolic sweep and in `BlockReverseAD`
  (one shared formula, `signals::ad_rules`).
- Compatibility impact: the source form and `BlockReverseAD` semantics have no
  pinned C++ source/backend oracle. Validation uses symbolic identities,
  FAD/RAD agreement, finite differences, and optimized/unoptimized runtime
  comparisons. `rad` is a reserved word of the faust-rs grammar, where both C++ compilers
  accept it as an identifier: a C++ program naming something `rad` (an angle,
  a radius) fails to parse, and since 2026-10-16 the parse error says so and
  proposes a rename (`crates/compiler/tests/diagnostic_hints.rs`).
- Evidence: [`docs/rad-note-en.md`](../docs/rad-note-en.md),
  [`docs/rad-usage-en.md`](../docs/rad-usage-en.md), and
  `crates/compiler/tests/rad_runtime.rs`.

### DIFF-SRC-003 — AD across clock-domain constructs

- Status: `extension`.
- Difference: Rust defines and checks FAD augmentation across supported
  `ondemand`, upsampling, and downsampling boundaries. This is a composition of
  Rust AD with clock-domain machinery, not a parity claim against C++.
  Implemented at `f18a50c8`: an augmented block owns a fresh domain (including
  nested state), shared by its primal and tangents; a separately consumed
  original retains its own domain. Clock decisions are not differentiated.
  A surviving original-domain annotation is reported as the compiler defect
  `FadTwinKeepsOriginalDomain`. Reconstruction remains incomplete for a nested
  `upsampling` factor carrying a twinned outer-domain annotation: `ZeroPad`
  retains the original factor, and the invariant check rejects the program.
- Compatibility impact: portable Faust code should not depend on these AD
  combinations unless it explicitly targets `faust-rs`.
- Evidence:
  [`ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md`](ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md),
  [implementation explanation and review, 2026-09-19](fad-clock-domains-explanation-and-review-2026-09-19-en.md),
  [`ondemand_pipeline.rs`](../crates/compiler/tests/ondemand_pipeline.rs),
  [`fad_recursive_runtime.rs`](../crates/compiler/tests/fad_recursive_runtime.rs).

### DIFF-SRC-004 — control inputs as boxes and the wildcard modulation target

- Status: `extension`.
- Rust surface: `cinputs(e)`, `cinput(i, e)`, `coutputs(e)`, `coutput(i, e)`
  (new keywords), and the modulation target `"*"` / `"group/*"`.
- Difference: the four primitives fold at evaluation to lists of `e`'s widget
  boxes (`cinputs`: the `par` of its sliders, numentries, buttons and
  checkboxes; `cinput`: `(widget, init, min, max, step)`; the bargraph twins
  `(bargraph, min, max)`), in the order of `e`'s own interface, dead widgets included (groups merged,
  children sorted by raw label, as the C++ UI is). A wildcard target rebinds
  every matched control input, bargraphs excepted, and gives a two-input
  modulator **one extra input per control** in that order, where a literal
  label shares one input between its matches (kept, as in C++). A wildcard
  matching nothing is the error `FRS-EVAL-0010`; a literal target matching
  nothing stays the C++ warning and dangling input (`FRS-EVAL-0008`). An index
  that is not a compile-time integer, negative or past the count is
  `FRS-EVAL-0009`.
- Compatibility impact: the four names become reserved words, so a program
  that defines `cinputs`, `cinput`, `coutputs` or `coutput` no longer parses.
  The C++ compiler rejects the primitives as unknown identifiers and parses a
  wildcard target as a label that matches nothing (a dangling input and a
  warning under `-wall`), so a program using them is a `faust-rs` program.
  `optimizers.lib` 0.11.0 (`adaptive_fad`, `adaptive_rad`) and the
  Rust-only library `controls.lib` 0.2.0 (smoothing, CV inputs, rebuilt
  interfaces, morphing, randomizing, sweeps, gradients over every control of
  a program) depend on them.
- Evidence: [`docs/control-inputs-en.md`](../docs/control-inputs-en.md),
  [analysis and contract](control-inputs-and-wildcard-modulation-analysis-2026-09-22-en.md),
  [`control_inputs.rs`](../crates/compiler/tests/control_inputs.rs) on the
  `tests/corpus/cinputs_*.dsp`, `wildcard_*.dsp` and `err_3{0,1}_*.dsp`
  fixtures,
  `adaptive_operators_follow_the_hand_written_loop` in
  [`optimizers_lib.rs`](../crates/compiler/tests/optimizers_lib.rs),
  [`controls_lib.rs`](../crates/compiler/tests/controls_lib.rs) on the
  `tests/corpus/ctl_*.dsp` fixtures.

## 4. Command-line additions and differences

### 4.1 Rust-only code-generation options

| ID | Rust option | Status | Difference and compatibility impact |
|---|---|---|---|
| DIFF-CLI-001 | `--table-init runtime|const` | `extension` | C++ behavior corresponds to `runtime`: every generated table is filled by a sub-container during initialization. Rust additionally keeps `const`, which evaluates the generator during compilation and emits literal table contents. The default is `runtime`. |
| DIFF-CLI-002 | `--table-init-sample-rate HZ` | `extension` | Required when `--table-init const` folds a generator that reads `ma.SR`. The positive integer is embedded permanently in the table. There is no implicit default. Under `--warn`, `FRS-COMP-0006` reports the frozen value and suggests `runtime` when the host SR must remain authoritative. |
| DIFF-CLI-009 | `-e` / `--export-dsp` | `1:1` | Expands a DSP into a self-contained program, as C++ `-e`. `-lang` is accepted and recorded in `compile_options`, matching C++, since it selects a backend the expansion does not use; two emitters (`-e --dump-cpp`) still conflict. With no `-o` faust-rs prints to standard output, where C++ produces nothing; see `DIFF-BEH-006` for the full list of deviations. |
| DIFF-CLI-003 | `--dlt N` | `extension` | Selects the delay-line threshold at which Rust changes from power-of-two circular storage to exact-size if-wrapped storage. The compiler records it in `compile_options`; the pinned C++ CLI has no direct counterpart. |
| DIFF-CLI-010 | `-mem` / `-mem0` / long aliases | `adapted` | The four C++ mode-zero spellings normalize to one typed per-request `MemoryManagerMode::Mem0` and canonical `-mem0` metadata. The option is intentionally limited to scalar C, C++, and native Cranelift; C and Cranelift are Rust extensions, while `mem1`–`mem3`, vector mode, `-it`, and other backends fail closed. M1 implements parsing/propagation; M2 implements the shared version-2 layout/cost analysis. M3 implements generated C++ allocation with the legacy `dsp_memory_manager` surface plus additive checked methods. It intentionally fixes the reference's empty `init`, shallow clone, wrong-manager destruction, unchecked failure/alignment, static-table sentinel, and non-transactional cleanup defects; the emitted `Int64`/`Bool` enum extension requires the faust-rs-compatible manager header when those types occur. M4 implements the Rust-only C surface through the versioned, context-carrying `faust_memory_manager` ABI in `ffi-common`, with self-contained header emission, checked class operations, captured per-instance managers, explicit alignment, and transactional instance allocation. M5 implements Cranelift pointer-slot lowering, managed object/instance/class storage, deep clone, unbound-factory failure, copied callback binding, and functional C/C++ factory set/get. The Rust wrapper remains cache-allocated outside the manager-visible logical JIT state, and the legacy C++ adapter maps the appended `Int64`/`Bool` categories to its generic object categories because upstream `dsp_memory_manager::MemType` has no such values. M6 emits the additive version-2 `memory_manager`, `memory_layout`, and corrected `compute_cost` blocks only under `mem0`, selects the backend-specific manager/target ABI, and replaces Cranelift's former minimal hand-written JSON with the shared strict serializer while retaining its status keys. Ordinary non-`mem0` strict JSON remains byte-stable. M7 adds the focused self-contained C/C++/Cranelift impulse audit; the completed gate also runs each public backend target over the full oracle-supported `dsp/` corpus, with semantic JSON and cross-backend cost equality. The expanded corpus additionally guards corrected helper-object layout, embedded helper arrays, helper lifetime, and strict-C soundfile behavior. M8 adds a live pinned-C++ structural/JSON differential, exact common-subset cost parity, focused D6 allowlist tests, C/C++/Cranelift O0/O3 parity, sanitizer coverage, and Cranelift serialization/rebinding coverage. See the authoritative [mem0 plan](custom-memory-manager-mem0-analysis-and-porting-plan-2026-08-13-en.md). |

`--table-init const` remains a permanent supported mode, not migration
scaffolding. See
[`siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`](siggen-subcontainer-table-init-port-plan-2026-08-05-en.md)
and `tests/corpus/rep_86_table_const_sr.dsp`.

### 4.2 Rust-only inspection, verification, and diagnostics modes

The following are `faust-rs` CLI surfaces. Some expose concepts that can be
inspected through C++ development tools, but they are not compatibility aliases
for a pinned C++ command-line contract:

- pipeline inspection: `--parse`, `--dump-box`, `--dump-sig`, `--dump-fir`,
  `--dump-fir-verify`, and `--check`;
- golden/debug inputs: `--golden`, `--fir-fixture`, and
  `--list-fir-fixtures`;
- interpreter conversion: `--dump-cpp-from-fbc` and `--cpp-class-name`;
- lowering selection/verification: `--signal-fir-lane`, `--no-fir-verify`,
  and `--fir-verify-strict`;
- structured presentation: `--error-format human|json`,
  `--error-verbosity concise|standard|debug|full`, `--diagnostic-paths`,
  `--color`, and `--help-error-format`.

These modes may be used by tests, CI, IDEs, and integrations, but scripts that
must run unchanged with Faust C++ should not pass them.

- **DIFF-CLI-015 — `--color auto|always|never`.** Status: `extension`. Human
  diagnostics carry ANSI color when stderr is a terminal (`auto`, the default),
  following `NO_COLOR`, `CLICOLOR` and `CLICOLOR_FORCE`; the pinned C++ compiler
  prints plain text and has no such option. Scripts reading stderr are
  unaffected: redirected output stays plain unless `CLICOLOR_FORCE` is set or
  `--color always` is passed, and color only adds escapes, so removing them
  gives the plain text byte for byte. The JSON channel and the text the FFI
  layers return (`rendered_diagnostics`, `getCCompleteCraneliftDSPFactoryError`)
  are plain for every C host. The one-line `... pipeline failed:` prefix and the
  option-validation messages are outside the diagnostic renderer and stay plain.
  `faustprobe` takes the same option with the same rules, also for the summary
  line and for any error other than a compile failure, which stay plain; the
  notes `--eval` adds are plain too. Its text crosses the C API, so it asks for
  color through `cranelift_ffi::factory::set_complete_error_color`, a per-thread
  Rust switch, off by default and out of reach of C. New public items, both
  Rust-only and both `extension`: that function and
  `compiler::diagnostics_human::StderrColor`, the decision the two tools share.
  Evidence: `colored_diagnostics_are_the_plain_text_plus_escapes_only` and
  `library_rendering_stays_plain_for_ffi_hosts` in
  `crates/compiler/src/cli/tests.rs`, and the color tests of
  `crates/compiler/tests/cli_diagnostics_channel.rs` and
  `crates/cranelift-ffi/tests/compile_error_probe.rs`.

### 4.3 Adapted option behavior

| ID | Surface | Status | Difference and compatibility impact |
|---|---|---|---|
| DIFF-CLI-004 | `--warn` | `adapted` | It covers warning classes related to C++ `-wall`/`-me`, but returns structured, source-linked diagnostics through the Rust warning bundle. Warnings are opt-in and never change successful compilation into failure. |
| DIFF-CLI-005 | `-ss N` parsing | `adapted` | Rust uses typed `clap` parsing: missing, non-integer, or negative values are errors. C++ `atoi`-style parsing can silently produce `0`. Rust applies the decoded scheduling strategy consistently to scalar and checked-vector scheduling. |
| DIFF-CLI-006 | compilation-options text | `adapted` | Rust prints flags only when they differ from the CLI default, except precision, which is always printed. C++ prints some defaults, notably `-mcd`, unconditionally. Rust also records its own `-table-init`, `--table-init-sample-rate`, and `-dlt` settings. |
| DIFF-CLI-007 | `-ec`/`-os` capability validation | `adapted` | Rust rejects unsupported backend combinations and block-sensitive one-sample programs with stable typed diagnostics. `-ec` on FIR is intentionally exposed as a Rust diagnostic/inspection extension. |
| DIFF-CLI-008 | unknown or invalid combinations | `adapted` | The Rust CLI generally fails early through typed option validation rather than relying on permissive legacy parsing or a later backend failure. Scripts depending on C++ option coercion must be corrected. |
| DIFF-CLI-012 | `--timeout` / `-timeout` deadline machinery | `adapted` | Rust combines a cooperative cancellation flag, a CLI watchdog, and phase-boundary checks. Both read one deadline rule (`cli/timer.rs`: `deadline`, `expired`), and `0` disables the limit for both; until 2026-10-10 the phase timer rejected every compilation under `--timeout 0`. A compilation that exceeds the limit fails with `FRS-COMP-0008`, also as a diagnostics-v2 document on stdout under `--error-format json` (it printed nothing on stdout before). C++ `-t` stops the process with a text message. Evidence: the tests of `cli/timer.rs` and `zero_timeout_disables_the_deadline_in_source_and_fixture_modes` in `crates/compiler/tests/cli_diagnostics_channel.rs`. |
| DIFF-CLI-013 | `-v` / `--version` text and the build commit | `adapted` | Rust prints `faust-rs <version>` where C++ prints `FAUST Version <version>`. Both name the build's commit, since the version is the same for every commit between two releases ([grame-cncm/faust-rs#20](https://github.com/grame-cncm/faust-rs/issues/20)), but differently: C++ adds a second line, `Source commit: <9 digits>` (`-modified` for an uncommitted tree, `unknown` without git); Rust appends `(<8 digits> <commit date>)` to the first line, as `rustc` and `cargo` do, and prints the version alone without git. Uncommitted changes are not reflected. The diagnostics JSON `compiler` block carries the full hash and date in optional `commit`/`commit_date` fields. Generated code names the package version only, in both. Scripts parsing the C++ lines must be adapted. Evidence: `crates/compiler/build.rs`, `version_text_reports_this_build_commit`, `json_compiler_block_names_the_commit_of_the_build`. |
| DIFF-CLI-014 | `-uim` / `--user-interface-macros` | `adapted` | Same `#ifdef FAUST_UIMACROS` block as C++ `CodeContainer::printMacros` for `-lang c` and `-lang cpp`: same macro names (the reference's `FAUST_COMPILATION_OPIONS` spelling kept), same entries and interface order, raw group labels with their metadata in the paths, the synthesized root and unnamed groups adding no segment, dead widgets left out, `FAUST_ACTIVES` counting soundfiles that `FAUST_LIST_ACTIVES` omits, and the C++ class (sub-containers included) declared `struct` without access specifiers. What differs follows the rest of the generated file (DIFF-BACK-004, DIFF-BEH-009): zone names are faust-rs's (`fEntry1` where C++ has `fEntry0`), numbers use the backend's literal spelling (`440.0f`, not `4.4e+02f`), paths and labels are escaped as C strings (C++ prints them raw, so a label with `"` broke the build), and lines are indented with spaces. The paths come from faust-rs's UI tree, so its own UI differences show up in them as they do in `buildUserInterface`: a relative group navigation is resolved (DIFF-BEH-008: `"Bar/gain"` where C++ prints `"Foo/../Bar/gain"`). Newlines in a label, kept as `\n` when `-uim` was ported, are spaces as in C++ since the string-literal fix of 2026-10-26. Other backends, `rust` included, ignore the option apart from echoing it in the compilation options, as in C++; the FIR fixture mode (`--fir-fixture`) has no UI tree and ignores it too. Evidence: `crates/compiler/tests/ui_macros.rs`, `crates/codegen/src/backends/ui_macros.rs` tests, `crates/transform/src/signal_fir/ui_macros.rs` tests; checked by building `architecture/minimal-static.cpp` against the output in `-single`, `-double` and `-vec`; compared with faust 2.90.15 on the 315 `.dsp` of `tests/` and `examples/` both compile: 297 blocks equal up to zone numbers and number spelling, the rest due to the soundfile zone name (`fSound`, not `fSoundfile`) or to UI differences `buildUserInterface` already has. |

Architecture wrapping now uses `FaustInstallPaths` defaults (including
`FAUST_ARCH_PATH`, executable-relative `share/faust` and `include`, and the
system fallback directories) before additional `-A` paths, following C++
`global::initDirectories` / `processCmdline`. Both the template and `-i`
inline headers use this list; previously the CLI used only explicit `-A`
paths, breaking installed `faust2juce` effect generation. This closes that
lookup gap; it does not add the C++ source-tree or `INSTALL_PREFIX` probes
to `FaustInstallPaths`. Evidence: `crates/compiler/tests/cli_architecture_paths.rs`.

### 4.4 Rust-only companion tool

- **DIFF-CLI-011 — `faustprobe`.** Status: `extension`. A second binary
  (`crates/cranelift-ffi`, `src/bin/faustprobe/`) that JIT-compiles a program
  with the Cranelift backend and measures it offline: controls set, swept and
  scheduled from the command line, statistics (subnormal outputs included),
  bargraph read-out, binary output (`--out`), expressions evaluated in a file's
  scope (`--eval`), two renders compared and the invariants of one checked
  (`--compare`, `--ref`, `--check`), a host loop for programs that output a
  loss and its gradients (`--train`, with a grid of starting points under
  `--sweep`, and `--fd-check` at the start or the end of the descent), the cost
  of the `compute` calls against real time (`--time`), a compile failure as
  the compiler's diagnostics-v2 JSON report (`--error-format json`), and the
  frequency response of a program checked to be linear and time-invariant
  (`--freqresp`), sweeps given point by point from a CSV file (`--points`)
  and sweeps of a polyphonic instrument, each point from a reset instrument
  (`PolyProbe::reset`), what a render sounds like as a few descriptors
  defined in the user guide (`--describe`: loudness, attack, decay, sustain,
  spectral centroid, stereo width; `crates/cranelift-ffi/tests/describe_probe.rs`),
  and the block diagrams of the probed program (`--svg DIR`,
  draw-only: written into `DIR` alone, named by definition with a `-2`, `-3`
  suffix for a repeated name instead of the C++ address suffix, an index on
  stdout; the C++ drawing options under `faust-rs`'s spellings, `-sd`
  simplifying the drawing only, not the compiled program;
  `crates/cranelift-ffi/tests/svg_probe.rs`). The `faust-rs` options that choose the program or shape its
  code are `faust-rs`'s own declaration, `compiler::CompileOptionArgs`,
  shared with the two impulse runners, and forwarded under their `faust-rs`
  spellings; `-ec`, `-os` and `-mem0` are refused
  (`crates/cranelift-ffi/tests/compiler_options_probe.rs`). It
  has no C++ counterpart; the closest reference tools are the `impulse-tests`
  runners, whose protocol it reproduces byte for byte under `--protocol
  impulse-test`. Evidence:
  [`docs/faustprobe-user-guide-en.md`](../docs/faustprobe-user-guide-en.md),
  [`faustprobe-generic-test-tool-design-2026-08-14-en.md`](faustprobe-generic-test-tool-design-2026-08-14-en.md).
  `--fd-check` never sets a control outside its range: within `--fd-step` of
  a bound its differences are one-sided (second order, extrapolated), named
  in the line and as `scheme` in the JSON, and a range narrower than two
  steps is refused (2026-10-24; before, they stepped across the bound, and a
  table read at `int(x * N)` with `x` below its minimum crashed the probe on
  faust-elements). Regression coverage: `fd_check_at_a_bound_stays_inside_the_range`
  and `fd_check_refuses_a_range_narrower_than_its_step` in
  `crates/cranelift-ffi/src/probe/train.rs`.
  Comparisons at zero absolute and relative tolerance require bit identity,
  including signed zeros. `--check determinism` runs its second compilation
  and render in a separate process so equal FIR keys cannot reuse the first
  JIT. Its hidden `--determinism-worker` mode and JSON bit-pattern response
  are a private implementation protocol, not a stable host API. This leaves
  the C/C++ factory cache contract unchanged. Regression coverage:
  `crates/cranelift-ffi/tests/compare_probe.rs` and
  `crates/cranelift-ffi/src/bin/faustprobe/determinism.rs`.

## 5. Runtime, lowering, and generated-code behavior

### DIFF-BEH-001 — const tables freeze initialization context

- Status: `extension`.
- With `--table-init runtime`, sub-containers receive the host's
  `init(sample_rate)` value, matching the C++ lifecycle.
- With `--table-init const`, table contents are compiler artifacts. A generator
  reading `ma.SR` therefore requires `--table-init-sample-rate`; changing the
  host initialization SR later does not change the table.
- A missing explicit SR is a typed `FRS-SFIR-0004` failure, not a hidden
  44.1-kHz or other default.

### DIFF-BEH-002 — nested generated tables are initialized

- Status: `reference-fix`.
- Rust initializes an inner generated table before the outer generator that
  consumes it.
- The pinned C++ implementation observed during the SIGGEN port declares the
  nested inner table but does not fill it, leaving zero content.
- Compatibility impact: programs accidentally depending on the pinned defect
  can produce different samples. Rust's behavior is the intended dependency
  order and has a structural regression test.
- Evidence: the nesting contract in
  [`siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`](siggen-subcontainer-table-init-port-plan-2026-08-05-en.md).

### DIFF-BEH-003 — checked vector admission and scalar fallback

- Status: `adapted`.
- Rust builds and verifies an explicit vector plan. A program shape that cannot
  be certified falls back to scalar lowering instead of emitting an unchecked
  vector schedule.
- `FirCompileOutput` exposes the requested/effective mode and the fallback
  reason. This observability surface has no equivalent C++ ABI contract.
- Scheduling ties are resolved deterministically from stable Rust IR ids;
  pointer-order byte identity with C++ is not promised.
- Compatibility impact: `-vec` can succeed while producing scalar Rust output.
  Callers that require effective vectorization must inspect the reported
  status, not infer it from exit code alone.
- Evidence:
  [`vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`](vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md).

### DIFF-BEH-004 — one-sample and external-control safety boundary

- Status: `adapted`.
- Rust preserves the public lifecycle intent of `-ec` and `-os`, but checks the
  selected backend and FIR requirements before emission. `-os -vec`, foreign
  `count` use, and block-sensitive reverse AD are rejected rather than assigned
  an invented one-sample meaning.
- Evidence:
  [`external-control-one-sample-port-plan-2026-07-23-en.md`](external-control-one-sample-port-plan-2026-07-23-en.md).

### DIFF-BEH-005 — diagnostic model

- Status: `extension` / `adapted`.
- Rust errors and warnings carry stable `FRS-*` codes, typed facts, source
  labels, traces, fixes, related diagnostics, verbosity filtering, and a clean
  JSON channel.
- Diagnostic wording, code, and stage are not generally byte-compatible with
  C++ stderr. Exit success/failure and the underlying semantic rejection remain
  the parity objective for portable cases.
- Duplicate UI paths, FIR verification, backend limitations, and const-table SR
  freezing have dedicated Rust diagnostics. A malformed FIR module (root not
  a module, a section not a block, a `sub_modules` entry not a sub-module)
  reads the same in every textual backend and carries its FIR node, each
  backend keeping its own code; cmajor and codebox gained
  `FRS-CGEN-CMAJ-0006`/`FRS-CGEN-CBOX-0003` for it (2026-10-18,
  `backends/module_view.rs`).
- Evaluator limits are logical resource budgets rather than the reference's
  stack-address check: default identity depth is 1,024 in debug and 32,768 in
  release; structural lowering also applies a 4,096/32,768 cap; syntactic nesting
  has a separate 400,000-entry budget. Each guard reports `FRS-EVAL-0099`
  ("stack overflow in eval", the C++ wording) naming itself, the limit that
  ran out and where it came from (profile default, environment variable, or
  API), with the variable to raise; for structural lowering, which of its two
  limits bound, both on a tie. JSON facts: `depth_guard`, `depth_limit`,
  `depth_limit_variables` (since 2026-10-10, W5 of
  [the 2026-10-08 audit](foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md#w5--explain-the-active-depth-guard-and-build-profile-p2);
  the variables are documented in the [CLI guide](../docs/user-cli-guide-en.md#12-evaluation-depth-limits)).
  The exception is a recursion on a non-constant numeric-pattern argument, reported as
  `FRS-EVAL-0012` ([DIFF-BEH-017](#diff-beh-017--a-recursion-on-a-non-constant-numeric-pattern-argument)).
  An evaluation cycle is `FRS-EVAL-0013`
  ([DIFF-BEH-018](#diff-beh-018--an-evaluation-cycle-names-its-definitions)).
- Native stack: C++ runs each compilation on a thread whose stack it sizes
  itself (`callFun`, `MAX_STACK_SIZE`). Rust does the same without a thread
  (`tlib::stack`). Each pipeline stage entered with less than 256 MiB of
  stack left runs on a fresh 512 MiB stack, the stack of the CLI's worker
  thread: the frontend after parsing, propagation, signal preparation, FIR
  lowering and verification, and each backend's code generation. The
  evaluator and the parser's import expansion, which no budget bounds, grow
  8 MiB segments as they recurse. A host thread of 8 MiB therefore compiles
  what the CLI compiles, at any depth the budgets accept
  ([#16](https://github.com/grame-cncm/faust-rs/issues/16)). Until
  2026-10-10, an 8 MiB thread aborted the process (`SIGABRT`, no diagnostic)
  between 2 000 and 8 000 levels of a chain no pass simplifies
  (`_ : +(1) : ...`) in a release build, in whichever pass came first, and
  the parser aborted even the CLI between 400 000 and 600 000 levels of
  `1+1+...+1`.
  Since 2026-10-10 every evaluator error a program, the API or a run can cause
  (a non-constant expression, a cancellation or timeout, a non-identifier
  parameter) has its own cause; the "internal: malformed or unsupported
  intermediate form" cause is left to internal forms.
- Constant folding during evaluation (pattern dispatch, box simplification,
  the patterns of rules) keeps its propagation and simplification caches for
  the whole pass (`propagate::FoldSession`, `normalize::ConstSimplifyCache`),
  as C++ 2.84.3 memoizes them on the trees. Before 2026-10-10 each fold
  started from empty caches, so a recursion on a growing argument compared
  with a number was quadratic: `g(n-1, x-1)` with `x` a slider took 35 s at
  `n = 4000` (C++ 2.84.3: 0.1 s), and in release the simplest runaway
  recursions reached the 120 s timeout before their depth budget. C++
  `master-dev` regressed the same way in `536ff8ca7`
  ([grame-cncm/faust#1345](https://github.com/grame-cncm/faust/issues/1345)).
  A bare `ffunction`/`fconstant`/`fvariable` statement is a syntax error in
  both grammars. Rust says it is an expression, and to name it or use it in
  `process`. A definition whose name lrpar's recovery inserted is dropped, so
  two such statements no longer report "multiple definitions of symbol ''"
  (until 2026-10-10;
  [the 2026-10-08 audit, W6](foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md#w6--diagnose-bare-foreign-statements-without-invented-definitions-p2)).
  No accepted syntax changed.
  Since 2026-10-16 three parse and propagation errors say more than C++
  (points 7 to 9 of the faust-coopiano port's `faust-rs-issues/RAPPORT.md`,
  outside this repository): a faust-rs reserved word used as a name (`fad`, `rad`,
  `cinput(s)`, `coutput(s)`, and `ondemand`, `upsampling`, `downsampling`,
  which the C++ reference branch reserves too but the C++ releases do not)
  is named as such with a rename proposed; a unary minus before a primitive
  (`-max(1, 2)`) or before an access (`-L.g(2, k)`, read `(0 - L).g`) states
  the rule and proposes `0 - …`, the access error now located at the `-`;
  and an arity error on a composition built by evaluation is located at the
  composition the program writes, through the evaluator's
  `EvalStats::composition_origins`, instead of the first located node inside
  the function it called (none when two places built it). C++ gives no
  position for the last two. Evidence: `crates/compiler/tests/diagnostic_hints.rs`.
- Evidence: [`docs/faust-error-model-en.md`](../docs/faust-error-model-en.md)
  and
  [`docs/diagnostics-codes-reference-en.md`](../docs/diagnostics-codes-reference-en.md);
  for the native stack, `crates/compiler/tests/deep_expressions.rs` (every
  backend on an 8 MiB thread) and `crates/parser/tests/deep_nesting.rs`.

### DIFF-BEH-006 — `-e` expansion and `expandDSP` result

- Status: `adapted`.
- Rust serializes the evaluated Box tree back to Faust source, as C++
  `boxppShared` does. On the 33-fixture corpus that has a recorded C++
  expansion, the two compilers produce byte-identical documents apart from the
  values listed below. Corpus and capture tool:
  [`tests/expand/`](../tests/expand/README.md),
  `cargo run -p xtask -- expand-oracle`.
- Values that differ by construction, and cannot match:
  - `declare version` carries the faust-rs version;
  - `declare compile_options` carries the faust-rs option spelling;
  - `declare library_path<i>` carries installation-dependent absolute paths.
- Deliberate deviations:
  - with no `-o`, faust-rs prints the expansion to standard output. C++ writes
    to `ofstream(gOutpath)` and, with `gOutpath` empty, produces nothing while
    exiting 0.
  - real literals take their suffix from the precision alone. C++ derives it
    from `gOutputLang` too (`compiler/generator/floats.cpp:49`), so the same
    program expands to `3.1415927f` under `-lang cpp`, `3.1415927` under
    `-lang rust`, and `3.1415927f0` under `-lang julia` — the last being Julia
    literal syntax in a `.dsp` file, which the Faust parser rejects with
    `syntax error, unexpected INT`. A backend must not leak into a document
    that is Faust source.
  - box shapes with no Faust source syntax (`with { ... }`, `letrec`, evaluator
    closures, partially-applied pattern matchers) are refused with
    `FRS-COMP-0007` rather than printed as the placeholders C++ emits
    (`closure[...]`, `PM[...]`, a raw tree dump inside `with { }`), none of
    which re-parse. They cannot occur in a successfully evaluated `process`.
  - `downsampling(...)` is printed. C++ cannot expand any program containing
    it: `boxppShared::print` tests `isBoxUpsampling` twice
    (`compiler/boxes/ppbox.cpp:615-617`) and throws on `BoxDownsampling`. The
    non-shared `boxpp` printer handles the node correctly at
    `compiler/boxes/ppbox.cpp:467`, so the defect is reachable only through
    `-e`.
- Shared behavior worth stating, because it looks like a defect and is not:
  expansion converges after the *second* pass, not the first, in both
  compilers. The second pass grows the header (an expansion declares its own
  `version` and `compile_options`, which the next pass reads as ordinary
  metadata) and can shrink the body (re-evaluating `(65536 : int)` folds it to
  `65536`). The third pass equals the second.
- Generated state-carrier names are program-local, as in C++: compiling a
  program and compiling its expansion produce the same `fRec`/`iRec` names.
  (Until 2026-08-12 they were numbered by arena node id and drifted; see
  `DIFF-BEH-009`.)
- `generateAuxFiles` returns owned artifact descriptions in the Rust facade;
  the C entry points in `crates/libfaust-ffi` write them to disk or return the
  single requested one, and report rather than guess when a request selects
  none, several, or a binary output.

### DIFF-BEH-009 — generated variable numbering

- Status: `adapted`.
- Generated names are numbered densely and in allocation order, per family:
  `fConst`, `iConst`, `fSlow`, `fTemp`, `tbl`, `Wave`, `SIG`, and — since
  2026-08-12 — the recursion carriers `fRec`/`iRec`/`fRecCur`/`iRecCur`. The
  numbering describes the program, not the compilation session.
- The numbers themselves are **not** expected to equal the C++ ones: both
  compilers assign in traversal order, and the traversals differ. What is
  guaranteed is that recompiling the same program — directly, or from its `-e`
  expansion — yields the same names.
- Evidence:
  `crates/compiler/tests/expand_corpus.rs::generated_names_do_not_depend_on_what_was_evaluated_first`.

### DIFF-BEH-007 — import delivery environments

- Status: `adapted`.
- Native Rust compilation supports local search directories, structural local
  imports, direct HTTP(S) entry sources, explicit URL imports, relative imports
  within remote graphs, and remote main architecture templates. The native
  transport requires the default-off `network-imports` Cargo feature and the
  per-run `--allow-network-imports` option.
- Unlike C++'s process-global socket fetcher, Rust exposes a per-compiler
  injected capability with URL policies, redirect re-authorization, bounded
  bodies, real HTTPS, and structured errors. Remote evaluator-driven
  `component(...)`/`library(...)`, remote inline architecture sub-includes, and
  C/C++ compatibility-facade opt-in are still deferred.
- The `wasm-ffi` embedded compiler ships standard libraries as read-only
  virtual sources and accepts host-prefetched URL/source bundles through
  repeated `--remote-source <url> <base64>` arguments. The module performs no
  browser network I/O and does not depend on an Emscripten filesystem; URL
  identity and relative URL imports are preserved inside the supplied graph.
- Compatibility impact: native CLI builds must opt in twice; browser callers
  must fetch the complete graph asynchronously and supply it explicitly before
  entering the synchronous compiler ABI.

### DIFF-BEH-008 — relative pathname navigation in explicit group labels

- Status: `extension`.
- Faust C++ applies pathname normalization such as `../` when a terminal UI
  element (slider, button, bargraph, soundfile, and so on) is propagated. It
  pushes explicit `hgroup`, `vgroup`, and `tgroup` labels directly onto the UI
  path instead, so `vgroup("../Bar", ...)` remains a group literally named
  `../Bar` in generated UI code.
- Rust preserves the C++ behavior for terminal UI elements and additionally
  resolves `./`, repeated `../`, and an absolute `/` prefix on explicit group
  labels. Navigation above the canonical UI root is clamped at that root.
- Compatibility impact: a DSP using pathname navigation in a group label can
  produce a different UI hierarchy, parameter addresses, JSON description,
  or OSC/API path under Rust. Portable DSPs should restrict relative pathname
  navigation to terminal UI element labels.
- Evidence: the C++ comparison and frozen Rust semantics in
  [`ui-ir-architecture-contract-2026-03-12-en.md`](ui-ir-architecture-contract-2026-03-12-en.md),
  plus corpus cases
  [`rep_63_ui_relative_group_rebase.dsp`](../tests/corpus/rep_63_ui_relative_group_rebase.dsp)
  and
  [`rep_64_ui_relative_group_root_clamp.dsp`](../tests/corpus/rep_64_ui_relative_group_root_clamp.dsp).

### DIFF-BEH-010 — division and remainder by a constant zero

- Status: `adapted` (same rejection, different report), with one `parity-gap`
  recorded as `DIFF-GAP-017`.
- Both compilers reject a division by a constant zero with exit status 1, in
  integers **and in reals**: neither folds `2.0 / 0` to an infinity. C++ prints
  `ERROR : division by 0 in 2 / 0`, thrown by `mterm::operator/=` while the
  evaluator folds a numeric sequence. Rust raises `FRS-EVAL-0007` at the same
  point, located in the source, with the division as written (`expr=(2 / n)`)
  next to the one that was folded (`2.0 / 0`); a divisor that becomes a constant
  zero only in the signal graph (`_ / (0 : *(0))`, `_ / 0`) is reported by the
  typing stage as `FRS-COMP-0004`, as is a remainder by zero (`% by 0`).
- Until 2026-09-17 the evaluator's folds were not under the boundary that
  catches the normalizer's unwind: `process = 2.0 / 0;` panicked the compiler
  and, through the FFI, aborted the host. `0 / 0`, `0.0 / 0.0` and `0 % 0`
  compiled to a silent 0, which the reference rejects. Both are fixed.
- Integer folds that overflow in Rust follow the reference: `i32::MIN % -1` is
  0, `i32::MIN / -1` is the real 2147483648, additions and multiplications wrap,
  a shift count is taken modulo 32.
- Evidence: `crates/compiler/tests/diagnostic_errors.rs`
  (`a_constant_division_by_zero_is_a_located_eval_error` and the four tests
  after it), `tests/corpus/err_18` to `err_20`, measured against Faust 2.89.

### DIFF-BEH-011 — compile-time mathematical domain

- Status: `adapted` (stricter on two functions) and `reference-fix` (sign of an
  infinite constant).
- Both compilers reject a constant argument outside a function's domain:
  `sqrt(-1)`, `log(-1)`, `acos(2)`, `asin(2)`, `fmod(x, 0)`, `remainder(x, 0)`
  (C++ `ERROR : out of domain in sqrt(-1)`, Rust `FRS-COMP-0004`, with the
  inferred intervals).
- **Rust also rejects `log(0)` and `log10(0)`**, the boundary of the domain;
  C++ accepts them and folds an infinity. A program that relies on it must
  write the infinity it wants. Both compilers accept `pow(-1, 0.5)` (NaN),
  `exp(1000)` (infinity) and `atan2(0, 0)` (0).
- **Sign of an infinite constant.** Faust 2.89 prints a negative infinity as
  `INFINITY`: `0 - exp(1000)` and `log(_ * 0)` both generate
  `static_cast<FAUSTFLOAT>(INFINITY)`, in the C and C++ backends. Rust generates
  `-INFINITY`. The outputs of the two compilers therefore differ, and the Rust
  one is the intended value.
- Evidence: `crates/compiler/tests/diagnostic_errors.rs`
  (`compile_time_math_domain_error_is_typed_and_source_located`); the table of
  cases is in `porting/journal/2026-09-17.md`.

### DIFF-BEH-012 — `abs` of an integer constant

- Status: parity reached on 2026-09-17; kept here because the behavior is the
  reference's and is not the mathematical one.
- `abs` is the one unary function that keeps an integer an integer. Its
  constant fold now does: `abs(-3)` is the integer 3 (it was the real 3.0, and
  `abs(-2147483647)` the float 2147483648.0). `abs(-2147483648)` is
  **-2147483648**, as in C++: there is no absolute value of `i32::MIN` in 32
  bits, and the fold wraps the way the `std::abs(int)` both compilers emit for a
  non-constant argument does, so that a folded expression and a computed one
  agree.
- Evidence: `crates/normalize/src/simplify.rs`
  (`abs_of_an_integer_constant_stays_an_integer`).

### DIFF-BEH-013 — one clocked block for one definition, whatever the slot environments it is read in

- Status: `adapted` (fewer clocked-block instances than the C++ reference, same
  samples), implemented 2026-09-21.
- C++ `propagate` memoises a call on `(slotenv, path, box, inputs)` and
  `makeClockEnv` names an `ondemand` block's environment by the same tuple: a
  definition holding a clocked wrapper, read at the top level and again inside
  the body of a `boxSymbolic` (every `f ~ g` whose `f` is an unapplied
  function), is propagated twice and the wrapper becomes two blocks, both
  computed. Rust keys the result memo on the environment *restricted to the
  box's free slots* (`propagate::flat::free_slots`,
  `SlotEnv::restricted_id`), the empty one for a closed box, leaves the
  `suppress_fad` mode out of the key of a box holding no `fad`, and memoises
  a recursion holding a `fad` from its first call: the second reading hits
  the first propagation and replays its domain, so the definition is one
  block. `t = (clk, x) : ondemand(exp); process = t + ((\(r).(t + 0.5 * r)) ~ _)`
  emits one `exp` where C++ emits two; a descent of `optimizers.lib` whose
  loss reads a clocked model that the program reads too runs one model per
  reading less (`porting/journal/2026-09-21.md`, `porting/MEMOIZATION.md`
  §2.22).
- Compatibility impact: the generated code differs from C++ in the number of
  block instances and their domain ids, never in the samples: the merged
  blocks had the same clock, the same inputs and the same body, so the same
  state. `DIFF-BACK-004` already frees the generated text from byte identity.
  A program that relied on two instances of one definition having two states
  cannot exist: the two were fed the same signals by construction.
- A definition that depends on an enclosing recursion's variable and is read
  at two binder depths is still propagated twice (the variable is lifted
  under the inner binder), as in C++; recorded as open in the journal.
- Tests: `crates/compiler/tests/closed_box_memo.rs`,
  `propagate::tests::closed_clocked_box_is_one_block_across_slot_environments`.

### DIFF-BEH-014 — a widget parameter that is not a compile-time number

- Status: `adapted` (same rejection, different report), fixed 2026-09-24.
- Both compilers reject a `hslider`, `vslider` or `nentry` init, min, max or
  step, or a bargraph min or max, that does not fold to a number. C++ stops in
  `eval2double` (`compiler/evaluate/eval.cpp`) with `not a constant expression
  of type : (0->1)` when the parameter box is not `0→1` (`_`, `(1, 2)`), and
  in `tree2double` (`compiler/tlib/tree.cpp`) with `the parameter must be a
  real constant numerical expression : SigInput[10001]` when it is a `0→1`
  signal known only at run time (`0.5 : \(x).(hslider("a", x, 0, 1, 0.1))`,
  a `case` rule applied with `:`, `vslider("a", 0.5, 0, button("b"), 0.1)`).
  Rust raises `FRS-EVAL-0011` at the same point, located in the source, with
  the C++ wording, the widget, its label, the parameter and the expression as
  written.
- Until 2026-09-24 Rust kept the unevaluated parameter and the UI builder read
  it as 0: those programs compiled, with an init, a min or a max of 0, and a
  bargraph whose max was a lambda argument failed later with a propagation
  arity error.
- Compatibility impact: none for a program C++ accepts; the programs Rust used
  to accept are the ones C++ rejects.
- Evidence: [`err_33_widget_parameter_not_constant.dsp`](../tests/corpus/err_33_widget_parameter_not_constant.dsp),
  `a_widget_parameter_that_is_not_a_number_is_refused_like_the_reference` in
  `crates/compiler/tests/diagnostic_errors.rs`.

### DIFF-BEH-015 — the order of widgets with equal labels in a group

- Status: `adapted`, 2026-09-27.
- Both compilers sort the items of a UI group by their label as written, so
  widgets with equal labels, typically unlabelled ones, are ordered by
  whatever order they had before the sort. Rust keeps the source order.
- C++ up to Faust 2.89.2 built each group's list by prepending (`addKey`,
  `compiler/generator/uitree.cpp`) and sorted it with an unstable
  `std::sort` (`sortPropList`): that order was left to the standard library,
  and `checkNullBargraphLabel` numbered the unlabelled bargraphs in it, so
  which one was `hbargraph0` depended on it too. Fixed in Faust 2.89.3
  (`ed7c12606`, a `std::stable_sort` over the insertion order): each
  unlabelled bargraph now gets the same number in both compilers (the first
  `hbargraph("")` of the source is `hbargraph0`).
- What remains: C++ inserts a widget when it compiles its signal, not in
  source order, so fixed C++ can still list equal-labelled widgets in another
  order than Rust: for `hslider("", …) + (1 : hbargraph("", …)) + (2 :
  hbargraph("", …)) + (3 : vbargraph("", …))`, fixed C++ declares
  `vbargraph0`, `hbargraph0`, `hbargraph1`, `0x00` and Rust `0x00`,
  `hbargraph0`, `hbargraph1`, `vbargraph0`, each name on the same widget.
- That concerns labels that compare equal only. Rust compared the others on a
  key rebuilt from the parsed `[n]`, not on the raw label as C++ does: the
  spaces of `%2i` were lost, `[10]` came before `[ 2]`, and with 11 or more
  bands (`an.spectral_level`) widgets were listed, and unlabelled bargraphs
  numbered, otherwise than in C++. Fixed on 2026-10-31: the key is now the raw
  label.
- The names match: an unlabelled group or input widget is `0x00` and an
  unlabelled bargraph `hbargraph<n>` / `vbargraph<n>`, and two unlabelled
  input widgets in one group are refused as in C++ (`FRS-UI-0001`,
  `path '…/0x00' is already used`).
- Compatibility impact: the order of equal-labelled widgets in
  `buildUserInterface` and the JSON; against Faust 2.89.2 also which
  unlabelled bargraph takes which number. Names, paths and values match
  Faust 2.89.3.
- Evidence: `crates/compiler/tests/anonymous_ui_labels.rs`,
  `unlabelled_controls_are_named_as_in_cpp_and_can_conflict` and
  `widgets_are_sorted_on_their_raw_labels_as_in_cpp`;
  `anonymous_widget_names_count_per_prefix_like_get_fresh_id` in
  `crates/ui/tests/core_api.rs`.

### DIFF-BEH-016 — soundfile channel wrap generated as `chan % fChannels`

- Status: `adapted`, 2026-10-01: ahead of the pinned C++ reference, in step
  with the C++ `rework-soundfile` branch (`49c6d9367`, `5c1b9b257`; follow-up
  to grame-cncm/faust#1322), not yet in `master-dev`.
- A `soundfile(label, N)` read whose resource has fewer than `N` channels
  reads real channel `chan % fChannels`. Rust generates that wrap in every
  backend (C, C++, Rust, Julia, WASM, AssemblyScript, Cranelift, interpreter),
  from one lowering rule shared by the scalar and vector lowerers
  (`emit_soundfile_buffer`, `crates/transform/src/signal_fir/leaf_emit.rs`)
  over a new FIR load `LoadSoundfileChannels` (`fSoundN->fChannels`).
  Constant channel 0 is not wrapped, as in C++.
- The pinned C++ reference (`8eebea429`) instead indexes `fBuffers[chan]`
  directly and relies on the architecture duplicating channel pointers up to
  `MAX_CHAN` (64, `Soundfile::shareBuffers`). That limit is gone in Rust: a
  soundfile can be read with any number of outputs.
- Runtime contract (the C++ branch's `Soundfile.h`): `fChannels >= 1`, and
  `fBuffers` holds at least `fChannels` pointers. A `Soundfile` that
  duplicates pointers still works; one with `fChannels == 0` divides by zero
  (no guard, as in C++).
- Rust-only host contracts that change with it: the Rust backend's host
  `Soundfile` type must expose `fChannels: i32`, the Julia one
  `fChannels::Int32`, and an AssemblyScript host must provide the import
  `env._soundfileChannels(slot): i32`. The interpreter encodes the field as
  `kLoadSoundFieldInt` with selector 2, which pops no part, and its
  `Soundfile::read_sample` no longer wraps (faust-rs `.fbc` was already not
  interchangeable with C++).
- Compatibility impact: generated code now requires `fChannels` to be set
  correctly, which every C++ runtime does but not faustwasm before its
  matching release. Code generated earlier, run against a runtime that no
  longer duplicates channels, reads out of bounds.
- The impulse fixtures follow the C++ branch's `TestMemoryReader`: each
  channel has its own phase (`sin(part + chan + ...)`) and only the real
  channels are provided. The C/C++ impulse paths, reference generation
  included, take that fixture from faust-rs's
  `tests/impulse-tests/archs/faustrs_soundfile_fixture.h` rather than from the
  C++ checkout. With the pinned checkout, the regenerated `sound` reference is
  byte-identical, and the `cpp` and `c` lanes pass `sound`.
- Evidence: `crates/compiler/tests/soundfile_channel_wrap.rs` (a 1-, 2- and
  3-channel soundfile read as 70 outputs by the interpreter, scalar and
  vector; C++ text wraps every channel but 0); the `sound` impulse test on all
  backend lanes.

### DIFF-BEH-017 — a recursion on a non-constant numeric-pattern argument

- Status: `extension` (same rejection, a diagnostic C++ does not have), added
  2026-10-10 ([#21](https://github.com/grame-cncm/faust-rs/issues/21)).
- A function defined by numeric patterns, `f(0) = ...; f(n) = f(n-1) ...`,
  applied to a signal (a UI control, an input, `fi.bandpass` with a slider as
  its order) can never reach its numeric base case. Both compilers reject it
  when a depth limit runs out. C++ reports `ERROR : stack overflow in eval`
  (2.84.3, 16 MB stack), or runs until its timeout (2.90.6, 256 MB stack and
  quadratic folding; `fi.bandpass` in both). Rust reports `FRS-EVAL-0012`,
  naming the argument's position, the numbers it is compared with, and the
  controls or inputs it depends on.
- The diagnostic is chosen only once a depth budget has run out, so no
  accepted program changes. It requires the same `case` nested at least three
  times on a non-numeric argument at one position, and no numeric dispatch of
  that `case` from the first of them on. A recursion that descends on a number
  while another argument is a signal keeps `FRS-EVAL-0099`.
- Compatibility impact: none on accepted programs. Scripts matching the C++
  text see another message.
- Evidence: [`err_34_case_argument_not_constant.dsp`](../tests/corpus/err_34_case_argument_not_constant.dsp),
  `a_recursion_on_a_non_constant_pattern_argument_names_it` in
  `crates/compiler/tests/diagnostic_errors.rs`, the tests of
  `crates/eval/src/loop_detector.rs`, and the
  [analysis](eval-runaway-recursion-and-cycle-diagnostics-analysis-and-plan-2026-10-10-en.md).

### DIFF-BEH-018 — an evaluation cycle names its definitions

- Status: `adapted` (same rejection, a different report), 2026-10-10
  ([#22](https://github.com/grame-cncm/faust-rs/issues/22)).
- A definition that needs itself, directly or through other definitions
  (`x = x;`, `a = b + 1; b = c * 2; c = a;`, `process` passed to a function
  that `process` uses), is rejected by both compilers. C++
  `loopDetector::detect` samples the last 1024 evaluations every 400 steps and
  reports `ERROR : after <n> evaluation steps, the compiler has detected an
  endless evaluation cycle of <k> steps`. Rust detects the re-entered frame at
  once on its call stack and reports `FRS-EVAL-0013 endless evaluation cycle:
  process → effect → cut → process`, with a label on each top-level definition
  of the cycle. The primary label is the use that closes the cycle, the
  `process` written in `cut`, inside its definition.
- Until 2026-10-10 Rust reported `FRS-EVAL-0099 recursive evaluation loop on
  node <n>`, with the catch-all cause of malformed internal forms.
- Compatibility impact: none on accepted programs. Scripts matching the C++
  text see another message.
- Evidence: [`err_35_evaluation_cycle.dsp`](../tests/corpus/err_35_evaluation_cycle.dsp),
  `an_evaluation_cycle_names_its_definitions` in
  `crates/compiler/tests/diagnostic_errors.rs` and in
  `crates/eval/src/loop_detector.rs`;
  `an_evaluation_cycle_is_blamed_on_the_use_that_closes_it` in
  `diagnostic_errors.rs`.

### DIFF-BEH-019 — a signal proved constant by its interval keeps its widgets

- Status: `parity-gap`, 2026-10-10.
- C++ `1aafc196a` (2026-10-06, first released in 2.90.4) runs
  `newConstantPropagation` in `InstructionsCompiler::prepare`, as the `ocpp`
  compiler has done since 2024. A signal whose interval is a single value
  becomes that value, and the widgets it alone used leave the interface, since
  Faust builds the interface from the simplified signals. Rust has no
  interval-driven constant propagation: such a signal is computed, and its
  widgets stay.
- Observed on `process = ["a" -> r] ~ _;` with
  `r = + : *(hslider("a", 0.5, 0, 1, 0.01));`. The recursion
  y = a·(x₁+x₂)·y' starts at 0 and stays 0. C++ 2.90.4 and later emits
  `output0 = 0.0f` with no slider; Rust computes `fRec0` (always 0) and
  keeps `/…/a`. The samples are equal (all zero), but the interface differs.
- Compatibility impact: none on samples. A host that reads the interface
  (JSON, UI, OSC paths) sees widgets that C++ ≥ 2.90.4 drops, on programs
  whose output does not depend on them.
- Evidence: until 2026-10-10 this was the fixture
  `modulation_35_in_recursion.dsp`, and the live differential of
  `crates/compiler/tests/modulation_corpus.rs` failed against any local
  `faust` ≥ 2.90.4. Its block now adds the slider (y = x₁ + x₂ + a·y'), which
  both compilers compute: the interface is the one frozen from 2.88.1, equal
  in 2.90.6, and the 15 000 impulse-test samples are equal to the bit.

### DIFF-BEH-020 — integer powers with a runtime exponent

- Status: `reference-fix` relative to the pinned `8eebea429`; aligned with
  C++ correction `eb21fcccff1ead7fe286f35614a464bea70a5fe2` (2026-10-08).
- Implemented: `pow` preserves the union of its operand natures, as C++
  `PowPrim::inferSigType` does. Integer runtime powers lower to double `pow`,
  double `rint`, then an int32 cast in both scalar and checked vector FIR.
  The pinned compiler computes at sample precision and truncates, which can
  lose a unit under fast math or beyond 2^24 in single precision.
- Positive integer constant folding preserves int32 wrapping arithmetic;
  small nonnegative constant exponents use wrapping multiplies. Compile-time
  table generators follow the same prepared integer semantics. Mixed/real
  powers retain sample precision. FIR uses `powf`/`rintf` for single-precision
  calls and `pow`/`rint` for double calls to avoid conflicting prototypes.
  `FirMathOp::from_symbol` accepts these two additional C spellings (`adapted`
  internal API; no signature or host DSP API change).
- Compatibility impact: integer arithmetic consuming `pow` now runs before
  conversion to an audio sample. For example, `pow(25, 6*s) - (1 + 244140624*s)`
  is exactly zero for `s` in {0, 1} in generated C++, including fast math.
  This does not define a new policy for NaN, infinity, or out-of-range casts.
- Scope limit shared with the C++ interpreter: FBC has one real precision per
  factory, so a single-precision FBC program cannot retain these double
  intermediates. Double FBC is covered with optimization levels 0 and max;
  a mixed-precision FBC extension is not implemented by this change. Source:
  C++ `interpreter_instructions.hh::visit(CastInst*)`, Rust
  `codegen/backends/interp/compiler/expressions.rs::compile_cast`.
- Evidence: `crates/compiler/tests/integer_pow.rs`,
  `crates/compiler/tests/integer_pow_rust.rs`, sigtype/normalization unit tests,
  and [the journal entry](journal/2026-10-11.md).

### DIFF-BEH-021 — recursions identical after simplification share one state

- Status: `extension`, implemented 2026-05-01; recorded 2026-10-16.
- Implemented: `normalize::merge_isomorphic_symrec_groups`
  (`crates/normalize/src/rec_merge.rs`, step 2.8 of
  `prepare_signals_for_fir_unverified`) runs between the two simplify passes
  and replaces every recursion group whose body, with its own references
  masked, is identical to another group's by that group (the one of
  smallest node id). The second simplify then folds what the sharing
  exposes, e.g. `x - x` to `0`. Faust C++ has no such pass: it shares two
  recursions only when their de Bruijn trees are already identical, which
  Rust does too through hash-consing, so the difference is limited to
  bodies that become identical only once simplified. The pass was added for
  `fad`, whose chain-rule bodies (`1·y + g·Dy`) equal a hand-written
  derivative (`y + g·Dy`) only after simplification
  ([plan](normalize-rec-isomorphism-merge-plan-2026-05-01-en.md)).
- Observed with the pinned `8eebea429` on
  `process = _ <: (+ ~ *(0.5)), (+ ~ (*(1.0) : *(0.5))) : -;`: C++ keeps
  two recursion states and emits their difference
  (`fVec0SE[0] + -(fVec2SE[0])`); Rust keeps one and emits `0`. Without the
  final `-`, Rust writes both outputs from one state where C++ computes
  two.
- Compatibility impact: fewer recursion states and less code than C++ on
  such programs; the sample values are those of C++, since the merged
  bodies compute the same expression, except where a difference of two
  merged recursions is folded: `x - x` is `0` in Rust where C++ gives NaN
  once the recursion has diverged to an infinity or a NaN. The generated
  C++ text is not a byte-identity contract (`DIFF-BACK-004`).
- Evidence: the unit tests of `crates/normalize/src/rec_merge.rs`
  (`merge_followed_by_simplify_gives_zero`,
  `pruned_opening_matches_the_full_walk`), the corpus fixtures
  `tests/corpus/rep_83_isomorphic_filter.dsp`,
  `rep_84_isomorphic_symrec_mutual.dsp` and
  `rep_85_isomorphic_symrec_nested.dsp`, and the
  [journal entry](journal/2026-10-16.md) of the cost fix.

### DIFF-BEH-022 — small integral real powers are inline products on every backend

- Status: `adapted`, 2026-10-16.
- C++ `powprim.hh::generateCode` expands `pow(x, n)` for a literal `n`
  whose value is an integer from 0 to 8 (an integer literal, or a real one
  without fractional part, constant and computable) into a call to a
  generated `<Klass>_faustpower<n>_{i|f}` helper whose body is the
  left-associated product `((x·x)·x)…`, but only where `gNeedManualPow`
  holds: C, C++, Rust, Java, Julia, C#, LLVM. Interp, WASM/WAST, JSFX,
  Codebox, Cmajor and JAX keep `pow`.
- Implemented: `leaf_emit::emit_pow` writes the same left-associated product
  inline, for a real result, on every backend and in scalar and vector code.
  The base is the same FIR value in every factor, so FIR
  common-subexpression materialization evaluates a non-trivial base once,
  as the helper's argument is: `pow(x + 0.5, 3)` becomes
  `fTemp0 = x + 0.5; … (fTemp0 * fTemp0) * fTemp0`. An integer base is
  converted to the real type before multiplying, as it is for `pow` (the C++
  helper takes an `int` argument and multiplies in `int`). Integer results
  keep the inline wrapping product of DIFF-BEH-020. A negative integer
  literal keeps `pow`: C++ accepts it, and its expansion returns the base
  unchanged. `pow(x, 1)` is `x` itself, so a `Konst` power read by
  `compute` whose base is an init-local `fConst*` is still copied into a
  struct field (`fConst1 = fConst0;`), where C++ stores the helper's result:
  the placement gate lifts its trivial-load exemption only for a load of an
  init-local constant (`reads_init_local_constant`) (2026-10-17; before,
  the local leaked into `compute`, FRS-FIR-0001 on faustlibraries
  `an.spectral_centroid`/`an.spectral_spread`).
- Compatibility impact: no helper function in the generated text, and no
  `pow`/`powf` call for these powers on any backend, the Cranelift JIT and
  the interpreter included (`pow(x, 2)`, which normalization makes of `x*x`,
  was a libm call there: 256 squares ran at 29× real time under faustprobe,
  133× now). Values are those of the C++ helper bit for bit; on the
  backends where C++ keeps `pow`, a power of 3 or more can differ from it in
  the last unit of precision (one rounding per product against one for
  `pow`). An integer base whose product overflows `int` gives the real
  product here, where the C++ helper overflows.
- Evidence: `crates/compiler/tests/real_pow_expansion.rs` (products in
  scalar and vector code with a certified vector plan, a foreign base
  evaluated once, exponents that keep `pow`, interpreter results equal to
  the left-associated products bit for bit at optimization levels 0 and
  max, an identity power of an init-local constant stored for `compute`
  and computing its value at both levels),
  `signal_fir::tests::placement::escaping_konst_lowered_to_an_init_local_load_gets_a_struct_field`,
  faust-coopiano `faust-rs-issues/RAPPORT.md` point 4, and the journal
  entries of [2026-10-16](journal/2026-10-16.md) and
  [2026-10-17](journal/2026-10-17.md).

### DIFF-BEH-023 — normalized sums join the terms that depend on state last

- Status: `adapted` (follows upstream Faust 2.90, not the pinned reference),
  2026-10-16.
- The pinned reference (`8eebea429`, Faust 2.84.3) buckets the terms of a
  normalized sum by order (numbers, constants, user interface, audio) and
  folds them from the highest order down. A sum mixing a constant, a control
  and a recursion's state, such as `1 - c - o`, is therefore
  `(1 - o) - c`: the slow `1 - c` is not one subtree and cannot be hoisted,
  and `1 - o` rounds. Upstream commit `d4d51e5ed` (Yann Orlarey,
  2026-08-09) adds a fifth order, "audio-recursive", for the terms that a
  recursion node reaches (tlib's `kContainsRec` bit). Those terms join the
  sum last, as the right operand: `(1 - c) - o`.
- Implemented, as upstream:
  - `TreeArena::contains_recursion`, the same bit, set when a node is
    interned. It is true for `SYMREC`, `SYMREF`, `DEBRUIJNREC` and
    `DEBRUIJNREF`, `SYMREF` included because faust-rs writes a
    self-reference as `proj(i, SYMREF(var))`.
  - `Aterm::normalized_tree` joins those terms last.
  - `Mterm::complexity` weighs such a factor one more than an audio-rate
    one, which steers `greatest_divisor` as upstream's uncapped `sigOrder`
    does.
  - The grouping of products keeps the audio level (upstream's cap).
  - A two-term sum `{slow, state}` keeps its operand order.
- Compatibility impact: generated code differs from the pinned reference in
  the programs whose sums mix a state term with several other terms:
  210 of the 632 programs under `tests/` and `examples/` (215 in `-vec`,
  five of them by renamed temporaries only). On the changed programs,
  faustprobe in single precision gives:
  - 191 bit-exact;
  - 12 that differ by rounding, at most 1e-4 of the peak;
  - 3 sensitive to rounding (a phase reading a noise table, phase
    unwrapping, an optimizer that restarts on thresholds), which diverge in
    single precision; the first two agree in double, and the third already
    diverges between single and double with the old normal form.

  The slow part of such sums is hoisted to block rate. Values
  change by rounding: a rate-limited follower now reaches 0 exactly where it
  stopped at the rounding error of `1 - o`. That rounding error kept
  faust-coopiano's damped sympathetic strings awake after the first pedal.
  Golden C++ snapshots taken from the pinned reference differ where these
  sums occur.
- Evidence:
  - `crates/normalize/src/aterm.rs` tests (`a_state_term_joins_the_sum_last`,
    operand order, mixed sums, complexity weight; the `greatest_divisor`
    property test now includes a state factor);
  - `crates/tlib/tests/recursive_trees.rs`
    (`contains_recursion_marks_recursion_nodes_and_everything_above_them`);
  - `crates/compiler/tests/late_state_join.rs` (the hoisted `1 - c` in C++,
    and the follower reaching 0 exactly in the interpreter at optimization
    levels 0 and max);
  - [plan](aterm-late-state-join-port-plan-2026-10-16-en.md),
    faust-coopiano `faust-rs-issues/RAPPORT.md` point 15, and the
    [journal entry](journal/2026-10-16.md).

### DIFF-BEH-024 — block-rate work in a boolean `ondemand` runs at its first fire

- Status: `adapted` (generated code; outputs unchanged).
- C++ `generateOD` (branch `8eebea429`) hoists every control-rate (`Block`)
  value a domain's payload needs before the sample loop, like any `fSlow`. A
  boolean `ondemand` whose payload depends only on controls therefore computes
  it at every block, even when the clock never fires.
- faust-rs computes such values at the first fire of each block, in one prologue
  at the top of the guard:
  `if (iSlowODDone<n> == 0) { {f|i}SlowOD<k> = …; fPerm<i> = …; iSlowODDone<n> = 1; }`.
  The hold store of such a value is made there, once per block, when nothing
  else in the guard uses its field. The flag, and the values the rest of the
  guard reads, are declared, zeroed, before the loop; the other values are
  locals of the prologue. A value also needed outside the guard's payload stays
  hoisted, as in C++.
- The scope is the boolean `ondemand` (`if (clock)` guard) only. Integer
  `ondemand`, `upsampling` and `downsampling` keep the C++ hoisting, and so
  does external control (`-ec`), whose control section lives in `control()`.
- Compatibility impact:
  - The outputs are bit-identical: a control-rate value is constant within a
    block.
  - The generated text differs from C++'s for such programs (15 corpus
    programs, all `ondemand_*` impulse tests).
  - The work runs at most once per block, and not at all in a block where the
    clock does not fire. This is what `docs/ondemand-note-en.md` promises:
    "the body genuinely does not execute".
- Evidence:
  - `porting/ondemand-block-rate-in-domain-analysis-and-plan-2026-10-17-en.md`
    (point 6 of faust-coopiano's RAPPORT.md);
  - `crates/compiler/tests/ondemand_lazy_block.rs`;
  - the `cpp`, `c`, `wasm`, `interp`, `cranelift`, `cpp-vec0` and `cpp-vec1`
    impulse lanes on the `ondemand_*` corpus.

### DIFF-BEH-025 — `-svg` folds shared sub-diagrams

- Status: `reference-fix` (drawings only; generated code unchanged).
- The pinned C++ (`8eebea429`, 2.84.3) folds only named sub-diagrams into
  files of their own and expands a shared unnamed one at every use, so a
  recursion with shared subproblems draws its expanded tree:
  `cheb(n,x) = 2*x*cheb(n-1,x) - cheb(n-2,x)` at `n = 23` writes 431 MB in
  7.1 s, and the drawing grows like Fibonacci. faust-rs did the same (373 MB
  in 1.3 s), and also expanded a second use of a folded *named* diagram, which
  C++ always links.
- faust-rs now follows upstream `master-dev` #1290 (`6789c5da8`) and its label
  fix `068fb9a80`: a sub-diagram with more than one parent in the box DAG is
  folded like a named one, every use of a folded diagram is a link to its one
  file, its label is its own expression printed within a 1024-node budget
  (`boxes::box_pp_bounded`, C++ `mBoxName`), and its file is
  `diagram-<hex>.svg` (`diagram.svg`, `diagram-2.svg`, ... under `faustprobe
  --svg`). `box_complexity` is memoized per drawing and saturates at 10^9.
- Compatibility impact:
  - `cheb(23)`: 192 KB, 23 files; `cheb(100)`: 808 KB in 0.01 s. Over the
    133 impulse-test programs, 1.28 GB in 14 058 files becomes 191 MB in
    17 536 files; every program still draws.
  - Drawings of programs with shared sub-diagrams have more, smaller files
    than before and than the pinned C++; folded names and labels differ from
    C++'s, whose own printer formats numbers differently.
- Evidence:
  - [`svg-shared-node-folding-port-plan-2026-10-22-en.md`](svg-shared-node-folding-port-plan-2026-10-22-en.md);
  - `crates/draw` tests (`a_shared_node_without_a_name_is_folded_once_and_linked_at_each_use`,
    `a_named_diagram_used_twice_is_drawn_once`,
    `a_fibonacci_shaped_dag_draws_in_linear_size`),
    `crates/boxes/tests/shared_dags.rs`,
    `crates/cranelift-ffi/tests/svg_probe.rs`
    (`a_recursion_with_shared_subproblems_draws_in_linear_size`).

### DIFF-BEH-026 — a variable delay amount with a negative lower bound

- Status: `adapted`.
- C++ `checkDelayInterval` (`sigtype.cpp`) and the delay rule of
  `sigtyperules.cpp` require a delay amount's interval to lie within
  `[0, INT_MAX)` ("possible negative values ... used in delay expression").
  `sigtype::check_delay_interval` accepts a negative lower bound when the
  upper bound is non-negative; variable-delay lowering clamps the index at run
  time. A program C++ refuses can therefore compile in Rust, for instance an
  audio input used as an amount (`_, _ : @`, interval `[-1, 1]`).
- Two intervals are refused nevertheless, being no bound: an upper bound of
  `INT_MAX` or more (as C++ refuses it), and an interval spanning the whole
  integer range, `hi - lo >= 2^32 - 1`, the range the analysis gives `int(x)`
  for an `x` it cannot bound, possibly shifted (`int(x) - 1` is
  `[INT_MIN - 1, INT_MAX - 1]`). Before this, such an amount was accepted and
  sized its delay line to 2^31 samples: an 8 GiB DSP structure, which crashes
  a C++ host that allocates it on the stack while `faustprobe` renders it.
- Related: C++ checks delay amounts on normalized signals, where a delay of a
  delay is folded (`x @ 100 @ (s - 100)` is `x @ (s - 100 + 100)`); faust-rs
  does not apply the delay normal form at the top-level rate and compiles
  `rep_67` through the tolerance above, with the numerical difference of
  `DIFF-GAP-016`.
- Compatibility impact: Rust accepts amounts with a negative lower bound that
  C++ refuses; it refuses, as C++, the amounts the interval analysis could not
  bound, with the diagnostic `delay-interval`.
- Evidence: `sigtype` tests `check_delay_interval_rejects_an_upper_bound_of_int_max_like_cpp`,
  `check_delay_interval_rejects_the_whole_int_range_even_shifted`,
  `check_delay_interval_accepts_negative_lower_bound_with_nonnegative_hi`;
  `crates/compiler/tests/diagnostic_errors.rs`
  (`a_delay_bounded_only_by_the_int_range_is_rejected_like_cpp`,
  `the_same_delay_clamped_to_its_real_range_compiles_with_a_small_line`),
  `crates/transform/src/signal_prepare/tests/typing.rs`
  (`prepare_signals_for_fir_keeps_sampling_frequency_in_delay_amounts_after_simplify`,
  whose sample rate is bounded as `ma.SR` bounds it).

### DIFF-BEH-027 — bargraph inputs are compared before simplification

- Status: `adapted`, 2026-10-26.
- Both compilers make one bargraph per bargraph signal: one bargraph box
  applied to two signals, as in `process = hmeter, hmeter;`
  (`tests/impulse-tests/dsp/vumeter.dsp`), is two meters, each written by its
  own signal. C++ gets this from the signal itself
  (`sigHBargraph(path, min, max, x)` carries `x`, and
  `InstructionsCompiler::generateHBargraph` adds one widget per bargraph
  signal compiled). Rust keys a control by its widget box and group context,
  so the propagation splits it: the first input a bargraph control is
  propagated with keeps it, each other input gets a copy of it, with its own
  zone and a leaf placed after the original's (`BargraphSplits`,
  `crates/propagate/src/ui_build.rs`). Until 2026-10-26 it did not, and every
  backend showed one meter written by both signals, the last write winning
  (one bargraph instead of two in `vumeter.dsp`).
- What remains: C++ compares bargraph inputs after simplification, Rust as
  propagated. Two inputs that only simplification makes equal are one meter in
  C++ and two in Rust, both showing the same value: for
  `m(x) = attach(x, x : hbargraph("m", 0, 1));` and
  `process = _ <: (_ <: + : m), (*(2) : m), (+(1) : -(1) : m);`, C++ shows two
  meters (`2x` and `x`), Rust three (`2x`, `2x`, `x`).
- Not affected: `coutputs(e)` / `coutput(i, e)` (DIFF-SRC-004) list the
  bargraph *boxes* of `e`, so the box of `hmeter, hmeter` is listed once.
- Compatibility impact: none on samples or on programs whose bargraph inputs
  are distinct after simplification. In the residual case the interface
  (`buildUserInterface`, JSON, `-uim` counts) has extra meters.
- Evidence: `crates/compiler/tests/bargraph_per_signal.rs` (the residual case
  is frozen by `bargraph_inputs_equal_only_after_simplification_stay_apart`);
  `a_bargraph_box_gets_one_control_per_input_signal` in
  `crates/propagate/tests/core_api.rs`; counts compared with faust 2.84.3 and
  2.90.15.

### DIFF-BEH-028 — C and C++ helper functions guarded as in C++

- Status: `reference-fix` relative to the pinned `8eebea429` for the wrapping
  helpers; aligned with C++ correction `88b5c93ae` (master-dev, 2026-08-26,
  released in 2.90.x).
- The C and C++ backends write integer add, sub and mul as
  `faust_wrap_add/sub/mul`, which compute on unsigned 32-bit values and
  convert back: signed overflow is undefined in C and C++, and compilers
  exploit it at `-O2`/`-O3` (the integer noise LCG). The pinned reference
  writes plain `+`, `-`, `*`.
- Since 2026-10-26 the helpers are printed under `#ifndef FAUST_INT_WRAP`, as
  `CPPCodeContainer::produceClass` and `CCodeContainer::produceClass` print
  them, so several DSPs, from either compiler, can share one translation unit.
  The C++ helpers are the reference's token for token
  (`inline int faust_wrap_add(int a, int b) { return int((unsigned int)a +
  (unsigned int)b); }`), so these external-linkage `inline` functions stay one
  definition (ODR) across translation units mixing the two compilers. The C
  helpers keep `static inline FAUST_UNUSED` and `uint32_t`: internal linkage,
  one copy per unit, no definition to share.
- The C backend's `faustmaxi` / `faustmini` are printed under
  `#ifndef FAUSTMAXI`, in the reference's order, as both the pinned reference
  and master-dev print them; faust-rs had left them unguarded (an unrecorded
  difference closed the same day). They are `static inline FAUST_UNUSED`, as
  master-dev's `static inline` (`220e37386`, 2026-05-29). The pinned reference
  prints them as plain `inline`, which in C99 is an inline definition without
  an external one: a C file that calls `faustmini` fails to link at `-O0`
  (`_faustmini` not found). That fix is cherry-picked on the local C++ branch
  `fix-faustmaxi-static-inline-FIR19` (from `8eebea429`), not on the pin.
- Before 2026-10-26, two faust-rs C++ classes, or two C DSPs, in one
  translation unit failed with `redefinition of 'faust_wrap_add'` (and of
  `faustmaxi` for C).
- Compatibility impact: none on samples. Generated C/C++ text differs from the
  pinned reference by the wrapping helpers; it matches C++ master-dev's for
  C++, and for C up to the helper bodies and `FAUST_UNUSED`.
- Evidence: `crates/compiler/tests/int_wrap_guard.rs` (the exact text in both
  backends; two C++ classes with a C++ Faust copy of the helpers, and two C
  DSPs, built at `-O0` and run in one translation unit).

## 6. Additional backends and delivery forms

### DIFF-BACK-006 — headers and libraries of foreign functions in C and C++

- Status: `adapted`, 2026-10-10.
- As in C++, the header of every `ffunction` and `fvariable` the program
  reaches, table generators included, is printed once as `#include`, before
  the class, and a header of two characters or fewer (`""`) is not. The
  libraries are printed as a `/* link with : ... */` comment; nothing is
  linked. In FIR they are `DeclareDependency` nodes in `globals`.
- Differences: the program's headers follow the backend's fixed prelude
  (minus the headers it already has) instead of being sorted together with
  it; an empty library is not printed (C++ prints `/* link with : "" */`);
  `ma.SR` (`fconstant(int fSamplingFreq, <math.h>)`) adds no `#include
  <math.h>`, the prelude having `<cmath>` (C++) or `<math.h>` (C).
- A foreign variable is not declared by the generated code: its header
  declares it, as in C++. Until 2026-10-10 the C++ backend declared it as a
  member, which shadowed the header's global. Until 2026-10-18 the rust,
  julia and asc backends declared it as a struct field initialized to zero;
  they now read it by name like c and cpp (test
  `a_foreign_variable_is_read_never_declared_as_state`).
- It replaces `DIFF-GAP-018` (until 2026-10-10 the headers were dropped and
  the generated code calling a custom `ffunction` did not build).
- Evidence: `crates/compiler/tests/foreign_dependencies.rs` builds and runs
  the generated C and C++ (scalar and `-vec`) against the minimal architecture
  with a test-local header; the design is the W3 amendment of
  [the 2026-10-08 audit](foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md#w3-design-amendment-2026-10-10-agreed-with-the-user-implemented).

### DIFF-BACK-001 — Cranelift

- Status: `extension`.
- `-lang cranelift` and the Cranelift factory/JIT APIs are Rust-native; the
  pinned C++ compiler has no direct Cranelift backend.
- The FFI follows the broad Faust factory lifecycle but intentionally omits or
  adapts LLVM-specific target, IR, machine-code, and object families. Its
  `-mem0` manager is an implemented Rust extension using the shared aligned C
  ABI; there is no direct C++ Cranelift oracle.
- Foreign host functions use an explicit registered name-to-address map; a
  header supplied by `ffunction` is not compiled into a JIT binding. Math
  functions run through backend host wrappers, one inventory (`HOST_MATH` in
  `backends/cranelift/host.rs`, 65 C symbols) that the subset pre-check, the
  lowering and the JIT registration all read. The other functions of
  `maths.lib` (`cbrt`, `hypot`, `erf`, `erfc`, `tgamma`, `lgamma`, `logb`,
  `ilogb`, `ldexp`, `scalbn`, `nextafter`, the Bessel `j0` to `yn`; 28 C
  symbols) are the system's C math library (`foreign_call::libm_functions`,
  since 2026-10-11), as the C++ LLVM JIT finds them among the process's
  symbols: the results are those of the C and C++ code generated on the same
  machine. They come after host registrations, which may replace them; the
  Bessel functions are `double` only, `_j0`... under MSVC, and wasm32 has
  none. This is not all of `<math.h>`. A declaration
  names a math function by its C symbol, `float sinf(float)` included since
  2026-10-10. As in the C code the other backends generate, where the
  `<math.h>` declaration decides, the arguments convert to the parameter
  types and the result to the declared one, precision included
  (`float tanhf(float)` under `-double` calls `tanhf`). A different argument
  count is refused, as the generated C fails to compile. So is an `int` result
  declared for a real function or the reverse (`int tanhf(int)`), naming both
  signatures; the generated C++ compiles that one, keeping the real result
  against the Faust typing of the call. A math name now takes precedence over
  a host registration of the same name. A host may give the C signature of
  the function it registers (`registerCCraneliftForeignFunctionWithSignature`,
  `"float(int, float, float)"`, a Rust extension since 2026-10-10); the call
  then follows the same rule against it. Registered by name and address only,
  as C++ `registerForeignFunction` takes a name only, the function is called
  with the types of its `ffunction` declaration, unchecked. A
  call the JIT cannot lower (a foreign function with no registered binding)
  makes the backend's non-strict default emit a no-op `compute`, with
  `compute_body_lowered=false` and the reason in `compute_subset_gap()`.
  Until `2199d069`, `faustprobe` rendered that body's zeros as a successful
  result and `-lang cranelift` exited 0. Both now fail with
  `FRS-CGEN-CLIF-0002`, naming the function: `-lang cranelift` and
  `--dump-cranelift` compile in strict mode, and every probe mode refuses an
  unlowered factory (`JitDspModule::require_lowered_compute`). A JIT panic no
  longer gets past strict mode. The C factory constructors and the bitcode
  restore compile in strict mode too, with no scaffold opt-in (the user's
  decision on
  [the 2026-10-08 audit, W1](foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md#w1--reject-non-executable-probe-factories-p0)):
  factory creation fails and `error_msg` names the function. Since
  2026-10-11 a backend error also has its diagnostics-v2 report in
  `getCCraneliftDSPFactoryErrorDiagnostics`, the CLI's `FRS-CODEGEN-0001`
  with `detail_code` `FRS-CGEN-CLIF-*` (it was text only), and `faustprobe
  --error-format json` prints it. Only the Rust backend API keeps the
  non-strict default, for corpus diagnostics.
- Evidence: [`cranelift-backend-plan-en.md`](cranelift-backend-plan-en.md),
  [`cranelift-dsp-ffi-parity-matrix-en.md`](cranelift-dsp-ffi-parity-matrix-en.md),
  `crates/cranelift-ffi/tests/math_signature_probe.rs` (the C math library
  functions included) and
  `crates/cranelift-ffi/tests/typed_foreign_function_probe.rs`.

### DIFF-BACK-002 — textual WAT and raw embedded compiler module

- Status: `adapted` / `extension`.
- Rust exposes `-lang wast` from its WASM backend and a standalone
  `wasm32-unknown-unknown` compiler module in `crates/wasm-ffi`.
- Textual WAT is rendered from Rust's WASM bytes with `wasmprinter`; it is not a
  byte-for-byte textual-codegen promise against C++. The raw embedded compiler
  module is a Rust delivery extension. Neither form implies full semantic or
  binary-layout parity with the mature C++/Emscripten WASM toolchain.

### DIFF-BACK-003 — backend selector surface

- Status: `narrower` plus extensions.
- The current Rust CLI accepts exactly: `asc`, `c`, `cmajor`, `codebox`,
  `codebox-test`, `cpp`, `cranelift`, `fir`, `interp`, `julia`, `rust`, `wasm`,
  and `wast` (including documented aliases).
- A C++ `-lang` value outside that set is not silently forwarded; it is not a
  Rust CLI backend. Conversely, `cranelift` is Rust-only.
- Several non-primary emitters remain narrower than their mature C++
  counterparts. Generated-code lifecycle compatibility is mandatory where the
  corresponding C++ backend exists, but full long-tail lowering coverage is
  not implied.

### DIFF-BACK-004 — generated C/C++ text is not a byte-identity contract

- Status: `adapted`.
- Rust-generated C and C++ preserve the public DSP/architecture contract on
  supported lowering paths, but do not reproduce the reference emitter's
  complete source text byte for byte. The compiler banner/version, formatting,
  declaration layout, helper decomposition, and some internal statement names
  and ordering can differ.
- Compatibility must therefore be judged through lifecycle, metadata, UI,
  arity, compilation, and execution checks rather than a raw whole-file diff.
  Conversely, a matching module-shell signature alone is not evidence of
  numerical equivalence.
- The semantic metadata defects discovered beneath those text differences were
  closed by the C-family metadata transport work recorded in the corpus audit.

### DIFF-BACK-005 — WASM companion JSON always populates `sr_index`

- Status: `extension`.
- The WASM/WAST companion JSON's optional `sr_index` field is part of the
  shared Faust JSON schema (`architecture/faust/gui/JSONUI.h`) and has a real
  reader on the host side (`JSONUIDecoder.h` uses it to read the sample rate
  straight out of WASM linear memory, the same way `ui[*].index` is used for
  widget parameters). The C++ WASM backend never populates it for this
  backend: `WASMCodeContainer::generateJSON()` constructs its `JSONInstVisitor`
  with `sr_index = -1`, and the shared serializer omits the key whenever the
  value is `-1`.
- Rust always fills it with the byte offset of `fSampleRate` in the module's
  memory layout (`WasmMemoryLayout::field_offsets`), because that offset is
  already computed for other struct-layout purposes and costs nothing extra to
  expose. A host reading the field gets a working, schema-conformant answer;
  a host that does not look for it is unaffected either way.
- This is additive, not a narrowing: no C++-documented consumer is broken by
  the extra key, and nothing the C++ JSON promises is missing from the Rust
  JSON. It is recorded here so a byte-level comparison against the C++
  companion JSON does not read the extra key as drift.
- Evidence: `porting/wasm-julia-maturity-diff-gap-005-analysis-and-plan-2026-08-14-en.md`
  (`G5-W4`).

### DIFF-BACK-007 — host functions called by the interpreter

- Status: `extension`.
- The C++ FBC interpreter calls no host function: an `ffunction` outside its
  math table is an error. faust-rs keeps that table and adds a registry
  (`registerCInterpreterForeignFunction`, name and address), which the
  executor calls through `foreign_call::invoke`.
- Rust has no call whose signature is built at run time, so `invoke` calls
  the signatures it enumerates: up to four arguments, each `int`, `float` or
  `double` in any combination, returning one of these or nothing, and the
  `bool` shapes of its first version (`foreign_call::is_supported`, which the
  bytecode compiler checks). The functions of the system's C math library that
  Cranelift calls (DIFF-BACK-001) are called the same way when no host
  registered the name, checked and converted as typed registrations; the C++
  interpreter has none of them (since 2026-10-11). Until 2026-10-10 it enumerated the homogeneous
  shapes of up to two arguments only, so `jn(int, float)`, `ldexp(float,
  int)` or `ilogb(float) -> int` from `maths.lib` failed to compile ("outside
  the interpreter foreign-call ABI"), and `void(int)`, accepted by the
  compiler, failed at execution. Cranelift calls any signature.
- As for Cranelift (DIFF-BACK-001), a host may give the C signature of the
  function it registers (`registerCInterpreterForeignFunctionWithSignature`,
  since 2026-10-10): the bytecode converts each argument to its parameter's
  kind and calls with the registered types, and a call of another shape is a
  compile error naming both signatures. Bytecode compiled under another
  signature of the function (read back, or cached) fails at the call
  (`foreign_function_signature_changed`). The registered signatures enter
  the factory cache key; without any, the key is the libfaust one.
- Evidence: the tests of `crates/foreign-call/src/lib.rs`;
  `a_mixed_signature_foreign_function_executes_in_interp_ffi`,
  `a_typed_host_function_is_called_with_its_signature`,
  `a_typed_host_function_declared_with_another_shape_is_refused` and
  `a_registered_signature_is_in_the_source_key` and
  `a_c_math_library_function_is_called_by_the_interpreter`
  (`crates/interp-ffi/src/factory.rs`);
  `test_typed_foreign_call_is_refused_after_its_signature_changes`
  (`crates/codegen/src/backends/interp/compiler/tests.rs`).

### DIFF-BACK-008 — WASM struct layout: small fields first, static tables last

- Status: `adapted`.
- The C++ WASM backend builds its field table in declaration order
  (`WASMInstVisitor`, `compiler/generator/wasm/wasm_instructions.hh`).
  faust-rs's WASM emitter lays the runtime prefix out as:
  - `Soundfile*` fields;
  - the mutable fields by increasing byte size, declaration order among equal
    sizes;
  - the static tables;
  - the compute-local arrays.

  `WasmMemoryLayout::from_module`, which the C++-style JSON of the other
  backends uses, keeps the declaration order.
- Why: V8 encodes a memory offset up to 32,760 bytes in the load itself and
  builds a larger one with one or two extra instructions. The declaration
  order puts every table and delay line before the per-sample state. On the
  faust-coopiano voice the new order removes 34 % of the instructions per
  sample; outputs are bit-identical.
- Compatibility impact:
  - The JSON `index` values of a `-lang wasm` module (and its `size`-relative
    layout) differ from those Faust C++ emits for the same program.
  - Hosts that read `ui[*].index` from the module's own JSON are unaffected,
    as are `getParamValue`/`setParamValue` and the I/O zone at `size`.
  - A host that hard-codes C++ offsets, or pairs a faust-rs `.wasm` with a C++
    JSON, breaks. Mixing is already unsupported (`wasm/mod.rs` header).
- Evidence: W5 of
  [`wasm-compute-speed-analysis-and-plan-2026-10-16-en.md`](wasm-compute-speed-analysis-and-plan-2026-10-16-en.md);
  tests `wasm_emitter_lays_out_small_fields_first_and_static_tables_last` and
  `wasm_json_indices_follow_the_emitted_layout` in
  `crates/codegen/src/backends/wasm/tests.rs`.

### DIFF-BACK-009 — `compute` scheduled for the JIT backends (W2/W3/W7)

- Status: `adapted`.
- The interp, Cranelift and wasm backends emit `compute` after a FIR-to-FIR
  pass, `fir::schedule::schedule_compute`; C, C++ and the other text backends
  keep FIR order, as Faust C++ does for every backend.
  - W3: after a store of a single-assigned local or a constant to a constant
    struct address, a later load of that address in the same straight-line
    region reads the stored value instead of memory.
  - W2: a pure declaration (`double fTemp36 = fRec2[1];`) is emitted shortly
    before its first reader — eight solid statements before it — or before
    the first statement that may write what it reads.
  - W7: stack scalars whose lifetimes do not overlap share one variable
    (`fReuseN`, `iReuseN`...) declared at the top of `compute`; their
    declarations become stores to it. faust-coopiano's voice goes from 37,720
    wasm locals in `compute` to 2,177.
  - Each rewrite is accepted only if an independent checker re-derives it;
    otherwise `compute` keeps FIR order. `keep_fir_order` in `InterpOptions`,
    `CraneliftOptions` and `WasmOptions` turns the pass off (for differential
    tests).
- Why: V8 (TurboFan) keeps wasm loads in program order and does no load
  elimination on linear memory, so a state read at the top of the sample stays
  live across the loop body and is spilled; LLVM, which compiles Faust C++'s
  output, sinks and forwards on its own. Measured on 2026-10-17 (wasm, cycles
  per sample in V8): faust-coopiano's light body −35 %, imaged body −10 %,
  voice −7 %, a 400-mode resonator bank −16 %; Cranelift (faustprobe) compute
  −17 % to −31 % on the same programs. W7: JavaScriptCore (Safari) gives each
  wasm local its own stack slot, and the voice's `compute` overflowed the
  512 KB stack of Safari's AudioWorklet thread (the page was silent); after
  W7 the whole faust-coopiano processor runs in 256 KB under `jsc`, and
  JavaScriptCore computes the voice 17 % and the light body 11 % faster. V8,
  Cranelift and the interpreter run at the same speed.
- Compatibility impact: none on outputs, which are bit-identical (impulse
  tests of the three backends, faust-coopiano's modules, faustprobe `--ref`).
  The generated wasm, interpreter bytecode and Cranelift code differ from Faust
  C++'s in statement order and in some loads. Compile time of the imaged body
  rises by about 10 % (wasm) and 12 % (Cranelift) with W2/W3, and by 3 % more
  with W7.
- Evidence:
  [`wasm-w2-w3-sinking-forwarding-design-2026-10-17-en.md`](wasm-w2-w3-sinking-forwarding-design-2026-10-17-en.md),
  [`fir-local-reuse-w7-design-2026-10-17-en.md`](fir-local-reuse-w7-design-2026-10-17-en.md);
  tests in `crates/fir/src/schedule/tests.rs` and `tests_reuse.rs` (producers
  and the checkers' mutations) and `crates/compiler/tests/fir_schedule.rs` (interpreter outputs
  with and without the pass, FIR verification).

## 7. Public API and representation adaptations

### DIFF-API-001 — owned Rust compiler facade

- Status: `adapted`.
- Rust entry points return owned `Result` values and artifact bundles instead
  of exposing the C++ global compiler session, raw factory pointers, or only
  filesystem side effects.
- WASM results include bytes, JSON, compile provenance, and optional warning
  bundles. Auxiliary generation returns a list of owned text/binary artifacts.
- Compatibility impact: semantic outputs are preserved where implemented, but
  Rust signatures and ownership are not source- or ABI-compatible with C++.

### DIFF-API-002 — explicit provenance and diagnostics side tables

- Status: `extension` / `adapted`.
- Signal and FIR outputs carry source-neutral provenance maps; compiler
  diagnostics join those maps with parser source snapshots when rendered.
- C++ commonly stores mutable properties on globally shared trees. Rust keeps
  provenance and analysis results in explicit session-owned structures.
- Compatibility impact: this changes public Rust result types and enables
  richer diagnostics, without changing the intended DSP semantics.

### DIFF-API-003 — Cranelift factory serialization

- Status: `adapted`.
- The Cranelift FFI's bitcode-named persistence surface uses a versioned
  source-backed payload that rebuilds the factory. It is not LLVM bitcode.
  IR/machine/object serialization remains deferred without exported V1 symbols.
- Evidence:
  [`cranelift-dsp-ffi-parity-matrix-en.md`](cranelift-dsp-ffi-parity-matrix-en.md).

### DIFF-API-004 — `libfaust.h` as inline C++ wrappers

- Status: `adapted`.
- The backend-agnostic libfaust API (`generateSHA1`,
  `expandDSPFrom{File,String}`, `generateAuxFilesFrom{File,String}[2]`) is
  exported as C symbols from `crates/libfaust-ffi`; the C++ header
  [`crates/libfaust-ffi/include/libfaust.h`](../crates/libfaust-ffi/include/libfaust.h)
  reproduces the reference signatures as header-only inline wrappers over that
  C ABI.
- This is forced, not chosen: the reference functions take and return
  `std::string`, whose symbols are mangled and whose layout has no stable ABI,
  so Rust cannot export them. `libfaust-box.h` already uses the same shape.
- Each wrapper adopts its returned `const char*` and releases it through
  `freeCMemory` before returning, so no allocation crosses back over the
  boundary and callers see only `std::string`.
- Compatibility impact: source-compatible for C++ callers that include the
  header; not ABI-compatible with a C++ libfaust built from the reference
  sources, since the symbols are the C ones.
- Evidence: `cargo run -p xtask -- libfaust-export-check`, which links and runs
  a C++ client of this header against the built library.

### DIFF-API-005 — alignment-aware `dsp_memory_manager` overloads under `-mem0`

- Status: `adapted`.
- The pinned reference's `dsp_memory_manager` (`architecture/faust/dsp/dsp.h`)
  declares only `allocate(size_t)` and `destroy(void*)`; unlike the Rust-only
  C ABI (`faust_memory_manager` in `crates/ffi-common/include/faust-memory-manager.h`,
  DIFF-CLI-010's M4), it never asks for alignment on `allocate` and never hands
  size/alignment back on `destroy`. Generated C++ under `-mem0` now additionally
  recognizes `allocate(size_t, size_t)` and `destroy(void*, size_t, size_t)` — a
  faust-rs `dsp_memory_manager` extension — and prefers them over the legacy
  overloads whenever the linked header declares them.
- Mechanism: every allocation/destruction call site routes through a
  `faust_mem0_detail::{allocate,destroy}` compile-time dispatch shim (one
  `int`/`long`-tagged SFINAE overload pair per operation) emitted once per
  `-mem0` translation unit. SFINAE probes the static type of
  `dsp_memory_manager`, not the concrete manager subclass, so the choice is
  made once at compile time with no runtime branch. Against the genuine,
  unextended upstream header the probe fails and generated code falls back to
  the legacy one-argument overloads unchanged — this is a strict superset, not
  a replacement, and needs no host-side change to keep compiling.
- A manager may adopt the extension two ways: declare the two additive
  overloads with a body that forwards to the legacy ones (so an existing
  subclass that overrides only `allocate(size_t)`/`destroy(void*)` keeps
  working through the base class's default, exercised by
  `mem0_generated_cpp_compiles_and_clone_is_independent`), or override the
  additive overloads directly to receive the requested alignment up front and
  the original size/alignment pair back on release (exercised by the same
  test's `aligned_manager`, which asserts the legacy overloads are never
  reached). `tests/impulse-tests/archs/faust_mem0.h`'s
  `AuditCppMemoryManager` does the latter, additionally auditing that the
  returned address satisfies the requested alignment and that `destroy`'s size
  argument matches what was recorded at allocation.
- The legacy one-argument overloads are the obsolete path from here on: kept
  for source compatibility with hosts still built against the unextended
  upstream header, and marked as such in the generated comment inside
  `faust_mem0_detail`. New memory managers should implement the alignment-aware
  overloads instead.
- Compatibility impact: none for existing hosts — an unmodified reference
  `dsp_memory_manager` still compiles and runs generated `-mem0` C++ code
  unchanged, proven by
  `mem0_generated_cpp_compiles_against_the_unextended_legacy_manager_header`,
  which defines the header exactly as documented in the reference manual and
  nothing else. Hosts that want the richer contract opt in by declaring the
  two additive overloads; nothing in the generated code requires it.
- Evidence: `crates/codegen/src/backends/cpp/mod.rs`
  (`emit_mem0_detail_namespace`), `tests/impulse-tests/archs/faust_mem0.h`,
  and the three compile-and-run C++ tests named above in
  `crates/codegen/src/backends/cpp/mod.rs`.

### DIFF-API-006 — the complete text of an error, and its typed form, through the C API

- Status: `extension`.
- The reference C API reports a failure through `error_msg`, a buffer the caller
  allocates and whose size is never passed: 4096 bytes by contract. Rust keeps
  that contract exactly (the buffer receives what it received, truncated at
  4095 bytes), because writing more would overflow every existing host. A
  compiler diagnostic does not always fit, and for a syntax error the summary
  the buffer gets is a count.
- Five functions, with no reference counterpart, return the whole text (the
  message, then the rendered diagnostics: location, source snippet, notes,
  fixes) from per-thread storage the library owns, the `dlerror` convention:
  `getCCompleteInterpreterDSPFactoryError`,
  `getCCompleteCraneliftDSPFactoryError`, `getCCompleteDSPError`
  (`libfaust-c.h`), `getCCompleteBoxError`, `getCCompleteSignalError`. The
  pointer is null before the thread's first error, valid until its next one,
  not reset by a success, and must not be freed.
- The C++ wrappers (`interpreter-dsp.h`, `cranelift-dsp.h`, `libfaust.h`) read
  them, so their `std::string& error_msg` holds the complete text, where the
  reference's holds its own one-line message. A host that parses that string
  must not assume one line.
- Four more functions, with no reference counterpart either, return the typed
  form of the same failure, the compiler's complete diagnostics-v2 JSON report
  (codes, byte ranges in each source, facts, fixes with their edits and
  applicability): `getCInterpreterDSPFactoryErrorDiagnostics`,
  `getCCraneliftDSPFactoryErrorDiagnostics`, `getCDSPErrorDiagnostics`,
  `getCBoxErrorDiagnostics`. Same storage and lifetime, with one difference:
  the pointer is null when the last error carried no typed diagnostics, even
  if an earlier one did. The Signal API has none, no failure of it being
  typed. Exporting the report ties the ABI to a document, not to its fields:
  it carries its own `schema_version` (2), fields may be added within a
  version, and a host is told to check the version. The C++ wrappers return it
  as a `std::string`; `faustprobe --error-format json` prints the Cranelift one.
- Compatibility impact: none for a host written against the reference API; the
  functions are additions and the buffer's behavior is unchanged.
- Evidence: `crates/ffi-common/src/complete_error.rs`, the `complete_error`
  tests of the FFI crates (which apply the report's fix and compile the
  result), `cargo run -p xtask -- libfaust-export-check`, and
  the workspace README, "Compile errors".

### DIFF-API-007 — the compiler options a factory constructor honors

- Status: `adapted` (narrower).
- The reference `createDSPFactoryFromFile` / `FromString` hand `argv` to the
  whole compiler command line. The Rust constructors (Cranelift and
  Interpreter), the Wasm `compile_dsp` entry and `generateAuxFiles` read two
  sets from it. The compile options are `faust-rs`'s own declaration,
  `compiler::CompileOptionArgs`, read by `CompileOptionArgs::from_argv` in
  every spelling `faust-rs` accepts: `-pn`, `-double`/`-single`,
  `-vec`/`-vs`/`-lv`, `-ss`, `-mcd`, `-dlt`, `-ct`, `-table-init`,
  `--table-init-sample-rate`, `-bra-tape`, `-ec`, `-os`, `-mem0`. The host
  options are read by `ffi_common::parse_ffi_compile_args`: `-I`, `-cn`,
  `-mem0` and its aliases (with the `-vec` and `-it` conflicts), `--warn`. A
  missing or malformed value is an error, `generateAuxFiles` included.
- Any other option is ignored, where the reference applies it or rejects it.
  Until 2026-09-25 each entry point read its own subset: the constructors
  dropped `-pn`, `-mcd`, `-dlt` and `-ct` (a host asking for another entry
  point got `process` without a word), the Interpreter also `-vec`, `-ss`,
  `-ec` and `-os`, `compile_dsp` everything but `-double`, and
  `generateAuxFiles` kept the default for a malformed value.
- Compatibility impact: a host passing an option outside the declaration gets
  the default behavior for it. Options that describe an output (`-o`,
  `-lang`, `-a`, the dumps) have no effect in either implementation.
- Evidence: `crates/compiler/src/compile_options.rs` (declaration, `argv`
  reading and writing, tests), `crates/ffi-common/src/args.rs` (host options),
  `process_name_delay_and_table_options_reach_the_compiler` in
  `crates/cranelift-ffi/src/factory/tests.rs` (each option changes the FIR),
  `the_compile_options_of_the_argv_reach_the_interp_compiler` in
  `crates/interp-ffi/src/factory.rs`,
  `the_compile_options_of_the_args_reach_the_compiler` in
  `crates/wasm-ffi/src/lib.rs`, `crates/cranelift-ffi/tests/compiler_options_probe.rs`.

### DIFF-API-008 — the `faust` crate, a Rust API over the C API

- Status: `extension`.
- `crates/faust` has no C++ counterpart: a safe Rust API (`Factory`, `Dsp`,
  `Param`, `CompileOptions`) over the Interpreter and Cranelift C entry
  points, which it drives with their exact lifecycle (programs shared by SHA
  key, reference counted). It is one of the two contracts of faust-rs, with
  the C API; `ffi-boundary-check` classifies it as a distribution crate.
- Its methods carry the names of the C++ `dsp` and `dsp_factory` classes
  (`dsp.h`) in snake case, as the `FaustDsp` trait of the Rust architectures
  does (`get_num_inputs`, `instance_clear`, `create_dsp_instance`, ...).
  Where it differs from the C API it wraps: a `Dsp` owns a reference to its
  program and is `Send + Sync`; parameters (`Param`, listed by `Dsp::params`)
  are addressed as `MapUI` addresses them (`set_param_value`, `get_param_value`: by path, shortname or label,
  shortnames computed by a port of `PathBuilder::computeShortNames`) instead
  of a `UI` the host implements, and a bargraph cannot be written; `create_dsp_instance` also
  initialises; `compute` is generic over `f32` and `f64` and converts host
  buffers to the compiled precision, and a `-double` interpreter program exchanges exact `f64`
  samples through `interp_ffi::instance::compute_f64`, which the C API does
  not export; `CompileOptions::args` alone selects `-double` or `-single`,
  and `Factory::precision` reports the backend's compiled width. The former
  `CompileOptions::precision` field was removed after
  [issue #19](https://github.com/grame-cncm/faust-rs/issues/19) showed that a
  conflicting flag could make the safe API access buffers and UI zones at
  the wrong width. Existing Rust clients using the field must move their
  precision choice into `args`; C and C++ clients are unaffected.
  `CompileOptions::import_dirs` lists directories first-first,
  where a command line's `-I` puts the last one first (it is emitted
  backwards so the C entry points see the C++ order).
- Compatibility impact: none on C and C++ hosts; a Rust host gets a
  documented, typed API instead of raw pointers.
- Evidence: `crates/faust/tests/api.rs` (both backends, including
  `precision_flags_control_factory_buffers_and_parameter_zones` and
  `import_dirs_are_searched_in_order_and_before_the_file_s_directory`),
  `crates/faust/tests/ddsp.rs`, `crates/faust/tests/allocation.rs`,
  [`docs/embedding-layers-en.md`](../docs/embedding-layers-en.md).

### DIFF-API-009 — `libfaust-box.h` and `libfaust-signal.h` cover a subset of the reference

- Status: `narrower`.
- The C APIs (`libfaust-box-c.h`, `libfaust-signal-c.h`) follow the
  reference. The C++ headers
  [`crates/box-ffi/include/libfaust-box.h`](../crates/box-ffi/include/libfaust-box.h)
  and
  [`crates/signal-ffi/include/libfaust-signal.h`](../crates/signal-ffi/include/libfaust-signal.h)
  are inline wrappers over them (see `DIFF-API-004` for why) and wrap mainly
  the constructors. Compared after preprocessing with the pinned reference's
  `architecture/faust/dsp/libfaust-box.h` and `libfaust-signal.h`, they do not
  declare:
  - Box: most of the 78 `isBox*` overloads — 13 are wrapped (`isBoxAbstr`,
    `isBoxAccess`, `isBoxAppl`, `isBoxButton`, `isBoxInt`, `isBoxReal`, ...),
    none of the one-argument predicate forms (`isBoxAbstr(Box t)`) —,
    `DSPToBoxes`, `boxesToSignals`/`2`, `createSourceFromBoxes`,
    `getBoxType`, `boxPrim2`, `extractName`, `ffname`/`ffarity`,
    `prim0name`–`prim5name`, `getDefNameProperty`;
  - Signal: most `isSig*` matchers (`isSigHSlider`, `isSigRDTbl`,
    `isSigWRTbl`, `isSigSoundfile*`, `isSigControl`, `isSigEnable`, ...),
    `getSigInterval`/`setSigInterval`, `getSigNature`, `sigBranches`,
    `xtendedName`/`xtendedArity`, `printBox`/`printSignal`.
- Box labels are `const char*` where the reference takes
  `const std::string&` (`boxVSlider`, `boxHBargraph`, `boxVGroup`, ...): a
  call with a string literal compiles against both, one with a `std::string`
  needs `.c_str()` here.
- Closed 2026-10-18: the applied constructors (`boxAdd(Box, Box)`,
  `boxSin(Box)`, `boxDelay(Box, Box)`, `boxSelect2(Box, Box, Box)`,
  `boxHBargraph(label, min, max, x)`, `boxAttach(Box, Box)`, ... — 50
  overloads) existed only under the C spelling `boxAddAux(Box, Box)`, so a
  reference client composing boxes that way did not compile. The reference
  spelling is now declared, the `...Aux` forms are kept as aliases, and
  `boxPar3`–`boxPar5` are wrapped. A C++ client of the built library
  (`box-overloads.cpp` in `libfaust-export-check`) checks each applied
  overload against the tree the reference builds, `(x, y) : prim`. It found
  that `CboxPar3`–`CboxPar5` nested to the left where the reference nests to
  the right, fixed in `2f0d598b`.
- The headers also declare the Rust-only `CboxFad`/`CboxRad` constructors of
  `DIFF-SRC-001`/`002` in the C API.
- Compatibility impact: C clients are unaffected. A C++ client of the
  reference headers that uses the constructors compiles; one that uses a
  missing function must call its C name (`CisBoxHSlider`, `CboxesToSignals`,
  ...) instead.
- Evidence: the comparison above (reference `8eebea429`); the headers'
  syntax check and the `box-overloads.cpp` client in
  `cargo run -p xtask -- libfaust-export-check`.

## 8. Internal architectural adaptations

These differences normally preserve Faust semantics, but matter to maintainers
and to consumers of Rust crate APIs.

### DIFF-ARCH-001 — session-owned compiler state

- Status: `adapted`.
- Rust replaces C++ `gGlobal`-style shared mutable compiler state with values
  owned by a compiler session and passed across crate boundaries.
- Cancellation, timing, diagnostics, imports, provenance, and lowering options
  are explicit inputs or owned outputs rather than implicit process globals.
- Compatibility impact: concurrent and embedded use has a Rust ownership
  contract; internal C++ globals and their mutation order are not an API to
  reproduce.

### DIFF-ARCH-002 — hash-consed arenas and stable ids

- Status: `adapted`.
- Box, Signal, and FIR structures use arena-owned ids and canonical
  builder/matcher APIs rather than exposing raw `Tree` pointers.
- Analyses that C++ stores as mutable tree properties commonly use explicit
  maps keyed by these ids. Pointer order is therefore not a textual ordering
  oracle; deterministic Rust ordering is used where output stability matters.

### DIFF-ARCH-003 — crate consolidation differs from C++ directories

- Status: `adapted`.
- The port intentionally merges C++ `patternmatcher` responsibilities into
  `eval`, integrates extended math signal nodes into `signals`, and integrates
  parallelization work into `transform`.
- Compatibility impact: source-file/module names and internal linkage are not
  preserved. Public behavior and documented external contracts remain the
  parity targets.

### DIFF-ARCH-004 — fallible typed phase boundaries

- Status: `adapted`.
- Rust phases return typed `Result`/diagnostic values and run FIR verification
  before code generation by default. C++ paths often rely on exceptions,
  mutable error strings, assertions, or later backend validation.
- Compatibility impact: failures can be detected earlier and carry a different
  stage/code while still representing the same invalid source or unsupported
  operation.

### DIFF-ARCH-005 — independent structural checking

- Status: `extension`.
- Scheduling, vector planning, FIR lifecycle, and selected routing artifacts
  have explicit checkers; finite high-risk artifacts may use the repository's
  producer/checker certificate methodology.
- This is additional assurance infrastructure. It is not evidence that the
  Rust implementation is formally proved equivalent to C++.

## 9. Known narrower behavior and exclusions

These entries are differences, not intentional semantic extensions. They must
remain visible until closed or explicitly reclassified.

| ID | Status | Current difference |
|---|---|---|
| DIFF-GAP-001 | `narrower` | Full non-trivial stream-wrapper lowering remains less complete than the mature C++ route; the shared-runtime differential confirms different outputs for `rep_18_stream_wrappers` under impulse, ramp, and sine inputs. |
| DIFF-GAP-004 | `narrower` | `getInfos` and some embedded-compiler/filesystem helper semantics remain partial or adapted. |
| DIFF-GAP-005 | `narrower` | WASM and Julia are functional on validated paths but do not claim the complete semantic, layout, packaging, or upstream impulse-suite maturity of the C++ implementations. The two backends were audited on 2026-08-14 and this blanket entry is now backed by an enumerated inventory (`G5-W1`…`G5-W6`, `G5-J1`…`G5-J7`) in [`wasm-julia-maturity-diff-gap-005-analysis-and-plan-2026-08-14-en.md`](wasm-julia-maturity-diff-gap-005-analysis-and-plan-2026-08-14-en.md). Closed so far: Julia's host-visible description surfaces — `metadata!` and `getJSON` now carry the same content as the reference (`G5-J1`, `G5-J2`, `G5-J3`) — the WASM companion JSON identity entries, `compile_options`/`filename`/`name` now appear in `meta` in C++ key order (`G5-W2`) — `sr_index` (`G5-W4`), which needed no code change, only its own registry entry: `DIFF-BACK-005` — and the impulse status table (`G5-J7`), re-derived on the full 133-DSP corpus (`tests/impulse-tests/README.md` §Status now has a Julia row; see the plan doc's P6 for the `FAUST_CPP` PATH-resolution trap that re-deriving it surfaced). Still open: `G5-W1`, `G5-W3`, `G5-W5`, `G5-W6`, `G5-J4`. This entry may only be removed when the inventory is empty; until then it must point at it. |
| DIFF-GAP-006 | `narrower` | The specialized reverse-time recursive RAD path is disabled; Rust uses `BlockReverseAD` for temporal/recursive AD and still rejects mutable-table, soundfile, and unsupported foreign-function derivatives. |
| DIFF-GAP-007 | `narrower` | The Rust CLI/backend set is smaller than the complete Faust C++ backend catalog; unsupported `-lang` values are rejected. |
| DIFF-GAP-008 | `excluded` | `backend-java` is outside the Rust port target scope. |
| DIFF-GAP-009 | `excluded` | Legacy `-lang ocpp` is outside the Rust port target scope. |
| DIFF-GAP-010 | `narrower` | There is no direct Rust LLVM backend matching the C++ LLVM factory/target/object toolchain; Cranelift is a distinct extension, not a drop-in identity mapping. |
| DIFF-GAP-014 | `narrower` | `rep_19_primitive_family` has different numerical results for the `control`/`enable` wrapper portion under the shared interpreter-runtime differential. |
| DIFF-GAP-015 | `narrower` | `rep_37_table_rwtable_negative_indices` has different numerical behavior for negative read/write table indices. |
| DIFF-GAP-016 | `narrower` | `rep_67_variable_delay_shifted_slider` differs for a variable delay whose shifted slider produces a negative intermediate delay expression. |
| DIFF-GAP-017 | `parity-gap` | A division by a zero that is constant without being a literal when its sequence is evaluated (`z = 0 <: _, !;`), used as a **pattern-matching argument whose value is then unused** (`f(0) = 1; f(n) = 2; process = f(1 / z);`), compiles in Rust and fails in C++ (`ERROR : division by 0 in 1 / 0`): C++ simplifies the argument eagerly and its exception is fatal, Rust folds a pattern argument as an optimization and gives up on this one. With a literal divisor both fail, and wherever the quotient is used (a signal, an iteration count, a route size, a label) Rust reports the division too. Pinned by `a_known_divergence_an_unused_late_zero_division_in_a_pattern_argument` in `crates/compiler/tests/diagnostic_errors.rs`; see `DIFF-BEH-010`. |
| DIFF-GAP-020 | `narrower` | The automatic mathematical documentation of C++ `-mdoc` (`compiler/documentator/`) is not ported: `faust-rs` has no `-mdoc` option and rejects it as an unknown argument, and `crates/doc`, the crate the porting plan assigns to it, is an empty scaffold. |

For a time-stamped quantitative snapshot rather than this durable registry,
use [`faust-rs-supported-faust-subset-en.md`](faust-rs-supported-faust-subset-en.md),
the reports under `porting/phases/`, and `tests/golden/METADATA.toml`.

The 219-case audit that exposed the now-closed root-ordering, source-identity,
and compilation-metadata gaps and the maintained numerical differential that
confirmed `DIFF-GAP-001` and exposed `DIFF-GAP-014` through `016` are recorded in
[`faust-rs-corpus-difference-audit-2026-08-11-en.md`](faust-rs-corpus-difference-audit-2026-08-11-en.md).

## 10. Maintenance rule

Update this file in the same change whenever any of the following occurs:

1. a new source form, CLI option, backend, FFI function, artifact, warning, or
   diagnostic is added without a 1:1 C++ counterpart;
2. a default, accepted input, lifecycle step, scheduling rule, generated-code
   contract, failure policy, or output representation intentionally differs;
3. a public mapping changes between `1:1`, `adapted`, `deferred`, or excluded;
4. a known gap closes, narrows, expands, or becomes an explicit non-goal;
5. the pinned C++ reference changes and invalidates a comparison here.

For each update:

- add or retain a stable `DIFF-*` identifier;
- state the compatibility impact, not just the implementation detail;
- link the controlling plan, API matrix, test, or corpus fixture;
- distinguish implemented behavior from planned behavior;
- update or remove stale entries when parity is reached;
- record the change in the daily journal.

Reviews of parity-sensitive changes must treat an omitted registry update as a
documentation defect, even when the implementation and tests are otherwise
correct.
