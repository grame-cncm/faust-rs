# Control inputs as boxes: `cinputs`, `cinput`, `coutputs`, `coutput` and the wildcard modulation target `"*"`

faust-rs extensions, not known to the C++ Faust compiler, in the same class as
`fad` and `rad`. Contract and rationale:
`porting/control-inputs-and-wildcard-modulation-analysis-2026-09-22-en.md`.
Implementation: `crates/eval/src/control_inputs.rs`, the wildcard in
`crates/eval/src/modulation.rs`, the list itself in
`crates/propagate/src/control_widgets.rs`. Tests:
`crates/compiler/tests/control_inputs.rs`, on the fixtures
`tests/corpus/cinputs_*.dsp`, `tests/corpus/wildcard_*.dsp` and
`tests/corpus/err_3{0,1}_*.dsp`.

## 1. The four primitives

```faust
cinputs(e)       // the control inputs of e as a list of its widget boxes
cinput(i, e)     // the i-th control input, 0-based, as (widget, init, min, max, step)
coutputs(e)      // the bargraphs of e as a list of its bargraph boxes
coutput(i, e)    // the i-th bargraph, 0-based, as (bargraph, min, max)
```

- **What counts.** A control input is a `hslider`, `vslider`, `nentry`,
  `button` or `checkbox`; a bargraph is a `hbargraph` or `vbargraph`.
  Soundfiles are neither.
- **What is listed.** The control inputs present in `e`, read or not, as
  `inputs(e)` counts the audio inputs of `e`: `inputs(_ : !)` is 1 although
  the input is cut, and `outputs(cinputs(hslider("dead",…) : !))` is 1
  although the slider is. The list is taken on the box, at evaluation, before
  signals exist; it does not depend on what the compiler later simplifies
  away. It is therefore a superset of the compiled interface, which shows only
  the widgets the final signals still read: `hslider("d",…) * 0 +
  hslider("l",…)` lists `d` and `l`, its interface shows `l`.
- **Order.** The order of the interface `e` would show if every widget in it
  were read, its `buildUserInterface` and its JSON: groups with the same label
  merged, the children of each group, controls and groups together, sorted by
  their raw label, `[n]` ordering prefix included, as the C++ compiler sorts
  them. The widgets the interface does show are in its order; a dead widget
  keeps its place among them. The order of declaration does not matter:
  `hslider("b",…) + hslider("a",…) + hgroup("z", hslider("c",…)) + hslider("[0]y",…)`
  lists `y`, `a`, `b`, `z/c`.
- **Identity.** A widget reached through several paths of the program is one
  control; the same widget box under two different groups
  (`par(i, 3, vgroup("Op %i", g))`) is one control per group, as in the
  interface.
- **The count** is `outputs(cinputs(e))`, a compile-time constant usable as an
  iteration count. A program without control inputs gives the empty box
  `0 : !`, so the count is 0.
- **What `cinputs(e)` is.** The `par` of the widget boxes, the same nodes as
  in `e`: a bus whose i-th signal is `ba.take(i + 1, cinputs(e))`, and a seed
  list `fad` and `rad` take as it is. `fad(f, cinputs(f))` equals
  `fad(f, (a, b))` written with the widgets. One caveat: a widget box repeated
  under several groups gives several entries, but the seed rule of `fad` and
  `rad` resolves every reference of a seeded widget to one control, so seeding
  it merges its copies.
- **What `cinput(i, e)` is.** Five boxes: the widget, the same node as in `e`,
  then its evaluated default, minimum, maximum and step (a button or checkbox:
  `0, 0, 1, 1`). Select with a cut pattern: `cinput(i, e) : (!, _, !, !, !)`
  is the default.
- **What `coutputs(e)` and `coutput(i, e)` are.** The same for bargraphs; a
  bargraph box has one input and one output, so `coutputs(e)` is a bus of N
  inputs and N outputs, and reading a bargraph means feeding it its signal.
- **Evaluation.** At box evaluation, like `inputs(e)`: `e` is evaluated and
  lowered, then folded. `e` must be a closed block diagram; a function of
  signals (`e(x) = …`) is one.
- **Errors.** An index that is not a compile-time integer, negative, or at or
  past the count is `FRS-EVAL-0009`; the message names the index as written,
  and when the expression is itself a constant (`cinput(freq, 0)`) it says the
  arguments look swapped and suggests `cinput(0, freq)`. An expression that is
  not a block diagram is `FRS-EVAL-0099`.

## 2. The wildcard modulation target `"*"`

```faust
P, x : ["*": (!, _) -> e]     // every control input of e replaced by an input, in cinputs order
["*": *(0.5) -> e]            // every control input halved
["amp/*": (!, _) -> e]        // every control input under the group `amp`
```

- **Matching.** A target whose last segment is `*` matches every control
  input whose group path contains the preceding segments in order, as for a
  literal target (subsequence, innermost group first); `"*"` alone matches
  every control input. Bargraphs are never matched. `*` is a whole segment:
  `"stage*"` is a literal label. A group prefix is written without its type,
  `"amp/*"`, not `"h:amp/*"`, as for literal targets.
- **One input per control.** With a two-input modulator the wildcard adds one
  input **per matched control**, in `cinputs` order, in front of the inputs of
  `e`: the i-th extra input drives the control `ba.take(i + 1, cinputs(e))`
  describes; a dead control gets its input too, which it ignores, so `"*"`
  adds `outputs(cinputs(e))` inputs. A literal label that matches several
  widgets gives them one shared input, as in C++; the wildcard does not. `["*": (!, _) -> e]` equals
  the same modulation written with one literal target per control, listed in
  interface order.
- **Modulator arity.** As for a literal target: 0 inputs replaces every
  matched control by the modulator, 1 input transforms each, 2 inputs pairs
  each with its own extra input. Only the 2-input form adds inputs.
- **The interface.** A control replaced by `(!, _)` is no longer read and
  leaves the interface.
- **`fad` and `rad` inside `e`.** A seed that is a widget of `e` is the same
  control as the body's use of it: both are rebound to the same input.
- **No match** is an error, `FRS-EVAL-0010`, where a literal target that
  matches nothing is the warning `FRS-EVAL-0008` and a dangling input, as in
  C++. The C++ compiler parses a wildcard target as a label and matches
  nothing, so a program using it fails there with its no-match behaviour, not
  with a syntax error.

### A trap of literal targets

`["a", "b": m -> e]` attaches `m` to `b` only; `a` gets the default modulator
`*`. Give each target its modulator: `["a": m, "b": m -> e]`, or use `"*"`.

## 3. What they are for

Every use below works on a program `e` as it is, `component("x.dsp")` or any
closed expression, without editing it. Most are packaged in
`libraries/controls.lib` (prefix `ct`, section 4), which imports nothing; the
learning operators are in `libraries/optimizers.lib`.

### 3.1 Learning the controls

A program learning its own sliders without being rewritten, in
`libraries/optimizers.lib` (0.11.0):

```faust
op = library("optimizers.lib");
e = component("model.dsp");
clock = (ba.time % 2048) == 2047;
process(x, t) = op.adaptive_fad(e, op.mse, op.adam_g(0.01, 0.9, 0.999, 1e-8), clock, button("reset"), x, t);
```

`adaptive_fad` / `adaptive_rad` read the number of controls with
`outputs(cinputs(e))`, their bounds and defaults with `cinput`, and rebind them
with `["*": (!, _) -> e]`; see their documentation in the library. Host-driven,
`fad(loss, cinputs(e))` or `rad(loss, cinputs(e))` gives the gradient with
respect to every control of `e`.

### 3.2 The pattern behind the others: a function of every control

`cinput(i, e)` gives each control's widget and its default, range and step,
`["*": (!, _) -> e]` gives `e` one input per control, so a `par` over the
controls feeds `e` any function of them:

```faust
N = outputs(cinputs(e));
f(i) = ...;   // built from cinput(i, e)
process = par(i, N, f(i)) : ["*": (!, _) -> e];
```

The widget boxes `f(i)` reads are the nodes of `e`: a function that reads its
widget keeps the knob in the interface, one that ignores it removes the knob.
The default, minimum, maximum and step are compile-time constants, so they can
set the parameters of a new widget, size a `par`, or scale a rate.
`ct.map(f, e)` is this pattern, with `f(i, w)` given the index and the widget.

One trap: to pass the constants to a widget, apply the function to them,
`w(i, ct.init(i, e), ...)`; a lambda composed with `cinput(i, e) : \(w, v, a,
b, s).(hslider("P", v, a, b, s))` receives signals, not constants, and the
widget is refused (`FRS-EVAL-0011`), as the C++ compiler refuses any widget
parameter that is not a number.

### 3.3 Uses

| Use | How | `controls.lib` |
|---|---|---|
| No zipper noise on any program | a one-pole on every control, starting at its default | `smooth(t, e)`, `smoother(t)` |
| A modular module, every knob with its jack | one CV input per control, added to the knob, scaled to its range, clamped | `cv(depth, e)` |
| Parameters driven by a host or another program | every control an audio input, in its units or normalized to `[0, 1]` | `external(e)`, `normalized(e)` |
| A new interface: knobs, numeric entries, other groups | new widgets built with each control's default, range and step | `relabel(wdg, e)`, `knobs(e)` |
| Preset morphing | from the knobs to a preset list, one amount for all | `morph(m, P, e)` |
| "Randomize" | a draw per control on a trigger, uniform on its range and step grid, mixed with the knob | `randomize(trig, amount, e)` |
| Unison voices, stereo spread, humanized doubles | copies of `e` with every control shifted by a fraction of its range | `offset(d, e)`, in a `par` |
| A test with no knowledge of the program | every control swept over its range; a count of non-finite outputs | `sweep(T, e)`, `sweep_one(k, T, e)`, `nonfinite(n)` |
| Which knobs matter here | the derivatives of the outputs with respect to every control | `gradient_fad(e)`, `gradient_rad(e)` |
| Learning the controls | a descent on all the controls against a target | `op.adaptive_fad`, `op.adaptive_rad` |

Measured on `ctl_06_testing.dsp`: `ct.sweep(100, 1 / hslider("d", 0.5, -1, 1,
0.01)) : ct.nonfinite(1)` is 1 on the samples where the sweep crosses `d = 0`
and 0 elsewhere, the kind of setting a hand-written test forgets.

### 3.4 Limits

- **Labels are not values.** `cinput` gives no label: a rebuilt interface
  names its widgets by index (`P0`, `P1`, ...), and the interface sorts labels
  as text, so past ten controls `P10` comes before `P2`. Renaming by the old
  label would need a primitive giving the i-th label for label interpolation.
- **`"*"` matches buttons and checkboxes too.** Smoothing every control
  smooths a `gate`; restrict with a group target, `["synth/*": ct.smoother(t)
  -> e]`, written in the program because a target is a literal.
- **Dead controls count** (section 1): `map` gives them an input they ignore.
- **Bargraphs are read-only.** `coutputs(e)` gives the bargraph boxes and
  their ranges, not the signals `e` feeds them: turning the meters of a
  program into outputs, to log them or learn on them, would need a primitive
  exposing those signals.
- **Nothing sets a knob.** A program cannot move its own sliders; `randomize`
  and `morph` replace what the program reads, the knobs keep their positions.

## 4. `controls.lib`

`libraries/controls.lib` (0.1.0, prefix `ct`) packages section 3; it imports
nothing, so a program needs only `-I libraries`. Sections and functions:

- **Reading the controls:** `count(e)`, `widget(i, e)`, `init(i, e)`,
  `lo(i, e)`, `hi(i, e)`, `step(i, e)`, `inits(e)`.
- **Rebinding the controls:** `map(f, e)`, `external(e)`, `normalized(e)`,
  `cv(depth, e)`, `smooth(t, e)`, `smoother(t)`.
- **Rebuilding the interface:** `relabel(wdg, e)`, `knobs(e)`.
- **Exploring settings:** `morph(m, P, e)`, `randomize(trig, amount, e)`,
  `offset(d, e)`.
- **Testing:** `sweep(T, e)`, `sweep_one(k, T, e)`, `nonfinite(n)`.
- **Sensitivity:** `gradient_fad(e)` (each output, then its derivatives with
  respect to every control), `gradient_rad(e)` (the outputs, then the gradient
  of their sum).

```faust
ct = library("controls.lib");
e = component("synth.dsp");
process = hgroup("synth", ct.knobs(ct.smooth(0.02, e)));
```

A function taking `e` needs `e` to have at least one control input (the
wildcard of `map` matching nothing is `FRS-EVAL-0010`). The inputs a function
adds come first, one per control in `cinputs` order, then the inputs of `e`.
Each function is documented in the library with a `#### Test` entry, compiled
and run by `tests/corpus/ctl_all_functions.dsp`; `tests/corpus/ctl_01` to
`ctl_07` state their expected outputs, checked by
`crates/compiler/tests/controls_lib.rs`.
