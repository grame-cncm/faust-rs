# Session Handoff

Date: 2026-10-10 (evening)

## Repo State

- Branch: `main-dev`, 15 commits ahead of `main`. **Nothing is pushed.**
- HEAD: the `modulation_35_in_recursion` fixture fix on top of `984f7ef6`.
- Tag `0.9.0` (annotated, "Release 0.9.0") on `5856b1eb`, **local only**.
  It contains none of the commits listed below after it: the tutorial
  figures, the audit documentation and the #16/#21/#22 work. `push_main.sh`
  fast-forwards `main` to `main-dev` (`git merge --ff-only`) and runs
  `git push --follow-tags`. To have the work in 0.9.0, move the tag to
  HEAD before pushing.

Commits since `main` (most recent first):

- this commit: `modulation_35_in_recursion` no longer has an identically
  zero output (`DIFF-BEH-019`), and this handoff
- `984f7ef6` #16, every pipeline stage runs on the compile stack (`tlib::stack`)
- `fd5f6e73` an evaluation cycle is blamed on the use that closes it
  (follow-up to WP3, #22)
- `015d2a6e` WP5: an eval failure in library code is not labelled on a
  hash-consed occurrence
- `c535bd3c` WP6 + W2: one deadline rule, `--timeout 0` disables it, a
  timeout is a diagnostic in JSON too (`FRS-COMP-0008`)
- `15ad1d02` WP2: constant folding keeps its caches for the evaluation pass
  (quadratic → linear)
- `ac35afcd` WP4: the catch-all cause is left to internal forms
- `f0f0a424` WP3: an evaluation cycle names its definitions
  (`FRS-EVAL-0013`, #22)
- `8c5e262c` parser: a definition is located at its name, even when the name
  is used earlier
- `a06cf99a` WP1: a recursion on a non-constant numeric-pattern argument is
  named (`FRS-EVAL-0012`, #21)
- `5c7ac277` the analysis and correction plan for #21 and #22
- `20669a61` the 2026-10-08 audit documentation and the `nffunction` proposal
- `8fb34c45` Tutorial: figures of 13.1/13.2 after two library fixes
- `5856b1eb` Release 0.9.0 (tag)
- `58bee35b` `--version` and the diagnostics JSON name the build's commit (#20)

## Working Tree

- Tracked changes: none.
- Untracked local scripts `build_all`, `dummy_fdn` and `push_main.sh`, listed
  in `.git/info/exclude` (local, not versioned).

## What Changed (2026-10-10)

- [#20](https://github.com/grame-cncm/faust-rs/issues/20): `--version` prints
  `faust-rs 0.9.0 (<hash> <date>)`; the JSON `compiler` block has
  `commit`/`commit_date` (`DIFF-CLI-013`). Replied on the issue; it is still
  open, to close once pushed.
- [#21](https://github.com/grame-cncm/faust-rs/issues/21) and
  [#22](https://github.com/grame-cncm/faust-rs/issues/22): the
  [analysis and plan](eval-runaway-recursion-and-cycle-diagnostics-analysis-and-plan-2026-10-10-en.md)
  is fully carried out (WP1 to WP6). Registry `DIFF-BEH-017`, `DIFF-BEH-018`,
  `DIFF-CLI-012` updated. Fixtures `err_34_case_argument_not_constant.dsp` and
  `err_35_evaluation_cycle.dsp`.
- WP2 also fixes valid programs: `g(n-1, x-1)` with a slider took 35 s at 4000
  levels, now 0.10 s (C++ 2.84.3: 0.08 s).
- The same quadratic regression in C++ `master-dev` (commit `536ff8ca7`,
  `PropagateMemoScope`) is reported as
  [grame-cncm/faust#1345](https://github.com/grame-cncm/faust/issues/1345),
  Yann pinged. That issue also proposes a regression methodology for
  `tests/TESTING.md`.
- W2 of the 2026-10-08 plan is done (with WP6). W1 and W3 to W7 are still
  open.

## Issue #16 (deep programs abort the host)

`2d477161` (2026-09-08, in `origin/main`) grew the evaluator's stack. This
commit does the rest (journal entry "#16: a deep program compiles on a host
thread"):

- `tlib::stack::on_compile_stack`, the C++ `callFun` without a thread: a
  stage entered with less than 256 MiB left runs on a fresh 512 MiB stack.
  It is at the entry of 23 stages: the compiler's `pipeline_to_boxes`,
  `pipeline_boxes_to_signals` and FIR lowering, then propagation, signal
  preparation, FIR lowering and verification, and every backend's
  `generate_*_module`. `codegen` reaches it through `fir::on_compile_stack`.
- `on_deep_stack` (8 MiB segments) only where no budget bounds the
  recursion: the evaluator (plus `a2sb`) and the parser's import expansion.
- A per-function guard in every recursive pass was tried and dropped (the
  decision agreed with the user): too many functions, and every new pass
  would be exposed.
- Measured on an 8 MiB worker: all 12 CLI backends compile 30 000-level
  chains, and `1+1+...+1` at 1 000 000 levels reports `FRS-EVAL-0099`.

Left open, noted in the journal: 7 500 `~` in series and 30 000 `sin` in
parallel hit the 120 s timeout (a cost problem; no issue filed yet). C++ 2.90.6
accepts `s+s+...` at 30 000 terms, which the structural budget rejects.

## Decisions / Constraints

- Diagnostic codes: `FRS-EVAL-0012` (non-constant numeric-pattern argument),
  `FRS-EVAL-0013` (evaluation cycle), `FRS-COMP-0008` (timeout).
- `FRS-EVAL-0012` is chosen only once a depth budget has run out, so no
  accepted program changes. The conditions are: the same `case`, nested at
  least three times, a non-numeric argument at one position, and no numeric
  dispatch in those applications.
- A cycle is blamed on the identifier that re-enters the definition. Its
  owner is the cycle's last definition when that is top-level, since the
  identifier is hash-consed.
- **Security**: the Faust interval/memory-safety exploit witness must not be
  published; it goes to Yann privately.
- The `rustc` form for the commit in `--version` (user choice).
- 56 CLI transcripts already differed from the CLI at `2199d069`. That drift
  is still unrecorded and needs its own reviewed commit.

## Validation Run

- `cargo fmt`, `clippy --workspace --all-targets -D warnings`, the five
  structure gates and `code-graphs --check`: pass.
- `cargo test --workspace --all-targets --no-fail-fast` on the final tree
  (the #22 and #16 commits): 157 targets, 3142 passed. The one failure was
  the live modulation differential against the local `/usr/local/bin/faust`
  (CI skips it). The fixture commit fixes it: the fixture's output was
  identically zero, and C++ ≥ 2.90.4 (`1aafc196a`, found by bisection) folds
  it and drops its slider. `modulation_corpus` and `golden-check` pass. The intermediate commit `fd5f6e73` was checked alone with
  fmt, clippy, the `eval` tests, `diagnostic_errors` and the gates.
- `compile-budget-check` cannot measure on this machine. Its calibration DSP
  takes 3 ms, below the 4 ms floor, so it stops before measuring.
- The CLI timeout (`FRS-COMP-0008` in JSON) was checked by hand only. An
  automated test would depend on machine speed.

## Next Steps

1. Post the replies to #16, #21 and #22. The drafts are ready and await the
   user's confirmation.
2. Push when the user confirms: optionally move tag `0.9.0` to HEAD, then
   `./push_main.sh`. Then close #16, #20, #21 and #22.
3. File the cost issue for long series of `~` and wide `par` (120 s timeout).
4. The 2026-10-08 audit work packages W1 and W3 to W7, in the plan's order.
5. Look at `compile-budget-check`'s calibration floor on fast machines.

## Useful Commands to Resume

```sh
git status --short
git log --oneline main..main-dev
./target/debug/faust-rs --check tests/corpus/err_35_evaluation_cycle.dsp
cargo test -p compiler --test diagnostic_errors cycle
```
