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
- **Order.** The order of the interface `e` would show on its own, its
  `buildUserInterface` and its JSON: groups with the same label merged, the
  children of each group, controls and groups together, sorted by their raw
  label, `[n]` ordering prefix included, as the C++ compiler sorts them. The
  order of declaration does not matter:
  `hslider("b",…) + hslider("a",…) + hgroup("z", hslider("c",…)) + hslider("[0]y",…)`
  lists `y`, `a`, `b`, `z/c`.
- **Identity.** A widget reached through several paths of the program is one
  control; the same widget box under two different groups
  (`par(i, 3, vgroup("Op %i", g))`) is one control per group, as in the
  interface. Widgets the program no longer reads (`hslider("dead",…) : !`) are
  counted: the list is taken on the box, before the compiled interface prunes
  dead widgets.
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
  describes. A literal label that matches several widgets gives them one
  shared input, as in C++; the wildcard does not. `["*": (!, _) -> e]` equals
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
