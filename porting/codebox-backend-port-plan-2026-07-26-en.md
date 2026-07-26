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
- **`@param` range literals are formatted inconsistently, on purpose.** Sliders
  and numeric entries go through `checkReal()` and print as `0.0f` / `1.0f`,
  while buttons and checkboxes have their range hardcoded in
  `CodeboxParamsVisitor` and print as `0.` / `1.`. Verified on the reference:

  ```
  @param({min: 0.0f, max: 1.0f}) RB_hslider_gain = 0.5f;
  @param({min: 0., max: 1.}) RB_checkbox_on = 0.;
  @param({min: 0., max: 1.}) RB_button_trig = 0.;
  ```

  Reproduce the asymmetry rather than normalising it: it costs nothing and it
  keeps a by-eye comparison against the reference readable. (Not a gate — see the
  correction in §5.2.)

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
body shape, but its `compile_options` provenance string will differ from the C++
one. Since §5.2's correction removes byte parity as a goal, that divergence is
acceptable and needs no flag filtering — just keep the header line out of the
snapshot's normalisation-sensitive part.

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
  to reject earlier, and record the choice. Nothing will decide it for us: the
  snapshot records whatever we emit, so this one needs an explicit call.
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

Hand-off point: C0–C5 plus §5.2 layer 1 are the deliverable. The manual RNBO
validation in the C++ architecture (§5.2 layer 3) happens after that, run by
Stéphane, and its outcome is journalled whether it succeeds or not.

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

Verification: the snapshot of §5.2 layer 1 recorded for a handful of DSPs
(`process = _;`, a recursion, a multi-channel case, a table read), plus a
by-eye comparison against `faust -lang codebox` on the same DSPs to confirm the
*syntax* matches — not the structure, which legitimately differs (see the
correction note in §5.2). Numeric verification arrives with layer 2.

### C2 — Params, control, update (including `codebox-test`)

`@param` declarations from the UI nodes via C0's shortnames, `control()` from
the externalizable statements, `update()` with the dirty-check protocol.

**Both label conventions ship here**: plain `codebox` (digit-initial labels get
`cb_`) and `codebox-test` (`RB_<widget>_` prefixes). The latter is what makes the
manual RNBO validation of §5.2 layer 3 possible at all — `rnbo-dsp.h` recovers
the Faust UI from those prefixes and from nothing else — so it is part of this
phase's deliverable, not a follow-up.

Verification: snapshot extended to DSPs with sliders, buttons, checkboxes and
numeric entries, including labels that need `cb_` prefixing and labels that
collide. Parameter *names* can be compared against the C++ reference as a real
equality check — they come from the shared shortname algorithm of C0, which is
`preserved`, unlike the surrounding code structure.

### C3 — Bargraphs as extra audio outputs

Collect `fHbargraph*` / `fVbargraph*` declarations, append them to `compute`'s
return list, and extend the top-level `outN` wiring.

Verification: a DSP with two bargraphs must emit `numOutputs + 2` outputs, in
bargraph declaration order. The output *count* and *order* are comparable
against the C++ reference; the surrounding code is not.

### C4 — Language quirks

`trunc()` on integer casts, `imod`/`iadd`/`imul` for int32 arithmetic, full
parenthesisation, `safemod` for `fmod`, `samplerate()` for `sample_rate`, and
the math-name mapping table (`gPolyMathLibTable`).

Verification: a DSP exercising each quirk, plus a **rejecting mutation** per
quirk — removing `trunc` must make a *numeric* test fail. This is why layer 2
is not optional: a snapshot notices that the text changed, which is exactly what
a deliberate change also does, so it cannot arbitrate. `trunc` versus `floor`
differs only on negative values, so the mutation needs a DSP that produces
them.

### C5 — CLI and capability wiring — **done**

`CliLang::Codebox` and `CliLang::CodeboxTest` (two `-lang` values, as in C++
Faust, not one value plus a modifier flag), the `BACKEND_CAPS` row in
`crates/compiler/src/execution.rs`, the six facade entry points in
`crates/compiler/src/emitters.rs`, and the CLI transcript snapshot regenerated
(148 modes).

#### The capability decision, and what was chosen

The open question was: codebox *requires* external control and one-sample and
*forbids* vector mode, which is not the shape of a table where `-ec` and `-os`
are opt-in per backend.

**No "forced" state was added.** `ExecutionCapability::Intrinsic` already meant
exactly this — "the backend's native contract already has a tick/control split;
the flag is accepted as an output-invariant compatibility alias" — and codebox
is the case it was written for. RNBO calls the generated code once per sample
and sets controls through `@param` identifiers, so both modes are what codebox
*is*, not modes it can be put into. So:

- both dimensions are `Intrinsic` in the row;
- `lower_signals_to_codebox` **forces** `ControlRateMode::External` and
  `ProcessingApi::OneSample` into the lowering context before validating, which
  is what makes the "output-invariant" half of `Intrinsic` true rather than
  merely asserted. This is the one dispatcher that overrides its caller instead
  of only validating, and it is documented as such at the call site.

`-vec` could not be absorbed the same way, so the table grew a `vector` column.
Reusing `OneSampleWithVectorMode` was rejected: its message blames `-os`, which
a caller writing `-lang codebox -vec` never typed. The new
`FRS-EXEC-VEC-BACKEND` names the backend instead. Every pre-existing row is
`Explicit`, which preserves their behaviour exactly and claims nothing about
whether their emitter has a certified vector lane.

Two consequences worth recording:

- `validate_execution_options` now checks the vector column **before** the
  `-ec`/`-os` early return, since a backend can reject `-vec` with neither flag
  in play. The CLI guard in `validate_cli_arguments` was widened to `cli.vec`
  for the same reason.
- `codebox-test` must resolve to the `codebox` capability row, so
  `cli_backend_id` was split from `cli_lang_name`. Looking a row up under the
  `-test` spelling fails closed and would reject a valid command line — a
  transcript run (`ec_codebox_test`) pins this.

#### Rejecting mutations

| Mutation | Rejected by |
| --- | --- |
| drop the two forcing assignments in `lower_signals_to_codebox` | `the_facade_forces_the_execution_modes_codebox_imposes` + the precision test |
| codebox row `vector: Unsupported` → `Explicit` | the execution unit test + `the_facade_rejects_vector_mode_by_name` |
| `cli_backend_id` no longer maps `CodeboxTest` → `codebox` | 3 CLI transcripts (`*_ec_codebox_test`) |

#### Two help-text tests were repaired, not weakened

Documenting a `CliLang` variant makes clap switch `--lang` from a one-line
`[possible values: …]` to a bulleted list with per-variant help. Two tests
parsed the one-line form:

- the CLI unit test now reads `CliLang::value_variants()` directly, which is
  the actual contract (the accepted tokens), not clap's layout;
- `backend_class_name_contract` still has to parse help — it runs the binary —
  so it now anchors on the `--lang` section and handles both renderings. Its
  previous version searched the whole help for the first `possible values: `
  and silently picked up `--error-format`'s `human, json`. Its own
  `values.len() >= 5` assertion is what caught this.

Codebox itself opts out of the class-name contract by construction: a codebox
file is flat and declares no class, so `-cn` has nothing to name.

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

> **Correction, 2026-07-26.** The first version of this section made a textual
> differential against the C++ compiler the primary oracle. That was wrong, and
> it was wrong because of an assumption I never checked: that our FIR lowering
> produces the same structure as the C++ one. It does not, and it does not have
> to — the contract with C++ Faust has always been *numerical* equivalence.
>
> Measured on `process = + ~ *(0.5);`, C++ lowers the recursion to a two-element
> shift buffer while faust-rs uses a single scalar:
>
> | | emitted loop body |
> |---|---|
> | C++ | `fVec0SE[0] = fTemp0SE; … fVec0SE[1] = fVec0SE[0];` |
> | faust-rs | `fRec36 = fRecCur36;` |
>
> And the divergence is not confined to state. On `(_,_:+),(_,_:*)`, which has
> none, C++ emits `static_cast<float>(x)` where we emit `((float)(x))`.
>
> So byte parity with the reference is unreachable, and the layers below are
> renumbered accordingly: the snapshot cannot prove correctness, only stability,
> and the numeric evaluator is the only thing that can prove correctness.

**Layer 1 — self-snapshot of our own output (regression detection only).**
Record faust-rs's own codebox output for a fixed DSP corpus and fail when it
changes unexpectedly, in the shape of the existing `cli-transcript-check`:

```
cargo run -p xtask -- codebox-snapshot-gen     # record OUR output
cargo run -p xtask -- codebox-snapshot-check   # detect changes
```

Be precise about what this buys and what it does not. It catches "a refactor
changed the emitted text", which is genuinely useful during C2–C4. It cannot
catch "the emitted text is wrong": a snapshot of a wrong output is a green
snapshot. Never regenerate it to make a failure go away without explaining the
diff.

The C++ reference output stays useful, but as a **specification read by a human,
not a target compared by a machine**: it is where the syntax of `@state`,
`@param`, `iadd`/`imod`, `trunc`, and the literal formatting come from (§1, §7).
Spot-comparing our output against it by eye during C1–C4 is worth doing; wiring
that comparison into a gate is not.

Traps to build into the snapshot anyway, learned from the impulse-test work:
- keep DSP inputs at a fixed path (the source name reaches the output);
- treat "no snapshot recorded" as a failure, not a skip;
- normalise only our own version line.

**Layer 2 — a codebox interpreter for numeric testing (PRIMARY oracle).**
This is now the only layer that can show the backend is *correct* rather than
merely unchanged. The subset codebox uses
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

**Layer 3 — RNBO round-trip in the C++ architecture (manual, by the project
owner).** This layer is *not* mine to run: it needs Max/RNBO. The agreed
sequencing is therefore:

1. I deliver phases C0–C5, the layer 1 snapshot, and — since it is now the only
   correctness oracle — layer 2.
2. Stéphane then attempts the manual validation in the C++ architecture: import
   the faust-rs-generated codebox into an RNBO patch, export `rnbo_source.cpp`,
   compile it against `architecture/faust/dsp/rnbo-dsp.h`, and run it.
3. The outcome is recorded in the journal either way. It is an *attempt*: a
   failure is information — it would mean our text passes the differential while
   RNBO still rejects or mis-executes it, which points at something the C++
   reference output does implicitly and we reproduce only in appearance.

**What the port must therefore deliver for step 2 to be possible at all**, and
this promotes one item from "nice to have" to a hard requirement:

- **`-lang codebox-test` must work, not just `-lang codebox`.** `rnbo-dsp.h`
  decodes RNBO parameters back into a Faust `UI` purely by prefix matching on
  `RB_button_`, `RB_checkbox_`, `RB_hslider_`, `RB_vslider_`, `RB_nentry_`,
  `RB_hbargraph_`, `RB_vbargraph_` (rnbo-dsp.h:120–170). Without the
  `codebox-test` label convention there is no way for the wrapper to see any
  control, so the manual run would produce a DSP with zero parameters and prove
  nothing. C2 owns this, and it is not optional.
- **Bargraphs need patch-level wiring that the codebox output cannot provide.**
  Verified on the reference: with `-lang codebox-test`, a `vbargraph` produces
  *no* `@param` at all — it appears only in `return [output0_cb, fVbargraph0_cb]`
  and as an extra `out2 = outputs[1]`. But `rnbo-dsp.h` discovers bargraphs by
  looking for parameters named `RB_vbargraph_*` / `RB_hbargraph_*`
  (rnbo-dsp.h:158–168). Nothing emits those.

  The missing link is in the RNBO *patch*, as the C++ header comment says: the
  extra audio outputs must be sampled with `snapshot~` + `change` and connected
  to `param` objects carrying those names. So step 2 requires building that
  wiring by hand, and its ordering — extra channels come after the real outputs,
  in bargraph declaration order — is the part to get right. A permutation there
  is invisible to the snapshot and shows up as correct-looking values on the
  wrong channels — only the numeric layer or your manual run can catch it.

  For a first attempt it is simpler to validate a DSP **without** bargraphs, and
  treat bargraph round-tripping as a separate exercise.
- **The emitted file should be usable as-is**: no faust-rs-specific header line
  or trailing content that RNBO's codebox parser would reject. Worth checking
  against the reference's exact header shape (§7).

Practical form for step 2, so it is reproducible rather than folkloric:

```bash
# 1. generate, with the test label convention
faust-rs -lang codebox-test -o mydsp.codebox mydsp.dsp
# 2. import mydsp.codebox into a codebox~ object in an RNBO patch, export C++
#    (Max/RNBO, manual) -> rnbo_source.cpp + rnbo/ headers
# 3. build against the Faust wrapper and the impulse architecture
c++ -O3 -I<rnbo-export-dir> -I<faust-arch> \
    -DFAUSTFLOAT=double impulse_rnbo.cpp -o mydsp_rnbo
# 4. compare against the same oracle every other backend uses
./mydsp_rnbo -n 15000 > mydsp.ir
tools/filesCompare mydsp.ir reference/mydsp.ir -part 2e-06
```

Note that `rnbo-dsp.h` implements `dsp::compute(count, inputs, outputs)` — a
block interface — so it plugs into the ordinary impulse architecture rather than
needing the one-sample driver, even though the codebox body is per-sample. RNBO
does the block loop.

Because this is a one-off attestation and not a gate, it does not remove the
need for Layer 2: only an automated numeric check catches a *future* regression.
If Layer 2 is skipped, the honest statement is that codebox is guarded by text
parity plus a dated manual attestation, and that a later change can break the
numbers without any test noticing.

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

## 7. Reference output, as a language specification

Read as *syntax to reproduce*, not as text to match: per the correction in §5.2,
our lowering legitimately differs in structure, so the value here is the shape of
the constructs, not the exact statements.

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
