# WASM `compute` speed: analysis and plan

Date: 2026-10-16

Status: **analysis only**, nothing implemented. Decisions D1 to D3 (section 6)
are open.

Origin: point 14 of faust-coopiano's `faust-rs-issues/RAPPORT.md`. The wasm
from `faust-rs -lang wasm -double` runs 1.3 to 1.8 times slower than faust-rs's
own C++ (`-lang cpp -double`) compiled to wasm by `em++ -O3`, with identical
output. The imaged body does not run in real time in a browser.

| Compiler | Version | Binary |
|---|---|---|
| faust-rs | `main-dev` at `354e2080` | release `faust-rs` |
| Faust C++ reference | 2.84.3, `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429` | `build/bin/faust` of that checkout |
| Faust C++ current | 2.90.10 (`15a26da0c`) | `/usr/local/bin/faust` |
| Emscripten | MacPorts `em++ -O3`, LLVM 23 | |
| Binaryen | `wasm-opt` version 132 | |
| JS engine | node 22.12.0, V8 12.4.254.21 | |

Neither C++ compiler accepts the coopiano bodies: 2.90 rejects `ondemand`, the
reference rejects a `route` with computed parameters. Where the two C++
compilers are compared below, it is on a shared DSP, `bank.dsp` (section 2.3).

## 1. Summary

1. **`wasm-opt -O3` recovers nothing.** On the light body it cuts the locals of
   `compute` from 22,982 to 623 and the static loads by 13 %, with no change in
   time (41–50 % before, 42–52 % after). On the bank it is slower (3.6 % to
   3.9 %). The cost is not in local handling, and a generic optimizer cannot
   reach it.
2. **Cause 1, 23 to 45 % of the cycles: how struct fields are addressed.**
   Every struct access is `(local.get 0 + i32.const K) + (i32.const i * i32.const
   size)`, emitted as separate i32 instructions. TurboFan value-numbers each
   `dsp + K` across the whole function. Each field address becomes a
   long-lived integer value, which is spilled to the stack and reloaded at
   each use. Faust C++ emits `i32.const K` (absolute addresses, "fast memory")
   whenever the memory is internal. Emscripten folds `K` into the load's
   `offset=` immediate. A WAT rewrite that only changes the address form
   (`memarg` or absolute) cuts cycles per sample by 23 % on the voice, 27 % on
   the light body and 45 % on the imaged body. The outputs stay bit-identical.
3. **Cause 2, on resonator-bank code: every state and coefficient is read at
   the top of the sample loop.** In the light body, `double fTemp36 =
   fRec2[1];` is line 1,397 of `compute` and its use is line 24,905. TurboFan
   keeps wasm loads in program order, so the value stays live across the
   whole loop body and is spilled. LLVM moves the load next to its use.
   Moving reads to their first use takes another 18 % off the bank. faust-rs
   then beats both C++ wasm outputs and em++. It gains only 0 to 6 % on the
   coopiano programs.
4. **Cause 3, small: stores reloaded in the same sample.** `fRec[0]` is stored,
   then loaded back for the output sum and for the `fRec[1] = fRec[0]` copy.
   V8 does no load elimination on linear memory. Forwarding the stored value
   gains 2 to 3 %.
5. **Open: rarely-taken `if` arms account for the light body's remaining
   gap.** After the three fixes the light body still runs about 1.35 times
   em++. As a code-shape probe, the control-change and gated arms were
   removed. Without them, faust-rs with W1 runs at em++'s level: 27–30k
   cycles per sample against 26–29k. The arms cost faust-rs 10k to 13k
   cycles per sample and em++ at most 2k. Branch hints have no effect in this
   V8. The cause is not established (W4).

Projected effect on the coopiano web build. These are the percentages that
`make web-check` reports (share of a core, node 22, eager TurboFan). The
ranges span runs under different machine loads:

| DSP | now | W1 | W1+W2+W3 | em++ -O3 |
|---|---|---|---|---|
| voice | 7.4–7.8 % (report) | ≈ 5.8 % (cycles −23 %; 9.0 → 6.7 % in one loaded run) | same as W1 | 5.7 % (report) |
| light body | 40–44 % | 30–36 % | 30–34 % | 20.5–21 % |
| imaged body | 93–102 % | 52–53 % | 48–51 % | 49 % (report) |

## 2. Measurements

The rewritten modules give the same 50-block output checksum as faust-rs, to
the last printed digit. em++ agrees with both to 11 significant digits on the
coopiano bodies, and exactly on the bank. The machine was shared with other sessions, with a load
average of 6 to 28. Wall-clock percentages are the minimum of several
interleaved runs. Cycle and instruction counts come from `/usr/bin/time -l`
("instructions retired", "cycles elapsed"): a run of N blocks minus a run of
0 blocks, divided by `N × 128`. Instruction counts repeat to 0.1 %. Cycle
counts vary by about 10 %, depending on the core macOS picks.

### 2.1 Light body (`dsp/coopiano_light.dsp`, `-pn effect`)

| Build | instr/sample | cycles/sample | node 22, % of a core |
|---|---|---|---|
| faust-rs | 152,100 | 54,000–57,000 | 40–44 (eager TF), 41–57 (tiering) |
| `wasm-opt -O3` of it | — | — | 42–52 (tiering) |
| W1 rewrite (absolute addresses) | 117,600 | 39,800–41,000 | 30–36 |
| W1 rewrite (`memarg` offsets) | 120,500 | 37,000–47,000 | — |
| W1+W2 rewrite (sinking) | 117,200 | 37,300–39,900 | 30–31 |
| W1+W2+W3 rewrite (forwarding) | 115,500 | 37,300–38,700 | 30–34 |
| em++ -O3 of faust-rs C++ | 106,200 | 27,700–27,900 | 20.5–21 |

Dynamic memory traffic per sample, from `wasm-opt --instrument-memory`:
faust-rs does 12,518 loads and 3,175 stores to the DSP struct, plus 2 calls to
the `_tanh` import; em++ does 10,663 loads and 3,177 stores. The gated blocks
do not run in this bench (constant input 1e-3, pedal up). The memory traffic
differs by 15 %, while cycles differ by a factor of 2.

TurboFan code of `compute` (`node --no-liftoff --print-wasm-code`, whole
function, arm64 instructions):

| | faust-rs | W1 rewrite | em++ |
|---|---|---|---|
| total | 545,462 | 495,086 | 366,480 |
| integer spill reloads / stores | 53,948 / 16,135 | 20,811 / 2,570 | 13,254 / 116 |
| `add` | 25,762 | 22,648 | 5,746 |
| `movn` (frame offsets beyond the immediate range) | 114,411 | 71,050 | 50,107 |

In faust-rs, each `fRecN[1] = fRecN[0]` copy at the end of the sample is six
instructions: `movn` and a spill reload for the source address, the load,
`movn` and a spill reload for the destination address, the store. em++ builds
both addresses with `movz`/`movk`, which are pure ALU instructions.

### 2.2 Voice and imaged body

| Build | voice (held C4) instr / cycles per sample | imaged body instr / cycles per sample | imaged body % |
|---|---|---|---|
| faust-rs | 33,900 / 6,800–7,900 | 295,700 / 130,000–134,000 | 93–102 |
| W1 (absolute) | 29,200 / 5,300–5,800 | 190,100 / 68,000–76,000 | 52–53 |
| W1 (`memarg`) | 30,000 / 5,500–5,900 | 196,300 / 72,000–79,000 | 53 |
| W1+W2+W3 | 29,200 / 5,300–5,400 | 181,900 / 67,000–69,000 | 48–51 |

### 2.3 Shared DSP: bank of 400 coupled-form resonators

`bank.dsp` uses no libraries, so both C++ compilers accept it. Each mode is
`step ~ (_, _)` with `step(re, im, x) = c*re - s*im + x, s*re + c*im`, the shape
of the coopiano body.

| Build | instr/sample | cycles/sample | % of a core |
|---|---|---|---|
| faust-rs | 21,300 | 3,370 | 3.6–4.3 |
| `wasm-opt -O3` of it | — | — | 3.9–4.2 |
| Faust C++ 2.84.3 reference, `-lang wasm` | 19,200 | 2,810 | 3.0–3.3 |
| Faust C++ 2.90.10, `-lang wasm` | 12,500 | 2,815 | 3.0 |
| W1 rewrite | 16,700 | 2,560 | 2.8–2.9 |
| W1+W2 rewrite | 15,200 | 2,100 | 2.3–2.4 |
| W1+W2+W3 rewrite | 15,900 | 2,045 | 2.2–2.3 |
| em++ -O3 of faust-rs C++ | 10,800 | 2,350 | 2.5–2.7 |

The reference C++ wasm reads struct fields as `f64.load offset=K (i32.const
0)` and writes recursion states through `IOTA`-indexed two-slot rings, with no
copy. 2.90 also folds `K` into `offset=`, and writes each state with one
`f64.store` of the whole expression, with no temporaries.

## 3. Causes

**C1, address form** (`crates/codegen/src/backends/wasm/mod.rs`).
`lower_store_var_struct`, `lower_store_table_struct`, the `LoadVar`/`LoadTable`
`kStruct` arms of `lower_expr` (around lines 1745 and 1870), and the
`WasmLocalStorage::Memory` arms all push `LocalGet(0); I32Const(offset);
I32Add` and use `memarg(0)`. `lower_index_offset` adds `index * size` as an
`I32Mul`, even when the index is a constant. TurboFan folds the constants. It
also value-numbers every distinct `dsp + K` and keeps each one live from its
first use to its last. The light body's `compute` holds 15,947 distinct `i32`
constants, most of them field offsets, so these values are spilled.
`wasm-opt` cannot fold `K` into `offset=`: `i32.add` wraps at 2^32 while
`offset=` does not, so the fold is only allowed under `--low-memory-unused`,
for constants below 1,024. Faust C++ makes this choice in `WASMInstVisitor`:
with `fFastMemory`, which `WASMCodeContainer` sets to `internal_memory`, the
`NamedAddress` and `IndexedAddress` visitors emit `i32.const K`, and
`local.get 0 + i32.const K` otherwise (`wasm_instructions.hh`).

**C2, schedule.** The FIR of `compute` reads each recursion state into a
`fTemp` at the start of the sample and computes the recursion bodies near the
end. Native C++ and em++ do not suffer from this, because LLVM sinks loads and
pure expressions to their uses. Cranelift (faustprobe) probably does: it is
not measured here.

**C3, store→load round trips.** These come from the same FIR shape: the
`fRec[0]` store, the output expression that reads `fRec[0]`, and the
`fRec[1] = fRec[0]` copy.

**C4, cold arms (open).** The light body has 100 control-change blocks
(`if (iTemp1 != 0)`: coefficients recomputed with `pow`, `exp`, `sin` and
`cos` imports when a slider moved) and 69 sympathetic-register blocks gated
by a level threshold. None runs in steady state. Removing them all is a probe
that zeroes the output (C++: `if (0)`; WAT: the `if` dropped). It gives these
instructions and cycles per sample:

| | with arms | without arms | cost of the arms (cycles) |
|---|---|---|---|
| faust-rs | 152,100 / 54,000–57,000 | 92,900 / 32,800–34,800 | ≈ 22,000 |
| faust-rs, W1 | 117,600 / 39,800–41,000 | 88,400 / 27,200–30,500 | 10,000–13,000 |
| faust-rs, W1+W2 | 117,200 / 37,300–39,900 | 88,500 / 28,200–31,400 | ≈ 9,000 |
| em++ | 106,200 / 27,700–27,900 | 82,000 / 25,700–28,600 | ≤ 2,000 |

Without the arms, W1 alone matches em++. Both programs execute 24k to 29k
more instructions per sample when the arms are present: spill code on the main
path. Only faust-rs's extra code is expensive. Marking all 169 `if`s unlikely
(`metadata.code.branch_hint`, behind `--experimental-wasm-branch-hinting` in
V8 12.4) changes neither the instruction count nor the cycles.

**Ruled out:**

- Math imports: 2 `_tanh` calls per sample in steady state.
- Alignment hints: `align=4` on `f64` accesses, like Faust C++; V8 ignores
  them.
- The number of locals: `wasm-opt` removes 97 % of them with no effect.

## 4. Plan

**W1. Fold struct addresses into `offset=`** (expected −23 % to −45 % cycles;
low risk).

- With internal memory, the base becomes `i32.const 0`, as in Faust C++ (or
  `local.get 0`, see D1). With external memory (`wasm-ffi`'s
  `memory=external`, or soundfiles), it stays `local.get 0`.
- The field offset plus `constant index × size` goes into the memarg
  `offset`.
- For a dynamic index, the base is `index << log2(size)`, plus `local.get 0`
  with external memory.
- The change covers every function lowered by `lower_function_subset`
  (`compute`, `instanceClear`, `instanceConstants`,
  `instanceResetUserInterface`, table fills) and the compute-local arrays
  placed in memory.
- `getParamValue`/`setParamValue` keep `dsp + index`, as in C++. Soundfile
  pointer loads are unchanged.
- The JSON, the memory layout and the exports do not change.
- Semantics are the same for every in-bounds access. With the absolute form,
  a host passing `dsp != 0` to an internal-memory module gets the C++
  behavior: the pointer is ignored.
- Tests:
  - the instruction-shape assertions in `crates/codegen/src/backends/wasm/tests.rs`;
  - a structural test that no `i32.add(local.get 0, i32.const)` address is
    left in `compute`, for both memory modes;
  - an external-memory execution test with `dsp != 0`;
  - `make wasm` in `tests/impulse-tests` (133/133);
  - a base-vs-new run of every `.dsp` in `tests/` and `examples/`, comparing
    outputs, not bytes.
- Registry: a line in `faust-rs-vs-faust-cpp-differences-en.md` if D1 picks a
  behavior that differs from C++.

**W2. Sink reads and pure temporaries to their first use, within the
straight-line region of the sample loop** (expected −18 % on resonator banks,
0 to −6 % on coopiano; medium risk).

- A candidate is a `DeclareVar`/`StoreVar` whose value has no call, no store
  and no local write, assigned once in the loop.
- It moves past a statement only if that statement cannot write what it reads:
  - struct fields are disjoint intervals of the layout;
  - a dynamic-index store kills only its own array's interval;
  - audio-buffer stores never alias the struct, because the audio heap starts
    at JSON `size`;
  - math imports are not barriers, `ffunction` calls are;
  - nested `if`/loops are summarized by the stores and local writes they
    contain.
- Moving a memory read past a write is the one change here that can
  miscompile silently. Following the phase methodology, it needs:
  - a short design note;
  - an independent checker that re-derives, for every moved read, that no
    write in between overlaps it;
  - mutations the checker must reject;
  - a qualification run: impulse tests, plus a bit-exact comparison of each
    DSP with and without W2.
- Where it lives is D2.
- Its compile cost must pass `compile-budget-check`, with a sorted
  store-position index per address, as in the measured prototype.

**W3. Forward stored struct scalars to later reads in the same region**
(expected −2 to −3 %). This reuses W2's alias model and checker, so it is done
together with W2 or right after it.

**W4. Investigate the cold-arm cost** (no code change before a cause is
established). Directions to test, in order:

1. TurboFan's code at an arm boundary in both programs. The instance and
   memory-base reloads after imported-JS calls are a suspect. Integer loads
   from non-stack memory, mostly those reloads, number 29,100 in faust-rs's
   whole `compute` against 5,800 in em++'s.
2. Outlining cold arms into separate wasm functions.
3. Branch hints in a current Chrome V8.
4. Hoisting the control-change recomputation out of the sample loop. This is
   a FIR change that affects every backend, so it needs its own plan.

Also measure in Chrome and Safari, where the AudioWorklet runs.

Order: W1, then the W2 design note, then W2 and W3, then W4.

## 5. Validation common to W1–W3

- Bit-identical outputs. The prototypes in section 2 match em++ and faust-rs
  checksums exactly.
- The wasm impulse tests, and an `opt_level` differential where the emitter
  has one.
- A wasm speed check in the repository. A node script times a test-local
  resonator bank, written inline like `bank.dsp`, and reports instructions
  retired where available. It is a developer tool, not a CI gate, because CI
  cycle counts are not stable.

## 6. Decisions for the user

- **D1, base for internal memory.**
  - Absolute `i32.const 0` (Faust C++ parity; `dsp` is ignored).
  - `local.get 0` + `offset=` (today's behavior for `dsp != 0`; one code path
    for both memory modes).

  The measured speed is the same: +3 % instructions with `local.get 0`,
  cycles within noise. I recommend `local.get 0` + `offset=`, since it changes
  no observable behavior. The absolute form is the choice if C++ codegen
  shape matters more.
- **D2, where W2/W3 live.** In the wasm emitter: no change to other backends'
  output and no golden churn. Or as a FIR pass shared with Cranelift (and
  interp), which would also change the C/C++ text. I recommend the wasm
  emitter first, and moving it to FIR only if a Cranelift measurement shows
  the same gain.
- **D3, scope now.** W1 alone already brings the imaged body to em++'s level
  (52 % vs 49 %) and the voice to it. W2+W3 add a few percent on coopiano and
  their checker is real work. W1 now and W2/W3 as a separate phase is the
  smaller step.

## 7. Reproducing

- `make web` in faust-coopiano builds the three modules.
- `node --no-liftoff` with the timing loop of `scripts/web_check.mjs` gives
  the steady-state percentages.
- `node --no-liftoff --no-wasm-lazy-compilation --print-wasm-code` dumps the
  TurboFan code.
- `/usr/bin/time -l node --no-liftoff …` on N and 0 blocks gives instructions
  and cycles.
- `wasm-opt --instrument-memory` with counting `load_ptr`/`store_ptr` imports
  gives dynamic loads and stores.
- The em++ bench wraps `-lang cpp -double` output with exported
  `setup`/`run`/`chan` functions, built with
  `EMSDK_PYTHON=/opt/local/bin/python3.13 em++ -O3 -sMODULARIZE -sEXPORT_ES6`.
- The W1–W3 prototypes are a throwaway Python rewrite of the binaryen WAT of
  `compute`, reassembled with `wasm-as`:
  - fold `i32.mul` of constants and `(dsp + K) + C`;
  - rebase memory operations to `offset=`;
  - sink single-assignment `local.set`s whose value is pure, checking store
    addresses in between;
  - forward stored locals to later loads of the same constant address.

  The prototype is not part of the repository. It is evidence for the
  expected gains, not a design for the implementation.
