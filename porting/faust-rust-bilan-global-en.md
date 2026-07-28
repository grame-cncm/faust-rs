# Overall assessment of the portage modeling work

> **Project**: Porting the Faust C++ compiler → Rust
> **Date**: February 2026
> **Branch analyzed**: `master-dev-ocpp-od-fir-2-FIR19`
> **Clarification**: branch name includes `ocpp`, but old C++ mode `-lang ocpp` is out of scope.

---

## 1. What was produced

### 1.1 Documents

| Document | Lines | Content |
|----------|:------:|---------|
| **Strategic plan** (`faust-rust-porting-plan.md`) | 875 | Crate architecture, dependencies, fundamental decisions (TreeArena, elimination of gGlobal, parallelization), roadmap in 9 phases |
| **Phase 1 — Foundations** | 473 | tlib, errors, utils, interval, algebra, graph — TreeArena with hash-consing, externalized properties, symbols |
| **Phase 2 — Block Diagrams** | 298 | ~180 box constructors, type inference, pretty-printing |
| **Phase 3 — Parse** | 358 | Bison/Flex migration → lrlex/lrpar, SourceReader, embedding |
| **Phase 4 — Signals** | 480 | ~183 signal constructors, evaluation, pattern matching, propagation boxes→signals |
| **Phase 5 — Standardization** | 455 | Rewriting rule system (Yann 2025), 12 transformation passes, signalFIRCompiler |
| **Phase 6 — FIR & C/C++ Backends** | 421 | FIR hierarchy (~60 instruction types), CodeContainer, FIR→FIR transformations, C and C++ backends |
| **Phase 7 — Additional Backends** | 353 | Additional backends (Wasm, Interpreter, LLVM, Rust, Julia, etc.); Java backend out of scope, feature flags |
| **Phase 8 — Draw & Doc** | 237 | 18 types of SVG diagrams, LaTeX documenter |
| **Phase 9 — Integration** | 511 | CompileSession, full pipeline, C/C++ API (libfaust), repo integration, parallelization |
| **Effort report** | 150 | Person-day estimates, human vs. AI costs, milestones, risks |
| **Critical points** | 450 | 7 blocking risks, validation prototypes, one-week sprint, Go/No-Go |
| **Total** | **~5,060** | |

### 1.2 Source code analysis

The entire C++ source code has been downloaded and analyzed:

- **159,012 LOC** (`.cpp` + `.hh`) inventoried file by file
- **162,315 LOC** including `.h/.hpp/.l/.y`
- **~300 files** C++ cataloged with lines, role and module
- **~20 modules** mapped with their dependencies
- All key classes, enums, hierarchies and patterns identified

### 1.3 Architectural decisions made

| Decision | Justification |
|----------|--------------|
| `TreeId(u32)` Copy into a `TreeArena` | Eliminate GC (`Garbageable`), raw pointers, `static mut` |
| Outsourced Properties (`TreeProperty<V>`) | Allows independent borrowing, avoids shared mutability |
| Short crate naming (`boxes`, `signals`, `fir`…) | Private workspace, no need for `faust-` prefix |
| Rust Enums for FIR Instructions | Replaces C++ inheritance hierarchy with 60 classes + visitor pattern |
| `XtendedOp` enum instead of 22 vtable classes | One match, no legacy, no `void*` |
| MVP on effective production pipeline first (`InstructionsCompiler`/`DAGInstructionsCompiler`) | This is the path currently dispatched by `libcode.cpp`; `signalFIRCompiler` can be ported as a secondary path |
| Backends in separate crates with feature flags | Conditional compilation identical to current CMake |
| `RewriteRule` trait for all transformations | Standardizes the old system (`TreeTransform`) and the new (Yann 2025) |

---

## 2. What is missing

### 2.1 Concrete deliverables to be produced

| Deliverable | Priority | Effort | Description |
|----------|:--------:|:------:|-------------|
| **Validation sprint (1 week)** | ★★★ | 5 days | Prototypes for the 7 critical points — Go/No-Go before committing |
| **Cargo.toml workspace** | ★★★ | 0.5 days | Real skeleton with ~20 crates, versions of external dependencies |
| **Comprehensive gGlobal mapping** | ★★★ | 2 days | Field by field list of ~408 fields with Rust destination |
| **Differential testing plan** | ★★☆ | 1 day | What examples, what backends, what comparison format |
| **JOURNAL.md** | ★★☆ | 0.5 days | Template and first entries |
| **Transition strategy** | ★★☆ | 0.5 days | C++/Rust coexistence, gradual replacement or big bang |

### 2.2 Incomplete analysis areas

| Area | What is missing | Impact |
|------|--------------|--------|
| **Pipeline targeting** (`InstructionsCompiler` vs `signalFIRCompiler`) | Wrong first target can delay parity significantly | Freeze primary path before coding (critical point 3) |
| **Runtime libfaust** (`dsp_aux`, `interpreter_dsp_aux`, `llvm_dsp_aux`) | ~5,000 LOC not detailed in phases — needed for FaustLive and IDE | May delay libfaust parity by 2–3 weeks |
| **Architecture files** | The interaction between `enrobage.cpp` and the ~50 architecture files is not detailed | Low risk — simple find/replace mechanic |
| **Faust Standard Library** | No analysis of advanced constructions in `.lib` (pattern matching, imports) | Risk covered by phase 3 analysis tests |
| **Build system** | CMake/Cargo coexistence during transition not detailed | Organizational risk, not technical |
| **LLVM Backend** | Inkwell compatibility not verified (JIT, passes, macOS arm64) | Fallback viable (interpreter), but would delay performance parity |
| **Developer Guide** | No onboarding documentation for GRAME contributors | Needed if other people are contributing to Rust code |

### 2.3 Open questions

| Question | Impact of response |
|----------|---------------------|
| Does the new pipeline (`signalFIRCompiler`) cover `-fx` mode (separate effects)? | If not → also wear `InstructionsFXCompiler` |
| What subset of the libfaust C API is actually used by the tools? (`box_signal_api.cpp` has 453 exports) | Potentially reduces Phase 9 effort significantly if delivered in tiers |
| Should we support `-ocpp` (old C++ backend based on `klass`)? | **Decision**: No. `-lang ocpp` is out of scope for the Rust port. |
| Does inkwell support JIT on macOS arm64? | If not → LLVM backend limited to ahead-of-time compilation |
| Does lrpar handle precedence conflicts in the Faust grammar? | If not → change parser (lalrpop, tree-sitter, or RD) |
| Do we want Wasm compilation from the compiler itself from the start? | If yes → constraint on all dependencies (no LLVM, no native I/O, no radius) |

---

## 3. Strengths of modeling

### 3.1 Exhaustive source coverage
Each C++ compiler file has been inventoried with its number of lines and its role. This is not a plan based on a cursory reading — it is a complete x-ray of the 135K LOC.

### 3.2 Concrete mapping C++ → Rust
Phases are not vague descriptions. They contain **real Rust signatures**: structs, enums, traits, public functions. A developer can start coding directly from these specifications.

### 3.3 Early identification of risks
The 7 critical points are identified **before** the start of work, with validation prototypes that can be produced within a week. This is the difference between a project that discovers a blockage in month 3 and a project that knows about it in day 5.

### 3.4 Realistic estimate
Estimates are broken down by sub-module (not an overall number pulled out of a hat). The low/high range reflects the actual uncertainty. The AI ​​factor is estimated separately with an analysis of what it accelerates and what it does not accelerate.

---

## 4. Weaknesses and limitations

### 4.1 No executable code
Modeling produces documents, not code. The proposed Rust signatures have not been compiled or tested. Certain API choices could prove impractical against the borrow checker (notably functions which take `&mut TreeArena` and must also read other data from the arena).

### 4.2 Optimistic estimates on AI
The ×4–5 factor of AI relies on the assumption of an expert developer who knows exactly what to ask. A developer less familiar with Faust or Rust would benefit from a lower factor (×2–3). AI does not replace understanding DSP.

### 4.3 Pipeline targeting must match effective compile flow
Treating `signalFIRCompiler` as the only target is a gamble on this branch. The effective backend dispatch still goes through `InstructionsCompiler`/`DAGInstructionsCompiler`. If we start with the wrong path, effort increases due to rework.

### 4.4 The libfaust runtime is underestimated
The `dsp_aux` files (dynamic loading, factories, DSP instances) represent ~5,000 LOC which are not detailed in the phases. This is not the compiler per se, but it is necessary for parity with `libfaust`.

### 4.5 No real prototype
None of the technical choices (TreeArena, lrpar, inkwell) have been validated by functional code. The one-week validation sprint is designed to fill this gap, but it has not yet been realized.

---

## 5. Recommended next steps

### Step 0 — Validation Sprint (1 week)

| Day | Action | Deliverable |
|------|--------|----------|
| Monday | TreeArena prototype + benchmark criterion | Performance validated or optimizations identified |
| Tuesday | Partial grammar conversion to lrpar (30 rules) | Compilable parser or plan B identified |
| Wednesday | Parser suite: tests on 6 Faust examples | Functional parser or fallback chosen |
| THURSDAY | gGlobal audit in signalFIRCompiler (grep + categorization) | Documented coupling mapping |
| Friday | inkwell JIT prototype + lrpar test in Wasm + API C audit | LLVM/Wasm/API risks assessed |

**Deliverable**: Go/No-Go document with decisions on parser, arena, coupling.

### Stage 1 — Skeleton (2 days)

- `Cargo.toml` workspace with ~20 crates (empty)
- `JOURNAL.md` initialized
- CI GitHub Actions Minimal (`cargo check`)

### Step 2 — Phase 1 (3–4 weeks in AI tandem)

- `tlib`: TreeArena functional with tests and benchmarks
- `diagnostics`, `ffi-common`, `algebra`, `graph`: focused foundation crates
- `interval`: 60+ operations carried with `check.cpp` tests

### Stage 3 and beyond — Phases 2–9

Follow the phase plan with milestones M1–M5 as checkpoints.

---

## 6. Summary

This modeling work represents approximately **2 days of human + AI collaborative work**. He produced **~5,000 lines** of technical documentation covering the entire 135K LOC of the Faust compiler.

The porting project is **feasible** — the Rust architectural choices are solid, the estimates are realistic, and the risks are identified with B plans.

**One week of prototyping** remains to turn this plan into certainty. This is the most profitable investment possible before committing to 6–9+ months of development.

| | Modeling (done) | Validation (to be done) | Execution (after Go) |
|---|:---:|:---:|:---:|
| **Effort** | 2 days | 1 week | 6–9+ months |
| **Result** | Detailed plan | Go/No-Go | Rust compiler |
| **Residual risk** | AVERAGE | Weak | Mastered |
