# Codebox backend port plan — 2026-07-26

Port the C++ Faust `codebox` backend (`-lang codebox`), the RNBO/`gen~` target
used to import Faust DSP into a Cycling '74 RNBO patch.

Sources analysed, all under `compiler/generator/codebox/` in the C++ tree:

| file | lines | role |
|---|---|---|
| `codebox_instructions.hh` | 589 | the emitter: one `TextInstVisitor` subclass plus five small helper visitors |
| `codebox_code_container.cpp` | 343 | module assembly: section order, `dspsetup`/`control`/`update`/`compute` |
| `codebox_code_container.hh` | 77 | container declarations, scalar-only |

Plus the C++ test path: `architecture/faust/dsp/rnbo-dsp.h` (a `dsp` subclass
wrapping an RNBO `CoreObject`) and the `codebox-test` output-language variant.

## 1. What the target language forces

Codebox is the textual language of RNBO's `codebox~` object. Six constraints
shape the whole backend, and each one is a place where a Rust port can silently
drift:

1. **Identifiers cannot end with a digit.** Every emitted name gets a `_cb`
   suffix (`codeboxVarName`). Applies to variables *and* array names, not to
   function names.
2. **Storage classes are syntactic.** `@state x : Type = 0;` for values that
   persist (FIR `Struct` / `StaticStruct` access), `let x : Type = …;` for stack
   and loop scope. A `@state` of basic type *must* carry an initializer — hence
   the `= 0` fallback in `visit(DeclareVarInst*)`.

   Arrays are different again, and the difference is easy to miss: they are
   declared **without a type annotation** and constructed, as
   `@state fVec0SE_cb = new FixedFloatArray(2);`, then filled element by element
   in `dspsetup` by `CodeboxInitArraysVisitor`. Observed on the reference output;
   the type-manager class (`CodeboxStringTypeManager`) is what produces that
   form.
3. **Parameters are declarations, not zones.** `@param({min: …, max: …}) name =
   init;`. There is no pointer-to-zone model: the host writes a parameter, and
   the DSP reads the declared name. This is why the backend needs the *shortname*
   machinery — the parameter identifier is derived from the widget label, not
   from the FIR variable name.
4. **Bargraphs cannot be read back as control.** They are emitted as
   **additional audio outputs** appended after the real ones, to be sampled
   host-side with `snapshot~`/`change`. So `compute` returns
   `numOutputs + numBargraphs` values.
5. **Sample-at-a-time only.** `compute(i0, …)` takes one sample per input and
   returns a list. There is no block loop, no `count`. Scalar mode only: vector,
   scheduler, OpenMP, OpenCL and CUDA all raise an error in
   `createContainer`.
6. **No soundfile support.** `visit(AddSoundfileInst*)` throws. This is a real
   gap upstream, flagged as `TODO` in the C++ header.

Two further language quirks that must be reproduced exactly or the numbers move:

- **`int(x)` truncation.** Codebox's `int()` floors instead of truncating, so
  `CastInst` to an integer type emits `trunc(...)`.
- **Integer arithmetic wraps through helpers.** `kRem`, `kAdd` and `kMul` on two
  int32 operands emit `imod(a, b)`, `iadd(a, b)`, `imul(a, b)` rather than the
  infix operator. Other operators are emitted infix but **fully parenthesised**,
  because codebox precedence is not C's.
- **`fmod` has no direct equivalent**: it maps to `safemod`.
- **`sample_rate` is not a field**: the `NamedAddress` named `sample_rate` emits
  `samplerate()`.

## 2. What faust-rs already has

The port is much smaller than the 1 009 C++ lines suggest, because three things
the C++ backend builds by hand already exist here:

- **The one-sample body.** The C++ container calls
  `fCurLoop->generateOneSample()`. faust-rs has that shape as
  `ProcessingApi::OneSample` (`-os`), landed in the execution-options port: the
  sample body lives in a `frame` entry point and the canonical `compute` is
  emitted empty.
- **The separate control function.** The C++ backend relies on `gExtControl` to
  get a `control()` split. faust-rs has `ControlRateMode::External` (`-ec`),
  and `ModuleSections.control_statements` already carries the
  `Externalizable` / `ComputePreamble` tag that decides what belongs in
  `control`.
- **The FIR UI nodes.** `FirMatch::{OpenBox, CloseBox, AddButton, AddSlider,
  AddBargraph, AddMetaDeclare}` exist with labels, ranges and variable names.

So codebox is, structurally: **a text emitter over a FIR module lowered with
`ControlRateMode::External` + `ProcessingApi::OneSample`**, plus codebox
syntax. That reuse is the single most important decision in this plan — it also
means codebox inherits the `-ec`/`-os` correctness work instead of duplicating
it.

Confirmed by running the pinned C++ compiler (`faust 2.84.3`, which does have
codebox built — checked, since `libcode.cpp:501` shows it is conditionally
compiled): the reported compile options contain **`-ec`** but *not* `-os`. So
external control is genuinely forced, while the one-sample shape comes from
calling `generateOneSample()` directly rather than from the `-os` option. A
faust-rs port that sets `ProcessingApi::OneSample` will therefore reach the same
body, but its `compile_options` provenance string will differ from the C++ one
unless the flag list is filtered — which matters, because that string is part of
the emitted header the text differential compares.

Two pieces genuinely do not exist yet and must be built:

- **Shortnames.** `ShortnameInstVisitor` gives each widget an identifier derived
  from its label, used both for `@param` names and for the `update` argument
  list. `crates/codegen/src/json.rs` has a `shortname` field but it is currently
  just the label (`shortname: label.clone()`, json.rs:859), so it does **not**
  implement the C++ algorithm. Measured on the reference for
  `vgroup("a", hslider("gain", …)) + vgroup("b", hslider("gain", …)) +
  hslider("0freq", …)`:

  | label | path | C++ shortname |
  |---|---|---|
  | `gain` | `/cbs2/a/gain` | `a_gain` |
  | `gain` | `/cbs2/b/gain` | `b_gain` |
  | `0freq` | `/cbs2/0freq` | `0freq` in JSON, **`cb_0freq`** in codebox |

  Three rules to port, and the third is codebox-specific: labels are normalised
  (spaces → `_`), collisions are disambiguated by prefixing enclosing group
  names, and *then* codebox alone prefixes `cb_` when the result starts with a
  digit (`buildButtonLabel`/`buildSliderLabel`). So the shared piece is the
  first two rules — the `cb_` prefix belongs in the backend, not in the shared
  helper.

  This is a shared gap worth fixing once: our JSON `shortname` is wrong today for
  any label needing disambiguation, independently of codebox.

  Also worth knowing before designing the tests: for two widgets whose *paths*
  collide (e.g. labels `my gain` and `my/gain` in the same group, since `/` is a
  path separator) the C++ compiler errors out with
  `ERROR : path '/cbs/my_gain' is already used` on the JSON path — but the
  **codebox backend does not check**, and happily emits two `@param` with the
  same identifier plus `update(…, P3_cbs_my_gain, P3_cbs_my_gain)`. That output
  is not valid codebox. Decide in C2 whether to reproduce it for byte parity or
  to reject earlier, and record the choice; the text differential will otherwise
  make the decision silently.
- **The `codebox-test` label convention.** For testing, labels are prefixed
  `RB_hslider_`, `RB_button_`, `RB_hbargraph_`… so the RNBO wrapper can map
  parameters back onto a Faust UI. Outside test mode, a label starting with a
  digit gets a `cb_` prefix.

## 3. Emitted module shape

The order is fixed by `produceClass` and must be reproduced, because RNBO parses
a flat file where declaration order matters:

```
// header comment: version + compile options
// Params        -> @param(...) <shortname> = init;      (one per widget)
// Globals       -> function declarations only
// Fields        -> @state declarations (+ bargraph vars kept aside)
@state fUpdated : Int = 0;
// Init
function dspsetup() { ... }        // array init, static init, reset UI, clear, constants
// Control
function control() { ... }         // compute block, then iSlow/fSlow declarations
// Update parameters
function update(<shortnames...>) { ... }   // per-param dirty check, then control() if fUpdated
// Compute one frame
function compute(i0, ..., iN) { ... }      // local input/output vars, one-sample body, return [...]
// top level
update(<shortnames...>);
outputs = compute(in1, ..., inN);
out1 = outputs[0]; ... outK = outputs[K-1];
```

Details worth pinning:

- `dspsetup()` is the *only* init entry point: RNBO calls it on start and on
  sample-rate change. It folds `classInit`, `instanceResetUserInterface`,
  `instanceClear` and `instanceConstants`, in that order, with array
  initialisation first.
- `update` sets `fUpdated` if any parameter changed, then calls `control()`
  once. The dirty-check line is exactly
  `fUpdated = int(fUpdated) | (p != p_cb); p_cb = p;`.
- `inputN` / `outputN` FIR declarations are **skipped** in the field pass and
  re-emitted as `let` locals at the top of `compute`.
- Sub-containers are inlined (`mergeSubContainers`, `produceInternal` empty);
  `inlineSubcontainersFunCalls` is applied to the static-init and init blocks.

## 4. Phases

Each phase is one commit, green on its own, with an English journal entry.

### C0 — Shared shortname support

Implement the C++ `ShortnameInstVisitor` algorithm once, in `crates/codegen`,
and use it both for the JSON `shortname` field (currently just the label) and
for codebox `@param` names. Independent value: it fixes the JSON field for
labels that need disambiguation, which is observable today.

Verification: unit tests over colliding labels, and a golden JSON diff showing
the field changing only where disambiguation is required.

### C1 — Emitter skeleton, no UI

`generate_codebox_module(store, module, &CodeboxOptions) -> Result<String, _>`
in `crates/codegen/src/backends/codebox/`, following `backends/asc/mod.rs` as
the structural template (options struct, `CodegenError`, `decode_module`,
per-statement and per-value emitters). Covers: header, `@state`/`let`
declarations with `_cb` suffix, `dspsetup`, `compute` with the one-sample body,
the top-level wiring. Rejects soundfiles with a typed error, and rejects vector
mode.

Verification: emitted text for a handful of DSPs (`process = _;`, a one-pole
filter, a table read) compared against `faust -lang codebox` output from the
pinned C++ reference, normalised for the version line.

### C2 — Params, control, update

`@param` declarations from the UI nodes via C0's shortnames, `control()` from
the externalizable statements, `update()` with the dirty-check protocol.

Verification: same textual differential, on DSPs with sliders, buttons,
checkboxes and numeric entries, including labels that need `cb_` prefixing and
labels that collide.

### C3 — Bargraphs as extra audio outputs

Collect `fHbargraph*` / `fVbargraph*` declarations, append them to `compute`'s
return list, and extend the top-level `outN` wiring.

Verification: a DSP with two bargraphs must emit `numOutputs + 2` outputs, and
the textual differential must match the reference.

### C4 — Language quirks

`trunc()` on integer casts, `imod`/`iadd`/`imul` for int32 arithmetic, full
parenthesisation, `safemod` for `fmod`, `samplerate()` for `sample_rate`, and
the math-name mapping table (`gPolyMathLibTable`).

Verification: a DSP exercising each quirk, plus a **rejecting mutation** per
quirk: removing `trunc` must make the numeric test below fail, not just the
text differential. A quirk that only the text diff catches is a quirk nobody
will notice when the reference text changes shape.

### C5 — CLI and capability wiring

`CliLang::Codebox` (`-lang codebox`, alias `codebox-test`), the
`BACKEND_CAPS` row in `crates/compiler/src/execution.rs`, the facade entry
points in `crates/compiler/src/emitters.rs` following the established six-point
convention, and the CLI transcript snapshot regenerated.

Note on capabilities: codebox *requires* external control and one-sample and
*forbids* vector mode. That is not the current shape of the table, where `-ec`
and `-os` are opt-in per backend. Either the table grows a "forced" state, or
the codebox lowering path sets both modes itself and rejects a conflicting
request. Decide in C5 and record which, because a silently ignored `-vec` here
would emit plausible-looking wrong code.

## 5. How to test the backend

This is the part with a real obstacle, and it should be settled before C1.

### 5.1 The C++ reference path, and why it is not enough

The C++ suite tests codebox through `codebox-test` + `rnbo-dsp.h`:

1. compile Faust → codebox with `-lang codebox-test` (labels prefixed `RB_*`);
2. import the codebox source into an RNBO patch and **export C++**, producing
   `rnbo_source.cpp`;
3. compile that against `rnbo-dsp.h`, which wraps an RNBO `CoreObject`, decodes
   parameters by their `RB_*` prefix into a Faust `UI`, and implements
   `dsp::compute`;
4. run it through the ordinary impulse architecture.

Step 2 needs RNBO's export tooling — a proprietary Cycling '74 toolchain, not
scriptable in CI and not present in this repo. `find` over the C++ tree confirms
there is no vendored RNBO SDK and no codebox impulse target: the only in-tree
codebox test upstream is `tests/compile-tests` (`Make.lang outdir=codebox`),
which checks that compilation **succeeds**, not that the output is correct.

So: upstream itself does not numerically test codebox in CI. Any claim that this
port is "validated like cpp/c" would be false.

### 5.2 Three layers this port can actually stand on

**Layer 1 — textual differential against the pinned C++ compiler (primary).**
For a fixed DSP corpus, `faust -lang codebox` and `faust-rs -lang codebox` must
produce the same text, modulo the version/options header line. This is a strong
oracle: codebox output is deterministic text, and the C++ backend is the
specification. Mechanise it as an `xtask` command in the shape of the existing
`cli-transcript-check`:

```
cargo run -p xtask -- codebox-diff-gen     # record reference output
cargo run -p xtask -- codebox-diff-check   # compare
```

Traps to build in from the start, learned from the impulse-test work:
- normalise only the version and compile-options lines, nothing else;
- keep DSP inputs at a fixed path (the source name reaches the output);
- treat "no reference recorded" as a failure, not a skip.

**Layer 2 — a codebox interpreter for numeric testing (recommended).**
Text equality proves we emit what C++ emits; it does not prove either is
*right*, and it breaks on any upstream cosmetic change. The subset codebox uses
is small — `@state`/`let` declarations, assignments, `if`, `for`, arrays, the
math functions of `gPolyMathLibTable`, and function definitions. A few-hundred
line evaluator in `crates/codegen/tests/` or a small `xtask` can execute the
emitted `compute` sample by sample and compare against the **existing
`tests/impulse-tests` reference `.ir`**, on the scalar prefix, exactly as the
interpreter and WASM targets do.

This is the layer that makes C4's quirks testable numerically: `trunc` vs
`floor` is invisible in a text diff once both sides emit the same wrong thing,
but a numeric run on a DSP with a negative integer cast catches it.

Cost estimate: the evaluator is comparable to the `-ec`/`-os` impulse driver
(~200–400 lines), and it needs no proprietary tooling. Its own faithfulness is
checkable: run it on the **C++ compiler's** codebox output for the same DSPs and
require the same numbers.

**Layer 3 — RNBO round-trip (manual, documented).**
For a handful of DSPs, do the full RNBO export by hand once, run it against
`rnbo-dsp.h`, and record the outcome in the journal. Not in CI, and honestly
labelled as a one-off attestation rather than a gate.

### 5.3 What must not be claimed

- Do not add a `make codebox` impulse target that silently only checks
  compilation; a target named like the others implies the same oracle.
- Do not regenerate a codebox reference from faust-rs itself. The oracle is the
  C++ compiler (layer 1) or the reference `.ir` (layer 2), never our own output.
- If layer 2 is skipped, say so in the README: the backend is then validated for
  *text parity with C++ Faust* and for *compilation*, not for numeric
  behaviour.

## 6. Out of scope

- Soundfile support (`AddSoundfile` throws upstream too; keep the same typed
  rejection and record it as a shared gap).
- Vector, scheduler, OpenMP, OpenCL, CUDA modes — all rejected upstream.
- MIDI and polyphony: upstream handles them in the RNBO patch, not in the
  emitted codebox, so there is nothing to port.
- `-double` is **accepted**, not rejected — checked against the pinned compiler.
  Codebox has a single `number` type, so the emitted types are unchanged; what
  changes is the float-literal suffix. Under `-single` the reference emits
  `0.5f` / `0.0f` (in `@param` ranges and in array initialisers alike); under
  `-double` it emits `0.5` / `0.0`. Both must be reproduced, and the text
  differential must cover both precisions or half the literal formatting goes
  unchecked.

## 7. Reference output, for anchoring

`faust 2.84.3 -lang codebox` on
`process = _ * hslider("gain", 0.5, 0, 1, 0.01) : + ~ *(0.5);` — the shape C1–C4
must reproduce:

```
// Code generated with Faust version 2.84.3
// Compilation options: -lang codebox ... -ec ... -single -ftz 0
// Additional functions
// Params
@param({min: 0.0f, max: 1.0f}) gain = 0.5f;
// Globals
// Fields
@state fHslider0_cb : number = 0;
@state fSlow0BE_cb : number = 0;
@state IOTA0_cb : Int = 0;
// Recursion delay fRec0SE is of type kZeroDelay
// While its definition is of type kZeroDelay
// Ring Delay
@state fVec0SE_cb = new FixedFloatArray(2);
@state fSampleRate_cb : Int = 0;
@state fUpdated : Int = 0;
// Init
function dspsetup() {
	fUpdated = true;
	fHslider0_cb = 0.5f;
	IOTA0_cb = 0;
	for (let l0_cb : Int = 0; (l0_cb < 2); l0_cb = iadd(l0_cb, 1)) {
		fVec0SE_cb[l0_cb] = 0.0f;
	}
	fSampleRate_cb = samplerate();
}
// Control
function control() {
	fSlow0BE_cb = fHslider0_cb;
}
// Update parameters
function update(gain) {
	fUpdated = int(fUpdated) | (gain != fHslider0_cb); fHslider0_cb = gain;
	if (fUpdated) { fUpdated = false; control(); }
}
// Compute one frame
function compute(i0) {
	let input0_cb : number = i0;
	let output0_cb : number = 0;
	let fRec0SE_cb : number = ((0.5f * fVec0SE_cb[((IOTA0_cb - 1) & 1)]) + (fSlow0BE_cb * input0_cb));
	let fTemp0SE_cb : number = fRec0SE_cb;
	fVec0SE_cb[(IOTA0_cb & 1)] = fTemp0SE_cb;
	output0_cb = fVec0SE_cb[(IOTA0_cb & 1)];
	IOTA0_cb = iadd(IOTA0_cb, 1);
	return [output0_cb];
}
// Update parameters
update(gain);
// Compute one frame
outputs = compute(in1);
// Write the outputs: audio ones and bargraph as additional audio signals
out1 = outputs[0];
```

Six things to read off it, each a trap:

- **Loop counters are `let … : Int` and increment with `iadd`**, not `++` or
  `+ 1` — the int-helper rule reaches the `for` header, not just expressions.
- **`&` stays infix** while `+`/`*`/`%` on int32 become helper calls: the helper
  rule is per-opcode (`kRem`, `kAdd`, `kMul`), not "all integer arithmetic".
- **Delay-strategy comments are emitted** (`// Recursion delay … kZeroDelay`,
  `// Ring Delay`). They are part of the text a differential compares, so the
  port has to emit them too — or the differential needs to strip them, which
  weakens it.
- **`fSampleRate_cb = samplerate();`** appears in `dspsetup`, from the
  `sample_rate` rename rule.
- **`fUpdated` is declared `Int` but assigned `true`** in `dspsetup`, and read
  through `int(fUpdated)` in `update`. Reproduce it verbatim rather than
  normalising the type; codebox tolerates it and the reference does it.
- The **`update(gain)` / `outputs = compute(in1)` top level** is emitted after
  the functions, at file scope. It is not inside any function.
