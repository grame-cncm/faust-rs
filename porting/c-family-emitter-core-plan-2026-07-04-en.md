# Shared C-family emitter core — plan for `crates/codegen/src/backends/{c,cpp}`

**Date**: 2026-07-04
**Status**: Analysis complete, design proposed, **Phase 1 implemented** on branch `main-dev`
(uncommitted in the working tree as of this writing) — see §4 for what landed and the guardrail
results.
**Scope**: `crates/codegen/src/backends/cpp/mod.rs` (1 756 lines, 30 fns, 1 439 non-test),
`crates/codegen/src/backends/c/mod.rs` (1 676 lines, 33 fns, 1 503 non-test); `julia/mod.rs`
(1 760 lines, 30 fns) and `asc/mod.rs` (1 543 lines, 27 fns) surveyed for comparison but out of
scope for the core itself (§5).
**Origin**: item 5 (§3.4, §6) of
[`rust-port-quality-review-2026-07-04-en.md`](rust-port-quality-review-2026-07-04-en.md): "near-
parallel textual backends… no shared emitter core… the `backend-align-smoke` xtask exists
precisely because this drift risk is real." (Note: as §6 below shows, `backend-align-smoke` and
`cpp-backend-diff-report`/`c-fastlane-diff-report`/`backend-full-corpus-diff-report` actually guard
*interp/Cranelift alignment* and *Rust-vs-upstream-C++ shell shape*, respectively — none of them
diff c-vs-cpp-vs-julia-vs-asc against each other. The drift found in §2 was invisible to CI
precisely because no such check exists.)

---

## 0. Where this code sits in the compilation chain

```
fir (FirStore, FirId graph) ──► codegen::backends::{c,cpp,julia,asc,interp,cranelift,wasm}
```

Every textual backend is handed the same `FirStore` + root `FirId` (the `Module` produced by
`transform::signal_fir`) and walks it recursively, rendering each `FirMatch` variant into that
language's source text. `interp`, `cranelift`, and `wasm` do the same walk but target an in-memory
representation or bytecode, not source text — they are architecturally different consumers and are
not part of this plan. This plan is about the four **textual, syntax-only** backends, and
concretely about the two closest ones: `c` and `cpp`.

The four textual backends' entry points:

| Backend | Entry fn | Options struct | Emits |
|---|---|---|---|
| `cpp` | `generate_cpp_module` ([`cpp/mod.rs:187`](../crates/codegen/src/backends/cpp/mod.rs)) | `CppOptions` | one C++ class |
| `c` | `generate_c_module` ([`c/mod.rs:214`](../crates/codegen/src/backends/c/mod.rs)) | `COptions` | one C struct + free functions |
| `julia` | `generate_julia_module` ([`julia/mod.rs:239`](../crates/codegen/src/backends/julia/mod.rs)) | (no shared options struct passed to most fns) | one Julia `mutable struct` |
| `asc` | `generate_asc_module` ([`asc/mod.rs:152`](../crates/codegen/src/backends/asc/mod.rs)) | `AscOptions` | one AssemblyScript class |

### 0.1 Upstream C++ already validates this exact idea

Upstream `/Users/letz/faust/compiler/generator/Text.hh` + `Text.cpp` is a **single shared
literal-formatting module used by every one of upstream's ~12 generator backends** (c, cpp, julia,
assemblyscript, dlang, cmajor, jax, rust, template, …) — `checkFloat`/`checkDouble`/`checkQuad`/
`T()` are called identically from every backend's instruction visitor. For example
[`Text.hh:73-79`](/Users/letz/faust/compiler/generator/Text.hh):

```cpp
inline std::string checkFloat(float val)  { return (std::isinf(val)) ? "INFINITY" : T(val); }
inline std::string checkDouble(double val) { return (std::isinf(val)) ? "INFINITY" : T(val); }
```

The Rust port instead **reimplements `trim_float`/string-literal-escaping independently in each of
the four backend files** (`cpp/mod.rs`, `c/mod.rs`, `julia/mod.rs`, `asc/mod.rs`), which is the
direct, traceable cause of the drift in §2 below — every one of the seven drifts is a variant of
"the shared-in-upstream literal-formatting logic was hand-copied once, then only some copies got a
later fix." This plan's shared core is not a novel idea for this codebase to try; it is recovering
an architecture upstream already runs in production across a dozen backends.

---

## 1. Measurement

### 1.1 Corrected LOC baseline

The quality review's "1 756 / 1 676 / 1 760" figures include each file's `#[cfg(test)] mod tests`
block. Source-only (pre-test-module) sizes, which is what a shared core would actually act on:

| Backend | Total LOC | Test-module start | Source-only LOC | Top-level fns (source) |
|---|---|---|---|---|
| `cpp` | 1 756 | line 1 440 | **1 439** | 30 |
| `c` | 1 676 | line 1 504 | **1 503** | 33 |
| `julia` | 1 760 | (not measured here — see §5) | — | 30 |
| `asc` | 1 543 | (not measured here — see §5) | — | 27 |

The rest of this document uses the **1 439 (cpp) / 1 503 (c)** source-only figures.

### 1.2 Function-by-function correspondence table (cpp vs c)

Classification legend: **IDENTICAL** (byte-identical modulo the type name / one extra parameter),
**SHAPE** (same algorithm and branching structure, different literal tokens because the target
language differs), **LANG** (genuinely different logic — a real language-capability gap), **ONLY**
(no counterpart in the other backend).

| cpp fn (lines) | c fn (lines) | Class | Note |
|---|---|---|---|
| `generate_cpp_module` (187–273, 87L) | `generate_c_module` (214–259, 46L) | SHAPE | Both decode the module, call header/section/API emitters in the same order; cpp additionally opens/closes a `class … : public dsp {}` wrapper and an optional namespace — c has no equivalent wrapping. |
| `emit_dsp_contract_methods` (274–387, 114L) | `emit_c_api` (424–622, 199L) | SHAPE | **Same seven-stub lifecycle contract** (`getNumInputs/Outputs`, `classInit`, `instanceConstants/ResetUserInterface/Clear`, `instanceInit`, `init`, `buildUserInterface`, `metadata`, `compute`), same "only synthesize a stub if the FIR module didn't declare the method" conditionals, same call-order wiring (`init` → `classInit`+`instanceInit`; `instanceInit` → the three instance-lifecycle calls). c's version is longer only because free functions repeat `{class_name}` in every signature and there is no implicit `this`. |
| `collect_module_function_names` (388–412, 25L) | (folded into `collect_module_functions`, 826–865, 40L) | SHAPE | Same purpose (which DSP-API names are declared, to decide which stubs to skip); c's version also builds the `DeclareFunView` list c needs for helper-function emission, cpp's doesn't. |
| `emit_cpp_header` (413–453, 41L) | `emit_c_header` (260–303, 44L) + `emit_c_footer` (304–313, 10L) | SHAPE | Both emit the `#include`/`#define FAUSTFLOAT`/`RESTRICT`/`exp10` preamble block; c additionally emits an `extern "C"` opener/closer pair cpp doesn't need. |
| (no equivalent) | `emit_struct_definition` (314–338, 25L) + `emit_struct_fields` (339–382, 44L) | ONLY (c) | cpp folds struct-field declarations into the generic `emit_stmt_with_mode` path (a class body is "just a block of statements" to the emitter); c needs a dedicated struct-shape pass because a C `struct` is a distinct grammatical construct from a function body. |
| `emit_section` (454–485, 32L) | (implicit in `generate_c_module`) | LANG | cpp threads section headers explicitly because the whole module is one `class { … }` block; c's sections are separate top-level declarations, no shared "section" concept needed. |
| `emit_stmt` (486–498, 13L) / `emit_stmt_with_mode` (499–883, 385L) | `emit_stmt` (896–1199, 304L) | **SHAPE, with two confirmed drifts** | The `match_fir` arm order and ~80% of arm bodies are identical (`DeclareVar`, `StoreVar`, `StoreTable`, `Drop`, `Return`, `Block`, `If`, `Switch`, `ForLoop`, `SimpleForLoop`, UI/soundfile arms). Differences: cpp routes variable references through bare names (implicit `this`), c through `emit_var_ref` (`dsp->name`); cpp has extra arms `DeclareStructType`, `DeclareBufferIterators`, `ShiftArrayVar` (debug-comment stubs, unreached by any current producer — see §2.8) that c lacks entirely (hard error if ever reached); **the `DeclareTable` arm has a live behavioral drift (§2.1, DRIFT 1)**; **and c is entirely missing the `Control`/`WhileLoop` arms cpp has (§2.7, DRIFT 7)**. |
| `emit_block` / `emit_block_with_mode` (884–915, 32L) | `emit_block` / `emit_block_with_mode` (866–895, 30L) | IDENTICAL | Trivial signature difference (`module_name` thread). |
| `block_declares_var` / `block_stores_var` (916–941, 26L) | same (383–423, 41L) | IDENTICAL | c's `block_stores_var` has slightly more lines only from formatting; logic identical. |
| `emit_declare_fun` (942–1058, 117L) | `emit_named_fun` (645–698, 54L) + `emit_helper_function` (699–734, 36L) | SHAPE | Both synthesize the canonical DSP-API signature (`compute`, `buildUserInterface`, `metadata`, …) when the FIR body omits explicit parameter names, both special-case `instanceConstants` to inject a `fSampleRate = sample_rate;` prologue line when the body doesn't already store it, both dispatch to `emit_compute_body`/`emit_block_with_mode` with the right `EmitMode`. cpp's version also handles prototype-only (`body: None`) forward declarations and `virtual`/`inline` qualifiers — genuinely C++-only concerns (§2/§3.3, LANG family). |
| `emit_compute_body` (1059–1070, 12L) | `emit_compute_body` (743–754, 12L) | IDENTICAL | |
| `is_dsp_api_method` (1071–1083, 13L) | (inline `matches!` in `emit_c_api`) | SHAPE | Same string set, not factored into a named helper on the c side. |
| `is_empty_block` (1084–1091, 8L) | (no direct counterpart; c uses `.is_empty()` differently in `collect_*`) | SHAPE | |
| `emit_value` (1092–1214, 123L) | `emit_value` (1200–1291, 92L) | **SHAPE, with two confirmed drifts** | Both match the same ~20 `FirMatch` value variants in the same order with near-identical bodies (`BinOp`→`emit_binop_expr`, `Neg`, `Cast`, `Select2`→ternary, `FunCall`, `LoadSoundfile*`). Differences: cpp additionally handles `Bitcast` (c does not — **§2.2, confirmed functional gap**); `NullValue` renders `nullptr` (cpp) vs `NULL` (c) — correct LANG difference; function-name mapping differs (`emit_cpp_fun_name` vs inline `min_i`/`max_i` match + `std::`-strip) — correct LANG difference, not drift (verified in §2, no behavioral divergence once namespace conventions are accounted for). |
| `emit_named_type` (1215–1220, 6L) | `emit_named_type` (1362–1367, 6L) | IDENTICAL | |
| `emit_type_base_and_suffix` (1221–1231, 11L) | same (1368–1379, 12L) | IDENTICAL | |
| `emit_cpp_fun_name` (1232–1249, 18L) | (inline in `emit_value`'s `FunCall` arm) | LANG | cpp must add a `std::` namespace prefix and special-case `exp10` (no `std::exp10`); c's C library already provides bare-named `sin`/`cos`/etc. and defines `exp10`/`exp10f` via macro in the header (`c/mod.rs:288-289`), so c never needs a name-rewrite table beyond `min_i`/`max_i`. |
| `emit_binop` (1250–1271, 22L) | `emit_binop` (1292–1313, 22L) | IDENTICAL | |
| `emit_binop_expr` (1272–1279, 8L) | `emit_binop_expr` (1314–1321, 8L) | IDENTICAL | |
| (folded into `emit_stmt`'s `StoreVar`/`LoadVar` arms) | `emit_var_ref` (1322–1329, 8L) | LANG | c needs an explicit `dsp->` rewrite for `AccessType::Struct` because state lives behind an explicit pointer parameter; cpp's implicit `this` makes this unnecessary. |
| `emit_type` (1280–1312, 33L) | `emit_type` (1330–1361, 32L) | **SHAPE** | Leaf-for-leaf identical match arms except three token substitutions: `Bool` → `"bool"` (cpp) / `"int"` (c); `UI` → `"UI*"` / `"UIGlue*"`; `Meta` → `"Meta*"` / `"MetaGlue*"`. This is the textbook case for a syntax-descriptor lookup table (§3.1). |
| `unsupported_node` (1313–1324, 12L) | `unsupported_node` (1451–1462, 12L) | IDENTICAL | |
| `trim_float` (1325–1342, 18L) | `trim_float` (1463–1480, 18L) | **IDENTICAL SHAPE, confirmed live drift** | See §2.3 — c normalizes `-0.0` → `"0.0"`, cpp does not. |
| `format_float32` (1343–1350, 8L) | `format_float32` (1481–1488, 8L) | IDENTICAL | |
| `format_array` (1351–1355, 5L) | (folds into inline `{{...}}` formatting at call sites) | SHAPE | |
| `cpp_string_literal` (1356–1365, 10L) | `c_string_literal` (1489–1676→~1503, ~15L) | **SHAPE, confirmed live drift** | See §2.4 — c (and julia) escape `\r`/`\t`; cpp escapes only `\\`, `"`, `\n`. |
| `emit_static_tables` (1366–1403, 38L) | `emit_static_tables` (1380–1417, 38L) | IDENTICAL SHAPE | Only `const static` (cpp) vs `static const` (c) token order differs — both legal, cosmetic, and not the C++ upstream convention either way (upstream cpp uses `static constexpr`, a gap versus C++ that predates and is orthogonal to this plan). |
| `decode_module` (1404–1435, 32L) | `decode_module` (1418–1450, 33L) | IDENTICAL | |
| `backend_id` (1436–1438(+tests), 3L) | `backend_id` (192–213, 22L) | IDENTICAL (trivial) | |

### 1.3 Line-count quantification

Summing the table above (source-only lines, excluding tests):

| Bucket | cpp lines | c lines | Combined |
|---|---:|---:|---:|
| **IDENTICAL** (byte-identical modulo naming) | ~330 | ~350 | ~680 |
| **SHAPE** (same algorithm, different tokens/signatures — the core's target) | ~640 | ~830 | ~1 470 |
| **LANG** (genuinely different — classes/virtual dispatch, `dsp->` threading, prototype-only decls, section wrapping) | ~470 | ~320 | ~790 |

A syntax-parameterized shared core absorbing the IDENTICAL + SHAPE buckets could remove
**roughly 970–1 100 of the combined ~2 940 source lines** (cpp 1 439 + c 1 503) down to one
generic implementation plus two small per-language syntax tables — a reduction of about a third
of the combined two-backend surface, concentrated in exactly the functions that are also where all
seven confirmed drifts live (`emit_stmt`'s `DeclareTable`/`Control`/`WhileLoop` arms, `emit_value`'s
`Bitcast` arm, `trim_float`, `*_string_literal`, the UI-widget-argument cast, the reset-fallback
initializer replay). This is the strongest argument for the refactor: the drift lives entirely
inside the "this should have been shared" bucket, never inside the LANG bucket.

---

## 2. Drift findings — same function, different behavior (the important part)

Seven confirmed drifts, in descending order of how likely they are to bite a real `.dsp` file, all
found by direct reading, cross-checked with `git log -L`, cross-backend comparison (julia and c
often side with the *correct* behavior, showing cpp is the outlier as often as c is), and — for
drift 5 — the upstream C++ source itself. Drifts 1–4 were found independently in this pass; drifts
5–7 were surfaced by a parallel research pass over the same files and independently re-verified
here (line numbers and upstream cross-checks below are all directly re-confirmed, not merely
relayed).

### 2.1 DRIFT 1 (highest severity, live, untested): local `DeclareTable` initializer values silently dropped in cpp

- **cpp**: [`crates/codegen/src/backends/cpp/mod.rs:524-537`](../crates/codegen/src/backends/cpp/mod.rs) —
  the statement-level `FirMatch::DeclareTable { name, elem_type, values, .. }` arm destructures
  `values` (needed only for `values.len()`, used to size the array) and emits
  `"{tab}{type} {name}[{len}];"` — **the literal contents of `values` are never rendered**.
- **c**: [`crates/codegen/src/backends/c/mod.rs:918-937`](../crates/codegen/src/backends/c/mod.rs) —
  the same arm renders every element through `emit_value` and emits
  `"{tab}{type} {name}[{len}] = {{{rendered}}};"`.
- **julia**: [`crates/codegen/src/backends/julia/mod.rs:868-879`](../crates/codegen/src/backends/julia/mod.rs) —
  also renders every element (`name = [v0, v1, …]`).
- **Why it's live, not a stub**: `FirBuilder::declare_table` ([`crates/fir/src/builder.rs:354`](../crates/fir/src/builder.rs))
  is documented as "C++ parity helper: explicit table declaration with literal initial values."
  All producers in `transform::signal_fir::module::tables` construct it with `AccessType::Static`
  (routed through the *separate*, correctly-implemented `emit_static_tables` path in both backends —
  not the bug). But `crates/fir/src/inliner.rs:1683-1691` and `:2266` construct a **local**
  `DeclareTable` (`AccessType::Stack | AccessType::Loop`) when the inliner clones a table-with-init
  local variable across a call-site boundary — this is a real, reachable shape produced by FIR
  inlining, not dead code.
- **Impact**: any `.dsp` program whose FIR inlining duplicates a local table with a non-empty
  literal initializer will produce **C++ that compiles but reads uninitialized/zero-filled memory**
  for that table (`const static Type name[N];` with no initializer zero-fills static storage in
  C++, so the failure mode is "silently wrong numbers," not a compile error) — while the same
  program's C and Julia output is correct. No existing cpp backend test exercises this path
  (`grep DeclareTable crates/codegen/src/backends/cpp/mod.rs` after the test-module boundary finds
  no case with non-static access and non-empty values), which is why it has never been caught.

### 2.2 DRIFT 2 (live, functional gap, not just formatting): C backend has no `Bitcast` handling

- **c**: [`crates/codegen/src/backends/c/mod.rs`](../crates/codegen/src/backends/c/mod.rs) `emit_value`
  (1200–1291) has **no `FirMatch::Bitcast` arm at all** — it falls through to the catch-all
  `_ => Err(unsupported_node("value", value, store))` at line 1287.
- **cpp**: [`crates/codegen/src/backends/cpp/mod.rs:1164-1167`](../crates/codegen/src/backends/cpp/mod.rs)
  handles it: `Ok(format!("bitcast<{}>({value})", emit_type(&typ, options)))`.
- **julia**: [`crates/codegen/src/backends/julia/mod.rs:1183`](../crates/codegen/src/backends/julia/mod.rs)
  folds it into the same arm as `Cast`.
- **Reachability**: `transform::signal_fir::module::core_lowering::lower_bitcast` is called from
  `SigMatch::BitCast(value)` in `core_lowering.rs:57` — a normal signal-lowering path, not a rare
  corner. Any `.dsp` construct that reaches `BitCast` at the signal level (integer/float
  reinterpretation) will compile successfully to C++/Julia but **hard-fail C code generation**
  with a `CodegenError`. This is a functional parity gap between backends, not merely a text
  difference, and it is exactly the kind of thing a shared value-emission core (with a per-language
  "how do you spell a bitcast" hook) would make structurally impossible to miss.

### 2.3 DRIFT 3 (live, narrow but real): `-0.0` normalization present in c/julia, absent in cpp

- **c**: [`crates/codegen/src/backends/c/mod.rs:1478`](../crates/codegen/src/backends/c/mod.rs):
  `if s == "-0.0" { "0.0".to_owned() } else { s }`.
- **julia**: [`crates/codegen/src/backends/julia/mod.rs:1658`](../crates/codegen/src/backends/julia/mod.rs):
  identical `if s == "-0.0" { "0.0".to_owned() } else { s }`.
- **cpp**: [`crates/codegen/src/backends/cpp/mod.rs:1325-1341`](../crates/codegen/src/backends/cpp/mod.rs)
  has no such check — a `Float64` constant folded to negative zero renders as literal `-0.0`.
- **History (confirmed via `git log -L`)**: Julia got the `-0.0` normalization first, in the same
  commit that introduced `trim_float` for Julia (`27be0cae`, later simplified by `f7109c6f Align
  Julia numeric casts with C++ backend` — ironic naming, since it never made it back to cpp). c
  received the fix later in `61407a68 "Preserve C backend double literal precision"`, which touches
  **only** `crates/codegen/src/backends/c/mod.rs` (`git show --stat 61407a68`) — cpp was never
  revisited. This is the single cleanest example of "fixed one backend, forgot the other(s)" the
  quality review predicted.
- **Impact**: narrow (only constants that fold to exactly negative zero, e.g. certain LFO/noise
  phase computations at their zero crossing), but a genuine byte-level output difference between
  cpp and c/julia for the same source `.dsp` file today.

### 2.4 DRIFT 4 (live, narrow): string-literal escaping misses `\r`/`\t` in cpp

- **c**: [`crates/codegen/src/backends/c/mod.rs:1489-1503`](../crates/codegen/src/backends/c/mod.rs)
  `c_string_literal` escapes `\\`, `"`, `\n`, `\r`, `\t`.
- **julia**: [`crates/codegen/src/backends/julia/mod.rs:1666-1679`](../crates/codegen/src/backends/julia/mod.rs)
  `julia_string_literal` — same five escapes.
- **cpp**: [`crates/codegen/src/backends/cpp/mod.rs:1356-1365`](../crates/codegen/src/backends/cpp/mod.rs)
  `cpp_string_literal` escapes only `\\`, `"`, `\n`.
- **Reachability**: used for UI labels, metadata keys/values, and URLs (`declare` metadata, widget
  labels) at [`cpp/mod.rs:770,786,807,829,839,840,855,856,864,865,871,872`](../crates/codegen/src/backends/cpp/mod.rs) —
  all user-authored strings from `.dsp` source text. A label or metadata value containing a literal
  tab or carriage return renders as a raw unescaped byte in the generated C++ literal instead of
  `\t`/`\r` — for `\r` specifically this risks the literal being altered or misread depending on
  how the generated file is later transported/diffed (CRLF-sensitive tooling), and is at minimum a
  confirmed textual divergence from the c/julia output for the same input.

### 2.5 DRIFT 5 (live, verified against upstream C++): cpp UI widgets emit values without the required `FAUSTFLOAT` cast

- **cpp**: [`crates/codegen/src/backends/cpp/mod.rs:804-812`](../crates/codegen/src/backends/cpp/mod.rs)
  (`AddSlider`) and [`:826-833`](../crates/codegen/src/backends/cpp/mod.rs) (`AddBargraph`) render
  `init`/`lo`/`hi`/`step` through bare `trim_float(...)`, with no cast:
  ```rust
  "{tab}ui_interface->{api}({}, &{var}, {}, {}, {}, {});",
  cpp_string_literal(&label), trim_float(init), trim_float(lo), trim_float(hi), trim_float(step)
  ```
- **c**: [`crates/codegen/src/backends/c/mod.rs:1116-1124`](../crates/codegen/src/backends/c/mod.rs)
  wraps every one of the same four arguments in `(FAUSTFLOAT){}`.
- **Upstream ground truth** (confirmed by direct read):
  [`/Users/letz/faust/compiler/generator/cpp/cpp_instructions.hh:44`](/Users/letz/faust/compiler/generator/cpp/cpp_instructions.hh)
  defines `cast2FAUSTFLOAT(str) = "FAUSTFLOAT(" + str + ")"`, and the `AddSliderInst`/
  `AddBargraphInst` visitors at lines 320-355 wrap **every** numeric argument in
  `cast2FAUSTFLOAT(checkReal(...))`. Upstream C++ always casts; Rust's `c` backend correctly
  mirrors this; Rust's `cpp` backend has never done so.
- **Impact**: when `FAUSTFLOAT` is `float` (the default) and the literal is written as a `double`
  token (e.g. `440.0`), the call relies on an implicit narrowing conversion at the call site instead
  of an explicit cast. Functionally harmless under default flags, but (a) it is a confirmed
  byte-for-byte divergence from real upstream `-lang cpp` output — the exact thing
  `cpp-backend-diff-report` exists to catch and does not, because its shell-signature comparison
  never inspects call-argument text (§6) — and (b) it is a live build break for any downstream
  consumer compiling the generated C++ with `-Wfloat-conversion`/`-Wconversion`/`-Werror`, which is
  not a hypothetical configuration for embedded/plugin toolchains that vendor generated Faust code.

### 2.6 DRIFT 6 (dormant today, real code-path asymmetry): cpp has no `StructInit`/`TableInit` reset-fallback replay; c and julia both do

- **c**: [`crates/codegen/src/backends/c/mod.rs:502-538`](../crates/codegen/src/backends/c/mod.rs)
  (inside `emit_c_api`'s `instanceResetUserInterface` fallback branch) replays `struct_inits`/
  `table_inits` — collected by `collect_struct_initializers`/`collect_table_initializers`
  ([`c/mod.rs:755-825`](../crates/codegen/src/backends/c/mod.rs)) — whenever the FIR module does not
  supply an explicit `instanceResetUserInterface` body, so UI-bound state still gets its declared
  Faust default value.
- **julia**: [`crates/codegen/src/backends/julia/mod.rs:524-561`](../crates/codegen/src/backends/julia/mod.rs)
  plus [`:1440-1500`](../crates/codegen/src/backends/julia/mod.rs) implements the identical pattern.
- **cpp**: [`crates/codegen/src/backends/cpp/mod.rs:331-334`](../crates/codegen/src/backends/cpp/mod.rs)
  has no such mechanism — the fallback is an unconditional empty stub:
  ```rust
  if !has_instance_reset_ui {
      let _ = writeln!(out, "{tab}virtual void instanceResetUserInterface() {{");
      let _ = writeln!(out, "{tab}}}");
  }
  ```
  No `StructInit`/`TableInit` type and no `collect_struct_initializers`/`collect_table_initializers`
  function exist anywhere in `cpp/mod.rs` (confirmed by exhaustive grep — zero matches).
- **Reachability today**: dormant for the standard pipeline — `transform::signal_fir::module::build`
  ([`build.rs:405`](../crates/transform/src/signal_fir/module/build.rs)) always synthesizes an
  explicit `instanceResetUserInterface` body whenever UI state needs resetting, so the fallback
  branch in all three backends is not exercised by `signal_fir`'s primary entry today. It remains a
  real, live gap for any alternate FIR-construction path (`signal_fir/siggen.rs`'s second entry
  point, hand-built FIR fixtures, or a future pipeline change to what counts as "FIR already
  supplies this function") — at which point cpp would silently leave UI-bound sliders/bargraphs at
  binary-zero instead of their declared init value, while c/julia would not.

### 2.7 DRIFT 7 (dormant today, structural asymmetry): c has no `Control`/`WhileLoop` statement handling

- **c**: exhaustive grep of [`crates/codegen/src/backends/c/mod.rs`](../crates/codegen/src/backends/c/mod.rs)
  finds no `FirMatch::Control` and no `FirMatch::WhileLoop` arm in `emit_stmt` — both fall through to
  the same `_ => Err(unsupported_node(...))` catch-all already noted for `Bitcast` (DRIFT 2).
- **cpp**: handles both — `Control` at [`cpp/mod.rs:661-667`](../crates/codegen/src/backends/cpp/mod.rs),
  `WhileLoop` at [`:729-735`](../crates/codegen/src/backends/cpp/mod.rs) (already listed in the §1.2
  table as a shared arm — the C-side absence was missed in the first pass over `emit_stmt` and only
  surfaced by an independent re-check of the full match arm list).
- **julia/asc**: both also handle `Control` and `WhileLoop` (`julia/mod.rs:934-940,1002-1008`;
  `asc/mod.rs:672,749`).
- **Reachability today**: `FirBuilder::control`/`FirBuilder::while_loop`
  ([`fir/src/builder.rs:537,602`](../crates/fir/src/builder.rs)) are currently only invoked from
  `crates/fir/src/inliner.rs` (lines 1779, 2388, 2449), and the inliner pass is not wired into the
  fast-lane FIR-generation pipeline that feeds `signal_fir` today (no reference to it under
  `crates/compiler/src`). Dormant, like DRIFT 6, but real: the moment the inliner (or any future
  pass) starts emitting `Control`/`WhileLoop` into FIR consumed by the C backend, C hard-fails while
  cpp/julia/asc do not.

### 2.8 Lower-confidence / footnote-level observations (not counted as drift)

- **`const static` (cpp) vs `static const` (c) token order** in `emit_static_tables` — both legal,
  purely cosmetic, and neither matches upstream C++'s `static constexpr` (`cpp_instructions.hh:419`)
  — a pre-existing gap versus upstream orthogonal to this plan, not a c-vs-cpp inconsistency worth
  fixing.
- **Default `metadata` stub content differs**: c's fallback declares
  `("faust-rs", "module-first c backend prototype")` ([`c/mod.rs:632-634`](../crates/codegen/src/backends/c/mod.rs));
  cpp's fallback declares `("filename", "<module>.dsp")` + `("name", "<module>")`
  ([`cpp/mod.rs:355-361`](../crates/codegen/src/backends/cpp/mod.rs)). Likely accidental (prototype
  placeholder text never revisited) rather than intentional, but low severity — only visible when a
  `.dsp` file declares no `metadata` calls at all. Worth aligning during the migration but not
  urgent standalone.
- **`DeclareStructType` / `DeclareBufferIterators` / `ShiftArrayVar` FIR arms**: cpp renders them as
  debug comments (`// struct type declaration: …`, etc.); c has no arms for them and would hard-error
  via `unsupported_node`. No producer in `transform::signal_fir` currently constructs any of the
  three (`grep` across `crates/transform/src` finds none), so this is dormant, not live — but it is
  exactly the shape of gap the shared core should close by construction rather than by convention.
- **`IteratorForLoop`**: cpp emits a `// iterator-for over […]` comment and unrolls the loop body
  **once** (semantically wrong if ever reached, since it drops the iteration entirely); c and julia
  both hard-error via `unsupported_node`. Not reachable today (no producer), but cpp's "succeeds
  with wrong output" is a worse failure mode than c/julia's "fails loudly," and would be worth
  fixing to hard-error consistently even before the shared core lands, independent of this plan.
- **`min`/`max`/math-function namespace prefixing**: cpp's `emit_cpp_fun_name` vs c's inline
  strip-`std::`-prefix logic look different but were verified to agree on every case actually
  reachable (both correctly special-case `min_i`/`max_i`; `exp10` is resolved differently — cpp via
  explicit non-`std::`-prefixed spelling, c via a header macro `#define exp10 __exp10` — but the
  *emitted token* matches what each language's preamble defines). Classified as LANG, not drift.

---

## 3. Design: a shared C-family emission core

### 3.1 Chosen abstraction

The codebase already prefers **plain data + functions over trait hierarchies** (`CppOptions` /
`COptions` are already syntax-parameter structs, just under-used — `emit_type` takes `options` and
ignores everything except two fields; `emit_var_ref`/`emit_cpp_fun_name` don't take `options` at
all even though they encode per-language syntax). The proposed core keeps that style:

1. A single `CFamilySyntax` **data** struct (not a trait — no per-language behavior needs dynamic
   dispatch, every "variation point" is a fixed set of string/bool leaves known at compile time)
   describing exactly the leaves that differ between c and cpp: type-name spellings for `Bool`,
   `UI`, `Meta`; whether state access needs an explicit `dsp->`/`self.` prefix or is implicit;
   function-name rewriting (namespace prefix, if any, and the small `min_i`/`max_i`/`exp10`
   exception table); the `NullValue` spelling (`nullptr` vs `NULL`); the struct/const-table keyword
   order.
2. Shared **functions** in a new `crates/codegen/src/backends/c_family.rs` (or `cfamily/mod.rs` if
   it grows) taking `&CFamilySyntax` where today's functions take `&CppOptions`/`&COptions`:
   `emit_type`, `emit_named_type`, `emit_type_base_and_suffix`, `emit_binop`, `emit_binop_expr`,
   `emit_static_tables`, `trim_float`, `format_float32`, `string_literal`, `emit_block`,
   `emit_block_with_mode`, `block_declares_var`, `block_stores_var`, `decode_module`,
   `unsupported_node`, and the shared 80% of `emit_stmt`/`emit_value` (see §3.2 for how the
   remaining 20% stays per-language without an `if language == …` branch soup).
3. `CppOptions`/`COptions` keep their *existing* per-language-only fields (`namespace`,
   `super_class_name` for cpp) but gain one field: `syntax: CFamilySyntax`, or the shared functions
   take the descriptor as a second parameter alongside the existing options struct. Given the
   codebase's preference for flat data (documented in `CODEGUIDELINES.md`'s ADT-first philosophy,
   §5.2 of the quality review), the simplest option — `CFamilySyntax` as one more field threaded
   everywhere `options` already is — is preferred over adding a second parameter to every call site.

This is deliberately **not** a trait object or generic-over-a-`Backend` trait design: every
variation point is representable as data (strings, bools, small enums), so `dyn Backend` or
`impl<L: Language>` machinery would add indirection for zero benefit and cut against the codebase's
established style.

```rust
/// Syntax parameters distinguishing the C-family textual backends (c, cpp).
///
/// Every field is a fixed leaf (string or bool), not behavior — this is data,
/// not a trait, matching the plain-data style already used by `CppOptions`/
/// `COptions`. Per-language modules construct one of these once and thread it
/// through the shared emission functions in `c_family.rs`.
#[derive(Clone, Debug)]
pub(crate) struct CFamilySyntax {
    /// Spelling for `FirType::Bool` (`"bool"` in C++, `"int"` in C).
    pub bool_type: &'static str,
    /// Spelling for `FirType::UI` (`"UI*"` in C++, `"UIGlue*"` in C).
    pub ui_type: &'static str,
    /// Spelling for `FirType::Meta` (`"Meta*"` in C++, `"MetaGlue*"` in C).
    pub meta_type: &'static str,
    /// Spelling for a null pointer value (`"nullptr"` in C++, `"NULL"` in C).
    pub null_value: &'static str,
    /// Whether `AccessType::Struct` variable references need an explicit
    /// receiver prefix (`"dsp->"` in C; implicit `this` in C++, so `None`).
    pub struct_access_prefix: Option<&'static str>,
    /// Rewrites a bare FIR function-call name to this language's spelling
    /// (namespace prefixing in C++; `min_i`/`max_i` exceptions in both).
    pub rewrite_fun_name: fn(&str) -> String,
    /// Keyword order for a top-level static const array declaration
    /// (`"const static"` in C++, `"static const"` in C — cosmetic but kept
    /// data-driven so it is never hand-duplicated again).
    pub static_table_keywords: &'static str,
}
```

### 3.2 Design against three representative emission sites

**Site A — scalar binop expression** (`emit_binop`/`emit_binop_expr`): already byte-identical
between c and cpp (§1.2). This family needs **no syntax parameter at all** — it moved to
`c_family.rs` verbatim as the first, risk-free migration step (§4, Phase 1 — implemented in this
pass).

**Site B — struct/type-name leaf** (`emit_type`): today two 32–33-line near-duplicate `match`
blocks. With the descriptor:

```rust
// c_family.rs — shared, takes the syntax descriptor instead of a per-language Options type.
pub(crate) fn emit_type(typ: &FirType, syntax: &CFamilySyntax, quad: &str, fixed: &str) -> String {
    match typ {
        FirType::Int32 => "int".to_owned(),
        FirType::Int64 => "long long".to_owned(),
        FirType::Float32 => "float".to_owned(),
        FirType::Float64 => "double".to_owned(),
        FirType::FaustFloat => "FAUSTFLOAT".to_owned(),
        FirType::Quad => quad.to_owned(),
        FirType::FixedPoint => fixed.to_owned(),
        FirType::Bool => syntax.bool_type.to_owned(),
        FirType::Void => "void".to_owned(),
        FirType::Obj => "void*".to_owned(),
        FirType::Sound => "Soundfile*".to_owned(),
        FirType::UI => syntax.ui_type.to_owned(),
        FirType::Meta => syntax.meta_type.to_owned(),
        FirType::Ptr(inner) => format!("{}*", emit_type(inner, syntax, quad, fixed)),
        FirType::Array(inner, size) => format!("{}[{size}]", emit_type(inner, syntax, quad, fixed)),
        FirType::Vector(inner, lanes) => format!("Vec<{},{lanes}>", emit_type(inner, syntax, quad, fixed)),
        FirType::Struct(name, _fields) => name.clone(),
        FirType::Fun { args, ret } => {
            let args = args.iter().map(|a| emit_type(a, syntax, quad, fixed)).collect::<Vec<_>>().join(", ");
            format!("{}({args})", emit_type(ret, syntax, quad, fixed))
        }
    }
}
```

`cpp::emit_type`/`c::emit_type` become one-line wrappers that unpack their existing `Options`
struct's `quad_type_name`/`fixed_type_name` and call the shared function with the module-level
`const CFamilySyntax` for that language. Fixes DRIFT 3/4 style bugs *by construction* for this
family (there is only one place left to forget a case).

**Site C — the compute-loop / statement skeleton** (`emit_stmt`): the ~80% shared arms
(`DeclareVar`, `StoreVar`/`StoreTable` modulo the access-prefix hook, `Drop`, `Return`, `Block`,
`If`, `Switch`, `ForLoop`, `SimpleForLoop`, `WhileLoop`, UI/soundfile arms, and — once DRIFT 1 is
fixed — `DeclareTable`) move to a shared `emit_stmt` in `c_family.rs` taking a small **per-language
hook struct** for the handful of arms that must differ (variable-reference rendering, and the
"extra" LANG-only arms cpp has today):

```rust
/// The seams `emit_stmt`/`emit_value` cannot share: everything else moves to
/// `c_family::emit_stmt`/`emit_value` verbatim.
pub(crate) struct CFamilyHooks<'a> {
    pub syntax: &'a CFamilySyntax,
    /// Renders a variable reference under `access` (`dsp->name` in C, bare
    /// `name` in C++ — implicit `this`).
    pub var_ref: fn(&str, AccessType) -> String,
    /// Renders language-only statement arms with no shared counterpart
    /// (`DeclareStructType`/`DeclareBufferIterators`/`ShiftArrayVar` today —
    /// C++-only debug-comment stubs). Returns `None` to fall through to the
    /// shared `unsupported_node` error, matching C's current behavior.
    pub extra_stmt: fn(&FirMatch, /* … */) -> Option<Result<(), CodegenError>>,
}
```

`emit_stmt` in `c_family.rs` matches the shared arms directly and calls `(hooks.extra_stmt)(...)`
only in the final `_ =>` fallback. This keeps the per-language "escape hatch" explicit and
auditable at the call site instead of letting cpp silently accumulate arms c never sees — the
exact failure mode that produced the `IteratorForLoop` and `DeclareStructType` divergences.

### 3.3 What does *not* move

`emit_dsp_contract_methods`/`emit_c_api` (class-vs-free-function lifecycle synthesis),
`emit_declare_fun`/`emit_named_fun` (prototype/virtual handling vs signature mangling), section
wrapping (`emit_section`, the `class {}`/`extern "C"` shells), and `emit_var_ref`/
`emit_cpp_fun_name`'s *rewrite tables themselves* (as opposed to the dispatch point, which does
move) stay per-language. These are the LANG bucket from §1.3 (~790 lines) — genuinely different
code because C has no classes and C++ has no explicit receiver parameter, not because anyone forgot
to share it.

---

## 4. Migration plan

Every phase must satisfy the invariant: **`cargo run -p xtask -- golden-check` output is
byte-identical before and after** for every case in `tests/golden/{rust,cpp}`, and
`cpp-backend-diff-report` does not regress. A phase that cannot keep output byte-identical is
deferred, not shipped with a "close enough" note — this is a pure restructuring.

1. **Phase 0 (already true)**: confirm `CppOptions`/`COptions` are structurally close enough that a
   `CFamilySyntax` field/parameter can be threaded without touching call sites outside
   `cpp/mod.rs`/`c/mod.rs`. (Verified in this pass — both already carry `quad_type_name`/
   `fixed_type_name`/`class_name` as plain fields.)
2. **Phase 1 (risk-free, byte-identical by construction) — IMPLEMENTED in this pass**: of the
   IDENTICAL bucket, only `emit_binop`/`emit_binop_expr` turned out to need **zero** backend-specific
   types (no `Options` struct, no `CodegenErrorCode`, no `ModuleView`) — every other IDENTICAL-bucket
   candidate examined (`emit_named_type`, `emit_type_base_and_suffix`, `emit_block`/
   `emit_block_with_mode`, `block_declares_var`/`block_stores_var` all take `&CppOptions`/`&COptions`;
   `unsupported_node`/`decode_module` take each backend's own `CodegenErrorCode`/`ModuleView`, which
   carry backend-specific stable error-code strings, e.g. `FRS-CGEN-CPP-0001` vs `FRS-CGEN-C-0001`,
   and are plausibly part of each backend's external contract) — so those move in Phase 2 once
   `CFamilySyntax` exists to carry the one or two leaves they need, rather than being duplicated
   again as a false-IDENTICAL move now. `emit_binop`/`emit_binop_expr` moved verbatim into the new
   `crates/codegen/src/backends/c_family.rs`; `cpp::emit_binop_expr`/`c::emit_binop_expr` are now
   one-line delegations, and the dead `emit_binop` wrapper functions were removed from both files
   (their only caller now calls `c_family::emit_binop` directly, and `c_family::emit_binop` is
   `pub(crate)` and exercised by `c_family`'s own unit tests). Net change: +110 lines (new file,
   including its own unit tests), −56 lines across `cpp/mod.rs`/`c/mod.rs`/`mod.rs` combined — well
   under the ~300-line guardrail-attempt budget. Guardrail results, all green:

   ```
   cargo fmt --all                                            # no diff
   cargo clippy --workspace --all-targets -- -D warnings      # clean
   cargo test -p codegen --all-targets                        # 262 passed, 0 failed, 1 ignored
   cargo run -p xtask -- golden-check                         # 190/190 OK
   cargo run -p xtask -- cpp-backend-diff-report               # 8/8 OK, 0 DIFF (no regression)
   ```

   `cpp-backend-diff-report` regenerates `porting/phases/phase-6-cpp-backend-diff-report-en.md`
   wholesale on every run (dropping any hand-appended content below the auto-generated table,
   unrelated to this change) — that file was reverted with `git checkout --` after confirming the
   `8/8 OK, 0 DIFF` result, so the working tree only carries the actual code change described above.
3. **Phase 2**: introduce `CFamilySyntax` and migrate the SHAPE bucket's pure leaf-lookup functions
   — `emit_type`, `emit_static_tables`, `trim_float` (fixing DRIFT 3 in the same commit that unifies
   it), `string_literal` (fixing DRIFT 4 the same way — both fixes become impossible to
   accidentally split across backends again, since there is one function).
4. **Phase 3**: migrate `emit_value`'s shared arms behind `CFamilyHooks`, closing DRIFT 2
   (`Bitcast`) by adding one shared arm instead of two divergent ones — c gains correct `Bitcast`
   support as a side effect of unification, not a separate fix.
5. **Phase 4**: migrate `emit_stmt`'s shared arms behind `CFamilyHooks`, closing DRIFT 1
   (`DeclareTable` initializer values) and DRIFT 7 (`Control`/`WhileLoop` missing in c) the same
   way, and making the `DeclareStructType`/`DeclareBufferIterators`/`ShiftArrayVar`/
   `IteratorForLoop` gaps an explicit, single-owner decision (either both backends comment-stub
   them, or both hard-error — no more silent asymmetry).
6. **Phase 5**: migrate the UI-widget statement arms (`AddSlider`/`AddBargraph`/`AddButton`/…)
   behind `CFamilyHooks`, closing DRIFT 5 (missing `FAUSTFLOAT` cast in cpp) by making the
   cast-wrapping a `CFamilySyntax` leaf (`c` always wraps, `cpp` gains the same wrap — verified
   against upstream in §2.5, this is a genuine behavior change to bundle with its own golden-diff
   callout, not a silent side effect).
7. **Phase 6 (optional, separate follow-up)**: revisit `emit_declare_fun`/`emit_named_fun` and
   `emit_dsp_contract_methods`/`emit_c_api` for a lighter-weight shared "which stubs does this
   module need" helper (the *decision* of which of the seven lifecycle methods to synthesize is
   identical; only the *rendering* differs), including unifying the `StructInit`/`TableInit`
   reset-fallback replay so cpp gains it too (closing DRIFT 6) — lower priority since this bucket is
   LANG-heavy and the abstraction payoff is smaller per line moved, but DRIFT 6 should not wait for
   it if it can be fixed standalone first (a ~70-line port of c's existing
   `collect_struct_initializers`/`collect_table_initializers` into cpp, ahead of the full Phase 6
   unification).

Each phase is its own PR-sized commit; `golden-check` and `cpp-backend-diff-report` run after every
phase, not just at the end, so a regression is bisectable to one phase.

### Phase 2–3 outcome (2026-07-05)

**Phase 2 — IMPLEMENTED.** `CFamilySyntax` (7 token leaves: `bool_type`, `ui_type`, `meta_type`,
`static_table_keywords`, `bool_true`/`bool_false`, `null_value`) landed in `c_family.rs`, with
per-backend `SYNTAX` consts in `cpp/mod.rs` and `c/mod.rs`. Migrated shared functions:
`emit_type`, `trim_float`, `format_float32`, `string_literal`, `emit_static_tables` (value
rendering injected via closure so each backend keeps its own error type). Drift closures:

- **DRIFT 3 closed**: the shared `trim_float` normalizes `-0.0` → `0.0`; cpp inherits the fix.
- **DRIFT 4 closed**: the shared `string_literal` escapes `\r`/`\t`; cpp inherits the fix.
- **No golden file changed**: `golden-check` stayed 190/190 without regeneration, i.e. no golden
  case exercises either drift in cpp output. Both fixes are covered by focused regression tests in
  `cpp/mod.rs`'s test module instead (`-0.0` constant emission; control-character label escaping).

**Phase 3 — IMPLEMENTED.** `emit_value_common` + `CFamilyValueCtx` (plain-data seams:
`var_ref`/`fun_name` fn pointers, `render_type`/`recurse` closures) own the shared value arms
(literals, loads, tee, binop, neg, cast, select2, funcall, null, soundfile loads). Both `cpp` and
`c` `emit_value` are now thin: shared-arm probe first, then language-only arms (cpp:
`Quad`/`FixedPoint`/array literals, `NewDsp`, `Bitcast`), then the backend's own
unsupported-node error. The `None`-contract keeps the core at the intersection.

- **DRIFT 2 deliberately NOT closed here** (deviation from the original Phase 3 line): cpp's
  current `bitcast<T>(v)` spelling matches neither upstream C++
  (`*reinterpret_cast<T*>(&v)`, `cpp_instructions.hh`) nor any helper the backend emits, and the
  upstream C spelling is a known-broken TODO (§2.2). Unifying on a wrong spelling would launder a
  bug into shared code; c gaining `Bitcast` needs its own oracle-checked fix first. Tracked as a
  standalone follow-up.
- Dead Phase 1 wrappers removed on both sides (`emit_binop_expr`, c's `format_float32`).

Guardrails after Phase 2+3 (all green): `cargo fmt --all` (no diff);
`cargo clippy -p codegen --all-targets -- -D warnings` (clean);
`cargo test -p codegen --all-targets` (270 passed, 0 failed — 8 new tests vs Phase 1);
`golden-check` 190/190 OK, zero golden regenerated; `cpp-backend-diff-report` 8/8 OK, 0 DIFF
(report file reverted after checking, as in Phase 1).

Next: Phase 4 (`emit_stmt` shared arms — closes DRIFT 1 `DeclareTable` initializers and DRIFT 7),
Phase 5 (UI widget arms — closes DRIFT 5 `FAUSTFLOAT` cast, golden diffs expected and to be
oracle-verified), then optional Phase 6.

### Phase 4–5 outcome (2026-07-05)

**Phase 4 — IMPLEMENTED.** `emit_stmt_common` + `CFamilyStmtCtx` landed in `c_family.rs`, using the
same plain-data seam pattern as `CFamilyValueCtx` (fn pointers for capture-free rules:
`var_ref`, `for_loop_step` — `i += step` in C++ vs `i = i + step` in C —,
`simple_loop_increment`; `&dyn Fn` seams for `render_named_type`/`render_type`/`render_value`;
`EmitNodeFn` recursion seams for block/statement re-entry so per-language arms stay visible under
shared containers). `EmitMode` moved into the shared module. 20 statement arms are now shared:
`DeclareVar`, `DeclareTable`, `StoreVar`, `StoreTable`, `Block`, `If`, `Switch`, `ForLoop`,
`SimpleForLoop`, `WhileLoop`, `Control`, `Return`, `Drop`, `NullStatement`, `OpenBox`, `CloseBox`,
`AddButton`, `AddSlider`, `AddBargraph`, `AddSoundfile`. Per-language remainders:
`AddMetaDeclare` and `Label` (structurally different renderings), cpp-only `DeclareFun`.
`CFamilySyntax` gained `ui_glue_arg`/`ui_glue_solo` (C threads `ui_interface->uiInterface`
through every glue call; C++ calls methods directly), `faustfloat_cast_open`/`_close`, and
`switch_default_break`.

- **DRIFT 1 closed**: cpp emits `DeclareTable` initializer values (c's behavior as reference);
  regression test in `cpp/mod.rs` (function-local table-with-init).
- **DRIFT 7 closed**: c gains `Control`/`WhileLoop` through the shared arms; regression test in
  `c/mod.rs`.
- **Single-owner decision** for the former cpp-only `DeclareStructType`/`DeclareBufferIterators`/
  `ShiftArrayVar`/`IteratorForLoop` arms: **both backends now hard-error.** The removed cpp arms
  were silent comment stubs — `IteratorForLoop` even unrolled its body once, i.e. emitted wrong
  code rather than failing — and c already hard-errored. Failing loudly in both is strictly safer;
  a regression test asserts `IteratorForLoop` is rejected. These nodes are only produced for the
  interp/cranelift/asc paths today, which have their own emitters (workspace tests confirm the
  scalar c/cpp pipeline never reaches them).

**Phase 5 — IMPLEMENTED.** The UI-widget arms moved into `emit_stmt_common` with the
`FAUSTFLOAT` cast as a `CFamilySyntax` leaf: cpp wraps functional-style (`FAUSTFLOAT(0.5)`), c
keeps its prefix cast (`(FAUSTFLOAT)0.5`).

- **DRIFT 5 closed**: cpp gains the cast on `AddSlider`/`AddBargraph` numeric arguments.
  Oracle evidence: the in-repo upstream reference `tests/golden/cpp/rep_09_ui_slider/`
  (`ui_interface->addHorizontalSlider("gain", &fHslider0, FAUSTFLOAT(0.5f), …)`) confirms the
  functional-cast form, matching `cpp_instructions.hh:44` (§2.5).
- **No `tests/golden/rust` file changed**: those goldens are compiler-stdout fingerprints
  (`faust-rs-golden-v1` header + FNV hash of a 2-line stdout), not generated-code text, so they
  are structurally insensitive to backend emission changes. The behavior change is locked by the
  updated integration test `crates/codegen/tests/cpp_fir_sine_phasor.rs` (asserts the
  `FAUSTFLOAT(...)`-wrapped slider lines) instead. `cpp-backend-diff-report` (which does compare
  against upstream-generated C++) stayed 8/8 OK, 0 DIFF.

**Metrics after Phase 5**: `cpp/mod.rs` 1 756 → 1 430, `c/mod.rs` 1 676 → 1 326,
`c_family.rs` 1 032 (including 9 unit tests). Combined two-backend surface shrank by ~680 lines
while gaining four drift fixes and their regression tests.

**Guardrails after Phase 4+5 (all green)**: `cargo fmt --all` (no diff);
`cargo clippy --workspace --all-targets -- -D warnings` (clean);
`cargo test -p codegen --all-targets` (273 passed, 0 failed);
`cargo test -p compiler --all-targets` (333 passed, 0 failed);
`golden-check` 190/190 OK; `cpp-backend-diff-report` 8/8 OK, 0 DIFF (report reverted after
checking, as in previous phases).

**Remaining**: optional Phase 6 (lifecycle-stub decision helper; DRIFT 6 `StructInit`/`TableInit`
reset-replay port into cpp — can be fixed standalone first) and the standalone DRIFT 2 `Bitcast`
fix (both backends, against the upstream `*reinterpret_cast<T*>(&v)` spelling).

### Follow-ups outcome (2026-07-05, second pass)

**DRIFT 2 closed — both backends, `Bitcast` is now a shared arm.** New oracle evidence gathered
before deciding the spellings:

- Upstream reachability: `InstructionsCompiler::generateBitCast`
  (`instructions_compiler.cpp:1248`) is produced **only in `-ftz 2` mode** (bitcasting a real to
  int to test exponent bits for flush-to-zero).
- Upstream **cpp** `-ftz 2` output, verified live on `process = + ~ *(0.999);`:
  `*reinterpret_cast<int*>(&fTemp0SE)` — valid, because the operand is always a cached named
  local (an lvalue).
- Upstream **c** `-ftz 2` output, same program: `((*((int*(&fTemp0SE) & 2139095040) ? …` —
  **garbled, uncompilable text** (unbalanced parens; the `// TODO : does not work` above
  `c_instructions.hh`'s `visit(BitcastInst*)` is accurate). There is no valid upstream C oracle.
- Additionally, Rust cpp's former `bitcast<{T}>({v})` spelling named a template that neither the
  backend's emitted header nor upstream defines — it could not have compiled if reached.

Decisions:

- **cpp**: `*reinterpret_cast<{T}*>(&{v})` — byte-matches upstream.
- **c**: `*(({T}*)&{v})` — the C-style pointer-cast transposition of the upstream C++ form, i.e.
  the spelling upstream C's broken visitor evidently *intends* (`*((type*(&` reads as a mangled
  `*((type*)(&`). **Documented deviation**: this is technically a strict-aliasing type pun
  (upstream C++'s `reinterpret_cast` form has the same property); the standard-blessed `memcpy`
  alternative was rejected because it needs a statement context or an emitted helper function,
  and matching upstream's expression-shaped, lvalue-operand contract keeps the two backends
  symmetric. If a real `-ftz 2` port ever lands, revisiting both (upstream included) with a
  proper helper is the right follow-up — upstream's own TODO says as much
  ("put this code in a function").
- **Placement**: shared arm in `emit_value_common` with three new `CFamilySyntax` leaves
  (`bitcast_open`/`bitcast_mid`/`bitcast_close`) — the shape is identical, only tokens differ,
  so it fits the descriptor pattern; cpp's per-language arm was deleted.
- **Reachability in faust-rs today**: `Signals::bit_cast` (mirroring upstream `sigBitCast`) is
  public API lowered at `signal_fir/module/core_lowering.rs:57`, but no `.dsp` pipeline path
  produces it (faust-rs has no `-ftz 2` implementation yet) — no corpus case exists, so the
  verification is the two backend regression tests (`bitcast_renders_upstream_reinterpret_cast_form`
  in `cpp/mod.rs`, `bitcast_renders_c_pointer_cast_form` in `c/mod.rs`), with the upstream
  `-ftz 2` C++ output as the shape oracle. Like upstream, the emission requires an addressable
  operand; FIR producers cache bitcast operands into named temporaries.

**DRIFT 6 closed — cpp gained the reset-fallback initializer replay.** `StructInit`/`TableInit`
and `collect_struct_initializers`/`collect_table_initializers` moved from `c/mod.rs` into
`c_family.rs` (generic over the backend error type via an `invalid_section` builder — each
backend keeps its stable error codes), rather than duplicating ~70 lines into cpp. cpp's
synthesized `instanceResetUserInterface()` fallback now replays declared state initializers
(`fFreq = (FAUSTFLOAT)(440.0);`) exactly as c does (`dsp->fFreq = (FAUSTFLOAT)(440.0);` — the
existing c test already asserted this on the same `sine_phasor` fixture, making the capability
gap directly visible). Regression test:
`synthesized_reset_ui_replays_declared_state_initializers` in `cpp/mod.rs`. The empty-module
case still emits an empty body (unchanged), and the standard pipeline
(`signal_fir/module/build.rs`) always supplies an explicit reset body, so no pipeline output
changes — this is a dormant-path capability fix, as classified in §2.6.

**asc `trim_float` fixed.** NaN/Infinity now render as the AssemblyScript/TypeScript globals
(`NaN`/`Infinity`/`-Infinity`) instead of the invalid `NaN.0`/`inf.0` literals, and `-0.0`
normalizes to `0.0` like every other textual backend. Deliberately **not** unified with
`c_family::trim_float`: the special-value spellings genuinely differ (C `NAN`/`INFINITY` macros
vs AssemblyScript globals), so forcing a shared function would need a spelling table for a
backend that stays outside the C-family core. Unit test:
`trim_float_spells_assemblyscript_special_values` in `asc/mod.rs`.

**Phase 6 skipped (optional item).** The "which lifecycle stubs to synthesize" decision is six
one-line `any()` checks in cpp and inline `find()`s interleaved with emission in c's
`emit_c_api`; unifying the decision without the (genuinely language-specific) rendering would
churn c's interleaved structure for almost no absorbed lines. Consistent with this plan's own
§4 assessment ("LANG-heavy, smaller payoff per line moved"). Can be revisited if the lifecycle
emission itself ever converges.

**Guardrails after the follow-ups (all green)**: `cargo fmt --all` (applied);
`cargo clippy --workspace --all-targets -- -D warnings` (clean);
`cargo test -p codegen --all-targets` (277 passed, 0 failed — +4 new regression tests);
`cargo test -p compiler --all-targets` (all suites pass, 0 failed);
`golden-check` 190/190 OK; `cpp-backend-diff-report` 8/8 OK, 0 DIFF (report file reverted after
checking, as in previous phases).

---

## 5. Julia and AssemblyScript — do they fit the same core?

**No, not the same core, for reasons found by direct inspection, not just structural guessing:**

- Julia's `emit_type`/`emit_cast`/`emit_var_ref`/`emit_binop` take **no `options` parameter at
  all** ([`julia/mod.rs:1353`](../crates/codegen/src/backends/julia/mod.rs) `fn emit_type(typ:
  &FirType) -> String`) — there is currently zero syntax-parameterization surface to plug a
  descriptor into without first adding it, unlike c/cpp where `Options` structs already exist and
  are already threaded everywhere.
- Julia needs functions with **no C/C++ counterpart at all**: `zero_value` (Julia's `mutable
  struct` fields must be initialized in a constructor-equivalent, since Julia has no
  `calloc`-zeroed storage or default member initializers the way C/C++ structs do) and
  `emit_struct_default_initializers` ([`julia/mod.rs:388`](../crates/codegen/src/backends/julia/mod.rs)).
  This is a structural difference, not a syntax difference — no leaf-substitution descriptor makes
  it go away.
- Julia's `Xor` binop has no infix token (`emit_binop_expr` renders `xor(lhs, rhs)` as a function
  call, [`julia/mod.rs:1261`](../crates/codegen/src/backends/julia/mod.rs)) and its `LRsh` uses
  `Int32(UInt32(...) >> ...)` constructor-call syntax rather than a cast expression — again
  structural, not a token swap.
- **Julia does independently confirm two of the seven drifts** (§2.3, §2.4): it already has the
  `-0.0` normalization and the `\r`/`\t` escapes that cpp lacks. This is useful corroborating
  evidence for the drift findings, not evidence that Julia belongs in the shared core.

**Recommendation**: build the `CFamilySyntax` core for c/cpp only (as designed above). Revisit
Julia only if, after the core stabilizes, its `emit_type`/`trim_float`/string-literal functions can
be pulled from `c_family.rs` with Julia supplying its own `CFamilySyntax`-shaped table for the
*subset* that does line up (float/string formatting in particular look shareable even though
struct/type declaration does not) — a smaller, separate follow-up, not part of this plan's Phase
1–5.

**Post-implementation note (2026-07-05)**: with the core now landed, the concrete Julia-reuse
candidates are narrower than the pre-implementation guess. `string_literal` is the one function
Julia could adopt verbatim today (its escape table is byte-identical to `c_family`'s).
`trim_float` is *almost* shareable — Julia differs only in the special-value spellings
(`NaN`/`Inf`/`-Inf` vs `NAN`/`INFINITY`) and in using `{value:?}` rather than `{value}`
formatting — so sharing would need two spelling leaves plus a formatting flag; worth doing only
if `trim_float` changes again (a third divergence event would tip the cost/benefit). Everything
else confirmed non-transferable: `CFamilyValueCtx`/`CFamilyStmtCtx` assume C-family expression
shapes (ternary `?:`, infix `^`, cast-style casts, `->` glue calls) that Julia renders
structurally differently (`ifelse`-style selects would not even apply — Julia uses `?:` too, but
its casts are constructor calls, xor is a function call, indexing is 1-based via
`emit_index_expr`), and the collectors (`StructInit`/`TableInit`) match Julia's independent
implementations in shape but feed a `mutable struct` constructor pattern, not a method body.
No implementation planned.

**AssemblyScript (`asc`)**: same conclusion, more strongly. `asc`'s `map_fun_name`
([`asc/mod.rs:1067`](../crates/codegen/src/backends/asc/mod.rs)) threads an entire
`options.double_precision`-driven suffix scheme (`_fmod`/`_fmodf`, `_isnan`/`_isnanf`, …) tied to
AssemblyScript's own math-namespace conventions, and `asc`'s `trim_float`
([`asc/mod.rs:1339-1345`](../crates/codegen/src/backends/asc/mod.rs)) is a plain

```rust
fn trim_float(value: f64) -> String {
    let mut text = format!("{value}");
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}
```

— **entirely missing** the `is_nan()`/`is_infinite()` special-casing that c/cpp/julia all added in
commit `2b615948` ("Fix C and C++ infinity literal emission") back in response to a real reported
bug (`ma.MIN * 1e307` folding to `INFINITY` upstream but `inf.0f` pre-fix in Rust). `asc`'s backend
was added seven weeks later and was evidently derived from the pre-fix `trim_float` shape, silently
reintroducing the exact bug the other three backends had already fixed: `format!("{value}")` on
`f64::INFINITY`/`NAN` yields `"inf"`/`"NaN"`, and the no-decimal-point guard then appends `.0`,
producing **`"inf.0"`, `"-inf.0"`, `"NaN.0"`** — none of which are valid AssemblyScript numeric
literals, so any `.dsp` file whose constant folds to `±inf`/`NaN` fails to *compile* under `-lang
asc` today while succeeding under c/cpp/julia. This is the fourth, independent confirmation that
this exact bug class (fix one backend, forget the others) is systemic across all four textual
backends, not specific to c-vs-cpp, and further evidence that `asc` needs its own audit pass —
but it does not change the conclusion that `asc` shouldn't join the c/cpp core, given its
class-based-but-not-C-family runtime model and richer precision-suffixed math-name table. Out of
scope for Phase 1–5; noted here because the `trim_float` gap is too directly relevant to this
plan's central thesis to omit, and because it is the cheapest of all seven drifts to fix
independently of the rest of this plan (a four-line change, mirroring the existing c/cpp/julia
special-case, with no dependency on the shared-core migration).

---

## 6. What the existing guardrails actually check (so this plan doesn't double-claim coverage)

- **`cargo run -p xtask -- backend-align-smoke`**
  ([`crates/xtask/src/backend_align.rs`](../crates/xtask/src/backend_align.rs)): golden-check +
  Cranelift strict-subset compilation + Cranelift-FFI runtime diff smoke + interp opt-level trace
  diff + FIR dump structural scan. This is about **interp/Cranelift/FIR alignment**, not c-vs-cpp
  textual drift — none of its phases render or compare c/cpp/julia output against each other.
- **`cargo run -p xtask -- cpp-backend-diff-report`**
  ([`crates/xtask/src/reports.rs:404`](../crates/xtask/src/reports.rs)), **`c-fastlane-diff-report`**
  ([`reports.rs:760`](../crates/xtask/src/reports.rs)), and **`backend-full-corpus-diff-report`**
  ([`reports.rs:956`](../crates/xtask/src/reports.rs)): all three compile a set of representative or
  full-corpus `.dsp` files through both the matching Rust backend (`cpp` or `c`) and the upstream
  `faust` binary, then compare a reduced **shell signature** —
  `extract_shell_signature`/`extract_c_shell_signature` — checking only ~10 boolean structural
  markers ("has a `buildUserInterface` fn", "has an `instanceInit` ordered call sequence", the
  `#define FAUSTCLASS`/`RESTRICT`/`exp10` macro presence, etc.). This is **Rust-vs-upstream-C++
  structural shell parity**, at a coarse boolean-presence granularity. It would not have caught any
  of the seven drifts in §2 — none of them change whether a function *exists*; they change what a
  function's body *emits* (a missing cast inside a call, a dropped initializer list, a missing match
  arm that only fires on rare FIR shapes, a wrong escape table). The shell-signature comparison
  intentionally normalizes past exactly this level of detail, on all three of these tools.
- **Conclusion**: none of the four existing reports would have caught any of DRIFT 1–7, and none of
  them exist to catch c-vs-cpp-vs-julia-vs-asc drift against *each other* (all four diff Rust output
  against upstream C++, never against Rust's own sibling backends). The shared core is the only
  mechanism proposed here that makes this class of bug structurally impossible rather than relying
  on a new comparison script to catch it after the fact (a `c-vs-cpp-diff` xtask report comparing
  full generated text — not just the shell signature — would be a reasonable complementary
  guardrail, and is worth adding regardless of this plan's timeline, but is not a substitute for
  removing the duplication: new code can still drift from a report's fixed case list, but it cannot
  drift from itself).

---

## 7. Validation checklist (every phase)

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p codegen --all-targets
cargo run -p xtask -- golden-check
cargo run -p xtask -- cpp-backend-diff-report   # must not regress vs the pre-phase baseline
```

Additionally, add one regression test per drift as it is closed, all currently absent from
`crates/codegen/src/backends/{c,cpp}/mod.rs`'s test modules:

| Drift | Phase | Regression fixture |
|---|---|---|
| 1 (`DeclareTable` init dropped) | 4 | Small FIR with a local `AccessType::Stack` `DeclareTable` carrying literal values, compiled through `cpp`. |
| 2 (`Bitcast` unsupported in c) | 3 | FIR with a `Bitcast` node, compiled through `c`. |
| 3 (`-0.0` in cpp) | 2 | `Float64` constant `-0.0`, compiled through `cpp`, assert output contains `0.0` not `-0.0`. |
| 4 (`\r`/`\t` escaping in cpp) | 2 | UI label containing a tab, compiled through `cpp`, assert the emitted literal contains `\t` not a raw tab byte. |
| 5 (missing `FAUSTFLOAT` cast in cpp) | 5 | `AddSlider`/`AddBargraph` FIR node, compiled through `cpp`, assert the call arguments are `(FAUSTFLOAT)`-wrapped. |
| 6 (`StructInit`/`TableInit` fallback missing in cpp) | 6 | FIR module omitting an explicit `instanceResetUserInterface` body but declaring UI-bound struct state with a non-zero init, compiled through `cpp`, assert the synthesized fallback assigns the declared value. |
| 7 (`Control`/`WhileLoop` unsupported in c) | 4 | FIR with a `Control`/`WhileLoop` node, compiled through `c`. |

The `asc` NaN/Infinity gap noted in §5 is independent of this plan's phases (it doesn't touch
`c`/`cpp`) and can be fixed standalone at any time by porting the four-line `is_nan`/`is_infinite`
special case already present in c/cpp/julia's `trim_float` into `asc::trim_float`.

---

## 8. Risks

- **Byte-identity discipline**: the invariant ("generated output must remain byte-identical") means
  Phases 1–2 cannot silently also fix DRIFT 3/4 as a side effect — fixing `trim_float`'s missing
  `-0.0` case for cpp *changes cpp's output*, which is desired but must land as an explicit,
  separately-reviewed behavior change bundled with (not hidden inside) the unification commit, with
  its own before/after golden diff called out, not folded silently into "no changes expected."
- **`emit_stmt`/`emit_value` are the two largest functions (385/304 and 123/92 lines)**; migrating
  them (Phases 3–4) is where most of the line-count win lives but also where the hook-struct design
  (§3.2's `CFamilyHooks`) needs the most care to avoid becoming an `if language == Cpp` branch farm
  in disguise. Keep the hook surface to the minimum set found in §1.2 (var-ref rendering, the LANG-
  only arm escape hatch) and resist adding a hook for anything classified SHAPE in the
  correspondence table.
- **Scope creep toward Julia**: §5's evidence against a unified 3-language core is strong, but it
  would be easy to over-generalize `CFamilySyntax` "just in case" during Phase 2. Keep the struct
  shaped by what c/cpp actually need until a concrete Julia migration is separately planned.
