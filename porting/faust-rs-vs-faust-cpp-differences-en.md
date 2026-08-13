# `faust-rs` Differences From Faust C++

Status: living compatibility registry

Last reviewed: 2026-08-13

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
- Compatibility impact: DSPs using this form are `faust-rs` programs and must
  not be expected to compile with the pinned C++ compiler.
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
- Compatibility impact: the source form and `BlockReverseAD` semantics have no
  pinned C++ source/backend oracle. Validation uses symbolic identities,
  FAD/RAD agreement, finite differences, and optimized/unoptimized runtime
  comparisons.
- Evidence: [`docs/rad-note-en.md`](../docs/rad-note-en.md),
  [`docs/rad-usage-en.md`](../docs/rad-usage-en.md), and
  `crates/compiler/tests/rad_runtime.rs`.

### DIFF-SRC-003 — AD across clock-domain constructs

- Status: `extension`.
- Difference: Rust defines and checks FAD augmentation across supported
  `ondemand`, upsampling, and downsampling boundaries. This is a composition of
  Rust AD with clock-domain machinery, not a parity claim against C++.
- Compatibility impact: portable Faust code should not depend on these AD
  combinations unless it explicitly targets `faust-rs`.
- Evidence:
  [`ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md`](ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md).

## 4. Command-line additions and differences

### 4.1 Rust-only code-generation options

| ID | Rust option | Status | Difference and compatibility impact |
|---|---|---|---|
| DIFF-CLI-001 | `--table-init runtime|const` | `extension` | C++ behavior corresponds to `runtime`: every generated table is filled by a sub-container during initialization. Rust additionally keeps `const`, which evaluates the generator during compilation and emits literal table contents. The default is `runtime`. |
| DIFF-CLI-002 | `--table-init-sample-rate HZ` | `extension` | Required when `--table-init const` folds a generator that reads `ma.SR`. The positive integer is embedded permanently in the table. There is no implicit default. Under `--warn`, `FRS-COMP-0006` reports the frozen value and suggests `runtime` when the host SR must remain authoritative. |
| DIFF-CLI-009 | `-e` / `--export-dsp` | `1:1` | Expands a DSP into a self-contained program, as C++ `-e`. `-lang` is accepted and recorded in `compile_options`, matching C++, since it selects a backend the expansion does not use; two emitters (`-e --dump-cpp`) still conflict. With no `-o` faust-rs prints to standard output, where C++ produces nothing; see `DIFF-BEH-006` for the full list of deviations. |
| DIFF-CLI-003 | `--dlt N` | `extension` | Selects the delay-line threshold at which Rust changes from power-of-two circular storage to exact-size if-wrapped storage. The compiler records it in `compile_options`; the pinned C++ CLI has no direct counterpart. |
| DIFF-CLI-010 | `-mem` / `-mem0` / long aliases | `adapted` | The four C++ mode-zero spellings normalize to one typed per-request `MemoryManagerMode::Mem0` and canonical `-mem0` metadata. The option is intentionally limited to scalar C, C++, and native Cranelift; C and Cranelift are Rust extensions, while `mem1`–`mem3`, vector mode, `-it`, and other backends fail closed. M1 implements parsing/propagation only; storage behavior lands in the later phases of the authoritative [mem0 plan](custom-memory-manager-mem0-analysis-and-porting-plan-2026-08-13-en.md). |

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
  `--error-verbosity concise|standard|debug|full`, `--diagnostic-paths`, and
  `--help-error-format`.

These modes may be used by tests, CI, IDEs, and integrations, but scripts that
must run unchanged with Faust C++ should not pass them.

### 4.3 Adapted option behavior

| ID | Surface | Status | Difference and compatibility impact |
|---|---|---|---|
| DIFF-CLI-004 | `--warn` | `adapted` | It covers warning classes related to C++ `-wall`/`-me`, but returns structured, source-linked diagnostics through the Rust warning bundle. Warnings are opt-in and never change successful compilation into failure. |
| DIFF-CLI-005 | `-ss N` parsing | `adapted` | Rust uses typed `clap` parsing: missing, non-integer, or negative values are errors. C++ `atoi`-style parsing can silently produce `0`. Rust applies the decoded scheduling strategy consistently to scalar and checked-vector scheduling. |
| DIFF-CLI-006 | compilation-options text | `adapted` | Rust prints flags only when they differ from the CLI default, except precision, which is always printed. C++ prints some defaults, notably `-mcd`, unconditionally. Rust also records its own `-table-init`, `--table-init-sample-rate`, and `-dlt` settings. |
| DIFF-CLI-007 | `-ec`/`-os` capability validation | `adapted` | Rust rejects unsupported backend combinations and block-sensitive one-sample programs with stable typed diagnostics. `-ec` on FIR is intentionally exposed as a Rust diagnostic/inspection extension. |
| DIFF-CLI-008 | unknown or invalid combinations | `adapted` | The Rust CLI generally fails early through typed option validation rather than relying on permissive legacy parsing or a later backend failure. Scripts depending on C++ option coercion must be corrected. |

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
  freezing have dedicated Rust diagnostics.
- Evidence: [`docs/faust-error-model-en.md`](../docs/faust-error-model-en.md)
  and
  [`docs/diagnostics-codes-reference-en.md`](../docs/diagnostics-codes-reference-en.md).

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

## 6. Additional backends and delivery forms

### DIFF-BACK-001 — Cranelift

- Status: `extension`.
- `-lang cranelift` and the Cranelift factory/JIT APIs are Rust-native; the
  pinned C++ compiler has no direct Cranelift backend.
- The FFI follows the broad Faust factory lifecycle but intentionally omits or
  adapts LLVM-specific target, IR, machine-code, object, memory-manager, and
  foreign-registration families.
- Evidence: [`cranelift-backend-plan-en.md`](cranelift-backend-plan-en.md) and
  [`cranelift-dsp-ffi-parity-matrix-en.md`](cranelift-dsp-ffi-parity-matrix-en.md).

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
| DIFF-GAP-005 | `narrower` | WASM and Julia are functional on validated paths but do not claim the complete semantic, layout, packaging, or upstream impulse-suite maturity of the C++ implementations. |
| DIFF-GAP-006 | `narrower` | The specialized reverse-time recursive RAD path is disabled; Rust uses `BlockReverseAD` for temporal/recursive AD and still rejects mutable-table, soundfile, and unsupported foreign-function derivatives. |
| DIFF-GAP-007 | `narrower` | The Rust CLI/backend set is smaller than the complete Faust C++ backend catalog; unsupported `-lang` values are rejected. |
| DIFF-GAP-008 | `excluded` | `backend-java` is outside the Rust port target scope. |
| DIFF-GAP-009 | `excluded` | Legacy `-lang ocpp` is outside the Rust port target scope. |
| DIFF-GAP-010 | `narrower` | There is no direct Rust LLVM backend matching the C++ LLVM factory/target/object toolchain; Cranelift is a distinct extension, not a drop-in identity mapping. |
| DIFF-GAP-014 | `narrower` | `rep_19_primitive_family` has different numerical results for the `control`/`enable` wrapper portion under the shared interpreter-runtime differential. |
| DIFF-GAP-015 | `narrower` | `rep_37_table_rwtable_negative_indices` has different numerical behavior for negative read/write table indices. |
| DIFF-GAP-016 | `narrower` | `rep_67_variable_delay_shifted_slider` differs for a variable delay whose shifted slider produces a negative intermediate delay expression. |
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
