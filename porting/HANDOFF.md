# Session Handoff

Date: 2026-10-10

## Repo State

- Branch: `main-dev`
- HEAD: the documentation commit on top of
  `8fb34c4507e24e3b059813d0692749da466c88f0`
- Tag `0.9.0` (annotated, "Release 0.9.0") on `5856b1eb`, **local only**: not
  pushed, `main` not fast-forwarded. `push_main.sh` does both
  (`git merge --ff-only main-dev` on `main`, then `git push --follow-tags`).

Recent commits (most recent first):

- this commit: the 2026-10-08 audit documentation (correction plan, registry
  entries, journal) and the `nffunction` proposal
- `8fb34c45` Tutorial: figures of 13.1 and 13.2 after two library fixes; why
  learning is sensitive to them
- `5856b1eb` Release 0.9.0
- `58bee35b` --version and the diagnostics JSON name the build's commit
- `2199d069` parser: make repair-order regression independent of recovery timeout

## Working Tree

- Tracked changes: none.
- Untracked local scripts `build_all`, `dummy_fdn` and `push_main.sh`, listed
  in `.git/info/exclude` (local, not versioned).

## What Changed (2026-10-10)

- [grame-cncm/faust-rs#20](https://github.com/grame-cncm/faust-rs/issues/20):
  `crates/compiler/build.rs` records the commit of `HEAD`. `--version` prints
  `faust-rs 0.9.0 (5856b1eb 2026-10-10)`, the diagnostics JSON `compiler`
  block has optional `commit`/`commit_date`, and generated code keeps the
  package version only. Registry `DIFF-CLI-013`. The issue has no reply yet.
- Release 0.9.0: workspace version, 70 CLI transcripts and 2 faustprobe
  snapshots, all version-only changes.
- Tutorial §0.1 (en/fr): why `fad`/`rad` learning is sensitive to fine
  library changes. The 13.1/13.2 figures were updated after two deliberate
  faustlibraries fixes, found by bisection.

## Decisions / Constraints

- The user chose the `rustc` form for the commit
  (`faust-rs X (hash date)` on the first line), not the C++ second line
  `Source commit:`. Uncommitted changes are not reflected (no `-dirty`).
- 56 CLI transcripts already differed from the CLI at `2199d069`
  (include-path order, generated code). That drift was left unrecorded; a
  re-record of it needs its own reviewed commit.
- The 2026-10-08 audit decisions still apply, unimplemented: see the
  [correction plan](foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md).
  The registry entries `DIFF-CLI-012`, `DIFF-BEH-005` (depth guards) and
  `DIFF-BACK-001` (foreign bindings) describe current, unfixed behavior.

## Validation Run

- `cargo fmt`, `clippy --workspace --all-targets -D warnings`, the five
  structure gates and `code-graphs --check`: pass.
- `cargo test --workspace --all-targets --no-fail-fast`: 3119 passed. One
  failure that CI skips: the live modulation differential against the local
  `/usr/local/bin/faust`, which is newer than the pinned reference
  (`modulation_35_in_recursion`'s controls). The two s13 tutorial failures
  are fixed in `8fb34c45`.
- `compile-budget-check` cannot measure on this machine. Its calibration DSP
  takes 3 ms, below the 4 ms floor, so the tool stops before measuring.

## Next Steps

1. Push when the user confirms: `./push_main.sh` (main and the `0.9.0`
   tag). The tag is before `8fb34c45`, so it can be moved first if the
   tutorial fix should be in 0.9.0.
2. Reply to issue #20 (Losera offered a PR; the change is done).
3. The audit work packages W1 to W7, in the plan's order.
4. Look at `compile-budget-check`'s calibration floor on fast machines.

## Useful Commands to Resume

```sh
git status --short
./target/debug/faust-rs --version
cargo run -q -p xtask -- cli-transcript-check
cargo test -p cranelift-ffi --test tutorial_examples
```
