# WASM and Julia Backend Maturity (`DIFF-GAP-005`) — Analysis and Plan

Date: 2026-08-14

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`
(`/Users/letz/Developpements/RUST/faust`)

Registry entry closed/narrowed by this work:
[`faust-rs-vs-faust-cpp-differences-en.md`](faust-rs-vs-faust-cpp-differences-en.md)
§9, `DIFF-GAP-005`:

> WASM and Julia are functional on validated paths but do not claim the complete
> semantic, layout, packaging, or upstream impulse-suite maturity of the C++
> implementations.

## 1. Purpose

`DIFF-GAP-005` is the only remaining `narrower` entry that names two whole
backends without naming a single defect. That formulation cannot be closed,
re-scoped, or tested: it is a disclaimer, not a difference. This document
replaces it with an enumerated, evidence-backed inventory, and sequences the
work that removes each item.

The rule applied throughout: a backend that also exists in C++ Faust must expose
the **same public contract** to hosts (AGENTS.md §5). Text identity is not the
contract (`DIFF-BACK-004`); the description surfaces a host actually calls are.

## 2. Method

Both compilers were run over the same sources with the same options, and the
observable artifacts compared. The probe corpus is deliberately minimal so the
differences are attributable:

```faust
// s2.dsp — identity-only
process = _ * hslider("gain", 0.5, 0, 1, 0.01);

// s3.dsp — with source metadata
declare name "Demo";
declare author "Alice";
declare version "1.2";
process = _ * hslider("gain", 0.5, 0, 1, 0.01);
```

Option-acceptance matrix over `-lang {wasm,wast,julia} × {-vec,-os,-ec,-double,
-mem0}` plus the memory-mode selectors, and artifact comparison for the emitted
`.wasm` / `.json` / `.jl`.

Note on the oracle: the pinned C++ branch rejects `-vec` outright
(`'-vec' is not yet supported with 'ondemand' primitive`), so no vector-mode
comparison against this reference is possible for any backend. Vector
qualification for WASM and Julia rests on the impulse gates alone.

## 3. Gap inventory

### 3.1 WASM

| ID | Severity | Difference | Evidence |
|---|---|---|---|
| `G5-W1` | host-blocking | `-lang wasm-i`, `wasm-e`, `wast-i`, `wast-e` are rejected by the Rust CLI. C++ accepts all four. The Rust backend already implements both memory modes (`WasmOptions::internal_memory`), so only the selector is missing: there is no way to request an external-memory module, which is what polyphonic and soundfile-carrying web hosts need. | `error: invalid value 'wasm-e' for '--lang <LANG>'` vs C++ `OK` |
| `G5-W2` | host-visible | The companion JSON `meta` array omits the three identity entries C++ always injects (`compile_options`, `filename`, `name`). Source `declare`s are transported correctly; the compiler-synthesized ones are not. | `s2.dsp`: C++ `meta` has 3 entries, Rust `meta` is `[]` |
| `G5-W3` | host-visible | The companion JSON has no `code` key. C++ embeds the base64 of the DSP source (`base64_encode(dsp_code)` in `WASMCodeContainer::generateJSON`). | key present in C++ JSON, absent in Rust JSON |
| `G5-W4` | extension | Rust emits `sr_index` (byte offset of `fSampleRate`); the C++ WASM container passes `sr_index = -1` and the key is omitted. This is a deliberate Rust addition, not a defect — it must be *declared* rather than left to look like drift. | Rust JSON `"sr_index": 0`, absent in C++ |
| `G5-W5` | layout | DSP struct field order differs: Rust places `fSampleRate` first, C++ follows FIR declaration order. Struct `size` agrees (8 = 8 on the probe) but widget byte offsets do not (`ui[0].index` = 4 in Rust, 0 in C++). The `.wasm`/JSON pair stays self-consistent, so hosts reading offsets from the JSON are correct either way; only a host hardcoding C++ offsets breaks. The cause is FIR-level declaration order shared with every backend (the same order is visible in the Julia struct), not a WASM emitter choice. | probe JSON diff |
| `G5-W6` | qualification | Vector WASM is emitted and gated by `make wasm-vec0` / `wasm-vec1`, but has no C++ oracle on the pinned branch (see §2). | `known.mk` |

### 3.2 Julia

| ID | Severity | Difference | Evidence |
|---|---|---|---|
| `G5-J1` | host-blocking | `metadata!` is emitted with an empty body **always** — source `declare`s are dropped too, not only the identity entries. C++ emits every `gMetaDataSet` entry plus `compile_options` / `filename` / `name`. The C-family transport (`CppOptions::metadata_entries` + `ordered_compilation_metadata`) was never extended to Julia. | `s3.dsp`: C++ emits 5 `declare!` calls, Rust emits none |
| `G5-J2` | host-blocking | `getJSON` returns the literal `"{}"`. C++ returns the full flattened JSON description. Any Julia host that introspects the DSP through the standard API gets nothing. | generated `.jl` |
| `G5-J3` | performance | `compute!` is emitted without `@inbounds`; C++ emits `@inbounds function compute!` and `@inbounds` on the view bindings. Every table and buffer access then pays a bounds check in the audio loop. | generated `.jl` |
| `G5-J4` | hygiene | Internal pipeline comments leak into the shipped artifact (`# signal_fir_fastlane_step2a: executable base slice`, `# io: inputs=1 outputs=1`, `# signals: 1`). | generated `.jl` |
| `G5-J5` | cosmetic | `instanceResetUserInterface!` assigns `REAL(0.5)` into a field declared `FAUSTFLOAT`, where C++ assigns `FAUSTFLOAT(0.5f0)`. **Initially filed as a correctness defect; it is not.** Julia's `setproperty!` converts the assigned value to the declared field type, so the store is correct even when the host defines `FAUSTFLOAT ≠ REAL`. Only the spelling differs. | generated `.jl`, Julia field-assignment semantics |
| `G5-J6` | cosmetic | The runtime preamble (`fmod`, `atan2`, `faust_wrap_int32`, `faust_fmin`, `faust_fmax`) is emitted unconditionally, including for DSPs that use none of it. | generated `.jl` |
| `G5-J7` | qualification | Julia has a `make julia` impulse gate but no row in the impulse-test status table, unlike the seven backends recorded there. | `tests/impulse-tests/README.md` §Status |

### 3.3 Not owned by this document

- `subcontainer1` fails to compile on **every** backend; it is the shared
  sub-container codegen gap already tracked in `KNOWN_FAILURES.md`.
- The strict `--json` output omits the same `compile_options` / `filename`
  identity entries inside `meta` (verified on `-lang cpp --json`). That is a
  cross-backend JSON-builder difference affecting every backend, with a
  byte-stability contract attached to it (`DIFF-CLI-010`). It is registered here
  as evidence but must be closed in its own change, with its own re-record.
- `G5-W5` is a FIR field-ordering question, not a WASM one. Closing it changes
  every backend's struct layout at once and needs its own differential.

## 4. Phases

Each phase ships producer + test in the same change, per the porting discipline.

Scope decision (2026-08-14): the WASM emitter is **not** to be modified in this
stream. `G5-W1`…`G5-W4` stay as inventory — the enumeration is what replaces the
vague registry entry — but P2 and P3 below are specifications held for a later
owner, not work scheduled here. Only the Julia phases are executed.

### P1 — Julia description surfaces (`G5-J1`, `G5-J2`) — **done 2026-08-14**

Extend the C-family metadata transport to Julia:

- `JuliaOptions` gains `metadata_name`, `metadata_filename`, `metadata_entries`,
  matching `CppOptions`/`COptions`;
- `emit_metadata` replays `ordered_compilation_metadata` when the FIR `metadata`
  function body is empty, exactly like `emit_named_fun` does for the C family;
- `getJSON` returns the real JSON description, built from the same
  `build_json_description_from_fir` used by the other backends and flattened
  into a Julia string literal;
- the facade (`lower_signals_to_julia_transform_fastlane`) fills the transport
  from the compilation-metadata snapshot plus `compile_options`, like
  `lower_signals_to_cpp_transform_fastlane`.

Pass criteria: for `s3.dsp`, `metadata!` declares the same five keys in the same
order as C++, and `getJSON` returns a JSON object whose `name`, `filename`,
`inputs`, `outputs`, `meta`, and `ui` agree with the C++ Julia output.

Result: met. `metadata!` reproduces the C++ key set and order exactly. The
embedded description agrees with C++ on `name`, `filename`, `inputs`, `outputs`,
`size`, and the whole `ui` tree. Three fields still differ, all of them already
classified elsewhere: `compile_options` (`DIFF-CLI-006`), `version` (different
compilers), and the `meta` array, which lacks the `compile_options`/`filename`
identity entries because that is the cross-backend strict-JSON gap of §3.3 — the
Julia `metadata!` callback carries them, the JSON payload does not yet.

One defect was found while implementing, not by the audit: the embedded
description took its `name` from the FIR module, which is the *class* identity
(`-cn`, default `mydsp`). A DSP compiled without `declare name` therefore
advertised `"name": "mydsp"` beside `"filename": "simple.dsp"`, contradicting
its own `metadata!` callback. This is the mirror image of the `-mem0`
`memory_layout` zone-naming defect fixed on 2026-08-13. The description now
takes the DSP name, and a regression test pins `-cn` against it.

### P2 — WASM memory-mode selectors (`G5-W1`)

Accept `wasm-i`, `wasm-e`, `wast-i`, `wast-e` as `-lang` values, mapping to the
existing `WasmOptions::internal_memory`. `wasm` stays an alias of `wasm-i`, as
in C++. Record the selected mode in `compile_options`.

Pass criteria: the four selectors compile; `wasm-e` emits an imported memory and
`wasm-i` an exported one; the soundfile auto-escalation to external memory
remains in force and is not silently contradicted by `-lang wasm-i`.

### P3 — WASM companion JSON identity and source (`G5-W2`, `G5-W3`)

Inject the three identity entries into the WASM JSON `meta`, and add the base64
`code` payload. Declare `sr_index` (`G5-W4`) in the differences registry as an
extension so it stops reading as drift.

Pass criteria: for `s2.dsp` and `s3.dsp` the Rust and C++ WASM JSON agree on
`meta` (keys, values, order) and on the presence and decoded value of `code`.

### P4 — Julia codegen hygiene (`G5-J3`…`G5-J6`) — **`G5-J3` done 2026-08-14**

`G5-J3` is closed: `compute!` is emitted as `@inbounds function compute!`, which
covers the whole body as in `JuliaCodeContainer::generateCompute`. The reference
additionally repeats `@inbounds` on the view bindings and the sample loop; the
definition-level annotation subsumes those, so they are not reproduced.

The remaining three are deliberately left open, with their reasons:

- `G5-J4` (internal comments) is **not** a Julia defect. The text comes from a
  FIR `Label` node emitted by the shared lowering
  (`transform/src/signal_fir/module/build.rs`); the Julia and Rust backends
  print FIR labels, the C family suppresses them. Deciding what a backend does
  with FIR labels is a cross-backend question, and changing it re-records the
  200 stored Rust golden files. It belongs with P5.
- `G5-J5` is cosmetic, not a defect — see the corrected inventory row.
- `G5-J6` (unconditional runtime preamble) buys a few unused one-line
  definitions in exchange for a use-tracking pass over every emitted expression.
  Not worth the machinery; C++ emits a fixed preamble of its own.

### P5 — Layout and cross-backend JSON (`G5-W5`, §3.3)

Field-ordering study against C++ plus the strict-JSON identity entries. Both are
cross-backend and each needs its own differential and re-record.

### P6 — Qualification widening (`G5-W6`, `G5-J7`)

Add the Julia row to the impulse status table, and state explicitly what the
vector gates do and do not prove in the absence of a C++ oracle.

Measured on 2026-08-14 while validating P1/`G5-J3`: `make -f Make.julia -j8`
runs 94 comparisons against genuine C++ references and all 94 pass with the new
emitter. That is a current-corpus number, not the historical 93-DSP sweep the
README's status table records, so it is *not* written into that table here:
mixing the two baselines in one table would make both unreadable. `G5-J7` stays
open until the table itself is re-derived on one corpus.

## 5. Maintenance

When P1–P4 land, `DIFF-GAP-005` is rewritten from a two-backend disclaimer into
the residual items only (`G5-W5`, `G5-W6`, `G5-J7` and whatever P5 leaves open),
each with its own identifier.
