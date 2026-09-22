# Control inputs as first-class boxes, and a wildcard modulation target: towards a generic adaptive operator

**Date:** 2026-09-22
**Status:** analysis and contract (no implementation yet)
**Scope:** four box-level primitives, `cinputs(e)`, `cinput(i, e)`, `coutputs(e)`, `coutput(i, e)`, and one extension of widget modulation, the target `"*"`, so that `optimizers.lib` can make an arbitrary Faust program learn its own sliders without the program being rewritten. Measured on the six faust-diff projects and on faust-rs `main-dev` at 7222dcc6.

---

## 1. Where the question comes from

Six projects (faust-diff-amp, -ts808, -rir, -jot, -demo, -ampmodeler) made a published or hand-written DSP model learn its parameters with `fad` and `rad`. In every one, the first step was the same and mechanical: rewrite the program as a function of a list `P` of its parameters in place of its sliders, then deduce a bounded learning space from the sliders' ranges, then differentiate (`fad(loss, P)` for the host, `descend_N_fad_clocked` in the program). The last project measured the cost of that step: `ampmodeler.lib`, 300 lines, for a 177-line program, equal to it to the bit. Nothing in that step needs to understand the program: the information is in the widgets, path, default, range, step, metadata.

Two things were then verified on faust-rs:

- **Widget modulation already rebinds a program's sliders to arbitrary signals.** `P, x : ["gain": (!, _), "bias": (!, _) -> e]` replaces each named widget by an input, and `fad` through the result gives the same lanes as the hand-written library (1e-13 on 24 lanes of `ampmodeler.dsp`, the rounding of a control-rate expression against a sample-rate one). The C++ compiler 2.88.1 does the same. One trap: `["a", "b": m -> e]` attaches `m` to `b` only, `a` gets the default `*`; a modulator per target is needed.
- **`optimizers.lib` 0.10.0 takes a rate, a range and a start per parameter** in its bus loops, as lists (`descend_N_fad_clocked(N, clock, loss, (upd...), (lo...), (hi...), (init...), reset)`), so a descent can work in the sliders' own units.

What remains is enumeration: the modulation needs the labels written by hand, and the bounds need the ranges copied by hand. The user's proposal, `cinputs(e)` and `coutputs(e)` after `inputs(e)` and `outputs(e)`, and a wildcard target rather than a new `rebind` primitive, is what this document specifies.

## 2. What exists, precisely

### 2.1 `inputs(e)` and `outputs(e)`

`crates/eval/src/lib.rs`, arms `BoxMatch::Inputs` / `BoxMatch::Outputs`: the inner box is evaluated, lowered by `a2sb`, its arity inferred by `infer_box_arity_cached`, and the result is `boxInt(n)`, usable as an iteration count. C++ does the same (`isBoxInputs` in `eval.cpp`). The new primitives follow this arm exactly, with a widget walk in place of the arity inference.

### 2.2 Widget modulation

`crates/eval/src/modulation.rs`. One modulation node carries one target label and one circuit (multiplication when absent; the parser turns `["a": m1, "b": m2 -> e]` into nested nodes). `eval_modulation` evaluates the label (interpolated, metadata stripped), the circuit (0, 1 or 2 inputs, 1 output), allocates **one** fresh slot when the circuit has two inputs, evaluates and lowers the body, then `implant_modulation` walks the lowered box tree with a group stack and, at every widget whose path matches the target (subsequence matching on the reversed path segments, `widget_matches_modulation_target`), replaces it by `widget : circuit`, `(widget, slot) : circuit` or `circuit`. The slot becomes one extra input, in front, through `b.symbolic(slot, rewritten)`.

Three measured consequences:

| program | result | reading |
|---|---|---|
| `["a": (!, _) -> e]` with `a` used twice | one extra input, both uses replaced | the same widget is one control, as in the UI |
| `["a": (!, _) -> e]` with two widgets named `a`, one in a group, one at the root | 22 = 2·1 + 10·2: **one** extra input feeds both | a label that matches several widgets gives them one shared slot |
| `["*": (!, _) -> e]` | faust-rs: no match, no slot (the sequential composition then fails on arity); C++ 2.88.1: no match but one slot (arity error with one input more) | `*` is free: no program uses it today |

The shared slot is the point the wildcard must not inherit: rebinding a program means one input per widget.

### 2.3 The UI builder

`crates/propagate/src/ui_build.rs` walks the validated flat box DAG after propagation, registers each control once (a widget reachable through several paths is one `ControlSpec`) and places it in the group of its body occurrence; the order is the depth-first order of the box tree with the group stack, which is the order of `buildUserInterface` and of `faustprobe --list-params` before it sorts. This is the order `cinput(i, e)` must use, and the one the wildcard must use for its extra inputs, so that a host and a program agree on what "the i-th control" is.

## 3. The contract

### 3.1 `cinputs(e)`, `cinput(i, e)`

```faust
cinputs(e)       // the number of control inputs of e: its sliders, nentries, buttons and checkboxes, a constant
cinput(i, e)     // the i-th control input of e (0-based), as the list (widget, init, min, max, step)
```

- **What counts.** Every widget that produces a signal: `hslider`, `vslider`, `nentry`, `button`, `checkbox`. Bargraphs do not. A widget reached through several paths of the box DAG is one input (the UI builder's rule); two widgets with the same label in different groups are two.
- **Order.** The UI order: depth-first traversal of the evaluated box tree, groups entered in place. Stable under `component`, `hgroup`, `library` prefixes.
- **What `cinput` returns.** A list of five boxes, so that `ba.take` reaches each: the widget box itself, its default, its minimum, its maximum, its step (a button or a checkbox: 0, 0, 1, 1). The widget box is **the same node** as in `e`: taken as a seed of `fad` or `rad`, it is recognised by identity, as the seed rule requires (`docs/fad-note-en.md` §1). The four numbers are the evaluated, folded constants of the widget (an expression such as `1.0 / base_tube_gain` is a number here, as `--list-params` shows it).
- **Metadata.** The label's metadata is kept on the widget box, not in the list. A sixth entry, the scale (`0` linear, `1` for `[scale:log]`, `2` for `[scale:exp]`), is the one metadata a learning space needs; it is proposed as a sixth entry rather than a separate primitive, and is the only open point of this contract (section 6).
- **When it is evaluated.** At box evaluation, after `eval` and `a2sb` of `e`, exactly as `inputs(e)`; `cinputs` folds to `boxInt(n)`, `cinput` to a `par` of five boxes. `e` must be closed (no free box variables), as `inputs(e)` requires.

With these two, the host-driven calibration of any program is:

```faust
e = component("x.dsp");
U = par(i, cinputs(e), cinput(i, e) : (_, !, !, !, !));      // the widgets, as seeds
process = fad(op.mse(e, target), U);                           // faustprobe --train on their paths
```

and the unit cube, or the sliders' own units, come from the same list.

### 3.2 `coutputs(e)`, `coutput(i, e)`

```faust
coutputs(e)      // the number of bargraphs of e
coutput(i, e)    // the i-th bargraph, as the list (bargraph, min, max)
```

Same order, same evaluation. The bargraph box is the `attach`ed signal's carrier: taking it as a signal reads what the program shows, its loss or its learned values, without a second output. Symmetric with `cinput`; not needed by the descents, needed by a page or a test that reads a program's own meters.

### 3.3 The wildcard target `"*"`

```faust
P, x : ["*": (!, _) -> e]                // e with every control input replaced by an input, in cinput order
["*": *(0.5) -> e]                       // every control input halved
["h:amp/*": (!, _) -> e]                 // every control input under the group `amp`
```

- **Matching.** A target whose last segment is `*` matches every control input whose path has the preceding segments as a subsequence (the existing rule), all of them if there is no preceding segment. Bargraphs are never matched (a modulated bargraph has no meaning). `*` is a whole segment: `"stage*"` is not proposed, the subsequence rule already gives group prefixes, and a glob inside a segment is a second language.
- **One slot per widget.** Unlike a literal label, whose matches share the modulation's one slot (section 2.2, kept as is for compatibility), a wildcard allocates a fresh slot for each matched widget, in UI order, and the extra inputs are prepended in that order: the first control input of `e` is the first input of the modulated block. `cinput(i, e)` and the i-th extra input of `["*": … -> e]` name the same widget by construction, which is what lets a program use both.
- **Arity of the modulator.** As today: 0 inputs replaces every matched widget by the circuit (a constant for all: `["*": 0.5 -> e]`, rarely useful), 1 input transforms each, 2 inputs pairs each with its own extra input. Only the 2-input form adds inputs.
- **No match.** An error, `FRS-EVAL`, "the modulation target `*` matches no control input of the expression", where a literal label that matches nothing is silent today (C++ adds a dangling slot, faust-rs adds none; the wildcard should say so).
- **The widget in the UI.** Under a 2→1 modulator that drops the widget, `(!, _)`, the widget remains in the interface, now reaching nothing, in both compilers today (`ampmodeler.dsp` shows the same for its four dead sliders). This document does not propose to change it: the rebound program's sliders are then the initial values a host could still read, and a pruning of controls whose signal reaches no output is a separate, general pass (an option of the UI builder) that benefits every program. A learner that wants its learned values visible attaches bargraphs, as the Web programs of the projects do.

### 3.4 The generic adaptive operator, in `optimizers.lib`

With 3.1 and 3.3 and the 0.10.0 bus loops, and nothing else:

```faust
//--- `(op.)adaptive_fad`, `(op.)adaptive_rad` ---
// A program learning all its control inputs while it runs: `e` with its
// widgets replaced by parameters descended, every firing of `clock`, on the
// frame mean of the gradients of `loss(model, target)` by `fad`, each
// parameter in its own units, bounded by its widget's range, started at its
// default, stepped by its own engine. Inputs: those of `e`, then the target.
// Outputs: those of `e` on the learned parameters, then the parameters.
adaptive_fad(e, loss, upd, clock, reset, target) = ...
with {
    N = cinputs(e);
    LO = par(i, N, cinput(i, e) : (!, !, _, !, !));
    HI = par(i, N, cinput(i, e) : (!, !, !, _, !));
    INIT = par(i, N, cinput(i, e) : (!, _, !, !, !));
    model(P) = P, si.bus(inputs(e)) : ["*": (!, _) -> e];
    P = descend_N_fad_clocked(N, clock, \(P).(loss(model(P), target)), upd, LO, HI, INIT, reset);
};
```

`upd` is one engine or a list of `N`; the rate in a widget's own units is the caller's, as it should be (0.01 on a gain, 0.5 dB on a master).

**Both modes, both named.** `adaptive_rad` is the same body on `descend_N_rad_clocked`, to a word: `cinput` and the wildcard supply seeds and bounds and do not know which mode consumes them. The pair is named in full, `adaptive_fad` and `adaptive_rad`, with no bare `adaptive`: the library's older loops leave the `fad` form unmarked and suffix the twin (`descend_N_fad`, `descend_N_rad`), an inheritance from `fad` having come first, but this is the entry point a reader meets first and the mode is a choice to make knowingly, not a default with an option; two names at the same rank say so, and a third for the same thing is what to avoid. The older pairs keep their names (renaming them would break the six projects for nothing) and the overview says in one sentence that new functions name both modes. What differs is the library's existing contract. Through a recursion `fad` carries the exact derivative at any block size, where `rad` consumed in the graph sees one sample, the direct term with the past state held fixed (pseudo-linear regression), the frame mean being a mini-batch of those; on a model without recursion between the parameters and the output the two follow the same trajectory (`opt_bus_fad_vs_rad_fir16`). `fad` costs one lane per widget (8.7 preamps for 24 lanes on ampmodeler), `rad` one sweep whatever the count, the choice past a few dozen parameters. `rad` refuses written tables, soundfiles, foreign functions and clock-domain crossings where `fad` emits zero tangents, so `adaptive_rad` says at compile time what `adaptive` would learn around in silence. The clocked descent is not a crossing: only the step is inside the `ondemand` block, the loss and its `rad` run at audio rate, as `descend_N_rad_clocked` already does. Host-driven, the same pair: `fad(loss, U)` or `rad(loss, U)` with `U` from `cinput`.

The five projects, rewritten on it:

| project | what the operator gives | what stays by hand |
|---|---|---|
| ampmodeler, online (3 knobs) | `["stage1_gain": (!, _), "tonestack_mid": (!, _), "master_volume": (!, _) -> e]` on the original, with the three ranges from `cinput`; or `"*"` and 29 parameters | the choice of the three knobs, the rates |
| ampmodeler, calibration (24) | `fad(loss, U)` on the original, the unit cube from `cinput`'s ranges | holding the two dead stage-4 values, stage 5 and one of the two volumes: identifiability |
| amp (209 parameters) | the same on `amp_effect.dsp`, by `rad` | the sigmoid space and `init`, the flat directions |
| ts808 (5 values) | the same on the stage with its sliders | the reparametrisation (V_k, β) that makes the loss well-conditioned; the leader–scout pair |
| jot, rir | the enumeration of the parameters | the EDR loss, the alignment of the response |

The operator removes the mechanical layer, one library per project; the loss, the coordinates, the choice of parameters and the identifiability remain the model's, and the tool can at most diagnose them (a `faustprobe --identifiability` reading zero gradients and collinear pairs off a render is the natural next step, outside this document).

## 4. Implementation in faust-rs

1. **Boxes.** Four box kinds in `crates/boxes` (`BOXCINPUTS`, `BOXCINPUT`, `BOXCOUTPUTS`, `BOXCOUTPUT`: tags, builder, matcher, printer), parsed as primitives with 1 and 2 arguments, next to `inputs`/`outputs` in the grammar's primitive table.
2. **Eval.** Four arms next to `BoxMatch::Inputs`: evaluate and lower the inner box, then a walk shared with `implant_modulation`, `collect_control_inputs(arena, lowered) -> Vec<(TreeId widget, kind, cur, min, max, step)>`, depth-first with the group stack, deduplicating by `TreeId` (the lowered tree is a DAG: the same widget reached twice is one entry, as `ui_build.rs` does with its `visited` cache), skipping bargraphs; the numbers folded by the same evaluation the widget's arguments already went through. `cinputs` returns `boxInt`, `cinput(i, …)` the `par` of the five boxes (an `i` out of range: an error naming the count). The bargraph twins likewise.
3. **Modulation.** In `eval_modulation`, detect a last segment `*`; then `implant_modulation` takes a `slots: Vec<TreeId>` it fills with a fresh slot per matched widget instead of `rewrite.slot`, and the result is wrapped in `symbolic` once per slot, last slot innermost, so that the first matched widget is the first input. The literal-label path is untouched. The no-match error is new.
4. **Tests.** Corpus fixtures: `cinputs` on `ampmodeler`-like programs with groups, duplicates and dead widgets (count, order, values against `--list-params`); `cinput` as seeds equal to the hand-written `fad` lanes; `"*"` on a program with 29 widgets equal to the 29 explicit targets to the bit; `"group/*"`; a wildcard that matches nothing. The `impulse-tests` reference cannot cover them (C++ has none of this: an extension, as `fad` and `rad` are, to be listed with them in `docs/`).
5. **Docs.** The syntax note for the four primitives and the wildcard, the modulation section of the manual port with the "one modulator per target" trap, `optimizers.lib` gaining `adaptive_fad` and `adaptive_rad` and its overview paragraph, the faust-ad skill.

Estimated size: the eval arms and the walk are a day; the wildcard is an afternoon, the tests and docs another day.

## 5. What it does not solve

- **Identifiability.** A program's sliders are not its identifiable parameters: two volumes in series, a bias under a cutoff's linear range, four cascaded low-passes. The operator learns all of them and the loss is flat along those directions; the projects held or reparametrised them by hand, and a diagnostic can only report it.
- **Coordinates.** A frequency learns on its logarithm, a diode slope on its logarithm, a knee voltage in volts rather than a saturation current in log units; `[scale:log]` on the widget carries the first, the others are the model's.
- **Derivative-hostile programs.** Tables written at run time, soundfiles, foreign functions, `select2` on a learned parameter, `abs` at zero: `fad` gives zero tangents where its rules stop, `rad` refuses; the operator inherits both, and a program written for use may hold any of them.
- **Cost.** `fad` through `"*"` is one lane per widget, 8.7 preamps for 24 lanes on ampmodeler; past a few dozen, `rad` in the program is the per-sample pseudo-linear gradient, and the host's `--train` on `rad` the exact one per block.

## 6. Open points

1. The scale metadata: a sixth entry of `cinput`, or a separate `cscale(i, e)`; the sixth entry is proposed.
2. Whether a widget rebound by `"*": (!, _)` should leave the UI; not proposed here (section 3.3), and worth an option of the UI builder later.
3. Whether the wildcard should also be accepted by the C++ compiler; this document only asks that faust-rs's form be one the C++ grammar already parses (it does: a string label), so that a program using it fails there with a clear "no match" rather than a syntax error.
