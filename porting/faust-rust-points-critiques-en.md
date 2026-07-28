# Porting Faust C++ → Rust — Critical points to validate

> **Objective**: Identify and validate the blocking risks **before** engaging in portage.
> **Recommended duration**: 1 week of prototyping (validation sprint).
> **Principle**: Each critical point has a **minimal prototype** that can be completed in 0.5–2 days. If a prototype fails, we know the problem before having invested months.
> **Implementation rule**: Prefer prototypes wired to real crate APIs and real data paths; avoid temporary stubs unless explicitly justified, owner-assigned, and scheduled for removal in the same phase gate.
> **Execution rule**: Every prototype must define explicit deliverables and pass criteria up front.
> **Documentation rule**: As code is ported, record C++ source provenance and parity-critical invariants directly in Rustdoc (`///`/`//!`).

---

## Overview

| # | Critical point | Impact if blocking | Probability | Validation effort |
|:-:|---------------|:------------------:|:-----------:|:--------------------:|
| 1 | Parsing lrpar and Faust grammar | ★★★ Fatal | AVERAGE | 2 days |
| 2 | TreeArena — performance hash-consing | ★★★ Fatal | Weak | 1 day |
| 3 | Main signal→FIR pipeline target (`InstructionsCompiler` vs `signalFIRCompiler`) | ★★★ Fatal | AVERAGE | 1 day |
| 4 | Decoupling the effective compile path from `gGlobal` (`libcode` + `InstructionsCompiler`) | ★★★ Fatal | AVERAGE | 2 days |
| 5 | LLVM backend via inkwell | ★★☆ Strong | AVERAGE | 1.5 days |
| 6 | Compiler Wasm Compilation | ★★☆ Strong | AVERAGE | 0.5 days |
| 7 | API C (libfaust) — real surface and prioritization | ★★☆ Strong | AVERAGE | 0.5 days |
| 8 | Structured diagnostics model and error UX parity envelope | ★★☆ Strong | AVERAGE | 1 day |

---

## Critical point 1 — Parsing lrpar and Faust grammar

### Why it's critical

The Faust grammar (`faustparser.y`, ~150 rules, ~800 lines) uses specific Bison features:

- Precedence declarations (`%left`, `%right`, `%nonassoc`) to resolve ambiguities
- Typed union (`%union`) for semantic values
- Error handling (`error` token)
- Some shift/reduce conflicts resolved by precedence rules

If **lrpar** (grmtools) does not handle these conflicts in the same way as Bison, the parser produces a different automaton → parsing errors on valid Faust code.

### Known conflicts in grammar

| Conflict | Source | Bison Resolution |
|---------|--------|-----------------|
| `:` (sequence) vs `:` (other contexts) | `expression SEQ expression` | `%left SEQ` |
| `<:` (split) vs `<` (comparison) | Lexer distinguishes the two tokens | OK if lexed correctly |
| Dangling else in `if/then/else` | `expression : IF expression THEN expression ELSE expression` | `%right` |
| `,` (by) vs `,` (list separator) | Different contexts | Precedence + context |
| Parentheses and application of functions | `atom : ident LPAR arglist RPAR` vs `atom : LPAR expression RPAR` | Solved by lexer |

### Validation prototype

```
Duration: 2 days
```

**Day 1:**
1. Create a crate `parser-proto` with `lrlex` + `lrpar`
2. Convert `faustlexer.l` tokens to `.l` grmtools format
3. Convert the first 30 rules of `faustparser.y` into `.y` grmtools format (expressions, compositions, atoms)
4. Compile the parser (`cargo build`)
5. Check: Does grmtools report unresolved conflicts?

**Day 2:**
6. Test the analysis of:
   - `process = _;`
   - `process = + ~ _;`
   - `process = hslider("freq", 440, 20, 20000, 1);`
   - `process = _ <: _, _;`
   - `process = par(i, 4, _);`
   - `import("stdfaust.lib"); process = os.osc(440);`
7. If errors → analyze the conflicts and try solutions
8. Document: does grmtools handle Bison precedence correctly?

### Success criterion

The 6 examples above parse without error and produce a coherent syntax tree.

### Plan B if failure

| Alternative | Effort | Compromise |
|-------------|:------:|-----------|
| **lalrpop** | +3 days | Rewriting the grammar in lalrpop format (not Yacc compatible) |
| **tree sitter** | +5 days | Separate grammar, incremental parsing (useful for IDE) |
| **Downward recursive parser** | +8 days | Full control, no dependencies, but lots of code |
| **pest** (PEG) | +4 days | Simple but not LALR — may have performance issues on large files |

---

## Critical point 2 — TreeArena: hash-consing performance

### Why it's critical

The `TreeArena` replaces the C++ hash-consing system (`CTree` + static hash table of 400,009 entries). **All** compiler representations (boxes, signals, types, lists) rely on it. If the arena is slow, the whole compiler is slow.

Risk factors:
- In C++, hash-consing uses **raw pointers** (address comparison = O(1)). In Rust, we use `TreeId(u32)` — comparison just as fast, but indirection via index adds memory access
- The Rust HashMap (`std::collections::HashMap`) uses SipHash (resistant to attacks, but slower than the C++ custom hash)
- The allocations in the `Vec<TreeNode>` are amortized, but the access pattern is different (cache locality)

### Validation prototype

```
Duration: 1 day
```

1. Implement `TreeArena` minimal:
   - `NodeValue` (Int, Double, Sym)
   - `TreeNode` (value, branches: SmallVec<[TreeId; 4]>, hash)
   - `make(value, branches) -> TreeId` with hash-consing
   - `node(id)`, `branches(id)`, `arity(id)`

2. Benchmark criteria:
   - **Creation**: 500K typical trees (2–4 branches, depth 5–10)
   - **Lookup**: 500K `make()` on already existing trees (must return the same `TreeId`)
   - **Traversal**: Traverse a tree of 100K nodes
   - **Properties**: 1M `set` + `get` on a `TreeProperty<i32>`

3. Compare with an equivalent C++ benchmark using `CTree`

### Success criterion

| Operation | Acceptable threshold |
|-----------|:----------------:|
| Creation of 500K trees | <100ms |
| Lookup of existing 500K | <50ms |
| Traversal of 100K nodes | < 10ms |
| No more than 2x slower than C++ | — |

### Optimizations if too slow

| Optimization | Expected gain |
|-------------|:------------:|
| Replace `HashMap` with `hashbrown::HashMap` (already the default in nightly) | 10–20% |
| Custom Hash (FxHash, AHash) instead of SipHash | 20–40% |
| Arena allocator (`bumpalo`) for nodes | 15–25% |
| Pre-allocate the table with 400K entries | 5–10% |

---

## Critical point 3 — Main signal→FIR pipeline target

### Why it's critical

The current branch contains two signal→FIR paths:

- Former/production path: `libcode.cpp` dispatching to `InstructionsCompiler` / `DAGInstructionsCompiler`
- Newer path: `signalFIRCompiler`

If we port the wrong one first, we can spend weeks without reaching practical parity.

### Validation prototype

```
Duration: 1 day
```

1. Check backend dispatch in `libcode.cpp` (`generateCode` and `compile<Lang>()` calls)
2. Check where `SignalFIRCompiler` is effectively used in end-to-end compilation
3. Build a decision matrix:
   - MVP parity target path
   - Optional secondary path
4. Freeze the order before writing Rust code

### Success criterion

- A clear decision on the primary path for Rust MVP parity
- Proof that selected path is the one used by `-lang c` / `-lang cpp` in current C++

### If decision is unclear

| Strategy | Effort | Description |
|-----------|:------:|-------------|
| Differential probe build | +1 day | Add temporary logs to assert which path is executed |
| Two-step migration | +5–10 days | Port production path first, then port secondary path |
| Delay secondary path | 0 day now | Keep non-primary path for a post-parity milestone |

---

## Critical point 4 — Decoupling the effective path from `gGlobal`

### Why it's critical

Even after choosing the primary path, parity requires extracting `gGlobal` dependencies from the files actually used in compilation:
- `libcode.cpp`
- `generator/instructions_compiler.cpp`
- `generator/dag_instructions_compiler.cpp`

### Validation prototype

```
Duration: 2 days
```

1. Count and categorize `gGlobal` usages in the selected files (config, symbols, counters, mutable runtime state)
2. Draft a first `CompilerContext` split (C++) for this path
3. Re-run representative compile commands to verify unchanged behavior
4. Check special cases:
   - Programs with tables (`rdtable`, `rwtable`)
   - Multi-rate programs (on-demand, downsampling)
   - Programs with complex pattern matching
   - `-fx` mode (separate effects)
   - Vectorized/scheduler options (`-vec`, `-omp`, `-sch`)

### Success criteria

- Context split documented and validated for effective compile flow
- No hidden mutable-global dependency left unidentified in selected files
- Migration signatures reviewed before coding

### If insufficient coverage

| Situation | Action |
|-----------|--------|
| Deep hidden coupling remains | Add a dedicated C++ pre-refactor step (+1–2 weeks) |
| Mostly config/symbol dependencies | Continue with Rust port, context split as planned |
| Stateful runtime coupling | Keep temporary compatibility layer, remove incrementally |

---

## Critical point 5 — LLVM backend via inkwell

### Why it's critical

The LLVM backend is essential for:
- **libfaust** in JIT mode (FaustLive, Faust IDE, real-time plugins)
- Maximum performance (optimized LLVM code is 2–5× faster than the interpreter)
- Native compilation (`.o`, `.so`, `.dylib`)

The C++ uses the LLVM C++ API directly. In Rust, we use **inkwell** (bindings safe) or **llvm-sys** (bindings raw).

### Specific risks

| Risk | Detail |
|--------|--------|
| Incompatible LLVM version | inkwell supports LLVM 14–19, but Faust can target a specific version |
| Optimization Pass API | The Pass API has changed (legacy PassManager → new PassManager) between LLVM 14 and 17 |
| JIT on macOS arm64 | JIT LLVM (OrcJIT v2) had issues on Apple Silicon |
| Cross compilation | inkwell does not support cross-compilation easily |
| Size of dependencies | LLVM adds ~200MB build dependencies |
| Incompatible Wasm | inkwell/LLVM cannot be compiled into Wasm — the LLVM backend must be feature-gated |

### Validation prototype

```
Duration: 1.5 days
```

**Day 1:**
1. Create a crate `llvm-proto` with `inkwell` (feature `llvm18-0` or the installed version)
2. Implement minimal DSP in IR LLVM via inkwell:
   ```
   define void @compute(i32 %count, float** %inputs, float** %outputs) {
       ; Load input[0][i], multiply by 0.5, store in output[0][i]
   }
   ```
3. JIT-compile and run on a test audio buffer
4. Check the result (signal attenuated by 6 dB)

**Day 1.5:**
5. Audit the LLVM APIs used by Faust:
   ```bash
   grep -h "llvm::" generator/llvm/*.cpp generator/llvm/*.hh | \
       sed 's/.*llvm::/llvm::/' | sed 's/[^a-zA-Z:].*//' | sort -u
   ```
6. For each LLVM API, check the inkwell coverage:
   - `llvm::Module`, `llvm::Function`, `llvm::BasicBlock` → ✓
   - `llvm::IRBuilder` → ✓ (via `builder`)
   - `llvm::PassManager` → ⚠ (check legacy vs new)
   - `llvm::ExecutionEngine` (MCJIT) → ✓
   - `llvm::orc::*` (OrcJIT) → ⚠ (partial support)
   - `llvm::TargetMachine` → ✓

7. Test on the 3 platforms:
   - Linux x86_64
   - macOS arm64 (Apple Silicon)
   - Windows x86_64 (if possible)

### Success criteria

- JIT prototype works on Linux and macOS arm64
- ≥ 90% of LLVM APIs used by Faust are covered by inkwell
- The PassManager (optimizations) works

### Strategy if problems

| Issue | Solution |
|----------|----------|
| inkwell does not cover a critical API | Use `llvm-sys` (unsafe) for this specific part |
| JIT fails on macOS arm64 | Use compilation to object file mode (no in-process JIT) |
| Incompatible PassManager | Use `opt` externally (text pipeline: `.ll` → `opt` → `.o`) |
| Everything is blocked | Report LLVM backend, use interpreter as libfaust runtime |

### Impact if we report the LLVM backend

The Faust interpreter (Phase 7) is a viable fallback:
- It works everywhere, including Wasm
- It is ~3–5× slower than LLVM JIT, but sufficient for many uses
- It is already used in production in the Faust web IDE

The LLVM backend can be added **after** milestone M5 (parity) without blocking the rest of the project.

---

## Critical point 6 — Wasm compilation of the compiler itself

### Why it's critical

One of the strong arguments of the Rust port is being able to compile **the compiler itself** into WebAssembly, replacing the current Emscripten chain. This would allow:
- A lighter and faster Faust web IDE
- No dependency on Emscripten for CI
- Direct compilation `cargo build --target wasm32-unknown-unknown`

### Risks

| Dependence | Wasm compatible? |
|-----------|:-----------------:|
| `lrlex` / `lrpar` | ❓ To check — no syscall a priori |
| `hashbrown` / `std::collections` | ✓ |
| `smallvec` | ✓ |
| `serde_json` | ✓ |
| `sha1` | ✓ |
| `inkwell` / `llvm-sys` | ✗ — must be feature-gated |
| File I/O (`std::fs`) | ✗ — you need a VFS or `wasm-bindgen` |
| `rayon` (parallelism) | ✗ — no threads in Wasm (except SharedArrayBuffer) |
| `reqwest` / `ureq` (HTTP) | ✗ — you need `wasm-bindgen` + `fetch` |

### Validation prototype

```
Duration: 0.5 day
```

1. Create a minimal crate depending on `lrlex` + `lrpar` + `serde_json`
2. `cargo build --target wasm32-unknown-unknown`
3. If it compiles → the main risk is removed
4. If that fails → identify the blocking dependency

For file I/O, the standard Wasm solution is an **in-memory VFS**:
```rust
#[cfg(target_arch = "wasm32")]
pub struct MemoryFileSystem { files: HashMap<String, Vec<u8>> }

#[cfg(not(target_arch = "wasm32"))]
pub use std::fs;
```

### Success criterion

- The analysis crate compiles to `wasm32-unknown-unknown`
- The complete compiler compiles in Wasm with `--no-default-features` (without LLVM, without native I/O, without radius)

---

## Critical point 7 — API C (libfaust): surface actually used

### Why it's critical

`box_signal_api.cpp` (3,085 lines) exposes a large C/C++ API. On the current branch, it contains **453 `LIBFAUST_API` declarations**. Carrying the full surface in one pass is expensive, so prioritization is mandatory.

### Validation prototype

```
Duration: 0.5 day
```

```bash
# In the Faust repository, search for API calls
cd faust/
grep -rn "createDSPFactory\|createInterpreter\|DSPToBoxes\|boxesToSignals\|createCDSP\|deleteDSP\|getCDSP" \
    architecture/ tools/ tests/ | grep -v "\.o:" | sort
```

### Probably essential API (minimal subset)

| Function | Used by |
|----------|-------------|
| `createDSPFactoryFromString` | FaustLive, web IDE, plugins |
| `createDSPFactoryFromFile` | faust2* scripts |
| `deleteDSPFactory` | All |
| `createDSPInstance` / `deleteDSPInstance` | All |
| `DSPToBoxes` / `boxesToSignals` | Intermediate API, analysis tools |
| `getCDSPFactoryFromSHAKey` | Build cache |
| `expandDSPFromString` | IDE, macro expansion |
| `generateAuxFilesFromString` | SVG generation, doc |

### Probably secondary API (may be postponed)

| Function | Reason |
|----------|--------|
| `createCDSPFactoryFromBoxes` | Little used — programmatic construction |
| `createCDSPFactoryFromSignals` | Little used |
| The 400+ individual box/signal helpers (`Cbox*`, `Csig*`, ...) | Very fine API — useful for bindings but not all critical for MVP |

### Success criterion

- The minimal subset identified (< 20 functions)
- Critical tools (FaustLive, faust2jack, IDE) only use this subset
- Estimated effort for the subset: < 5 days (instead of 7)

---

## Critical point 8 — Structured diagnostics model and error UX parity envelope

### Why it's critical

Even with semantic parity, weak error reporting slows development, regression triage, and user adoption. The C++ model is mostly string/exception and global-counter based; Rust should keep failure-class parity while providing structured diagnostics.

Without an explicit prototype, diagnostics quality is likely to regress into mixed ad hoc strings across parser/eval/propagate/compiler boundaries.

### Validation prototype

```
Duration: 1 day
```

1. Define a minimal diagnostics schema in `diagnostics` (`severity`, `stage`, `code`, `labels`, `notes/help`).
2. Convert one parser error class and one eval/propagate error class into this schema.
3. Expose two renderers:
   - human text output,
   - JSON output for tooling.
4. Validate deterministic snapshots on malformed fixtures.

### Success criterion

- One shared diagnostics schema is used by at least parser + one Phase 4 crate.
- Stable diagnostic code families exist (`FRS-PARSE-*`, `FRS-EVAL-*`/`FRS-PROP-*`).
- Human and JSON outputs are deterministic on CI fixtures.

### Plan B if insufficient coverage

| Situation | Action |
|-----------|--------|
| Schema too narrow for cross-phase use | Expand labels/notes/help model before broad rollout |
| Source spans unavailable in some passes | Keep structured code/stage first, add spans incrementally with explicit TODO owner/date |
| CLI integration delayed | Keep schema rollout in crates, add renderer in next integration sprint |

Reference architecture:
- `porting/faust-rust-diagnostics-model-en.md`

---

## Validation sprint planning

```
          Mon         Tue         Wed         Thu         Fri
        ┌───────────┬───────────┬───────────┬───────────┬───────────┐
  AM    │ TreeArena │ Parser    │ Parser    │ Audit     │ LLVM      │
        │ prototype │ lrpar (1) │ lrpar (2) │ signalFIR │ inkwell   │
        ├───────────┼───────────┼───────────┤ Compiler  │ prototype │
  PM    │ Benchmark │ Parser    │ Coverage  │ gGlobal   ├───────────┤
        │ criterion │ lrpar     │ new       │ audit     │ Wasm test │
        │           │ suite     │ pipeline  │           │ API C     │
        └───────────┴───────────┴───────────┴───────────┴───────────┘
         Point 2     Point 1     Point 4     Point 3     Points 5-8
```

### Sprint deliverables

At the end of the week, a decision document containing:

1. **Parser**: ✅ lrpar works / ❌ → plan B retained (lalrpop / tree-sitter / RD)
2. **TreeArena**: ✅ performance OK / ⚠ → necessary optimizations (which ones)
3. **Pipeline target**: ✅ primary path chosen for MVP / ❌ decision blocked
4. **gGlobal decoupling**: ✅ effective path decoupling feasible / ❌ prior C++ refactoring necessary
5. **LLVM**: ✅ inkwell OK / ⚠ → fallback interpreter / ❌ → LLVM postponed
6. **Wasm**: ✅ compiles / ❌ → blocking dependency identified
7. **C API**: documented minimal subset
8. **Diagnostics model**: shared schema validated across parser + one downstream phase

### Go/No-Go decision

| Result | Decision |
|----------|----------|
| Points 1–4 all ✅ | **Go** — porting can begin |
| Point 1 ❌ (parser) | **Go conditional** — change parser, add 3–8 days |
| Point 2 ❌ (TreeArena) | **No-Go** — fundamental problem, reassess the architecture |
| Point 3 ❌ (pipeline target unclear) | **No-Go** — decide target path before implementation |
| Point 4 ❌ (gGlobal coupling) | **Go conditional** — allow 1–2 weeks of C++ refactoring first |
| Points 5–8: Problems | **Go** — these points have viable fallbacks |
