# Analysis: what to factor in `crates/codegen`

**Date:** 2026-10-18
**Scope:** `crates/codegen/src` — 80 `.rs` files, 57,773 lines including tests.
The seven textual backends hold about 14.9 k source lines (c 1,831, cpp 2,092,
`c_family` 1,182, rust 2,948, julia 1,889, cmajor 1,861, asc 1,715, codebox
1,257, `textual` 146). The three machine backends hold about 23 k non-test lines
(interp 13.7 k, cranelift 5.2 k, wasm 4.0 k).
**Status:** **implemented 2026-10-18** on branch `codegen-factorization`,
fast-forwarded into `main-dev` on 2026-10-19 (`e4cdb246`). The
[summary of the work](#summary-of-the-work-done) comes first; the detailed
outcome, with what was built, what was rejected on reading the code and
why, is [§9](#9-outcome-2026-10-18). §0–§8 are the analysis as written
before the work, kept as the record of what was proposed. It proposed four
restructurings (R1–R4) and four quick wins (Q1–Q4), each landing on its own
with byte-identical output, and drift fixes in their own commits after the
moves, each with a test.
**Method:** four read-only surveys ran in parallel: module skeleton, leaves,
statement walkers and UI, machine backends. Every finding used here was then
checked in the source. The most serious ones were reproduced with the release
binary built at HEAD `d4f3f6a5`. The tables mark each row **V** (verified while
writing this document) or **S** (taken from a survey reading, not re-checked).
**Companions:**
[`c-family-emitter-core-plan-2026-07-04-en.md`](c-family-emitter-core-plan-2026-07-04-en.md)
(the c/cpp core, done),
[`textual-precedence-aware-printing-cross-backend-analysis-2026-08-05-en.md`](textual-precedence-aware-printing-cross-backend-analysis-2026-08-05-en.md)
(precedence follow-up, still open),
[`rust-port-quality-review-2026-07-04-en.md`](rust-port-quality-review-2026-07-04-en.md)
§3.4 and §6, and
[`transform-legibility-analysis-2026-08-25-en.md`](transform-legibility-analysis-2026-08-25-en.md)
(format reference).

---

## Summary of the work done

**Where it is.** 37 commits on `main-dev`, from `d4f3f6a5` to `e4cdb246`,
history linear and not pushed:
- the analysis;
- 34 code commits, each compiling and tested on its own;
- two documentation commits (journal, difference registry, handoff).

**Factored without changing the output.** The codegen-identity
differential compared 632 `.dsp` × 23 backend configurations against the
base binary after every move, with 0 differing outputs.
- **`backends/module_view.rs` (R1).** The FIR module view and its decode,
  the function and variable lookups, the module-shape diagnostics, the
  function collectors, the sub-module views, and the flatten/schedule/
  survivor preamble of the JIT backends. Seven `decode_module` copies, six
  `find_function_body` and five `block_declares_var` became one each.
- **`backends/literals.rs` (R2).**
  - Real and string literals: one `RealSpelling` descriptor per backend
    replaces seven formatters, and three string dialects replace seven
    escapers. Old and new agree on 600,035 values.
  - The generated-file banner.
- **Operators (R2).** One C operator table (`textual::c_like_token`), plus
  `FirBinOp::is_comparison` and `FirMathOp::arity` in `fir`.
- **`backends/table_bytes.rs` (R4).** One static-table encoder for
  cranelift and wasm. Also: the wasm import walk on `fir_match_children`,
  and one host-type conversion for cranelift and the interpreter.
- **Smaller sharing (Q1–Q4).** One error bridge that forwards the FIR node
  to the diagnostic for every backend; one `RealPrecision` enum; shared test
  builders; the c/cpp `-mem0` helpers moved into `c_family`.

**Bugs fixed, live today.** Each has a test that fails without the fix.
- **JSON addresses.** A widget `address` in the JSON (wasm, `-json`,
  cranelift-ffi) kept raw labels where upstream's `PathBuilder` replaces
  spaces, `/`, parentheses and so on. Web hosts address parameters by that
  path. The corrected short names now match upstream too.
- **codebox `min`/`max`.** codebox emitted `fmin`/`fmax`, which it does not
  have, in nearly every program (`ma.SR` clamps with them).
- **Foreign variables.** A foreign variable was declared as a zero-initialized
  struct field in rust, julia and asc.
- **Rust infinity.** A Rust infinite literal was spelled `f64::INFINITYf32`,
  which is not valid Rust.
- **Rust `metadata()`.** It was an empty stub. It now reports the
  compilation metadata through new `metadata_*` fields on `RustOptions` and
  `CmajorOptions`.
- **CLI `-lang rust`.** It named sub-modules and tables after the file
  instead of the class, unlike the facade and upstream.
- **Interpreter `Maxf`/`Minf`.** The interpreter folded them differently
  from how it executes them when an operand is NaN.

**Bugs fixed, latent.** These are reachable only through FIR the production
lowering does not originate today, or through the box/signal API:
- nodes accepted and dropped silently (asc, codebox);
- a `Switch` condition re-evaluated per case (julia, cmajor, wasm, interp);
- a lifecycle prototype that suppressed its stub (cpp, asc);
- wasm imports missed under `WhileLoop`, or shadowed across two precisions;
- `FaustFloat` tables written 4 bytes wide under `-double`;
- `>>>` emitted as an arithmetic shift;
- a metadata `declare` with a zone, which does not compile in c/cpp;
- Rust integer negation panicking in debug builds;
- an unknown UI zone bound to parameter 0;
- sub-modules surviving flattening (julia, codebox).

**Made consistent.**
- `tests/textual_backend_contract.rs` checks one contract on all seven
  textual backends: every FIR statement is emitted or refused, `Switch` is
  evaluated once, a foreign variable is not state, a prototype changes
  nothing, a metadata `declare` has no zone.
- The module-shape diagnostics read the same everywhere and carry their FIR
  node.
- The c, cpp, rust and cmajor banners copy upstream's five metadata keys,
  identical to the reference over the 133 impulse-test programs.

**Not done, and why** (details in §9.3).
- An error-code macro: the text-based public-API baseline cannot see
  macro-generated items.
- A shared statement-walker trait: every arm is the target's own syntax.
- Merging cranelift's layout into `memory_layout`: the two compute different
  things.
- A shared scalar-math module.

Still open, listed in §9.5:
- codebox NaN/∞ spelling;
- `fbc_to_cpp`;
- the interpreter's name heuristics;
- the six UI tree walkers;
- the stale CLI transcripts.

**Size.**
- `crates/codegen/src`: −924 lines (2,435 added, 3,359 removed).
- The seven textual `mod.rs` files: 16,428 → 15,321 lines; asc, cmajor and
  julia drop under the 2,000-line structure threshold.
- A 537-line contract test is added.

The gain is in single sources of truth and in consistency more than in
volume. Most of each backend is still its own statement walker.

**Validation on the final tree.**
- Workspace tests (`--no-fail-fast`): 0 failures.
- clippy `-D warnings`: clean.
- The five structure gates: OK.
- `golden-check`: 292 OK.
- Impulse harness from clean: all nine lanes pass.
- Compile-budget A/B against the base binary: no case moved.

## 0. Summary

- **Only `c` and `cpp` share an emitter.** They have shared `c_family.rs` since
  2026-07-05. The other five textual backends (`rust`, `julia`, `cmajor`, `asc`,
  `codebox`) each re-implement the whole FIR walk and every leaf. The
  precedence-aware printer in `textual.rs` has a single caller, `cmajor`.
- **Upstream shares one visitor across all its text backends.** It has one
  `TextInstVisitor` (`compiler/generator/text_instructions.hh`, 578 lines) and
  one literal module (`Text.hh`/`Text.cpp`, 969 lines). Every upstream text
  backend inherits from them; c, cpp, rust, julia, cmajor, codebox, dlang,
  csharp, jax, jsfx and template all do. Each overrides only its
  target-specific visits.
- **The default visits are what the port copied.** `TextInstVisitor` provides
  defaults for `Block`, `If`, `ForLoop`, `WhileLoop`, `Switch`, `Select2`,
  `Neg`, `BinOp` (with precedence), `StoreVar`, `Drop`, `Ret` and every
  literal. These are exactly the pieces the port has copied five to seven
  times.
- **The copies have drifted (§3).**
  - One drift is live and reproducible from a one-line program: the Rust
    backend spells an infinite literal `f64::INFINITYf32`, which is not valid
    Rust.
  - A second is a real inconsistency. The interpreter constant-folds float
    `min`/`max` with Rust semantics but executes them with `std::min`/`std::max`
    semantics. The two disagree on a NaN operand.
  - Most of the others are latent. They are reachable only through FIR the
    production lowering does not originate today, or only through the
    box/signal API. Several of them break the stated module contract:
    "Unsupported FIR nodes must fail with stable backend-specific error codes"
    (`backends/mod.rs`).
- **Proposal:**
  - **R1:** one module skeleton (decode, lookups, lifecycle-stub decisions,
    preamble, error boilerplate).
  - **R2:** one leaf layer, a Rust `Text.hh` (float and string literals,
    operator tokens, math-name tables).
  - **R3:** one structural statement walker, a Rust `TextInstVisitor` with
    statically dispatched hooks.
  - **R4:** the machine backends' duplicated object layout, scalar math
    semantics and FIR child walker.
  - **Quick wins Q1–Q4:** error bridge, shared precision enum, test fixtures,
    and the c/cpp leftovers.
- **Order of magnitude:** about 2,500–3,000 duplicated lines removable without
  changing output (estimates per item in §4). The bigger gain is structural:
  each drift class in §3 gets one place where it can be fixed.

---

## 1. Where `codegen` sits, and what is already shared

```
… transform::signal_fir ──► fir (FirStore + Module root)
        ──► fir::subcontainer (flatten/promote)  ──► fir::schedule (JIT backends)
        ──► codegen::backends::{c,cpp,rust,julia,cmajor,asc,codebox}   (text)
        ──► codegen::backends::{interp,cranelift,wasm}                  (machine)
```

Every backend receives the same `FirStore` and `Module` root. Each one
destructures the module, synthesizes the DSP lifecycle surface (`init`,
`instanceInit`, …; contract in
[`backend-lifecycle-contract-en.md`](backend-lifecycle-contract-en.md)), and
walks the FIR statements and values recursively.

What is already shared, so it is **not** re-proposed here:

| Shared piece | Location | Users |
|---|---|---|
| Literal/type/statement core | `backends/c_family.rs` (`emit_stmt_common`, `emit_value_common`, `emit_type`, `trim_float`, `format_float32`, `string_literal`, `emit_static_tables`, struct and table initializer collectors, `EmitMode`, `ForeignDependencies`) | c, cpp |
| `ordered_compilation_metadata` | `c_family.rs` | c, cpp, julia, wasm |
| Precedence-aware infix layout | `backends/textual.rs` | cmajor only |
| Lifecycle name list | `backends/mod.rs` `is_lifecycle_function` | asc, c, rust, julia, cmajor (not cpp, not codebox) |
| Sub-module names | `backends/mod.rs` `sub_module_names` | wasm, interp, cranelift (no textual backend) |
| Error carrier | `backends/codegen_error.rs` `BackendError<C>` | the seven textual backends |
| DSP API signature check | `backends/faust_api.rs` | asc, c, cpp, julia, rust (not cmajor, not codebox) |
| Math op identity | `fir::FirMathOp::{symbol, from_symbol}` | cpp, asc, wasm, cranelift, transform (not c, rust, julia, cmajor, codebox, interp) |
| Host function signatures | `backends/host_function.rs`, `foreign_call` crate | cranelift, interp (not wasm) |
| Flatten / schedule | `fir::subcontainer`, `fir::schedule` | julia, codebox, cmajor, wasm, interp, cranelift |
| JSON / short names / paths | `json.rs`, `shortname.rs` | wasm, compiler, cranelift-ffi, cmajor, codebox, interp-ffi |
| `-mem0` zones | `memory_layout::analyze_mem0` | c, cpp, cranelift |

---

## 2. Duplication inventory

Classification: **IDENTICAL** means the same code modulo names. **SHAPE** means
the same algorithm with different target tokens. **LANG** means the copies are
genuinely target-specific; those are listed only so they are not mistaken for
duplication.

### 2.1 Module skeleton (→ R1)

| Item | Copies | Class | |
|---|---|---|---|
| `decode_module` + private `ModuleView` | 7: codebox, asc, cmajor, rust, cpp, julia, c (24–33 lines each); wasm inline at `wasm/mod.rs:353` | IDENTICAL. asc ≡ cpp byte for byte; the others differ only in which of `name`/`sub_modules` they keep and in error wording | V (asc/cpp diffed) |
| `find_function_body` | 6: cpp, asc (`declared_function_body`), cmajor, codebox, wasm, `json.rs`; plus inline `iter().find(…)` at about 15 sites in c/rust/julia, and the same predicate in `memory_layout.rs`, `compute_cost.rs`, `cranelift/core.rs` | IDENTICAL | S |
| `is_dsp_api_method` | 4: codebox ≡ cmajor (14 names), asc ≡ cpp (8 names; the latter is `LIFECYCLE_FUNCTIONS` minus `staticInit`) | IDENTICAL per pair | S |
| `collect_module_functions` / `…_names` | 5: rust ≡ julia, c; asc ≡ cpp | IDENTICAL per pair, **but the two pairs have different policies** (§3 D12) | S |
| `block_declares_var` / `block_stores_var` | 5 pairs: asc, c, cpp, rust, julia | IDENTICAL (rustfmt layout only) | V |
| `invalid_section` / `unsupported_node` | 5 | IDENTICAL; cpp alone adds `.at_node()` | S |
| Sub-module preamble (flatten → promote → map_err → rebind) | codebox, julia, cmajor, wasm, interp, cranelift (~20–35 lines each) | SHAPE | S |
| `emit_sub_modules` | 5: asc, cmajor, rust, cpp, c (89–138 lines) | SHAPE. A ~25-line common skeleton (guard, destructure, nested first, tables, struct, `getNumInputs<sub>`, method loop); the rest is LANG | S |
| Lifecycle-stub synthesizer ("use the declared X, else emit a stub") | 7: c `emit_c_api` 227 lines, cpp 213, rust 199, julia 189, asc 130, cmajor 67, codebox 33 | SHAPE: one decision table, seven renderings | S |
| Header banner + compile-options line | 7 (~12–15 lines each) | SHAPE, with wording drift (D18) | S |
| Error-code boilerplate (enum, `as_str`, `BackendErrorCode` impl, alias) | 7 × ~22 lines | IDENTICAL | S |
| Precision enum `{Float32, Float64}` | cmajor, julia, rust (3 identical enums); asc, c, cpp, codebox use a `double_precision: bool` | IDENTICAL / SHAPE | S |

### 2.2 Leaves (→ R2)

| Item | Copies | Class | |
|---|---|---|---|
| Float literal | 7 algorithms: `c_family::trim_float`, asc, rust, julia (`{:?}`), cmajor `float_literal`, codebox `float_literal`, `fbc_to_cpp::fmt_real_lit` | SHAPE. Five policy axes vary: NaN/Inf spelling, `-0.0`, `{}` vs `{:?}`, f32 widened to f64 or not, precision taken from the node type or from the options | S, with D1 and D3 V |
| String escape (`\\ " \n \r \t`) | 7: `c_family::string_literal` ≡ `rust_string_literal`; `cmajor_string`, asc `escape_as_string`, `fbc_to_cpp::escape_str` (same set); julia (+`$`); `json.rs` (+`\uXXXX`) | IDENTICAL ×5, SHAPE ×2 | S |
| `emit_type` | 8 | LANG leaves, SHAPE recursion. c `emit_named_type` + `emit_type_base_and_suffix` ≡ cpp's (19 lines each, left out of `c_family`) | S |
| Binop token table | 5: `c_family`, codebox ≡ cmajor, asc (LRsh → `>>>`), julia (Xor → `xor`) | IDENTICAL except 2 entries | S, LRsh V |
| `is_comparison` | rust, cmajor, wasm, plus inline in cranelift | IDENTICAL | S |
| Math name → target | ~10 tables: c, cpp, asc (86 lines), rust (154), julia, codebox, cmajor, interp (`math_lib_lookup`), `fbc_to_cpp/math_ext.rs`, cranelift (`math.rs` + `host.rs` `HOST_MATH` 262 lines), wasm (~270 lines). The "f suffix" knowledge is encoded five different ways | SHAPE/LANG; coverage gaps D7 | S |
| Math arity | `transform/.../module/build.rs:656` ≡ `vector/module/lifecycle.rs:330`; implied again in cranelift and wasm; `FirMathOp` has no `arity()` | IDENTICAL | S |

### 2.3 Statement walkers, state collectors, UI (→ R3)

| Item | Copies | Class | |
|---|---|---|---|
| Control-flow arms (`If`, `Control`, `ForLoop`, `SimpleForLoop`, `WhileLoop`, `Switch`) | 6: `c_family` (146 lines), rust, julia, asc, cmajor, codebox (75–120 each) | SHAPE | S, Switch V |
| `emit_block` / `emit_block_with_mode` | 5 IDENTICAL ("expected block" error, then loop); cmajor/codebox run a non-Block as one statement; one codebox path emits nothing | IDENTICAL + policy drift | S |
| `EmitMode` `{Default, Metadata, Ui[, Compute]}` + the name→mode `match` | c_family, rust, julia; the `match` repeated in c, cpp, rust, julia | IDENTICAL (`Compute` behaves like `Default` everywhere) | S |
| UI widget arms in `emit_stmt` | c_family (100 lines), rust (114), julia (109); asc comments only; cmajor/codebox handle UI elsewhere | SHAPE | S |
| `AddMetaDeclare` arm with its `var == "0"` sentinel | c, cpp, rust, julia | SHAPE, with zone drift (D16) | S |
| Struct/table initializer collectors | `c_family` (shared by c/cpp), re-implemented in rust (52 lines) and julia (72) with their own `StructInit`/`TableInit` | IDENTICAL traversal (the copies lack `access`) | S |
| `emit_struct_definition` / `emit_struct_fields` | c, rust, julia, asc | SHAPE, with D13 | V (rust) |
| `emit_static_tables` | `c_family` + c/cpp `-mem0` branches (~45 lines each, SHAPE of each other), asc, rust, julia | SHAPE | S |
| UI tree walkers (`OpenBox`/`CloseBox` path stack) | 6: `json.rs` `parse_ui_items`, cmajor `collect_ui_in`, codebox `collect_params_in`, rust `collect_ui_stats`, interp `compiler/ui_soundfile.rs`, `cranelift-ffi/src/runtime.rs` | SHAPE, with D17 | S |
| UI address builder `/a/b/label` | `json.rs` inline, cmajor `ui_path`, codebox `ui_address`, `interp-ffi/src/json.rs` (re-implements `shortname::build_path`) | IDENTICAL ×3 + one sanitizing variant | S |

### 2.4 Machine backends (→ R4)

| Item | Copies | Class | |
|---|---|---|---|
| DSP object byte layout (fields in order, `align_up`, size rounded to max alignment) | `memory_layout.rs` `object_layout` + `type_layout` (~170 lines); `cranelift/core.rs` `build_struct_layout_for_module` + `fir_type_layout_scalar` (~230) | SHAPE. Same result under `-mem0`, but the only cross-check is a runtime one (`cranelift-ffi/src/types.rs:177`, "mem0 DSP object/layout mismatch") | S |
| `align_up` | `memory_layout.rs` (checked: power of two, overflow), `cranelift/core.rs`, `wasm/layout.rs` (neither checked) | SHAPE | S |
| Static-table byte serialization | `cranelift/jit_data.rs` (55), `wasm/mod.rs` (78), interp `compiler/blocks.rs` | SHAPE, with D20 | S |
| Scalar math semantics | `interp/real.rs` vs `cranelift/host.rs` (same `a - (a/b).round_ties_even()*b` remainder) | IDENTICAL formulas | S |
| `ScalarType → HostScalarType`, `NamedFunction → HostFunctionSignature`, call-shape check | `cranelift/math.rs` vs `interp/foreign.rs` / `compiler/control.rs` (same `unreachable!` text) | IDENTICAL / SHAPE | S |
| Preamble flatten → `schedule_compute` → "sub-modules survived" check | `wasm/mod.rs:330`, `interp/mod.rs:212`, `cranelift/api.rs:63` | SHAPE | S |
| FIR children walker | `wasm/mod.rs:2664` hand-rolled `fir_children` vs shared `fir::fir_match_children` | SHAPE, with D9 | V |
| `fbc_to_cpp` vs `c_family` | `escape_str` ≡ `string_literal`; `fmt_real_lit` vs `trim_float`; `FAUSTFLOAT` prelude vs `cpp`'s; `std::` names re-listed | IDENTICAL / drift D3 | S |
| Statement/expression dispatch (`lower_stmt`, `lowering.rs`, `compiler/blocks.rs`) | 3 | **LANG**: do not factor | — |
| WASM linear-memory layout, interp two-heap layout | 2 | **LANG** | — |

### 2.5 Tests

- `trivial_block` is IDENTICAL in `interp/{factory,fbc_to_cpp,instance}/tests.rs`.
- About 30-line `FbcDspFactory::new(…)` builders of the same shape appear in
  four interp test files.
- The compute signature (`dsp`/`count`/`inputs`/`outputs` `NamedType` plus
  `FirType::Fun`) is written inline 23 times in `wasm/tests.rs` and 25 times in
  `cranelift/tests.rs`. The `output0` alias boilerplate appears 13 and 21 times.
- `fixtures.rs` already has the matching builders (`compute_fun_args`,
  `compute_fun_type`, `io_aliases_for_mono_compute`, `module_with_functions`,
  `math_prototype`), but they are private. Today the interp tests use no fixture
  from it at all, cranelift tests use one, and wasm tests use six. (S)

---

## 3. Drifts found between copies

These are the reason to factor. Each one is a place where one copy received a
fix, a case or a policy that its siblings lack. "Latent" means the production
lowering does not originate the triggering FIR today. Per the walkers survey,
`Switch`, `Control`, `WhileLoop`, `IteratorForLoop`, `ShiftArrayVar`,
`DeclareStructType` and `DeclareBufferIterators` are only rebuilt by
`fir::inliner`, `fir::schedule` and `transform/.../cse.rs`, which copy what
they are given. A latent drift becomes live the day a producer starts emitting
that FIR.

| # | Drift | Backends | Reach | | Fixed by |
|---|---|---|---|---|---|
| D1 | **Infinite/NaN literals are invalid Rust.** `trim_float` returns `f64::INFINITY` and the caller appends the suffix: `process = _ * (1e30*1e30);` emits `f64::INFINITYf32 * …` (cpp emits `INFINITY`). An f32 NaN is also spelled `f64::` | rust | **live** | V (reproduced) | standalone fix now, then R2 |
| D2 | **Float `min`/`max` on a NaN operand behave six ways.** cpp `fmin`/`fmax` (absorbs NaN); julia was fixed to match after `rad_tbptt_softclip_drive` (`julia/mod.rs:368`); the interp executor uses `std::min`/`max` (order-dependent); the interp optimizer folds with Rust `min`/`max` (absorbs), **so a folded result can differ from the executed one**; `fbc_to_cpp` `std::min`; wasm and asc native `f32.min` (propagates); cranelift `a.max(b)` | interp, wasm, asc, fbc_to_cpp vs cpp, julia, cranelift | live on NaN inputs | V (interp) | decision 3, then R2/R4 |
| D3 | **`-0.0` and literal text.** c_family, asc, rust and julia normalize `-0.0` to `0.0`; cmajor, codebox and `fbc_to_cpp` keep `-0.0`. codebox has no NaN/Inf case (`NaN`, `inf`); cmajor and codebox narrow to f32 first, so a finite f64 above `f32::MAX` becomes `inf`. The asc comment says its normalization matches "every other textual backend"; it does not | cmajor, codebox, fbc_to_cpp | live for those literals | S | R2 |
| D4 | **Logical right shift spelled `>>`** (arithmetic). The others keep it logical (`uint32_t`, `UInt32`, `u32`, `>>>`, `I32ShrU`) | cmajor, codebox | box/signal API only (`>>>` is not `.dsp` syntax) | V | R2 |
| D5 | **Unknown UI zone maps silently to `ParamIndex(0)`**: `ctx.ui_params.get(&var).copied().unwrap_or_default()`, which aliases the first parameter; json gives `None`, interp `-1` | rust | latent (needs a FIR/UI inconsistency) | V | R3 |
| D6 | **Sub-module `globals` never emitted.** `emit_sub_modules` destructures with `..` and never reads `globals`; asc, cmajor, cpp and c emit it. It holds foreign declarations and `global_declarations` | rust | latent (a generated table whose fill uses a foreign symbol; not reproduced) | V (code) | R1 |
| D7 | **Math coverage gaps.** rust lacks `exp2`; asc emits a bare `log2`; interp, cranelift and wasm lack `log2`/`exp2`; codebox lacks `isinf`, `copysign` and the canonical `fmin`/`fmax`; interp rejects `std::`-prefixed names that `FirMathOp::from_symbol` strips for everyone else; asc picks `pow`/`rint` precision from the options instead of the node type | several | live for those functions | S | R2 (one table per `FirMathOp`) |
| D8 | **Silent acceptance of unsupported nodes.** asc: `ShiftArrayVar`/`DeclareStructType` → `Ok(())`, `IteratorForLoop` → body once (the shape cpp removed, comment still claims parity); codebox: `Drop(_) => Ok(())` drops an effectful call; interp: `Return(Some(v))` ignored | asc, codebox, interp | latent | V (asc, codebox) | R3 default arm |
| D9 | **WASM import collection walks a partial child list.** `fir_children` omits `WhileLoop`, `Switch`, `TeeVar`, `Bitcast` and `LoadSoundfile*`. A math call found only under them is never imported, giving "missing imported WASM function index" | wasm | latent | V | R4 (use `fir_match_children`) |
| D10 | **WASM hyperbolic imports.** `acoshf` (F32) and `acosh` (F64) both import `_acosh`, and likewise for asinh, atanh, cosh, sinh and tanh. Imports are keyed by field name, so a module using both precisions shadows one. Upstream names the import `_` + the FIR name (`_acoshf`, `wasm_instructions.hh:646`, `wasm_dsp_aux.cpp:133`) | wasm | latent (mixed precision) | V | R2/R4 |
| D11 | **`Switch` condition evaluated once vs once per case.** Once in c, cpp, asc, rust and cranelift; per case in julia (`elseif {cond} == …`), cmajor, interp and wasm. The results differ if the condition holds a `TeeVar` or an effectful call | julia, cmajor, interp, wasm | latent | V (julia) | R3 |
| D12 | **Prototype-only lifecycle declarations.** c, rust and julia ignore a body-less declaration, so the stub is still synthesized. cpp and asc let it suppress the stub: cpp then emits `virtual void instanceClear();` with no definition, and asc emits nothing while `instanceInit` still calls `this.instanceClear()` | cpp, asc | latent | V (asc) | R1 (decision 4) |
| D13 | **Foreign variables as struct fields.** `DeclareVar(AccessType::Global)` (`core_lowering.rs:531`) is skipped by c and cpp but emitted as a struct field by rust, julia and asc (`emit_struct_fields` over `globals`) | rust, julia, asc | live if a foreign variable reaches them (not checked) | V (code) | R3 |
| D14 | **Metadata/UI fallbacks.** c without a `metadata` function emits the placeholder `m->declare(…, "faust-rs", "module-first c backend prototype")`; cpp and julia replay the compilation metadata; rust has no metadata transport; asc emits no method. With an empty `buildUserInterface`, cpp alone synthesizes `openVerticalBox(name); closeBox();`. c derives the metadata name and filename from `class_name`, the others from the module name | all | latent (production declares both functions) | V (c, cpp) | R1 |
| D15 | **FIR provenance dropped.** `codegen_error.rs` lets every backend attach `fir_node`, but only cpp calls `.at_node()` (2 sites), and the six `impl_backend_error_via_methods!` bridges in `backend_error.rs` never forward `fir_node`, even when attached | 6 textual + wasm | **live** (diagnostic quality) | V | Q1 |
| D16 | **Metadata-mode `declare` zone.** c always passes one (`&dsp->var` or `0`); cpp only when `var != "0"`; rust and julia never | c, cpp, rust, julia | live, cosmetic in practice | S | R3 |
| D17 | **UI walkers disagree.** json errors on a nested `Block`; cranelift-ffi silently drops widgets inside one; cmajor, codebox and rust recurse (rust also into `If`/`Control`/`Switch`); cmajor clears pending metadata on `OpenBox` and `CloseBox` where json keeps it. The json `address` keeps spaces and `/` where interp-ffi's `build_path` sanitizes them | json, cranelift-ffi, cmajor, codebox, rust, interp-ffi | live for address spelling (not checked against upstream) | S | R3 + R4 |
| D18 | **Banner and default options string.** c and cpp default to `-lang c`/`-lang cpp` without `-single`/`-double`, the others append it; there are five wordings of the generator line; the `name:` line is unescaped in c, escaped in cpp, unquoted in asc, and absent elsewhere | all | live, cosmetic | S | R1 |
| D19 | **Integer wrapping is uneven.** c_family wraps Int32 add/sub/mul only; rust wraps add/sub/mul/div/rem and shifts for i32 and i64, but `Neg` is a plain `(-x)`, which panics on `i32::MIN` in a debug build; `fbc_to_cpp` emits raw int `+ - *` (the C++ UB c_family fixed); float→int casts saturate (rust, wasm, cranelift), wrap (julia), truncate (codebox) or are UB (C/C++) | rust, fbc_to_cpp, … | live on overflow | V (rust Neg) | R2 + decision 5 |
| D20 | **Static tables and layout coverage.** cranelift rejects `Bool` tables and skips mistyped values silently, while wasm accepts and errors; wasm writes a Float32 value of a `FaustFloat` table as 4 bytes under `-double`; `memory_layout` accepts `Vector`/`Struct`/`Fun` where cranelift rejects them; wasm lays out `Int64` as an F64 slot but types it `I64` | cranelift, wasm | latent (not checked whether FIR produces these) | S | R4 |

Checked and **not** a drift: codebox's `dspsetup` order (staticInit, classInit,
resetUI, clear, constants) differs from the other backends but matches upstream
`codebox_code_container.cpp:191-202`. julia skipping `control`/`frame` is
guarded upstream of the backend by `external_control: Unsupported` in
`compiler/src/execution.rs`. asc's `DeclareTable` arm ignoring `values` is
harmless on the production path, because struct tables are refilled explicitly
in `instanceConstants` (`register_constant_table_init`); it stays a latent copy
of the cpp DRIFT 1 shape.

---

## 4. Proposed restructurings

The four are independent: none needs another to have landed first, and they
touch disjoint code. Inside each one, every step is a **move with
byte-identical output**. The shared code is parameterized so that it reproduces
each backend's current output exactly, bugs included. Fixing a drift is a later
commit with its own test.

All code stays `pub(crate)` inside `codegen`; no new public item is needed
except in Q2. Dispatch is static (generic parameter or `const` descriptor,
never `dyn`), as `c_family` already does.

### R1 — One module skeleton: `backends/module_view.rs`

**Goal:** a backend starts from a decoded, validated module instead of
re-deriving it.

- `ModuleView` holds the union of today's seven copies (`name`, `dsp_struct`,
  `globals`, `functions`, `static_decls`, `sub_modules`, `num_inputs`,
  `num_outputs`), plus `decode(store, root) -> Result<ModuleView, SkeletonError>`.
- `ModuleView::function(name) -> Option<FunctionView>` replaces
  `find_function_body`, `declared_function_body` and the ~15 inline `find`s.
  `FunctionView` keeps the body as an `Option`, so the prototype policy (D12)
  is an explicit argument, not an accident of which collector a backend copied.
- `block_declares_var` / `block_stores_var` move into `module_view.rs` once.
- `is_dsp_api_method` is derived from `LIFECYCLE_FUNCTIONS` in
  `backends/mod.rs`; the codebox/cmajor 14-name list becomes that list plus
  their extra entries.
- `LifecyclePlan::of(&ModuleView)` makes the shared decision behind the seven
  synthesizers. For each lifecycle entry it says *declared with a body /
  declared empty / absent*. Each backend keeps its own rendering, but stops
  re-deciding. D14 becomes a visible per-backend table instead of seven
  scattered `if`s.
- `prepare_module(store, root, PrepareOptions) -> (FirStore, FirId)` covers the
  flatten → promote → (schedule) → "sub-modules survived" sequence. The check
  for surviving sub-modules is the one julia and codebox skip.
- `emit_sub_modules` gets a shared iterator over `SubModuleView`, so rust (D6)
  is handed `globals` like everyone else.
- `SkeletonError { RootNotModule, InvalidSection, … }` is mapped by each backend
  into its own `CodegenErrorCode`. The step-1 mapping keeps every current
  message, so no diagnostic text changes. Unifying the wording is decision 1.
- Error-code boilerplate: one `macro_rules!` declares the enum, `as_str`, the
  `BackendErrorCode` impl and the alias.

**Size:** ≈1,000 duplicated lines removed. **Risk:** low; the logic is
selection, not emission. **Proof of identity:** the codegen-identity
differential (§6) plus the existing per-backend tests; diagnostics are compared
via the backends' error tests.

### R2 — One leaf layer: `backends/literals.rs` (the Rust `Text.hh`)

**Goal:** a constant, an escaped string, an operator token or a math name is
spelled by one function, driven by a per-backend `const` descriptor.

- `FloatSpelling { nan, pos_inf, neg_inf, neg_zero: Normalize|Keep, style:
  Display|Debug, widen_f32: bool, suffix: fn(precision) -> &str }` plus
  `format_real(value, precision, &FloatSpelling)`. The seven current algorithms
  are seven descriptors. D1 shows why the suffix belongs in the descriptor: the
  bug is a suffix appended outside the function that knows the value is
  special.
- `escape_string(input, &EscapeSet)`, where the set is the base five plus
  julia `$` and json `\uXXXX`. `fbc_to_cpp` and `rust` switch to it unchanged.
- `binop_token(op, &BinopSpelling)` with two override slots (LRsh, Xor), plus
  `FirBinOp::is_comparison()` in `fir`. This is also where `textual.rs`'s
  precedence helpers are offered to every backend. Adopting them changes output,
  so that is the open precedence follow-up, not part of this move.
- `MathTable`: keyed by `FirMathOp` × precision → target spelling. `FirMathOp`
  gains `arity()`; that is the one `fir` change, which also removes the two
  identical tables in `transform`. Each backend's table becomes data. D7's gaps
  show up as missing cells that a test can enumerate
  (`every_backend_maps_every_FirMathOp_or_rejects_it`).
- c/cpp `emit_named_type` + `emit_type_base_and_suffix` move into `c_family`.

**Size:** ≈500–600 lines. **Risk:** low for literals and escapes, medium for
math tables (rust's method table, asc's precision rule). **Proof:** identity
differential; plus a table test that renders every special value (NaN, ±Inf,
±0, subnormal, `f32::MAX`+, 1e-10, 1e300) through every descriptor. That test
is what makes D1/D3 visible before they are fixed.

### R3 — One structural walker: a `TextTarget` trait (the Rust `TextInstVisitor`)

**Goal:** the control-flow and statement arms exist once. A backend supplies
hooks, and every FIR variant it does not handle falls to one default arm that
returns the stable "unsupported node" error.

- Generalize `c_family`'s `CFamilyStmtCtx`/`CFamilyValueCtx` seam into a trait
  with required hooks: `var_ref`, `declare`, `store`, `call`, `block_open` /
  `block_close`, `if_syntax`, `for_syntax`, `while_syntax`, `type_name`, and
  `ui_call`. It also has *default methods* for the structural arms: `Block`,
  `If`, `Control`, `ForLoop`, `SimpleForLoop`, `WhileLoop`, `Switch` (condition
  evaluated once into a local, decision 6), `Drop`, `Return`, `Select2`,
  `Neg`, `BinOp` and `StoreTable`. Monomorphized per backend; no `dyn`.
- `EmitMode` and the name→mode `match` move into the walker.
- The UI widget and `AddMetaDeclare` arms become one default method that calls
  the `ui_call` and `meta_zone` hooks (D5, D16).
- Struct-field emission skips `AccessType::Global` in the walker (D13, after
  decision).
- The UI tree walk (the `OpenBox`/`CloseBox` path stack) becomes one iterator
  in `json.rs` or `shortname.rs`, used by json, cmajor, codebox, rust, interp
  and cranelift-ffi. The nested-`Block` policy is decided once (D17).

**Order inside R3:** start with backends whose arms are closest to
`c_family`'s (asc, then julia), then codebox and cmajor, and rust last. rust's
locked-table access, wrapping-method arithmetic and `ParamIndex` UI are the
most LANG-heavy; the precedence analysis §4 already stopped a rust attempt for
this reason. A backend can stay on its own walker indefinitely; R3 is
worthwhile even at three adopters.

**Size:** ≈600–900 lines. **Risk:** highest of the four, because emission
order and whitespace must be reproduced exactly. **Proof:** identity
differential per backend as it adopts. Also add a structural test that feeds
every `FirMatch` statement variant to every adopted backend and asserts *either
output or the stable unsupported error, never `Ok` with nothing*. That test
fails today on asc and codebox (D8).

### R4 — Machine backends: one layout, one math semantics, one child walker

- **Layout.** `cranelift/core.rs::build_struct_layout_for_module` is computed
  through `memory_layout::object_layout`. They agree today under `-mem0`; the
  only check is a runtime assertion in `cranelift-ffi`. One `align_up` (the
  checked one) serves cranelift and wasm. WASM's slot rules stay LANG, but use
  the same `align_up`.
- **Scalar math.** `interp/real.rs` and `cranelift/host.rs` share one
  `scalar_math` module (remainder, min/max, the libm-backed set). The
  `ScalarType → HostScalarType` and `NamedFunction → HostFunctionSignature`
  conversions and the call-shape check move into `host_function.rs`, which
  both already use. The interp optimizer's fold then uses the same
  `min`/`max` as its executor (D2).
- **Child walk.** wasm `fir_children` is replaced by `fir::fir_match_children`
  (D9). The only difference is that the shared walker visits more nodes, which
  can only add imports that are actually used. Proof: identical `.wasm` bytes
  over the corpus, which never reaches the omitted nodes.
- **Static tables.** cranelift and wasm share the value-to-bytes encoding with
  one precision rule (D20).
- **`fbc_to_cpp`** uses `c_family::string_literal` and the R2 float descriptor.
  Its output is a debugging aid compared to itself, so the identity check is
  its own tests (D3 stays visible in the descriptor).

**Size:** ≈400 lines. **Risk:** low for the walker and `align_up`, medium for
layout. **Proof:** `cargo test -p codegen`, the impulse harness on interp,
cranelift and wasm, and the existing mem0 layout cross-check.

### Quick wins (each a single small commit)

- **Q1 — error bridge (D15).** One blanket
  `impl<C: BackendErrorCode> BackendCodegenError for BackendError<C>` replaces
  the six `impl_backend_error_via_methods!` calls and the hand-written cpp impl,
  and forwards `fir_node` for everyone. Then `decode_module`/`unsupported_node`
  attach `.at_node()` in R1. Move `WasmBackendError` onto `BackendError<C>` in
  the same commit.
- **Q2 — precision enum.** The three identical `{Float32, Float64}` enums
  (cmajor, julia, rust) become one. This **touches public option types**, so
  it needs a `code-graphs` regeneration and a decision (decision 2).
- **Q3 — test fixtures.** Expose `fixtures.rs`'s compute-signature and alias
  builders as `pub(crate)` under `#[cfg(test)]`. Then replace the 23 + 25
  inline compute signatures in `wasm/tests.rs` and `cranelift/tests.rs`, and
  the triplicated `trivial_block` / `FbcDspFactory` builders in interp tests.
- **Q4 — c/cpp leftovers.** `allocated_sub_containers` (IDENTICAL, 19 lines),
  `is_sub_module_method`, `is_empty_block`, the two `-mem0` static-table
  branches and the `faust_wrap_*` preludes go into `c_family`. Delete the dead
  check at c `names.contains(&f.name)` (always true).

---

## 5. What not to factor

- **The machine backends' lowering dispatch** (`wasm::lower_stmt`,
  `cranelift/lowering.rs`, interp `compiler/blocks.rs`). These are three
  different targets (stack bytecode, SSA, register bytecode). A shared visitor
  would add an abstraction no reader needs.
- **WASM linear-memory layout and interp's two heaps**: each one *is* its
  target's ABI.
- **The rendering half of each lifecycle synthesizer.** cpp's class shell, c's
  free functions, rust's trait impl, julia's `mutable struct`, cmajor's
  processor and codebox's `dspsetup` differ in substance. R1 shares the
  decision, not the text.
- **rust's arithmetic** (`wrapping_*`, casts, locked tables) and **cmajor's
  graph/endpoint structure**: LANG. They plug into R2/R3 as hooks, or stay out.
- **The seven scaffold backends** (`csharp`, `dlang`, … 17 lines each): they are
  intentionally identical placeholders, and the `lib.rs` README-consistency test
  depends on their shape.

---

## 6. Migration and validation

Every step of R1–R4 and Q1, Q3 and Q4 must show **no change in emitted output**:

1. **Codegen-identity differential.** This is the decisive gate. Build the
   base and new trees. Compile every `.dsp` under `tests/` and `examples/`
   with every textual backend (`c`, `cpp`, `rust`, `julia`, `cmajor`, `asc`,
   `codebox`), plus `-lang wasm` and `-mem0` where applicable. Require a zero
   byte diff. `golden-check` alone is not enough: the Rust goldens fingerprint
   compiler stdout, not backend text.
2. **Gates:** `cargo test -p codegen`, then the full workspace gates of
   `AGENTS.md` §3: fmt, clippy `-D warnings`, `cargo test --workspace`, and
   the five structure checks, with `code-graphs --check` for any `pub` change.
3. **Numerics:** the impulse harness (`tests/impulse-tests`) for R4 and for
   any R3 backend it covers.
4. **Compile budget:** `codegen` is in the gated set. The local gate cannot
   conclude on this machine (calibration under the timer floor), so do a manual
   base-vs-new A/B over the basket. Emission is not expected to move;
   monomorphized hooks cost nothing.
5. **Drift fixes come after, one per commit.** Each one carries a regression
   test that fails before the fix, and a C++ oracle comparison where upstream
   has the backend. Use the pinned 2.84.3 `RUST/faust/build/bin/faust`, not
   `/usr/local/bin/faust` (2.90). A fix that changes generated text relative to
   upstream updates
   [`faust-rs-vs-faust-cpp-differences-en.md`](faust-rs-vs-faust-cpp-differences-en.md).
6. **Structural tests that outlive the move:**
   - the R2 special-value matrix;
   - the R2 `FirMathOp` coverage table;
   - the R3 "every statement variant is emitted or refused" test.

   These three would have caught D1, D3, D7 and D8, and they keep the copies
   from drifting again.

---

## 7. Decision points (AGENTS.md §12)

1. **Diagnostic wording.** Unify the seven `RootNotModule`/`InvalidSection`
   messages and codes (cmajor uses `InvalidStructure` where others use
   `InvalidModuleSection`), or keep them per backend. This is observable text
   in diagnostics and JSON errors.
2. **Public option types.** Share one precision enum and, possibly, a common
   `TextualOptions { class_name, compile_options, metadata_* }`. This widens
   the public API (`public-api-baseline.txt`) and changes how callers build
   options.
3. **Float `min`/`max` NaN semantics (D2).** Upstream itself differs per
   backend: cpp `fmin`/`fmax`, the interpreter `std::min`, wasm native. Options
   are per-backend upstream parity (fix only the interp fold/execute mismatch)
   or one semantics everywhere (C `fmin`/`fmax`, as julia was changed to). This
   is parity-sensitive.
4. **Prototype-only lifecycle declarations (D12).** Choose either "ignore the
   prototype, synthesize the stub" (c/rust/julia) or "the prototype suppresses
   the stub" (cpp/asc, which emits a dangling call on asc).
5. **Integer overflow in generated Rust (D19).** `Neg` as `wrapping_neg()` to
   match the other wrapped ops, or keep `(-x)`. This changes the generated-code
   contract for `faust2*rust` projects in debug builds.
6. **`Switch` evaluation (D11).** Evaluate once into a local everywhere (C
   semantics, the recommended choice), or keep per-case re-evaluation where
   upstream does it.

## 8. Recommended order

1. **D1 now, standalone** (move the suffix into rust's `trim_float`, plus a
   test). It is live, three lines long, and does not need to wait for R2.
2. **Q1, Q3, Q4**: mechanical, an afternoon each, no decision needed.
3. **R1**: lowest risk, and it makes `LifecyclePlan` and `SkeletonError`
   available to the rest.
4. **R2**: its special-value matrix and math coverage table immediately expose
   D3/D7 as data.
5. **R4**: independent of the textual work; can run in parallel.
6. **R3** last, backend by backend (asc → julia → codebox → cmajor → rust),
   stopping wherever a backend's LANG weight outweighs the gain.

---

## 9. Outcome (2026-10-18)

Implemented on branch `codegen-factorization`: 32 code commits on top of
the analysis (`2c91ac52`, itself on `d4f3f6a5`), each compiling and tested
on its own, then two more for the §9.5 metadata follow-up (§9.2). Net effect on
`crates/codegen/src`: −1,049 lines (2,310 added, 3,359 removed, test code
included); the seven textual backends' `mod.rs` shrink from 16,428 to 15,250 lines.
Another 500 lines are a new shared contract test,
`crates/codegen/tests/textual_backend_contract.rs`. That is well under the
2,500–3,000 lines §0 estimated, because R3 and the R4 layout merge were not
built (§9.3).

### 9.1 Decisions taken (§7)

| # | Decision | Taken |
|---|---|---|
| 1 | Diagnostic wording | Unified. The root error reads "expected FIR module root, got … at node N" and a section error reads "section 'X' must be a FIR block, got … at node N", in every backend, with the FIR node attached. cmajor and codebox gain an `InvalidModuleSection` code (FRS-CGEN-CMAJ-0006, FRS-CGEN-CBOX-0003). |
| 2 | Public option types | Enum only, aliased. One `backends::RealPrecision`; `CmajorRealType`, `JuliaRealType` and `RustRealType` are aliases of it. No `TextualOptions`. |
| 3 | Float `min`/`max` on NaN | Per-backend upstream semantics kept. Only the interpreter's fold-vs-execute mismatch is fixed. |
| 4 | Prototype-only lifecycle declarations | The prototype is ignored and the stub synthesized (cpp and asc aligned on c, rust and julia). |
| 5 | Integer negation in generated Rust | `wrapping_neg()`. |
| 6 | `Switch` evaluation | Once, into a local, everywhere. |

### 9.2 What landed

| Item | Commit | Effect |
|---|---|---|
| D1 | `43b650f1` | rust: `f32::INFINITY`, not `f64::INFINITYf32` |
| Q1 | `2009e406` | one `BackendCodegenError` impl over `BackendError<C>`; `fir_node` reaches the diagnostic for every backend; `WasmBackendError` is the shared carrier |
| Q3 | `9545d7b3` | the compute signature and `trivial_block` shared in tests (−986 test lines) |
| Q4 | `6dd091c5` | c/cpp `-mem0` static tables, declarator split and sub-container pairs moved into `c_family` |
| Q2 | `0004d17a` | `RealPrecision` |
| R1a | `528a3f53` | `backends/module_view.rs`: `ModuleView`, `decode_module`, `section_items`, `invalid_section`, `unsupported_node`, `non_sub_module`, the function and variable lookups; unified diagnostics |
| R1b | `2e5ee0b8` | `FunctionDecl`/`collect_defined_functions`, `is_dsp_api_method`, and `is_receiver_lifecycle_method` derived from `LIFECYCLE_FUNCTIONS` |
| R1d | `c1de62d6` | `prepare_flat_module`: flatten, schedule, survivor check for wasm, interp and cranelift |
| R1e | `8cdc4714` | `sub_module_views` for the five nesting backends |
| R1 | `00e36300` | `literals::faust_banner` for c and cpp |
| R2a | `80dca40e` | `backends/literals.rs`: `RealSpelling` and the string dialects replace seven formatters and seven escapers |
| R2b | `60e6655e` | `textual::c_like_token`, `FirBinOp::is_comparison`, `FirMathOp::arity` |
| R4a | `56ffec04` | wasm import walk on `fir::fir_match_children` (D9) |
| R4b | `dbb9f186` | `HostScalarType::of_abi`/`abi`, `HostFunctionSignature::of_typed` |
| R4 | `527ba7c7` | `backends/table_bytes.rs`, one static-table encoder for cranelift and wasm (D20) |
| R3 (policy) | `00c9449c` | the contract test; asc refuses the four nodes it swallowed, and codebox evaluates a dropped call (D8) |
| D8 | `2d04f242` | interp refuses `Return(Some)` |
| D2 | `92f11ab7` | interp fold uses `cpp_max`/`cpp_min`, as execution does |
| D3 | `b28c8c16` | cmajor and codebox normalize `-0.0` |
| D4 | `c03c70c3` | cmajor and codebox write `>>>` (upstream's table name) |
| D5 | `bcb98812` | rust: an unknown UI zone is an error |
| D13 | `b2993ecb` | rust, julia and asc: a foreign variable is not a struct field (`rep_77_foreign_variable` changes) |
| D12 | `7edb871d` | cpp and asc: a prototype does not suppress its stub |
| D11 | `8e17ba33` | julia, cmajor, wasm and interp evaluate a `Switch` condition once |
| D7 | `ef74c34b` | codebox writes `min`/`max` for `fmin`/`fmax` (53 corpus outputs); rust writes `exp2` like `log2` |
| D14 | `46f51725` | c: an absent `metadata` replays the compilation metadata |
| D19 | `b0ae9772` | rust: `wrapping_neg` |
| D10 | `e38e84f9` | wasm: one import name at two precisions is refused |
| — | `c3e050a7` | julia and codebox refuse a sub-module that survives flattening |
| D18 | `1d2751ec` | the upstream generator line in the rust, julia, cmajor, codebox and asc banners |
| D16 | `609e65cc` | c and cpp: a metadata `declare` passes no zone |
| D17 | `6c481627` | JSON widget addresses built by `shortname::build_path` (C++ `PathBuilder`); short names now equal upstream's |
| §9.5 follow-up | `550ea14b` | `RustOptions`/`CmajorOptions` carry `metadata_name`/`metadata_filename`/`metadata_entries`; the compiler fills them (one `fill_compilation_metadata` for the five backends); rust `metadata()` replays them; the CLI `-lang rust` goes through the signal lowering, naming the FIR module after the class as everywhere else |
| §9.5 follow-up | `e5bd488d` | the c, cpp, rust and cmajor banners copy upstream's five metadata keys (`author`, `copyright`, `license`, `name`, `version`), identical to the reference over the impulse corpus |

### 9.3 Proposed, then rejected on reading the code

- **R1c, an error-code macro.** It was built and reverted. The public API
  baseline (`docs/code-graphs/public-api-baseline.txt`) is extracted from
  the source text, so a `pub enum` declared through `macro_rules!` showed up
  as `enum _` and its `as_str` disappeared. The gate exists to make a
  boundary change visible, so the boilerplate stays written out.
- **R3, a `TextTarget` trait with default structural arms.** Per backend,
  each construct's shape belongs to the target. julia lowers a `ForLoop` to
  a `while` with a trailing increment and a `Switch` to an `if`/`elseif`
  chain; asc and codebox use typed C-like headers; rust binds locked tables.
  A hook per construct would be the arm itself. The part of R3 that pays is
  its policy: every FIR statement is emitted or refused, and `Switch` is
  evaluated once. That part is checked by
  `tests/textual_backend_contract.rs` and enforced by D8/D11.
- **R4, cranelift's struct layout computed through
  `memory_layout::object_layout`.** They compute different things. cranelift
  needs per-field offsets in every mode, with arrays inline outside `-mem0`.
  `memory_layout` produces the `-mem0` size summary, in which the main DSP's
  arrays are pointers. Only the alignment arithmetic is common.
- **R4, a shared scalar-math module.** The interpreter and cranelift overlap
  in one-line delegations to `std`. The only meaningful difference was the
  `min`/`max` NaN semantics (D2), and that one is per backend upstream.

### 9.4 Survey findings corrected

- **D2** is not one bug. The upstream backends themselves differ (cpp
  `fmin`/`fmax`, interpreter `std::min`/`max`, wasm native). The only
  internal inconsistency was the interpreter's fold.
- **D6** (rust never emits sub-module `globals`) is not a drift. Those
  `globals` hold only foreign prototypes, dependencies and foreign
  variables, and rust emits none of these for the main module either.
- **D10.** The single hyperbolic import name is the host contract. Upstream's
  compiler emits `_acoshf`, but its own JS wrappers, like
  `tests/impulse-tests/tools/impulsewasm.js`, provide only `_acosh`. The name
  stays, and only the silent shadowing is fixed.
- **D16** was a compile error, not a cosmetic difference: `Meta::declare`
  takes no zone.
- **D17** is live, and the survey under-rated it. Upstream builds a JSON
  `address` with `PathBuilder::buildPath`
  (`/my_group/my_gain__dB_`), while faust-rs kept the raw labels
  (`/my group/my gain (dB)`). Web hosts address parameters by that path.
  The corrected `shortname`s now match upstream's as well.
- **D7** was live too. Through `ma.SR`'s clamp, nearly every codebox program
  called `fmin`/`fmax`.
- **A third digit style.** cmajor and codebox write integral reals with all
  their exact digits. It is kept as `Digits::ExactIntegral` and pinned by
  `backend_spellings_of_special_values`.

### 9.5 Left as is, recorded

- **codebox NaN/infinity spellings**: there is no oracle.
- **`fbc_to_cpp`**: its literal spelling and raw integer `+ - *` are kept,
  since it is a debugging rendering compared only with itself.
- ~~**The rust backend has no metadata transport.**~~ Done in the follow-up
  (`550ea14b`, `e5bd488d`, §9.2): the rust and cmajor options carry the
  compilation metadata, and the c, cpp, rust and cmajor banners copy
  upstream's five metadata keys.
- **cpp alone synthesizes `openVerticalBox`/`closeBox`** for an absent
  `buildUserInterface`.
- **The interpreter's name heuristics** (the `input`/`output` prefix, the
  `fSamplingFreq` fallback) and its rejection of `std::`-prefixed math names
  are unchanged.
- **The UI tree walkers** (json, cmajor, codebox, rust, interp,
  cranelift-ffi) are not merged. The live part of D17 was the address, and
  that is fixed. The nested-`Block` policy differences are latent.
- **The local CLI transcripts** (`tests/cli-transcripts`, not a CI gate) were
  already stale before this branch (recorded 2026-10-10) and are not
  re-recorded.

### 9.6 Validation

- **Identity differential.** Every commit ran the codegen-identity
  differential: the 632 `.dsp` files under `tests/` and `examples/`, in 23
  backend configurations (c, cpp and rust in several modes, julia, cmajor,
  asc, codebox, interp, cranelift, wast, wasm by binary hash, json), against
  the base binary. The cranelift JIT addresses were normalized after a
  self-check showed them to be the only nondeterminism. Moves showed 0
  differing outputs. The drift fixes that change output (D13, D7, D18, D17)
  were reviewed diff by diff and then re-baselined.
- **Literal equivalence.** The R2a move was also checked outside the corpus:
  the old and new real formatters agree on 600,035 values (0 mismatches).
- **Regression tests.** Each drift fix carries a test that fails before it.
- **Final tree.**
  - `cargo test --workspace --all-targets --no-fail-fast`: 0 failures.
  - clippy `-D warnings` and fmt: clean.
  - The five structure gates: OK.
  - `golden-check`: 292 OK.
  - Impulse harness after `make clean`: all nine lanes pass (cmajor at 87
    comparisons, the others at 93).
  - Compile-budget A/B against the base binary: no case moved.
