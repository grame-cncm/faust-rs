# Plan for porting the Faust compiler to Rust

**Project**: `faust-rs` — Porting the Faust compiler (C++) to Rust  
**C++ reference**: <https://github.com/grame-cncm/faust/tree/master-dev/compiler>  
**Plan start date**: February 2026  
**Authors**: GRAME Team

**Audit baseline (re-check, February 2026):**
- Branch analyzed: `master-dev-ocpp-od-fir-2-FIR19` (`8eebea429`)
- Clarification: the branch name contains `ocpp`, but old C++ mode `-lang ocpp` is out of scope for the Rust port target.
- Measured size: `159,012` LOC for `*.cpp + *.hh`, and `162,315` LOC for `*.cpp/*.cc/*.cxx/*.h/*.hh/*.hpp/*.l/*.y`
- Current production compile path is still driven by `libcode.cpp` + `InstructionsCompiler`/`DAGInstructionsCompiler`
- `signalFIRCompiler` exists but is not the default end-to-end path today
- `box_signal_api.cpp` exposes a very large API surface (`453` `LIBFAUST_API` declarations)

---

## 1. Overview

The Faust compiler is currently written in C++ and structured in around twenty subfolders in `compiler/`. The build pipeline follows a well-defined path:

```
Source Faust (.dsp)
  → Lexer/Parser (lex/yacc)
    → Block Diagrams (boxes)
      → Evaluation (environments, pattern matching, λ-calculus)
        → Symbolic propagation (boxes → signals)
          → Signal normalization
            → Type & interval annotation
              → Transformation (scheduling, vectorization)
                → FIR generation (Faust Intermediate Representation)
                  → Target backend (C, C++, Rust, LLVM, WASM, Interp, …)
```

The objective of the port is to reproduce this pipeline in idiomatic Rust, taking the opportunity to:

- **Eliminate the global state** (`gGlobal`) in favor of an architecture with independent compilation sessions, allowing native parallelization
- Simplify the most complex parts (signal translation → FIR)
- Unify the visit of signal trees with the rewriting rules methodology (work by Yann, summer 2025)
- Obtain a native cross-platform toolchain (Linux, macOS, Windows) via `cargo`
- Export naturally to WebAssembly (target `wasm32-unknown-unknown`)
- Offer intermediate APIs (boxes, signals) that can be properly exported in C, C++ and WASM
- **Allow parallel compilation** of multiple `.dsp` files and simultaneous generation to multiple backends
- Prefer real end-to-end integrations over temporary stubs; if a stub is unavoidable, it must be explicitly time-boxed, tracked, and removed within the same phase gate.
- Require explicit deliverables and pass criteria for each phase/prototype before implementation starts.
- Document migrated source provenance continuously in Rustdoc format (`///`/`//!`) with C++ source references and parity-relevant invariants.
- For tree-encoded IR layers, standardize on builder + canonical matcher APIs:
  - `boxes`: `BoxBuilder` + `match_box`
  - `signals`: `SigBuilder` + `match_sig`
  - `fir`: `FirBuilder` + `match_fir`
  This avoids duplicated constructor/matcher ladders across passes and keeps dispatch logic explicit.
- Standardize diagnostics and error reporting on one structured cross-phase model (see `faust-rust-diagnostics-model-en.md`).

Related design note (recursion representation and RouteIR coexistence):
- `faust-rust-recursion-model-note-en.md`
Related implementation addendum (recursive trees and `deBruijn2Sym` porting):
- `phases/phase-5-recursive-trees-debruijn2sym-en.md`
Related design note (structured diagnostics and error-reporting model):
- `faust-rust-diagnostics-model-en.md`
Related design note (canonical FIR architecture for backends):
- `faust-rust-fir-architecture-en.md`

### Public API migration policy (1:1 vs adaptations)

- **Not all APIs are ported 1:1 at signature level.**
  - Internal Rust crate APIs are allowed to adapt signatures for idiomatic Rust (ownership/borrowing, `Result`-based errors, explicit context parameters, typed enums).
  - External compatibility surfaces (CLI behavior and C/C++ exported APIs) remain parity targets by tier.
- **Behavioral parity is the default objective.**
  - Any signature adaptation must preserve documented semantics or explicitly declare intentional divergence.
- **Mapping traceability is mandatory for touched public APIs.**
  - For each migrated API, record:
    - C++ source symbol(s) and file path(s),
    - Rust symbol(s),
    - status: `1:1`, `adapted`, or `deferred`,
    - rationale and compatibility impact,
    - differential/unit tests covering the mapping.

### Representation integrity policy (mandatory for C++ -> Rust adaptations)

- **Default rule: keep semantically coupled data co-localized.**
  - If C++ ties payload/state to a node/instruction object, Rust should keep
    the same semantic ownership model unless there is a strong documented reason
    not to.
  - Avoid index-keyed side tables for semantically owned payloads in
    optimization/rewrite-heavy paths.
- **If a non-1:1 representation is chosen (`adapted`):**
  - document invariants and expected ownership boundaries,
  - document concrete failure modes introduced by the adaptation,
  - add a dedicated structural non-regression test that would fail on payload
    detachment/misassociation.
- **Optimization parity gate for runtime/backends:**
  - for optimization-sensitive execution paths, run targeted parity checks
    between unoptimized and optimized behavior (`opt_level=0` vs `opt_level=max`)
    on representative corpus cases (including table/state-heavy DSPs).

### Mandatory quality gate for each porting step

- Every implementation step must pass this local validation gate before commit:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --all-targets`
- If one command cannot be run in the current step, the reason and follow-up owner/date must be recorded in `JOURNAL.md`.

### CLI argument parsing policy

- Use `clap` as the default CLI argument parser for user-facing binaries (`compiler` CLI and related tools).
- Alternative parsers are acceptable only when justified with a concrete constraint (for example binary size or startup profile) and documented in `JOURNAL.md` and the relevant phase document.

---

## 2. Current C++ source code mapping

The structure below comes from the project's `CMakeLists.txt` (`build/CMakeLists.txt`) which references the include directories:

| C++ folder | Role | Proposed Crate Rust |
|---|---|---|
| `compiler/tlib/` | Tree library, hash-consing, symbols, lists, properties, nodes | `tlib` |
| `compiler/boxes/` | Constructors and destructors of block diagrams (boxes), type checking of boxes | `boxes` |
| `compiler/parser/` | Lexer (`.l`) + Yacc parser (`.y`) → box AST | `parser` |
| `compiler/evaluate/` | Evaluation of block diagrams: environments, pattern matching, λ-calculation, `eval.cpp` | `eval` |
| `compiler/patternmatcher/` | Pattern matching on boxes (case/rules) | `eval` (integrated) |
| `compiler/propagate/` | Symbolic propagation: boxes → signals | `propagate` |
| `compiler/signals/` | Signal constructors/destructors, typical signal (nature, variability, computability, vectorizability, Boolean, intervals) | `signals` |
| `compiler/normalize/` | Normalization and algebraic simplification of signal trees | `normalize` |
| `compiler/extended/` | Extended math functions (`xtended`): sin, cos, sqrt, etc. | `signals` (integrated) |
| `compiler/interval/` | Interval arithmetic for signal boundaries | `interval` |
| `compiler/FaustAlgebra/` | Abstract algebra on signals (ring, simplifications) | `algebra` |
| `compiler/DirectedGraph/` | Directed graphs for dependency analysis | `graph` |
| `compiler/transform/` | Transformations: scheduling, vectorization, parallelization, loop optimization | `transform` |
| `compiler/parallelize/` | Parallel code generation (OpenMP, work-stealing) | `transform` (integrated) |
| `compiler/generator/` | Common code generation infrastructure, code containers | `codegen` |
| `compiler/generator/fir/` | Faust Intermediate Representation: instructions, types, visitors | `fir` |
| `compiler/generator/c/` | Backend C | `codegen::backends::c` |
| `compiler/generator/cpp/` | C++ Backend | `codegen::backends::cpp` |
| `compiler/generator/rust/` | Rust backend (Rust code generation) | `codegen::backends::rust` |
| `compiler/generator/interpreter/` | Backend interpreter | `codegen::backends::interp` |
| `compiler/generator/llvm/` | Backend LLVM IR | `codegen::backends::llvm` |
| `compiler/generator/wasm/` | WebAssembly Backend (WAST/WASM) | `codegen::backends::wasm` |
| `compiler/generator/cmajor/` | Cmajor Backend | `codegen::backends::cmajor` |
| `compiler/generator/codebox/` | Backend Codebox (Max/RNBO) | `codegen::backends::codebox` |
| `compiler/generator/csharp/` | C# backend | `codegen::backends::csharp` |
| `compiler/generator/dlang/` | Backend D | `codegen::backends::dlang` |
| `compiler/generator/julia/` | Backend Julia | `codegen::backends::julia` |
| `compiler/generator/java/` | Java Backend (**no longer needed / out of scope**) | `N/A (excluded)` |
| `compiler/generator/jsfx/` | JSFX Backend | `codegen::backends::jsfx` |
| `compiler/generator/jax/` | JAX Backend | `codegen::backends::jax` |
| `compiler/generator/vhdl/` | VHDL Backend | `codegen::backends::vhdl` |
| `compiler/generator/sdf3/` | SDF3 backend | `codegen::backends::sdf3` |
| `compiler/draw/` | Generating SVG diagrams (block diagrams) | `draw` |
| `compiler/draw/schema/` | Visual diagrams of block diagrams | `draw` (integrated) |
| `compiler/draw/device/` | Abstract drawing devices (SVG, PS) | `draw` (integrated) |
| `compiler/documentator/` | Automatic mathematical documentation generation | `doc` |
| `compiler/errors/` | Error management, warnings | `diagnostics` |
| `compiler/utils/` | Miscellaneous utilities (names, files, global state) | `utils` |
| `compiler/*.cpp` (root) | Entry point (`main.cpp`), `global.cpp`, `libfaust.cpp` | `compiler` (binary + lib) |

---

## 3. Cross dependencies and special cases

### 3.1 Widget Modulation (scattered code)

In the current C++ version, the modulation widget code (MIDI → UI settings) is scattered between several folders:

- `compiler/generator/`: management of `UIInstruction` and widget declarations in code containers
- `compiler/propagate/`: extraction of widget metadata during propagation boxes → signals
- `compiler/evaluate/`: evaluation of label metadata and widget paths (e.g. `"chan 3/gain"`)

**Rust Recommendation**: Centralize all modulation widget logic in a dedicated module of `codegen` or in a submodule of `propagate`, with clear lines (`WidgetDescriptor`, `MidiMapping`, `WidgetPath`).

### 3.2 Pattern Matching and Evaluation

The `patternmatcher/` is historically separate but intimately linked to `evaluate/`. In Rust, it is recommended to unify them in `eval` as two submodules:

```
eval/
  src/
    eval.rs          // Block-diagram evaluation
    environment.rs   // Environments and scoping
    pattern.rs       // Pattern matching (case/rules)
    lambda.rs        // Abstractions and applications
```

### 3.3 Extended and Signals

The extended functions (`xtended`) in `compiler/extended/` are actually specialized signal nodes. They must live in `signals`:

```
signals/
  src/
    signals.rs       // Signal constructors/destructors
    sigtype.rs       // Signal types (nature, variability, etc.)
    sigprint.rs      // Signal printing
    xtended/         // Mathematical functions (sin, cos, sqrt, ...)
      mod.rs
      trig.rs
      math.rs
      ...
```

### 3.4 Transform and Parallelize

`parallelize/` is a specialization of `transform/`, the two will be merged into `transform`:

```
transform/
  src/
    scheduling.rs    // Signal scheduling into loops
    vectorize.rs     // Vectorized mode (-vec)
    parallelize.rs   // OpenMP, work-stealing (-omp, -sch)
    loop_opt.rs      // Loop optimization
```

### 3.5 Diagnostics model (cross-phase)

The Rust port should not replicate the C++ string/exception diagnostic style one-to-one. Behavioral parity remains mandatory, but diagnostics should be structured and phase-aware:

- Stable diagnostic codes (parser/eval/propagate/compiler families),
- explicit severity/stage/source labels,
- human and JSON renderers for CLI/CI/IDE.

Detailed architecture and rollout are specified in:
- `faust-rust-diagnostics-model-en.md`

Near-term diagnostics UX priorities after baseline rollout:
- enrich eval/propagate failures with explicit node context and expression preview,
- add rule-aware actionable explanations for arity/composition mismatches,
- propagate source spans into Phase 4 diagnostics when available,
- lock human+JSON renderer behavior with snapshot tests.
- add operator-level source pointing (column-precise spans on composition operators),
- include alias-resolution context in diagnostics (`process -> ... -> owner`),
- expose paired left/right mismatch context with computed arities,
- expand complex negative corpus and operator-specific correction hints.
- add C++-style readable `Here A / while B` blocks in human output for composition errors,
- pretty-print UI/primitive expressions in user-facing notes (no internal tag/list forms),
- add explicit owner-definition notes plus computed numeric target suggestions,
- lock these readability rules with dedicated human/json snapshots.
- close eval readability gap by enriching node-carrying `EvalError` variants and
  extending eval-focused human/json snapshots and actionable hints.
- continue with eval diagnostics v2:
  - multi-label call-site/definition-site coverage,
  - explicit scope-resolution context for unresolved symbols,
  - deterministic correction templates,
  - wording normalization across eval/propagate,
  - nested eval negative corpus,
  - optional IDE-oriented structured JSON fields.
- finalize diagnostics quality gate (parser/eval/propagate/compiler):
  - origin-site precision for alias-chain failures (with explicit fallback note if unavailable),
  - root-cause-first primary messages (single cause, consequences in notes),
  - deterministic fix template for each frequent error family,
  - stable wording/order contract across stages (`rule -> computed -> context -> help`),
  - expanded realistic nested negative corpus with human/json snapshot locks.
- diagnostics polish tranche (non-blocking after quality gate):
  - add explicit `cause:` line on top frequent errors,
  - expand compound negative fixtures (stacked eval+propagate contexts),
  - lock fallback-path behavior when origin span is unavailable,
  - complete residual wording normalization across variants.
  - complete `cause:` coverage on secondary variants,
  - lock JSON note ordering (`cause -> rule -> computed -> context`) in snapshots,
  - add one pipeline-level corpus fixture exercising fallback-origin messaging,
  - evaluate standard human-mode noise reduction for low-level internal notes.
  - introduce diagnostics verbosity modes (`standard`/`debug`) for controlled
    visibility of low-level internals,
  - extend JSON ordering assertions to broader error families (`merge`, `rec`,
    additional eval classes),
  - lock a dedicated human snapshot for pipeline-level fallback-origin messaging,
  - perform final concision sweep on help/template strings,
  - add short end-user guidance for reading structured diagnostics.
  - status: first implementation slice completed (verbosity modes, JSON debug
    enrichment in debug mode, and initial help/template concision pass).

---

## 4. Cargo Workspace Architecture

```
faust-rs/                          # Cargo workspace root
├── Cargo.toml                     # [workspace] members = [...]
├── crates/
│   ├── tlib/                # Foundation: trees, hash-consing, symbols
│   ├── errors/              # Error types, diagnostics, source locations
│   ├── interval/            # Interval arithmetic
│   ├── algebra/             # Abstract algebra (ring over signals)
│   ├── graph/               # Directed graphs, dependency analysis
│   ├── boxes/               # Block diagrams (depends on tlib)
│   ├── parser/              # Lexer + Parser (depends on boxes, tlib)
│   ├── signals/             # Signals + extended (depends on tlib, interval)
│   ├── eval/                # Evaluation + pattern matching (depends on boxes, signals)
│   ├── propagate/           # Propagation boxes → signals (depends on eval, signals, boxes)
│   ├── normalize/           # Signal normalization (depends on signals, algebra)
│   ├── transform/           # Scheduling, vectorization (depends on signals, graph)
│   ├── fir/                 # FIR: instructions, types, visitors
│   ├── codegen/             # Generation infrastructure + backends
│   │   └── src/backends/    # c/, cpp/, rust/, wasm/, interp/, llvm/, ...
│   ├── draw/                # SVG diagram generation
│   ├── doc/                 # Mathematical documenter
│   ├── utils/               # Shared utilities
│   └── compiler/            # Binary entry point + libfaust
├── cffi/                    # API C/C++ (via cbindgen/cxx)
└── tests/                         # End-to-end integration tests
```

---

## 5. Dependency graph between crates

```
tlib ──────────────────────────────────────────────-┐
    │                                                     │
    ├──→ boxes                                      │
    │       │                                             │
    │       ├──→ parser (+ lrlex/lrpar)             │
    │       │                                             │
    ├──→ signals ←── interval                 │
    │       │              algebra                  │
    │       │                                             │
    │       ├──→ eval ←── boxes               │
    │       │                                             │
    │       ├──→ propagate ←── eval           │
    │       │                                             │
    │       ├──→ normalize ←── algebra        │
    │       │                                             │
    │       └──→ transform ←── graph          │
    │                   │                                 │
    │                   ▼                                 │
    │            fir                                │
    │                   │                                 │
    │                   ▼                                 │
    │            codegen                            │
    │               │  │  │  │                            │
    │               ▼  ▼  ▼  ▼                            │
    │      codegen::backends::{c, cpp, wasm, ...}        │
    │                                                     │
    └──→ errors, utils (cross-cutting) ────────┘

                        │
                        ▼
                 compiler (binary + lib crate)
                        │
                        ▼
                    cffi (API C/C++)
```

---

## 6. Recommended carrying order

The portage must follow the outbuildings, starting from the foundations to the upper layers. Each phase produces a testable crate in isolation.

### Phase 0 — Validation sprint (weeks 1-2)

Before implementation, run a focused validation sprint on the current C++ branch:

- Parser migration prototype (`faustparser.y`/`faustlexer.l` to lrpar/lrlex)
- `TreeArena` hash-consing performance prototype (Rust vs C++ baseline)
- `gGlobal` decomposition map (especially `global.hh`, `libcode.cpp`, `instructions_compiler.cpp`)
- Differential harness (C++ vs Rust outputs on a representative DSP corpus)
- API surface inventory of `box_signal_api.cpp` exports (full prioritization deferred to Phase 1 scope planning)
- Stub-minimization policy for prototypes (real crate APIs first, no parser-local placeholder layers unless explicitly justified and short-lived)

This phase is mandatory to avoid locking into incorrect assumptions early (especially around pipeline choice and API scope).
Each validation/prototype task in this phase must have explicit deliverables and pass criteria before execution.

Detailed checklist and Go/No-Go criteria: `phases/phase-0-validation-en.md`.
Compiler Memoization Strategy comparison: `phases/phase-0-memoization-strategy-en.md`

### Phase 1 — Foundations (months 2-3)

| Stage | Crate | Description | Validation tests |
|-------|-------|-------------|---------------------|
| 1.1 | `diagnostics` | Diagnostic reports, source locations/ranges, stable codes, phase-aware bundles, rendering contracts | Unit testing |
| 1.2 | `utils` | Common utilities (names, paths, global config) | Unit testing |
| 1.3 | **`tlib`** | **High priority.** Trees with hash-consing, symbols, functional lists (cons/hd/tl), tagged nodes, properties on the nodes. This is the fundamental data structure of the entire compiler. | Comprehensive testing of hash-consing, structural equality, list operations |
| 1.4 | `interval` | Interval arithmetic (bounds, propagation) | Testing arithmetic operations on intervals |

**Note on `tlib`**: In C++, `tlib` uses a `Tree` which is a shared pointer to a hash-conseed node. In Rust, the approach adopted is the **`TreeArena`** described in section 8.3.4: an arena with identifiers `TreeId` (32-bit indices, `Copy`, comparison in O(1)) and an `HashMap` for interning. Each `CompileSession` has its own arena (see section 8.3.5). Structural identity (two identical trees = same `TreeId`) is essential for compiler performance. For future parallelism, the `TreeRead` / `TreeIntern` traits (section 9.5) allow switching to a competing arena without modifying the consumer crates.

### Phase 2 — Block Diagrams (months 3-4)

| Stage | Crate | Description | Validation tests |
|-------|-------|-------------|---------------------|
| 2.1 | **`boxes`** | Block diagram constructors/destructors: `boxPar`, `boxSeq`, `boxSplit`, `boxMerge`, `boxRec`, `boxRoute`, UI widgets (`boxHSlider`, `boxVSlider`, etc.), `boxWaveform`, iterators (`boxIPar`, `boxISeq`, `boxISum`, `boxIProd`), foreign functions. Type checking of boxes (calculation of inputs/outputs). | Build testing and type checking |
| 2.2 | `algebra` | Abstract algebra (simplification of expressions on signals/boxes) | Testing algebraic properties |
| 2.3 | `graph` | Directed graphs for dependency analysis | Path tests, topological sorting |

### Phase 3 — Parser (months 4-5)

| Stage | Crate | Description | Validation tests |
|-------|-------|-------------|---------------------|
| 3.1 | **`parser`** | Porting the parser with **`lrlex` + `lrpar`** (grmtools). The existing `faustparser.y` file can be adapted quite directly to the Grmtools format. The `faustlexer.l` lexer is converted to lrlex format. The semantic actions of `.y` directly construct the boxes via `boxes`. | Test suite: analysis of all Faust examples in the repository |

**Strategy for parsing it**:

1. Adapt the `faustparser.y` file to Grmtools format (the grammar rules remain almost identical)
2. Convert `faustlexer.l` to lrlex compatible `.l`
3. Semantic actions directly call `boxes` constructors
4. The advantage of lrlex/lrpar is built-in error recovery, production Rust types, and static grammar compilation via `build.rs`

After Gate B prototype validation, completion work must follow an explicit remaining-step roadmap (lexer/grammar/action parity, diagnostics/recovery parity, SourceReader integration, differential expansion, and merge into production `crates/parser`).
Detailed sequence with deliverables and pass criteria: `phases/phase-3-parser-en.md` (section `7.4`).
Diagnostics model details for this phase: `faust-rust-diagnostics-model-en.md` (sections 4–6).

### Phase 4 — Signals, Evaluation, Propagation (months 5-6)

| Stage | Crate | Description | Validation tests |
|-------|-------|-------------|---------------------|
| 4.1 | **`signals`** | Signal constructors/destructors (`sigInt`, `sigReal`, `sigInput`, `sigOutput`, `sigDelay`, `sigPrefix`, `sigSelect2/3`, `sigTable`, `sigWaveform`, etc.), signal type system (nature, variability, computability, vectorizability, boolean, intervals). Includes extended functions (`xtended`). | Construction tests, pattern matching, types |
| 4.2 | **`eval`** | Block diagram evaluator: environments, scoping, pattern matching (`case`/`rules`), lambda abstractions, iterator evaluation (`par`, `seq`, `sum`, `prod`), `with`/`letrec`/`environment`. Integrates code from `evaluate/` and `patternmatcher/`. | Tests: evaluation of complete Faust programs |
| 4.3 | **`propagate`** | Symbolic propagation: conversion of evaluated block diagrams into signal trees. This is where boxes become signals. | Tests: product signal verification vs. C++ reference |

**Phase 4 signals restructuring checklist (from `signals/` audit):**

1. Replace duplicated `isSig*` ladders with one canonical typed signal-node dispatch layer shared by typing, ordering, sub-signal transforms, and printers.
2. Replace `gGlobal->nil`-sentinel encodings (rdtable/rwtable shape, OD/US/DS branch conventions, clock-env list layout) with explicit enum/struct variants.
3. Move signal annotations (`type`, `order`, `recursiveness`, `sharing`, `clkEnv`) from global Tree properties to session-scoped analysis stores.
4. Split `sigtyperules.cpp` into focused passes (core inference, recursive fixpoint, FIR/IIR gain analysis, UI/soundfile checks, diagnostics).
5. Move typing controls (`causality`, narrowing/widening limits, diagnostics) into explicit type-inference config/context objects.
6. Replace `sigtype` inheritance + `dynamic_cast` + global memoized type table with an immutable Rust enum model and an interned `TypeId` store.
7. Consolidate printers (`ppsig`, `ppsigShared`, `sigprint`) into one renderer with formatting modes instead of duplicated signal-case chains.
8. Keep debug/test helpers out of production paths (`testFIR`, ad hoc stderr tracing in core signal helpers).
9. Put explicit complexity bounds/caching around FIR/IIR gain estimation used during type inference.
10. Preserve behavior first with differential tests, then apply structural refactors module by module.
11. Adopt the shared diagnostics model for `eval`/`propagate` error conversion and source labeling (`faust-rust-diagnostics-model-en.md`).

### Phase 5 — Standardization and Transformation (months 6-8)

| Stage | Crate | Description | Validation tests |
|-------|-------|-------------|---------------------|
| 5.1 | **`normalize`** | Algebraic normalization of signals. Simplifications, normal formatting. | Tests: standardized programs vs reference (-norm) |
| 5.2 | **`transform`** | Scheduling of calculations, analysis of dependencies (upstream/downstream), detection of recursive loops, vectorization (-vec), parallelization (-omp, -sch). | Testing programs with different options |

**Critical Point — Visiting Signal Trees**: Historically, visiting signal trees has been implemented in different ways in the C++ compiler (visitors, direct recursion, etc.). The most recent methodology is that of **rewriting rules** implemented by Yann in the summer of 2025. The port to Rust is the ideal opportunity to **use this new methodology uniformly** in all passes that visit or transform the signal trees (normalization, type annotation, transformation, code generation). In Rust, this naturally translates into a system of pattern matching + functional rules, potentially with traits like `Rewriter`:

```rust
trait SignalRewriter {
    fn rewrite(&self, sig: &Signal) -> Option<Signal>;
}

fn apply_rules(sig: &Signal, rules: &[Box<dyn SignalRewriter>]) -> Signal {
    // Bottom-up application of rules until a fixed point
}
```

**Phase 5 normalize restructuring checklist (from `normalize/` audit):**

1. Replace legacy property-mutating recursive walkers (`sigMap`/`sigMapRename`) with session-scoped pass contexts and explicit caches.
2. Replace hard-coded normalization sequencing with a declarative pass pipeline (`NormalizationPipeline`) and explicit pass ordering/invalidation.
3. Reduce repeated full `typeAnnotation` recomputation by batching typing phases and tracking which passes invalidate types.
4. Move policy flags (`range-ui`, `freeze-ui`, `ftz`, auto-diff, table/int-range checks) into explicit normalization options structs.
5. Replace pointer/serial-based canonicalization heuristics with deterministic structural order keys.
6. Redesign algebra normal forms around typed keys (coeff + sorted factors) instead of tree-signature recomputation at merge points.
7. Improve additive factorization scalability (current pairwise GCD scan) with grouped/bucketed candidate selection.
8. Remove global cache side effects (`SIMPLIFIED`, `NORMALFORM`) in favor of per-session memoization stores.
9. Isolate pretty-print state from global resets (`gGlobal->clear`) with dedicated printer contexts.
10. Preserve behavior but harden edge-case arithmetic checks in normalize core (notably divisor/zero handling paths in mterm operations).

**Phase 5 transform restructuring checklist (from `transform/` audit):**

1. Replace duplicated large `isSig*` dispatch chains (`SignalVisitor`, `SignalIdentity`, `SignalRenderer`, `SignalFIRCompiler`) with a shared typed signal-node traversal kernel.
2. Split planning from execution: one shared resource-planning pass (delays/tables/UI) reused by interpreter-like and FIR-emission paths.
3. Break the `sigPromotion` monolith into composable ordered passes (cast insertion, table safety, UI policies, FTZ, autodiff, diagnostics).
4. Move transform options and warnings out of `gGlobal` into explicit pass/session context objects.
5. Consolidate `TreeTransform`, `sigTransform`, and `RewriteRule/Normalize` into one canonical transform engine.
6. Replace recursion sentinels encoded as temporary AST mutation (`rec(..., nil)`) with explicit recursion-state tracking.
7. Unify recursive/dependency analyses under one graph service with mode-specific queries.
8. Split oversized transform headers into focused modules (node semantics, resource planner, runtime/emitter backends, API wrappers).
9. Standardize diagnostics (`Result` + warning collector) and remove mixed assert/stderr/global-warning patterns from pass logic.
10. Keep `signalRenderer` and `signalFIRCompiler` as experimental/test tracks, but treat them as non-blocking for Rust MVP parity.

**Phase 5 parallelize restructuring checklist (from `parallelize/` audit):**

1. Converge duplicated loop models (`Loop` and `CodeLoop`) into one typed loop-graph IR reused by scheduler and FIR/codegen.
2. Replace duplicated topological sorting implementations (`graphSorting.cpp` and `CodeLoop::sortGraph`) with one shared scheduler service.
3. Replace pointer-based dependency sets (`std::set<Loop*>` / `std::set<CodeLoop*>`) with stable loop IDs and deterministic ordering.
4. Move scheduling bookkeeping (`fOrder`, `fUseCount`) out of mutable loop nodes into explicit analysis outputs/maps.
5. Replace `dynamic_cast`-driven block-stack handling (`IF`/`OD`/`US`/`DS`) with enum-based scoped blocks and compile-time-checked transitions.
6. Route parallelization options and loop generation parameters through explicit context/config objects instead of direct `gGlobal` reads.
7. Replace textual OpenMP pragmas injected as labels/comments with structured parallel semantics in IR (annotation/effect nodes).
8. Consolidate repeated sequence-grouping logic (`computeUseCount`/`groupSeqLoops`) currently duplicated across `parallelize/` and generator paths.
9. Harden arithmetic guard generation in downsampling paths (avoid non-short-circuit conditions and modulo-by-zero risk).
10. Replace floating `pow`-based loop-stride scaling with integer-safe scaling helpers and explicit overflow-checked arithmetic.

**Phase 5 Dependencies restructuring checklist (from `Dependencies/` audit):**

1. Replace implicit graph identity keyed by root `Tree` (`siggraph[signalList]`) with explicit graph IDs/role tags (`main`, `controls`, `subgraph(id)`).
2. Replace recursive dependency builders (`addDependencies`/`simpleAddDependencies`) with one reusable traversal engine parameterized by policy.
3. Split dependency classification into explicit edge kinds (`Immediate`, `Delayed`, `External`, `Control`) instead of ad hoc vector/set channels.
4. Replace sentinel-based OD/US/DS branch interpretation (`nil` separators) with explicit typed node payloads.
5. Move clock-environment lookup/classification from throw-based global access to typed per-session analysis context/results.
6. Replace mutable cross-phase global/property coupling (`getCertifiedSigType`, `ClkEnvInference::getClkEnv`) with explicit analysis prerequisites in API signatures.
7. Consolidate schedule numbering/printing/DOT generation into one formatter backend with modes to avoid duplicated traversal logic.
8. Treat graph auditing as a real validation pass (currently effectively disabled) with structured diagnostics and test integration.
9. Keep debug/trace hooks outside core graph construction code paths and remove commented debug blocks from production flow.
10. Ensure deterministic schedule output (stable ordering over sets/maps) independent of pointer/map iteration artifacts.

**Phase 5 migration priority (low-risk):**

1. Port core transform infrastructure and high-value passes first (`RewriteRule`, `Normalize`, dependency graph, promotion/normalization pipeline).
2. Lock behavior with differential tests on normalized signals and scheduling before structural rewrites.
3. Port experimental `signalRenderer`/`signalFIRCompiler` after core parity is stable, on a separate non-blocking track.

### Phase 6 — FIR and Backends (months 7-10, partially in parallel)

| Stage | Crate | Description | Validation tests |
|-------|-------|-------------|---------------------|
| 6.1 | **`fir`** | Faust Intermediate Representation. Instructions (declarations, loops, conditions, operations), FIR types, transformers. Canonical API contract: `FirBuilder` for construction and `match_fir`/`FirMatch` for dispatch. Storage policy: use `tlib::TreeArena` for structural hash-consing and automatic sharing of identical FIR trees. It is the pivot between the world of signals and the world of code generation. | Tests: dump FIR vs reference (-lang fir) |
| 6.2 | **`codegen`** | Common infrastructure: code containers, management of declarations, variables, buffers, DSP structures. Translation of signals → FIR (the heart of the compilation). | Integration testing |
| 6.3 | **`codegen::backends::c`** | First backend: generation of C code. Used to validate the entire end-to-end chain. | Comparison of C output with the reference C++ compiler |
| 6.4 | **`codegen::backends::cpp`** | C++ backend (`-lang cpp`): very similar to the C backend, useful for C/C++ parity checks. | Idem |

**Audit correction (important)**: on the current branch, the end-to-end compile path for major backends is still centered on `InstructionsCompiler`/`DAGInstructionsCompiler` through `libcode.cpp`. The Rust MVP should therefore port this effective path first. `signalFIRCompiler` should be treated as a secondary/experimental path until explicitly promoted in upstream C++.

**Temporary testing lane (accepted)**: in parallel with the official path, a
`signalFIRCompiler`-style Rust lane can be used to accelerate end-to-end testing
from parser to C/C++ backends:

`parse -> eval -> propagate(signals) -> signal->FIR fast-lane -> backend c/cpp`

Rules for this lane:

1. Keep it explicit/opt-in (do not silently replace default production route).
2. Keep it differentially tested against C++ `signalFIRCompiler`.
3. Keep unsupported signal families explicit (typed diagnostics, no silent stubs).
4. Keep backend entrypoint canonical (`FirBuilder` module + `match_fir` consumers).

Detailed execution plan and pass criteria are tracked in:

- `porting/phases/phase-6-fir-backends-en.md` section
  **"Experimental End-to-End Fast-Lane: signalFIRCompiler-Style Rust Port"**.

**Critical point — Signals → FIR translation**: This is the most complex part of the current compiler. The code in C++ is dense and has accumulated a lot of historical complexity. Taking advantage of porting to **simplify this translation** is a major opportunity. Suggestions:

- Break down the translation into clearly identified passes (type analysis → signal classification → declaration generation → loop generation → peephole optimizations)
- Use Rust's type system to make certain FIR build errors impossible
- Document each pass with examples Faust → FIR expected

**Phase 6 restructuring checklist (from `instructions.hh/.cpp`, `instructions_type.hh`, `type_manager.hh`, `struct_manager.hh` audit):**

1. Replace inheritance + `dynamic_cast`-heavy instruction handling with enum-based FIR nodes and `match`-based passes.
2. Replace raw-pointer instruction ownership with arena IDs and contiguous collections (`Vec`/`SmallVec`).
3. Standardize FIR construction/inspection on one shared crate-level API:
   `FirBuilder` + `FirMatch` + `match_fir` (no backend-local ladders).
4. Split `IB` into separate concerns:
   - pure FIR node construction
   - canonicalization/constant folding
   - target-aware lowering
5. Route FIR type/memory decisions through explicit context objects (no hidden global-state coupling).
6. Redesign FIR type modeling to be compositional (`BaseType`, `Pointer`, `Vector`, `Array`, `Function`) instead of enum-variant proliferation.
7. Replace `TypeManager` class hierarchy with Rust traits + backend formatting tables.
8. Split DSP struct responsibilities into dedicated subsystems (layout, metadata/usage, emission).
9. Replace repeated field-name linear scans with indexed lookup tables for struct layout.
10. Carry value result types directly in Rust FIR value nodes to avoid backend-local type
    reconstruction passes (`typing_instructions.hh`-style).

**Phase 6 CodeContainer/codegen restructuring checklist (from `code_container.hh/.cpp` and related container machinery audit):**

1. Split the monolithic mutable `CodeContainer` state into explicit typed sections (declarations, init/static-init, UI, compute/control, metadata/memory).
2. Replace imperative option-driven `processFIR()` sequencing with an explicit pass pipeline and pass context objects.
3. Isolate zone rewriting (`iZone`/`fZone`) into dedicated pure passes rather than mutating many sections in place.
4. Replace side-effectful subcontainer merge/clear behavior with deterministic merge outputs that preserve source containers.
5. Cache flattened FIR views during checking/analysis phases instead of rebuilding repeatedly.
6. Replace pointer-based loop DAG plumbing with stable loop IDs and deterministic ordering.
7. Replace backend inheritance matrix (scalar/vector/OpenMP/WSS crossed with language backends) with composition/strategy traits.
8. Replace backend factory `if/else` chains with registry-driven backend/strategy selection.
9. Deduplicate repeated local input/output slice generation patterns shared by vector/OpenMP/WSS paths.
10. Represent OpenMP/work-stealing semantics as structured IR annotations/effects, not textual labels embedded in instruction streams.
11. Move memory-layout and access accounting into dedicated analysis modules decoupled from textual emission.
12. Remove remaining global-state coupling in code-container logic by routing options and memory planners through explicit context objects.

**Phase 6 `libcode.cpp` orchestration restructuring checklist (from `libcode.cpp` audit):**

1. Replace mutable global orchestration state (`gGlobal` and static globals in `libcode.cpp`) with explicit `CompileSession`/`CompileRequest` objects.
2. Replace backend-specific `compileX` wrappers with a registry of backend descriptors plus a shared compile template.
3. Replace backend dispatch `if/else` chains with table-driven lookup and explicit backend profiles (language, scheduling, memory options).
4. Separate architecture/enrobage injection from core signal-to-code compilation into dedicated pipeline stages.
5. Replace `dynamic_cast<ostringstream*>` output handling with typed sink abstractions (`OutputSink`) and explicit capabilities.
6. Unify API entry points (`expandDSP`, `DSPToBoxes`, factory creation) around a shared compile lifecycle.
7. Remove orchestration-level `.cpp` includes and keep compilation/link boundaries explicit.
8. Use scope-based timing/instrumentation guards so early returns cannot skip teardown or measurements.
9. Replace fixed-size temporary `argv` staging in API paths with dynamic validated argument vectors (no hard-coded limits).
10. Replace hard-coded CLI/backend option checks with a declarative backend capability matrix validated by tests.
11. Guarantee per-request orchestration state reset (no stale compiler/container pointers surviving early-return backends).
12. Normalize C/C++ API context ownership to one explicit lifecycle model (no mixed implicit/explicit context contracts).
13. Normalize output writer mode/capability handling (text vs binary) through typed sink traits instead of ad hoc stream paths.
14. Isolate legacy/excluded backend residue (`ocpp`, template scaffolding) behind explicit non-target modules outside the core path.
15. Replace thread trampoline stack workarounds (`callFun`/custom stack size) with explicit recursion-depth controls and iterative passes where feasible.

**Execution order for low-risk migration:**

1. Keep branch parity by porting the currently effective pipeline first (`InstructionsCompiler`/`DAGInstructionsCompiler` path).
2. Add differential and golden tests before structural rewrites.
3. Apply architecture changes incrementally:
   - FIR data model and ownership
   - type system + type manager layer
   - CodeContainer section model + pass manager
   - struct layout/memory subsystem
   - backend lowering and strategy cleanup
   - compile orchestration/session model and backend registry (`libcode.cpp` replacement)

### Phase 7 — Additional backends (months 10-14, parallelizable)

The additional backends are relatively independent of each other and only depend on `codegen` and `fir`. They can be worn in parallel.

**Priority 1** (essential):
- `codegen::backends::wasm`: WebAssembly
- `codegen::backends::interp`: Interpreter
- `codegen::backends::llvm`: LLVM IR (requires `llvm-sys` or `inkwell`)

**Priority 2** (important):
- `codegen::backends::rust`: Rust backend (Rust code generated by Faust)
- `codegen::backends::cmajor`
- `codegen::backends::codebox`

**Priority 3** (to be worn next):
- All other backends (Julia, C#, D, JSFX, JAX, VHDL, SDF3)

**Scope update**: `backend-java` is no longer needed and is removed from the Rust port target scope.
**Scope update**: old C++ backend `-lang ocpp` is no longer needed and is removed from the Rust port target scope.

### Phase 8 — Diagrams and Documentation (in parallel)

| Stage | Crate | Description |
|-------|-------|-------------|
| 8.1 | `draw` | Generating SVG block diagrams |
| 8.2 | `doc` | Automatic mathematical documenter |

### Phase 9 — Integration and API (months 12-15)

| Stage | Crate | Description |
|-------|-------|-------------|
| 9.1 | `compiler` | Entry point: binary `faust` + library `libfaust` |
| 9.2 | `cffi` | C and C++ API export via `cbindgen` / `cxx` |
| 9.3 | | Conditional compilation of backends (feature flags) |
| 9.4 | | Integration into the existing Faust repository |

---

## 7. Advantages of the Rust toolchain

### 7.1 Native cross-platform compilation

With `cargo`, compilation on Linux, macOS and Windows is direct, without depending on CMake:

```bash
cargo build --release                     # Native platform
cargo build --release --target x86_64-pc-windows-gnu  # Cross-compilation Windows
```

### 7.2 Library version with C and C++ API

Thanks to `cbindgen` (for a C API) and `cxx` (for an idiomatic C++ API), we can expose `libfaust`:

```rust
// cffi/src/lib.rs
#[no_mangle]
pub extern "C" fn createDSPFactoryFromString(
    name: *const c_char,
    code: *const c_char,
    // ...
) -> *mut FaustDspFactory { ... }
```

### 7.3 WebAssembly version

Rust compiles natively to WebAssembly:

```bash
cargo build --release --target wasm32-unknown-unknown
# or with wasm-pack for JavaScript integration
wasm-pack build --target web
```

This makes it possible to replace the current Emscripten compilation of `libfaust-wasm` with a direct compilation in Rust, simpler and often more efficient.

### 7.4 Export of intermediate APIs (Boxes and Signals)

The box and signal APIs, currently exported in C via `LIBFAUST_API`, will naturally be available in Rust, and can be exported in C, C++ and WebAssembly:

```rust
// boxes/src/lib.rs - public API
pub fn box_par(a: BoxExpr, b: BoxExpr) -> BoxExpr { ... }
pub fn box_seq(a: BoxExpr, b: BoxExpr) -> BoxExpr { ... }
pub fn box_hslider(label: &str, init: f64, min: f64, max: f64, step: f64) -> BoxExpr { ... }

// Automatically exportable via cbindgen as:
// BoxExpr* boxPar(BoxExpr* a, BoxExpr* b);
```

### 7.5 Conditional compilation of backends

As with the current CMake machinery, but more properly via Cargo's **feature flags**:

```toml
# compiler/Cargo.toml
[features]
default = ["backend-c", "backend-cpp", "backend-wasm"]
backend-c = ["codegen/backend-c"]
backend-cpp = ["codegen/backend-cpp"]
backend-wasm = ["codegen/backend-wasm"]
backend-llvm = ["codegen/backend-llvm"]
backend-interp = ["codegen/backend-interp"]
backend-all = ["backend-c", "backend-cpp", "backend-wasm", "backend-llvm", "backend-interp",
               "backend-rust", "backend-cmajor", ...]
```

```bash
cargo build --release --features backend-c,backend-cpp    # Only C and C++
cargo build --release --features backend-all              # All backends
```

This cleanly replaces the `backends/*.cmake` files from the current build system.

### 7.6 Integration into the Faust repository

The Rust port can be integrated directly into the existing repository:

```
faust/
├── compiler/         ← replaced by the Rust workspace
│   └── Cargo.toml    ← workspace root
├── Makefile          ← adapted to call `cargo build`
├── architecture/     ← unchanged
├── libraries/        ← unchanged
├── tools/            ← unchanged (faust2xxx scripts)
└── ...
```

The main `Makefile` will simply be adapted to call `cargo build --release` instead of CMake commands, preserving compatibility with existing `faust2xxx` scripts.

---

## 8. Simplification points when carrying

### 8.1 Translation of signals → FIR

This is the most complex point of the current implementation. When carrying, you must:

1. **Document the existing**: before porting, produce detailed documentation of the current data flow in the C++ compiler (what structures are created, in what order, with what transformations)
2. **Decompose into passes**: instead of a monolithic process, identify distinct passes (classification, allocation, scheduling, generation)
3. **Type strongly**: use `enum` Rust to make invalid states impossible
4. **Test differentially**: for each Faust program, compare the FIR produced by the C++ version and the Rust version

### 8.2 Unified visit to signal trees

Use the **rewriting rules** methodology everywhere (Yann, summer 2025). This means:

- A unique `SignalVisitor` / `SignalRewriter` trait
- Declarative rules (pattern → action)
- Configurable bottom-up or top-down application
- All passes (normalization, annotation, transformation, generation) use the same mechanism

In Rust, native pattern matching makes this approach very natural:

```rust
fn rewrite_signal(sig: &Signal) -> Signal {
    match sig {
        Signal::BinOp(Add, Signal::Int(0), b) => b.clone(),  // 0 + b → b
        Signal::BinOp(Mul, Signal::Int(1), b) => b.clone(),  // 1 * b → b
        Signal::BinOp(Mul, Signal::Int(0), _) => Signal::Int(0), // 0 * b → 0
        // ... other rules
        _ => sig.clone(),
    }
}
```

### 8.3 Elimination of global state — Architecture without `gGlobal`

#### 8.3.1 The `gGlobal` problem in C++

In the current C++ compiler, the singleton object `gGlobal` (type `global`) accumulates very heterogeneous responsibilities:

- **Configuration**: compilation options (`-vec`, `-lang`, `-double`, etc.)
- **Interning tables**: hash-consing of trees (`gHashTable`), symbol table
- **Mutable compilation state**: counters, tables of signals already visited, type cache, properties on nodes
- **Intermediate results**: signal lists, UI metadata, etc.

This singleton creates an **implicit coupling** between all modules: each compiler folder actually depends on the entire global state, which makes crates impossible to test in isolation and parallelization impossible.

#### 8.3.2 Guiding principle: decomposition into explicit layers

The strategy is to **decompose `gGlobal` into separate structures by level of responsibility**, each explicitly passed where it is needed. No mutable global state exists in the Rust version.

#### 8.3.3 Layer 1 — Configuration (immutable after initialization)

```rust
/// Compilation options, built once from command-line arguments,
/// then shared read-only everywhere.
/// Arc<CompilerConfig> = zero-cost cloning, thread-safe.
pub struct CompilerConfig {
    pub target_lang: TargetLanguage,
    pub float_precision: FloatPrecision,  // Single / Double / Quad
    pub vec_size: usize,
    pub vectorize: bool,
    pub openmp: bool,
    pub scheduler: bool,
    pub math_approx: bool,
    pub math_exceptions: bool,
    pub in_place: bool,
    pub timeout: Duration,
    pub import_dirs: Vec<PathBuf>,
    // ... all CLI options
}
```

- Created only once at startup
- Shared as `Arc<CompilerConfig>` — no cloning costs, thread-safe
- **Never modified** after construction → no `Mutex`, no breed condition

#### 8.3.4 Layer 2 — Interning Arena (TreeArena)

Hash-consing of `Tree` is the heaviest shared state. Rather than a mutable global, each compilation receives **its own arena**:

```rust
/// Lightweight identifier for an interned node (32 bits, Copy, Eq, Hash).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TreeId(u32);

/// Hash-consing arena. Guarantees: two structurally identical trees
/// map to the same TreeId (so O(1) equality check with ==).
pub struct TreeArena {
    /// Interning table: structural key → identifier
    intern: HashMap<NodeKey, TreeId>,
    /// Compact node storage
    nodes: Vec<Node>,
}

impl TreeArena {
    pub fn new() -> Self { ... }

    /// Interns a node. If a structurally identical node already exists,
    /// returns the same TreeId (hash-consing).
    pub fn intern(&mut self, node: Node) -> TreeId { ... }

    /// Read access to a node by identifier.
    pub fn get(&self, id: TreeId) -> &Node { ... }
}
```

**Key advantage**: each compilation has its own arena → no pollution between compilations, automatic cleanup when the session ends (Rust's `Drop` releases everything).

#### 8.3.5 Layer 3 — Build session (`CompileSession`)

This is the central object which **replaces `gGlobal`**, but it is passed explicitly and each compilation has its own:

```rust
/// Full state for one .dsp file compilation session.
/// Replaces gGlobal, but is passed explicitly and never global.
pub struct CompileSession {
    /// Configuration (immutable, shareable across sessions)
    pub config: Arc<CompilerConfig>,

    /// Tree hash-consing arena (private to this session)
    pub arena: TreeArena,

    /// Symbol table (names → TreeId)
    pub symbols: SymbolTable,

    /// Diagnostic collector (errors, warnings)
    pub diagnostics: DiagnosticCollector,

    /// Source map for error messages
    pub source_map: SourceMap,
}
```

#### 8.3.6 Layer 4 — Local states per compilation pass

Each compilation phase defines its **own local state** which does not leak to other passes. This avoids the accumulation of fields in a monolithic object:

```rust
/// Local state for the normalization pass
struct NormalizeCtx<'s> {
    session: &'s mut CompileSession,
    /// Cache: original signal → normalized signal (local to this pass)
    cache: HashMap<TreeId, TreeId>,
    /// Counter of applied rules (for debug/metrics)
    rules_applied: usize,
}

/// Local state for the signal type-checking pass
struct TypeAnnotCtx<'s> {
    session: &'s CompileSession,  // read-only here
    /// Inferred type for each signal
    types: HashMap<TreeId, SignalType>,
    /// Computed interval for each signal
    intervals: HashMap<TreeId, Interval>,
}

/// Local state for FIR generation
struct CodegenCtx<'s> {
    session: &'s CompileSession,
    declarations: Vec<FirDecl>,
    loop_stack: Vec<LoopContext>,
    var_counter: usize,
}
```

#### 8.3.7 Impact on crate function signatures

With this architecture, each crate receives **exactly what it needs** as parameters. No crate depends on an implicit "state of the world":

```rust
// normalize: depends only on what it actually uses
pub fn normalize(
    sig: TreeId,
    arena: &mut TreeArena,     // to create new trees
    config: &CompilerConfig,   // to check which options are enabled
) -> TreeId { ... }

// propagate: takes only the required inputs
pub fn propagate(
    box_expr: TreeId,
    inputs: &[TreeId],
    session: &mut CompileSession,
) -> Vec<TreeId> { ... }

// codegen::backends::c: depends only on FIR and config
pub fn generate_c(
    fir: &FirProgram,
    config: &CompilerConfig,
    output: &mut dyn Write,
) -> Result<(), CodegenError> { ... }
```

Each crate is **testable in isolation**: we create a test `TreeArena`, an `CompilerConfig::default()`, and we call the functions directly.

#### 8.3.8 C++ vs Rust comparison table

| Appearance | Current C++ (`gGlobal`) | Proposed Rust |
|---|---|---|
| Configuration | `gGlobal->fVecSize`, etc. | `Arc<CompilerConfig>` immutable |
| Hash consing | `gGlobal->gHashTable` (mutable global) | `TreeArena` per session |
| Symbols | `gGlobal->gSymbolTable` | `SymbolTable` in `CompileSession` |
| Pass cache | Properties on nodes via `setProperty/getProperty` | `HashMap` local on each pass |
| Compilation status | `gGlobal` monolithic (>100 fields) | `CompileSession` explicit + pass states |
| Testability | Difficult (global to initialize) | Natural (all in parameter) |
| Parallelism | **Impossible** (shared mutable global) | **Natural** (independent sessions) |

---

## 9. Compilation parallelization

The elimination of the global state opens the door to several levels of parallelism, which can be progressively exploited.

### 9.1 Level 1 — Independent parallel compilations (multi-file)

This is the most immediate and simplest gain. When we compile N files `.dsp` (for example in a multi-voice project, or via a compilation service), each file receives its own `CompileSession`:

```rust
use rayon::prelude::*;

let config = Arc::new(CompilerConfig::from_args(&cli_args));

let results: Vec<Result<CompiledDsp, CompileError>> = dsp_files
    .par_iter()   // parallelism via rayon
    .map(|file| {
        // Each file has its own session, its own arena,
        // and its own diagnostic collector → zero sharing
        let mut session = CompileSession::new(config.clone());
        compile_one(file, &mut session)
    })
    .collect();
```

**Prerequisites**: no mutable shared state between sessions → this is exactly what the architecture without `gGlobal` guarantees.

**Estimated gain**: quasi-linear with the number of cores for multi-file projects (e.g. compilation of the entire test suite, faustservice type compilation web service).

### 9.2 Level 2 — Multiple backends in parallel (same file)

For the same `.dsp` file, we often want to generate several target languages ​​(eg: C++ for native + WASM for the web). The FIR being immutable once generated, it can be shared:

```rust
// Phase 1: Faust compilation → FIR (sequential, only once)
let fir: Arc<FirProgram> = Arc::new(compile_to_fir(&mut session)?);

// Phase 2: parallel generation to multiple backends
let backends: Vec<Box<dyn Backend>> = vec![
    Box::new(CBackend::new()),
    Box::new(CppBackend::new()),
    Box::new(WasmBackend::new()),
];

let outputs: Vec<_> = backends
    .into_par_iter()
    .map(|backend| {
        let fir = fir.clone();        // Arc::clone = atomic refcount increment
        let config = config.clone();   // Arc::clone, no deep copy
        backend.generate(&fir, &config)
    })
    .collect();
```

**Prerequisites**: the `FirProgram` must be `Send + Sync` (no `Rc`, no `RefCell`). Use `Arc` for shared trees, or `Copy` structures by value.

### 9.3 Level 3 — Intra-pass parallelism (future)

Within a compilation pass, certain operations are potentially parallelizable:

- **Normalization**: independent subexpressions of a `par(A, B)` can be normalized in parallel
- **Type annotation**: same for the typing of independent branches
- **FIR generation**: independent loops (compute groups) can be generated in parallel

This level is more complex and should **not** be an initial priority. The architecture makes it possible thanks to the lack of mutable global, but the gains are modest because the bottleneck is rarely in intra-file parallelism.

**Recommended approach**: design data structures to be `Send + Sync` from the start (no `Rc`, prefer `Arc` if sharing necessary), but only implement intra-pass parallelism if profiling shows a real need.

### 9.4 Level 4 — TreeArena thread-safe (optional)

If intra-pass parallelism is necessary, the arena must become thread-safe:

```rust
/// Thread-safe version of TreeArena for intra-compilation parallelism
pub struct ConcurrentTreeArena {
    intern: DashMap<NodeKey, TreeId>,  // concurrent HashMap (dashmap crate)
    nodes: AppendOnlyVec<Node>,        // Vec append-only lock-free
}
```

**Compromise**: the `DashMap` has an overhead of around 10-20% vs a simple `HashMap`. This is why the non-competing version (`TreeArena`) remains the default, and the competing version is opt-in.

### 9.5 Line design for parallel compatibility

To make the crates "ready for parallelism" without forcing it, use abstract traits for access to the arena:

```rust
/// Trait for read access to the arena (shareable across threads)
pub trait TreeRead: Send + Sync {
    fn get(&self, id: TreeId) -> &Node;
}

/// Trait for write access (interning new nodes)
pub trait TreeIntern: TreeRead {
    fn intern(&self, node: Node) -> TreeId;
}

// TreeArena implements both (single-threaded usage with &mut self)
// ConcurrentTreeArena implements both (multi-threaded usage with &self)
```

Crate functions accept `impl TreeRead` or `impl TreeIntern` depending on their needs, making them compatible with both modes without modification.

### 9.6 Summary of the parallelization strategy

| Level | Description | Difficulty | Gain | When |
|--------|-------------|------------|------|-------|
| 1 | Multi-file in parallel | Trivial (thanks to architecture) | Linear with hearts | From the start |
| 2 | Multiple backends in parallel | Easy (`Arc<FirProgram>`) | Moderate (2-4x if multi-backend) | Stage 6+ |
| 3 | Intra-pass (normalization, etc.) | Complex | Modest | If profiling justifies it |
| 4 | Competing TreeArena | Moderated (`DashMap`) | Depends on workload | Optional |

**Golden rule**: design the structures to be `Send + Sync` from the start (no `Rc`, no `RefCell`, no mutable global), implement effective parallelism gradually.

---

## 10. Detailed porting procedure

For **each phase** of the porting, the following procedure must be followed:

### 10.1 Before carrying each crate

1. **Read and document the corresponding C++ source** code
2. **Produce an architecture document** describing types, public functions, and invariants
3. **Identify validation tests** (reference Faust programs, expected outputs)

### 10.2 During carrying

1. **Write the full Rustdoc documentation** as you go:
   - Each `pub fn` has a comment `///` with examples
   - Each `pub struct` and `pub enum` is documented
   - Modules have an introductory comment `//!`
   - Ported logic includes a short provenance block in Rustdoc, for example:
     - source C++ file/function path(s),
     - parity-sensitive invariants and behavior notes kept during migration.
2. **Write unit tests** alongside the code
3. **Maintain public API mapping traceability** for touched APIs:
   - classify each as `1:1`, `adapted`, or `deferred`,
   - document rationale + compatibility impact,
   - point to tests validating parity/adaptation.
4. **Enrich the logbook** (`JOURNAL.md`) with:
   - Date and description of what was done
   - Design decisions made and their justifications
   - Differences from C++ implementation
   - Problems encountered and solutions adopted
   - Metrics (number of lines, test coverage, validated Faust programs)

### 10.3 After porting each crate

1. **Verification by differential tests**: compile the same Faust programs with the C++ version and the Rust version, compare the results
2. **Corpus-wide C++ status differential (mandatory gate)**:
   - run **every** `tests/corpus/*.dsp` with:
     - Rust (`cargo run -p compiler -- --dump-sig <file.dsp>`),
     - C++ reference compiler (`faust <file.dsp>`), using `/Users/letz/Developpements/RUST/faust` as source-of-truth tree for the reference binary/toolchain.
   - classify each case as:
     - `OK/OK` (both succeed),
     - `ERR/ERR` (both fail),
     - `OK/ERR` or `ERR/OK` (parity mismatch, blocking until documented and triaged).
   - record results in a persistent report under `porting/phases/` and list remaining mismatches as explicit TODO items.
3. **Negative corpus discipline**:
   - `err_*` fixtures are not "Rust-expected failure" by default.
   - each `err_*` case must be validated against C++ first; if C++ succeeds and Rust fails, treat as Rust regression gap and move/fix classification accordingly.
4. **Rustdoc documentation review**: ensuring it is complete and consistent
5. **Update of the logbook** with the report of the phase
6. **Public API mapping review**: ensure mapping table/status is current for touched surfaces

### 10.4 Logbook (`JOURNAL.md`)

The `JOURNAL.md` file at the root of the workspace follows this format:

```markdown
# Logbook — Faust Porting to Rust

## [2026-MM-DD] Phase X.Y — crate name

### Summary
Short description of what was accomplished.

### Design Decisions
- Choice 1: description and rationale
- Choice 2: ...

### Differences from the C++ Version
- Point 1
- Point 2

### Problems and Solutions
- Problem: description → Solution: description

### Metrics
- Rust lines of code: XXXX
- Corresponding C++ lines of code: XXXX
- Tests: XX passing / XX total
- Validated Faust programs: XX / XX

### Remaining TODO
- [ ] Item 1
- [ ] Item 2
```

---

## 11. Test suite and validation

### 11.1 Unit tests (per crate)

Each crate contains its own `#[cfg(test)]` tests.

### 11.2 Integration tests

The `tests/` folder at the root of the workspace contains end-to-end tests:

```bash
# For each .dsp file in tests/reference/:
# 1. Compile with faust-rs to C
# 2. Compile with faust (C++) to C
# 3. Compare outputs (ignoring formatting differences)
```

### 11.3 Regression tests

Use the existing test suite from the Faust repository (`tests/`) as a reference.

### 11.4 Benchmarks

Compare compilation performance between the C++ version and the Rust version on a set of representative Faust programs.

---

## 12. Volume estimation

Measured on the audited branch:

- **159,012 LOC** for `.cpp` + `.hh`
- **162,315 LOC** for `.cpp/.cc/.cxx/.h/.hh/.hpp/.l/.y`

The Rust port should be expected in a comparable order of magnitude, with possible reduction in some modules but increase in explicit typing, ownership boundaries, and test harness code.

---

## 13. Risks and mitigations

| Risk | Impact | Mitigation |
|--------|--------|------------|
| Complexity of signal translation → FIR | Phase 6 delay | Start documentation from Phase 1, prototype early |
| Compatibility of the lrlex/lrpar parser with the Faust grammar | Blocking Phase 3 | Quickly test with a subset of the grammar |
| Performance of hash-consing in Rust (vs. raw C++ pointers) | Degraded performance | Benchmark early with `criterion`, explore `TreeId` (hints) vs `Arc` |
| LLVM backend: complex Rust binding (`inkwell`/`llvm-sys`) | Delay Phase 7 | Start with low-risk text backends in scope (Rust, Julia, C#, etc.), defer LLVM |
| Volume of code to port | Project duration | Parallelize Phase 7 backends and prioritize high-value targets first (Rust, Wasm, Interpreter) |
| Switching from `gGlobal` to explicit sessions: verbosity of signatures | Friction when carrying | Group the parameters in `CompileSession`, use the traits `TreeRead`/`TreeIntern` |
| Guarantee `Send + Sync` on all structures from the start | Late refactoring if forgotten | Add `#[cfg(test)] fn assert_send_sync<T: Send + Sync>()` to each crate from day 1 |
| Keeping signals semantics split across duplicated dispatch chains and property-mutating analyses | Drift between passes and high-cost rewrites in Phases 4–5 | Introduce a canonical signal-node dispatch layer + session-scoped analysis stores early in Phase 4 |
| Porting the wrong FIR path first (`signalFIRCompiler` only) | Rework and schedule slip | Start from the currently used `InstructionsCompiler` path, add `signalFIRCompiler` later |
| Keeping normalization tied to global property caches and imperative pass ordering | Non-determinism and costly later rewrites | Move normalize to explicit pass pipeline + session-scoped caches early in Phase 5 |
| Keeping parallelize logic split across duplicated loop/sorting implementations and pointer-ordered sets | Scheduling drift, non-deterministic generation, and expensive late refactors | Introduce one loop-graph scheduler service with stable loop IDs and explicit context early in Phase 5 |
| Keeping Dependencies graph/scheduling logic tied to implicit roots, sentinel encodings, and disabled auditing | Hidden dependency bugs and difficult schedule/debug maintenance | Introduce explicit graph IDs + typed dependency edges + active validation pass early in Phase 5 |
| Treating experimental transform bridges (`signalRenderer`/`signalFIRCompiler`) as MVP blockers | Schedule drift with limited parity gain | Keep them maintained but move them to a non-blocking experimental lane after core transform parity |
| Leaving backend orchestration as ad hoc `libcode.cpp` branching | Hard-to-maintain Rust core and duplicated compile paths | Introduce a registry-driven backend selector and shared compile pipeline early in Phase 6 |
| Keeping CLI/backend option constraints as imperative branch chains | Silent validation drift and contradictory rules | Use a declarative capability matrix plus consistency tests in Phase 6/9 |
| Keeping fixed-size temporary argument buffers in API entry paths | Overflow/undefined behavior under large argument sets | Use dynamic bounded argument vectors with explicit validation and error returns |
| Keeping ad hoc string-only diagnostics and counter-only failures across parser/eval/propagate/compiler | Weak developer UX, poor CI/IDE integration, and hard cross-phase triage | Implement the structured diagnostics model from `faust-rust-diagnostics-model-en.md` with staged deliverables and snapshot-tested human/JSON outputs |
| Underestimating C API surface (`box_signal_api.cpp`) | Major Phase 9 delay | Deliver API in tiers (minimal subset first, broad compatibility second) |

---

*Living document — will be updated as the port progresses.*
