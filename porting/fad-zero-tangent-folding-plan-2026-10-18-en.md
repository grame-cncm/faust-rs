# FAD: folding literal-zero tangents at clock-domain boundaries — plan

Date: 2026-10-18. Base: `main-dev` at `ad868e46`. Branch: `fad-ondemand-cost`.
Origin: the faust-coopiano feasibility study of `fad` for voicing calibration
(`faust-coopiano/tools/ad/README.md`, commit `c6cb028`), which measured one
`fad` seed at about six plain renders of the voice.

## 1. Measurement

Program: the bridge force of one coopiano voice (`cp.voice_drawn(...) : !, _`,
key 40, gate held), `--double`, 3 s of audio (132,300 frames).

| | plain | one seed (`cp._h_stiffness`) |
|---|---|---|
| faustprobe (Cranelift) compute | 174 ms | 1.09 s (6.3×) |
| generated C++, `clang++ -O1`, same render | 114 ms | 593 ms (5.2×) |
| held values (`fPerm`) | 3,097 | 6,194 |
| held values equal to 0 after 200 blocks | 197 | 2,723 |
| arithmetic ops executed per sample | 9.1 k | 28.2 k |

The native build has the same ratio as the JIT: the cost is in the generated
program, not in a backend. No sample-rate statement computes a value that only
an `ondemand` guard reads, and no guarded block fires more often than in the
plain program: the tangents of the note parameters are computed in their
domain, as they should be. What is wrong is how many there are. The seed is a
hammer control; the 2,500 string and mode parameters do not depend on it, yet
2,526 of the 3,097 held tangents are computed, held and read every sample by
the sample-rate tangent code (`c' · y` terms of every resonator), and all of
them are 0 at run time.

## 2. Cause

A held tangent such as `fSlowOD252` never reads the seed: it is built from
terms `0.0 * x` that nothing folded. Reduced (`/tmp` reproducer, no library):

```faust
m = hslider("m", 40, 21, 108, 1);
g = hslider("g", 1, 0, 2, 0.01);
trig = button("gate") > button("gate")';
f(mm, gg) = pow(0.9, (mm - 21) / 48) * gg + log(mm);
process = fad((trig, m, g) : ondemand(f), g);
```

emits the tangent `(0.0 / m) + (1.0 - (0.0 * c) * g) * pow(...)` where
`pow(...)` is the answer. The input `m` enters the twin domain as
`Clocked(τ, Clocked(nil, TempVar(m)))`. It does not depend on the seed, so its
inner part gets the zero tangent `0.0`, but it carries the twin's token
(`needs_rebuild`), so it is not short-cut: the `Clocked` pass-through arm
rebuilds its tangent as `Clocked(τ', 0.0)`. From there on:

- every rule sees a non-literal operand (`is_literal_zero` is false), so `log`,
  `pow`, `*` build their full chain/product rules over it;
- the signal simplifier, which folds `x · 0` later, cannot see through the
  wrapper either, nor can the FIR (which does not fold `0.0 * x`, an IEEE
  rewrite);
- `augment_block` keeps the lane in the twin's payload (it drops only literal
  tangents) and `transform_seq` sequences it, so the zero becomes a held value
  `fPerm`, read at the sample rate as an unknown.

`TempVar`, `PermVar`, `Seq`, `ZeroPad` and `select2` of zeros have the same
effect when they wrap a zero.

## 3. Fix

`ForwardADTransform::transform` folds each tangent it returns with
`signals::ad_rules::DerivativeZeroFolder` (memoized; the module that already
holds the AD algebra shared by `propagate` and `transform`, so that the
reverse-mode passes can use it too):

| node | fold |
|---|---|
| `0 · x`, `x · 0`, `0 / x` | `0` |
| `x + 0`, `0 + x`, `x − 0` | `x` |
| `TempVar(0)`, `Clocked(τ, 0)`, `Seq(B, 0)`, `ZeroPad(0, h)`, `delay1(0)`, `delay(0, d)` | `0` |
| `PermVar(0)` | `0` (a hold starts at 0) |
| `float_cast(0)` | `real(0)` |
| `select2(c, 0, 0)` | `0` |

The rules build each tangent over the already-folded tangents of the operands,
so folding the new nodes on the way out of `transform` is enough. Each primal
is entered in the folder as a fixed point (`keep`): the walk visits only the
nodes the rule just built and never descends a primal sum, which also bounds
its recursion depth.

Why not the shared simplifier. `normalize` already applies these folds to bare
literals, but it runs after `propagate`, too late: by then the transformer has
decided from the shape of each tangent what the twin holds, and a lane held as
`PermVar(Clocked(τ, e))` stays a hold read at the audio rate even once `e`
simplifies to 0 (removing payload lanes and rewriting `Seq(B, 0)` in
`signal_prepare` would be a structural change of the blocks there, with the
`FRS-SFIR-0008` constraint of `transform_seq`). It also treats `Clocked` as
opaque, per domain, so it would not see `x · Clocked(τ, 0)` as zero; and
`propagate` does not depend on `normalize`. The folds are therefore made
during the transform, from a module of `signals` both crates can use. The existing
machinery then does the rest: the held lane is dropped from the twin's
payload, the `Seq` consumer reads the literal, and `pow`'s exponent lane test
takes its short path.

Semantics. These are the folds the signal simplifier already applies to the
same literals after FAD, so the convention (`0 · x` is 0 even for an infinite
`x`) is unchanged; only its timing moves, before the transformer decides what
to hold. A tangent that was `0 · inf = NaN` at run time becomes 0, which is
what a seed-independent subtree has always produced outside a domain. The
primal is untouched.

`ZeroPad(0, h)` must fold to `0`, not keep the `ZeroPad` over a bare constant:
the lowering attributes a `ZeroPad` to its upsampling block through its
clocked child and rejects one it cannot (`FRS-SFIR-0007`). The
`fad_beside_plain` matrix caught this on the first prototype.

Out of scope here: a recursion slot whose tangent folds to zero still gets a
tangent state (the projection into it is not a literal); a tangent `TempVar(1.0)`
of a seed snapshot is not folded to `1.0`.

## 4. Assurance level

An algebraic simplification inside the FAD transform: the standard tier of
AGENTS.md §5 (unit, structural and numerical differential tests), not the
producer/checker certificate tier, which is reserved for scheduling, vector
planning and routing artifacts. A wrong fold is not silent: it changes a
tangent value, which the runtime tests compare with analytic derivatives and
the differential below compares with the pre-change compiler.

## 5. Qualification

1. Unit: the reproducer's twin holds one tangent lane, not two, and the zero
   lane's tangent is the literal (structural); the two propagate tests that
   spelled `1.0 + 0.0` now read the folded `1.0`.
2. Runtime: the reproducer's tangent equals `pow(0.9, (m - 21) / 48)` on the
   interpreter; the existing suites (`ondemand_pipeline`, the 78-case
   `fad_beside_plain` matrix, `fad_recursive_runtime`, `rad_runtime`,
   `vector_clock_ad`, `optimizers_lib`, `aes_autodiff_paper`) pass.
3. Differential against the base compiler (`ad868e46`): every `.dsp` under
   `tests/` and `examples/` that uses `fad`, rendered by faustprobe with both
   binaries; primal lanes bit-identical, tangent lanes within rounding.
4. faust-coopiano: the voice's primal bit-identical, the tangent equal to the
   base one within rounding (which agreed with central differences to 1e-8).
5. Compile cost: no `fad` program is in the compile-budget basket, so the new
   code is unreachable there; the basket is timed base-vs-new by hand anyway
   (the gate cannot conclude on this machine).

## 6. Results

faust-coopiano voice, base `ad868e46` vs this change, run one after the other,
faustprobe release, `--double`, key 40, 3 s:

| program | compile | compute | outputs |
|---|---|---|---|
| plain | 13.0 → 13.5 s | 155 → 153 ms | identical |
| one seed | 26.1 → 15.3 s | 993 → 617 ms | primal identical, tangent within 2.8e-15 relative |
| three seeds | 31.9 → 19.6 s | 2.54 → 1.75 s | primal identical, tangents within 4.6e-15 relative |

Held values of the one-seed program: 6,194 → 3,674 (plain 3,097), of which 203
are 0 at run time (plain 197). Per-sample arithmetic operations 28.2 k →
21.1 k (plain 9.1 k). Generated C++ 32.5 → 23.8 MB (plain 22.9 MB). The
tangents differ from the base ones by rounding only: removing `+ 0` terms
changes the order the normal form gives the remaining ones.

Differential (`tests/` and `examples/`, every `.dsp` using `fad`,
`optimizers.lib` or `controls.lib`: 118 programs, 4,096 frames of white noise,
base vs new faustprobe): 113 bit-identical on every lane; three more turn
non-finite under white noise with both binaries (the probe then exits 1) and
are bit-identical under an impulse (`auto_chorus_stereo_fad_host`,
`fad_recursive_nested_parametric`, `fad_recursive_triple_nested_parametric`);
the two `err_fad_*` cases are rejected by both with the same diagnostic.

Rejecting mutations, run against the `propagate` unit tests and the
`ondemand_pipeline`, `fad_recursive_runtime`, `rad_runtime`,
`vector_clock_ad`, `optimizers_lib`, `aes_autodiff_paper` and
`fad_seed_sharing_cost` suites:

| mutation | caught by |
|---|---|
| `0 − y` folded to `y` | `neuron_fad_lanes_match_the_closed_form`, `iir_fad_tangents_match_central_differences`, 2 more |
| `0 · x` folded to `x` | `neuron_fad_lanes_match_the_closed_form`, `iir_fad_equals_the_library_filter`, 1 more |
| `select2(c, 0, y)` folded to `0` | three `optimizers_lib` tests (`bank_loss_learns_the_string_from_above`, …) |
| any literal, not only 0, passed out of a wrapper | `fad_around_ondemand_constant_tangent_is_held_from_the_first_fire` (added for it: the first version of the test differentiated `2 · g`, whose tangent is not a literal, and let it through) |
| `PermVar(c)` folded for any literal child | equivalent: a hold's child is always a `Clocked` node, never a bare literal |

Without the fold, `fad_around_ondemand_folds_the_zero_tangent_of_an_unreached_input`
fails on its structural assertion.

Compile cost: the compile-budget basket contains no `fad` program, so the
folder is never constructed there (see the journal entry for the hand A/B).

## 7. What remains: the phantom domain's single expression

After the fix, at key 60 (no phantom partials) one seed costs one plain render
(105 ms plain, 202 ms with one seed). At key 40 the phantoms' `ondemand`
domain, which fires every sample below G#3, takes 444 ms of the 646: seven
times its plain cost for 2.9 times its operations. Its body is one sum of
several hundred pair products: in the plain program a single 54 KB
expression, in the differentiated one a 130 KB tangent expression over
2,100 temporaries that the primal sum shares, all live until that one
statement. Register pressure is the suspect (clang `-O1` runs that statement
at 2.2 operations per cycle against 5.1 for the plain one; Cranelift is
2.6–3.3 times slower than clang on the block). The W2 sinking pass cannot
help, since every temporary is read by the one statement. This is a FIR
scheduling question (splitting a large expression into statements so that
sinking can interleave the definitions), not a tangent rule, and it also
costs the plain voice; it is left to its own plan.
