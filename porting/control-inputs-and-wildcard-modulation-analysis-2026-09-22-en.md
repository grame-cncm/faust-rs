# Control inputs as first-class boxes, and a wildcard modulation target: towards a generic adaptive operator

**Date:** 2026-09-22
**Status:** implemented 2026-09-24 (branch `control-inputs-wildcard`), with the decisions and corrections of section 7; revised 2026-09-23, `cinputs` and `coutputs` return lists rather than counts (section 3.1)
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
| `["*": (!, _) -> e]` | no match in either compiler; C++ 2.88.1 adds the slot anyway (one input more, unconnected) and warns under `-wall`; faust-rs added none until a81c87d2's successor, and does the same since (`FRS-EVAL-0008` under the semantic warnings) | `*` is free: no program uses it today |

The shared slot is the point the wildcard must not inherit: rebinding a program means one input per widget.

**Verified against the reference, 2026-09-23.** 42 programs covering the manual's rules and the edge cases (`tests/corpus/modulation_*.dsp`), rendered by faust 2.88.1 through the impulse-test architecture and by `faustprobe --protocol impulse-test`, then their `-json`: samples equal to the bit, same arity, same interface, once three faust-rs defects were fixed the same day. A label opening a group (`hslider("h:sub/x", …)`) was one segment `h:sub/x` and matched neither `x` nor `sub/x`; a label starting with metadata (`"[1] Wet [tooltip: …]"`, every `*_demo` slider) was cut at its first `[` to nothing, so the manual's own example `["Wet" -> dm.freeverb_demo]` failed; a target matching nothing added no slot where the reference adds one. `crates/compiler/tests/modulation_corpus.rs` freezes the reference's arity and interface for the 42 and compares them live with the `faust` binary when one is installed. Two facts the manual does not say, read off the reference: a group prefix written with its type (`"v:in/x"`, the manual's `h:group/label`) matches nothing in 2.88.1, `target2path` keeping `v:in` verbatim, where `"in/x"` matches; and a target that names only a group modulates every widget under it, with one shared slot.

### 2.3 The UI builder

`crates/propagate/src/ui_build.rs` walks the validated flat box DAG after propagation, registers each control once (a widget reachable through several paths is one `ControlSpec`) and places it in the group of its body occurrence; the order is the depth-first order of the box tree with the group stack, which is the order of `buildUserInterface` and of `faustprobe --list-params` before it sorts. This is the order `cinput(i, e)` must use, and the one the wildcard must use for its extra inputs, so that a host and a program agree on what "the i-th control" is. *Corrected 2026-09-24:* the registration order is depth-first, but the order of `buildUserInterface` is not: the builder sorts each group's children, controls and groups together, by raw label (`[n]` prefix included) and merges groups of the same label, as the C++ compiler does (`b + a + hgroup("z", c) + [0]y + hgroup("g", d)` shows y, a, b, g/d, z/c in both). The implementation uses the interface order (section 7).

## 3. The contract

### 3.1 `cinputs(e)`, `cinput(i, e)`

```faust
cinputs(e)       // the control inputs of e, its sliders, nentries, buttons and checkboxes, as a list of the widget boxes
cinput(i, e)     // the i-th control input of e (0-based), as the list (widget, init, min, max, step)
```

The count is `outputs(cinputs(e))`, a compile-time constant like `inputs(e)`: no counting primitive. (A first version of this document had `cinputs` return the count; the list form replaces it, for the reasons at the end of this section.)

- **What counts.** Every widget that produces a signal: `hslider`, `vslider`, `nentry`, `button`, `checkbox`. Bargraphs do not. A widget reached through several paths of the box DAG is one input (the UI builder's rule); two widgets with the same label in different groups are two.
- **Order.** The UI order: the order of `e`'s own interface, groups merged by label and children sorted by raw label (section 7; the depth-first traversal first written here is not the interface order). Stable under `component`, `hgroup`, `library` prefixes.
- **What `cinputs` returns.** A `par` of the N widget boxes, in that order, the same nodes as in `e`, without metadata: a bus of N signals whose i-th is `ba.take(i + 1, cinputs(e))`, and a list of seeds `fad` and `rad` take as they are. A program without control inputs gives the empty box `0 : !` (no input, no output), so that `outputs(cinputs(e))` is 0; a `par` over it downstream is then an error, and `adaptive_fad` must say so rather than fail on the `par`.
- **What `cinput` returns.** A list of five boxes, so that `ba.take` reaches each: the widget box itself, its default, its minimum, its maximum, its step (a button or a checkbox: 0, 0, 1, 1). The widget box is **the same node** as in `e`: taken as a seed of `fad` or `rad`, it is recognised by identity, as the seed rule requires (`docs/fad-note-en.md` §1). The four numbers are the evaluated, folded constants of the widget (an expression such as `1.0 / base_tube_gain` is a number here, as `--list-params` shows it).
- **Metadata.** The label's metadata is kept on the widget box, not in the list. A sixth entry, the scale (`0` linear, `1` for `[scale:log]`, `2` for `[scale:exp]`), is the one metadata a learning space needs; it was proposed as a sixth entry rather than a separate primitive (section 6). *Decided 2026-09-24:* no sixth entry for now, the tuple has five.
- **When it is evaluated.** At box evaluation, after `eval` and `a2sb` of `e`, exactly as `inputs(e)`; `cinputs` folds to a `par` of the N widget boxes, `cinput` to a `par` of five boxes. `e` must be closed (no free box variables), as `inputs(e)` requires.

With these two, the host-driven calibration of any program is one line:

```faust
e = component("x.dsp");
process = fad(op.mse(e, target), cinputs(e));                  // faustprobe --train on the widgets' paths
```

and the unit cube, or the sliders' own units, come from `cinput`.

**Why a list and not a count.** The two forms carry the same information: with a count, the seeds are `par(i, cinputs(e), cinput(i, e) : (_, !, !, !, !))`, one line more and the width of the tuple written by the caller. The list form makes the most frequent gesture, seeding a descent, a single identifier, drops the counting primitive (`outputs` already counts a bus), and keeps the tuple's width inside `cinput`, the only place a sixth entry (section 6) would change. What it gives up is the analogy of `cinputs` with `inputs`, a number: read as "the control inputs" rather than "their number", the plural is more exact. A list of tuples was considered and rejected: Faust has no list of lists, `(a, b, c, d, e), (f, g, h, i, j)` is ten boxes in parallel, so the count would be `outputs(…) / 5` and every index would move the day the tuple grows.

### 3.2 `coutputs(e)`, `coutput(i, e)`

```faust
coutputs(e)      // the bargraphs of e, as a list of the bargraph boxes
coutput(i, e)    // the i-th bargraph, as the list (bargraph, min, max)
```

Same order, same evaluation, count by `outputs(coutputs(e))`. The bargraph box is the `attach`ed signal's carrier: taking it as a signal reads what the program shows, its loss or its learned values, without a second output; `coutputs(e)` is all of a program's meters as one bus, what a page or a test reads in one line. Symmetric with `cinput`; not needed by the descents. *Corrected 2026-09-24:* a bargraph box has one input and one output, so `coutputs(e)` is a bus of N inputs and N outputs; the box alone does not read the value `e` shows, it has to be fed its signal.

### 3.3 The wildcard target `"*"`

```faust
P, x : ["*": (!, _) -> e]                // e with every control input replaced by an input, in cinput order
["*": *(0.5) -> e]                       // every control input halved
["amp/*": (!, _) -> e]                   // every control input under the group `amp` (no `h:` prefix, section 2.2)
```

- **Matching.** A target whose last segment is `*` matches every control input whose path has the preceding segments as a subsequence (the existing rule), all of them if there is no preceding segment. Bargraphs are never matched (a modulated bargraph has no meaning). `*` is a whole segment: `"stage*"` is not proposed, the subsequence rule already gives group prefixes, and a glob inside a segment is a second language.
- **One slot per widget.** Unlike a literal label, whose matches share the modulation's one slot (section 2.2, kept as is for compatibility), a wildcard allocates a fresh slot for each matched widget, in UI order, and the extra inputs are prepended in that order: the first control input of `e` is the first input of the modulated block. `ba.take(i + 1, cinputs(e))`, `cinput(i, e)` and the i-th extra input of `["*": … -> e]` name the same widget by construction, which is what lets a program use all three.
- **Arity of the modulator.** As today: 0 inputs replaces every matched widget by the circuit (a constant for all: `["*": 0.5 -> e]`, rarely useful), 1 input transforms each, 2 inputs pairs each with its own extra input. Only the 2-input form adds inputs.
- **No match.** An error, `FRS-EVAL`, "the modulation target `*` matches no control input of the expression", where a literal label that matches nothing keeps the reference behaviour, a dangling slot and the warning `FRS-EVAL-0008` (the wildcard should refuse: an operator that learns nothing is a mistake, not a program).
- **The widget in the UI.** Under a 2→1 modulator that drops the widget, `(!, _)`, the widget's signal reaches nothing and the widget leaves the interface, in both compilers: the reference builds its interface while generating code, so a widget the simplified graph no longer reads never appears, and faust-rs, which registers every widget of the box tree at propagation, prunes them in the fast lane once the final signals are known (`UiProgram::pruned`, 2026-09-23; before that day faust-rs kept them: `ampmodeler.dsp` showed 29 controls against the reference's 25, its four dead sliders). A program rebound by `"*"` therefore shows the widgets its `e` still reads and none of the replaced ones, as the reference would.

### 3.4 The generic adaptive operator, in `optimizers.lib`

With 3.1 and 3.3 and the 0.10.0 bus loops, and nothing else. The two operators differ by one word, the loop they call, so the envisaged code is one helper and two names:

```faust
//--- `(op.)adaptive_fad`, `(op.)adaptive_rad` ---
// A program learning all its control inputs while it runs: `e` with its
// widgets replaced by parameters descended, every firing of `clock`, on the
// frame mean of the gradients of `loss(model, target)`, by `fad` or by `rad`,
// each parameter in its own units, bounded by its widget's range, started at
// its default, stepped by its own engine.
//
// adaptive_fad(e, loss, upd, clock, reset, X, t) : si.bus(outputs(e) + N)
// adaptive_rad(e, loss, upd, clock, reset, X, t) : si.bus(outputs(e) + N)
//
// Where:
// * `e`: the program, closed, with N = outputs(cinputs(e)) control inputs
// * `loss`: a function of two signals, the model's output and the target's
//   (`mse`, `pseudo_huber(d)`); summed over the outputs of `e`
// * `upd`: one engine (`adam_g(lr, b1, b2, eps)`, `sgd_g(lr)`, ...) or a list of N,
//   the rate in each widget's own units (0.01 on a gain, 0.5 dB on a master)
// * `clock`: 1 on the samples where a step is taken (a frame's end, gated or not)
// * `reset`: 1 sends the parameters back to their defaults
// * `X`: the inputs of `e`, as signals (one for a mono program, `(l, r)` for two)
// * `t`: the target, outputs(e) signals
// Outputs: those of `e` on the learned parameters, then the N parameters, in
// `cinputs` order.
adaptive_fad(e, loss, upd, clock, reset, X, t) = _adaptive(descend_N_fad_clocked, e, loss, upd, clock, reset, X, t);
adaptive_rad(e, loss, upd, clock, reset, X, t) = _adaptive(descend_N_rad_clocked, e, loss, upd, clock, reset, X, t);

_adaptive(descend, e, loss, upd, clock, reset, X, t) = model(P), P
with {
    N = outputs(cinputs(e));
    K = outputs(e);
    LO = par(i, N, cinput(i, e) : (!, !, _, !, !));
    HI = par(i, N, cinput(i, e) : (!, !, !, _, !));
    INIT = par(i, N, cinput(i, e) : (!, _, !, !, !));
    model(P) = P, X : ["*": (!, _) -> e];                                        // e on P in place of its widgets, X its audio
    lossN = model(si.bus(N)) : par(i, K, \(y).(loss(y, ba.take(i + 1, t)))) :> _;   // a box of N inputs, one scalar
    P = descend(N, clock, lossN, upd, LO, HI, INIT, reset);
};
```

and a program that follows a real preamp on every knob of `ampmodeler.dsp`, the model rewritten by nobody:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
e = component("ampmodeler.dsp");
FRAME = 2048;
clock = (ba.time % FRAME) == (FRAME - 1);
process(x, t) = op.adaptive_fad(e, op.mse, op.adam_g(0.01, 0.9, 0.999, 1e-8), clock, button("reset"), x, t);
// 1 + 29 outputs: the preamp, then its 29 sliders as learned, including the four
// dead ones (a zero gradient: they stay at their default) and the two volumes
// that share one gain (section 5)
```

**Why `X` and `t` are arguments and not inputs.** The clocked loops compose the parameters with the loss, `params : loss`, so `loss` is a box of N inputs; the programs written so far build it with a lambda of literal arity, `\(g, m, v).(mse(model(x, g, m, v), t))`, which a generic N cannot write, Faust having no lambda over a bus. The helper builds the box the other way round: the parameters enter the modulated `e` as its first N inputs, and the only place a signal is used twice, `loss(y, t)`, is after the model, on one wire per output, through a lambda of one parameter. That works only if the audio inputs and the target are signals closed over by the function, hence arguments, as `x` and `t` are in `ampmodeler_online.dsp`. Writing `loss(model(si.bus(N)), t)` instead would fail: `mse(y, t) = (y - t) * (y - t)` mentions `y` twice, and a box of N inputs mentioned twice is a box of 2N inputs. The first draft of this section had `\(P).(loss(model(P), target))`, a box of one input whatever N: the same mistake.

**Both modes, both named.** `adaptive_rad` is `adaptive_fad` on `descend_N_rad_clocked`, one word in `_adaptive`'s call: `cinput` and the wildcard supply seeds and bounds and do not know which mode consumes them. The pair is named in full, `adaptive_fad` and `adaptive_rad`, with no bare `adaptive`: the library's older loops leave the `fad` form unmarked and suffix the twin (`descend_N_fad`, `descend_N_rad`), an inheritance from `fad` having come first, but this is the entry point a reader meets first and the mode is a choice to make knowingly, not a default with an option; two names at the same rank say so, and a third for the same thing is what to avoid. The older pairs keep their names (renaming them would break the six projects for nothing) and the overview says in one sentence that new functions name both modes. What differs is the library's existing contract. Through a recursion `fad` carries the exact derivative at any block size, where `rad` consumed in the graph sees one sample, the direct term with the past state held fixed (pseudo-linear regression), the frame mean being a mini-batch of those; on a model without recursion between the parameters and the output the two follow the same trajectory (`opt_bus_fad_vs_rad_fir16`). `fad` costs one lane per widget (8.7 preamps for 24 lanes on ampmodeler), `rad` one sweep whatever the count, the choice past a few dozen parameters. `rad` refuses written tables, soundfiles, foreign functions and clock-domain crossings where `fad` emits zero tangents, so `adaptive_rad` says at compile time what `adaptive` would learn around in silence. The clocked descent is not a crossing: only the step is inside the `ondemand` block, the loss and its `rad` run at audio rate, as `descend_N_rad_clocked` already does. Host-driven, the same pair: `fad(loss, cinputs(e))` or `rad(loss, cinputs(e))`.

The five projects, rewritten on it:

| project | what the operator gives | what stays by hand |
|---|---|---|
| ampmodeler, online (3 knobs) | `["stage1_gain": (!, _), "tonestack_mid": (!, _), "master_volume": (!, _) -> e]` on the original, with the three ranges from `cinput`; or `"*"` and 29 parameters | the choice of the three knobs, the rates |
| ampmodeler, calibration (24) | `fad(loss, cinputs(e))` on the original, the unit cube from `cinput`'s ranges | holding the two dead stage-4 values, stage 5 and one of the two volumes: identifiability |
| amp (209 parameters) | the same on `amp_effect.dsp`, by `rad` | the sigmoid space and `init`, the flat directions |
| ts808 (5 values) | the same on the stage with its sliders | the reparametrisation (V_k, β) that makes the loss well-conditioned; the leader–scout pair |
| jot, rir | the enumeration of the parameters | the EDR loss, the alignment of the response |

The operator removes the mechanical layer, one library per project; the loss, the coordinates, the choice of parameters and the identifiability remain the model's, and the tool can at most diagnose them (a `faustprobe --identifiability` reading zero gradients and collinear pairs off a render is the natural next step, outside this document).

## 4. Implementation in faust-rs

1. **Boxes.** Four box kinds in `crates/boxes` (`BOXCINPUTS`, `BOXCINPUT`, `BOXCOUTPUTS`, `BOXCOUTPUT`: tags, builder, matcher, printer), parsed as primitives with 1 and 2 arguments, next to `inputs`/`outputs` in the grammar's primitive table.
2. **Eval.** Four arms next to `BoxMatch::Inputs`: evaluate and lower the inner box, then a walk shared with `implant_modulation`, `collect_control_inputs(arena, lowered) -> Vec<(TreeId widget, kind, cur, min, max, step)>`, depth-first with the group stack, deduplicating by `TreeId` (the lowered tree is a DAG: the same widget reached twice is one entry, as `ui_build.rs` does with its `visited` cache), skipping bargraphs (*as implemented:* the list is the UI builder's own, keyed by widget and group context, section 7); the numbers folded by the same evaluation the widget's arguments already went through. `cinputs` returns the `par` of the widget `TreeId`s (the empty box `0 : !` when there are none), `cinput(i, …)` the `par` of the five boxes (an `i` out of range: an error naming the count). The bargraph twins likewise.
3. **Modulation.** In `eval_modulation`, detect a last segment `*`; then `implant_modulation` takes a `slots: Vec<TreeId>` it fills with a fresh slot per matched widget instead of `rewrite.slot`, and the result is wrapped in `symbolic` once per slot, last slot innermost, so that the first matched widget is the first input. The literal-label path is untouched. The no-match error is new.
4. **Tests.** Corpus fixtures: `cinputs` on `ampmodeler`-like programs with groups, duplicates and dead widgets (`outputs(cinputs(e))`, order, `cinput`'s values against `--list-params`); `fad(loss, cinputs(e))` equal to the hand-written `fad` lanes; a program without controls; `"*"` on a program with 29 widgets equal to the 29 explicit targets to the bit; `"group/*"`; a wildcard that matches nothing. The `impulse-tests` reference cannot cover them (C++ has none of this: an extension, as `fad` and `rad` are, to be listed with them in `docs/`).
5. **Docs.** The syntax note for the four primitives and the wildcard, the modulation section of the manual port with the "one modulator per target" trap, `optimizers.lib` gaining `adaptive_fad` and `adaptive_rad` and its overview paragraph, the faust-ad skill. `adaptive_fad` on `ampmodeler.dsp` against `ampmodeler_online.dsp` with the 26 other parameters' rates at zero: the three knobs must follow the same trajectory to the bit, the test of section 3.4's code.

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

## 7. As implemented (2026-09-24)

Branch `control-inputs-wildcard`. Syntax note: `docs/control-inputs-en.md`; registry entry `DIFF-SRC-004`.

**Decisions taken with the user.**

- *Order:* the interface order, not a depth-first walk (section 2.3 corrected). The list is read off the UI program the propagation builds for the lowered `e` (`propagate::control_widgets`, `crates/propagate/src/control_widgets.rs`), so it cannot drift from the interface: groups merged by label, children sorted by raw label, label paths (`h:sub/x`) and relative group navigation as the UI builder decodes them.
- *Identity:* one control per widget box **and group context**, the UI builder's key, not per `TreeId`: `par(i, 3, vgroup("Op %i", g))` is three controls, as in the interface, and `"*"` gives it three inputs. Caveat, not changed: as seeds of `fad`/`rad` the three copies of the same box resolve to one control (the seed aliasing of `DIFF-SRC-001`), so `fad(e, cinputs(e))` on such a program merges them.
- *Scale:* no sixth entry; `cinput` returns five boxes (open point 1 closed).
- *Scope:* primitives, wildcard, `optimizers.lib` 0.11.0 (`adaptive_fad`, `adaptive_rad`), tests and docs.

**How the wildcard resolves an occurrence.** `implant_wildcard` (`crates/eval/src/modulation.rs`) walks the lowered body with `propagate::UiGroupContext`, the same group-context key the UI builder uses, restarting from the root context at `fad`/`rad` seeds as the builder does, and looks every widget occurrence up in the control list; matching is decided per control on its interface path, so every occurrence of one control (body and seed alike) gets the same slot. The walk is memoized per (box, context).

**Codes.** `FRS-EVAL-0009` (index out of range), `FRS-EVAL-0010` (wildcard matching nothing); an operand that is not a block diagram is `FRS-EVAL-0099`.

**Evidence.** `crates/compiler/tests/control_inputs.rs` (order against the program's own JSON, `cinput` values, empty list, a box under three groups, dead widgets, `"*"` equal to one literal target per control to the bit, one-input modulators, `fad` on `cinputs`, a rebound `fad` seed, both error codes, `"stage*"` staying literal); `adaptive_operators_follow_the_hand_written_loop` in `crates/compiler/tests/optimizers_lib.rs` (fixture `tests/corpus/opt_adaptive_vs_hand.dsp`): `adaptive_fad` on a three-slider model follows `descend_N_fad_clocked` written by hand to the bit, `adaptive_rad` converges to the same values. The ampmodeler comparison of section 4 step 5 lives in the sibling project and was not run here.
