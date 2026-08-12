# `-e` export-DSP corpus

Fixtures and captured C++ reference expansions for the `-e` / `--export-dsp`
option. See
[`porting/export-dsp-e-option-and-libfaust-api-port-plan-2026-08-12-en.md`](../../porting/export-dsp-e-option-and-libfaust-api-port-plan-2026-08-12-en.md).

## Layout

- `dsp/` — one fixture per construct family; each is a complete Faust program.
- `oracle/` — the corresponding `faust -e` output, captured from the reference
  C++ compiler.

## Regenerating the oracle

```bash
cargo run -p xtask -- expand-oracle
```

Verify without rewriting:

```bash
cargo run -p xtask -- expand-oracle --check
```

The reference binary is `FAUST_CPP_BIN`, then the pinned local build, then
`faust` on `PATH`.

## Normalized lines

Three lines cannot be recorded verbatim without tying the corpus to the machine
that captured it, so they are replaced with placeholders:

| Line | Placeholder | Reason |
|---|---|---|
| `declare version` | `"<version>"` | reference compiler version |
| `declare compile_options` | `"<options>"` | embeds the absolute paths passed on the command line |
| `declare library_path<i>` | `"<path>"` | absolute installation paths |

The line's presence and position remain asserted; only the value is elided.

## Fixtures without an oracle

Two kinds of fixture legitimately have no captured expansion, and the capture
tool reports them as skipped rather than failing:

- **faust-rs extensions** the reference binary does not implement — currently
  `031_fad`.
- **`034_downsampling`**, which the reference binary cannot expand at all:
  `boxppShared::print()` tests `isBoxUpsampling` twice
  (`compiler/boxes/ppbox.cpp:615-617`) and never reaches a `BoxDownsampling`
  branch, so `faust -e` fails with
  `boxppShared::print() : BoxDownsampling[...] is not a valid box`. The
  non-shared `boxpp` printer handles the node correctly at
  `compiler/boxes/ppbox.cpp:467`, which is why the bug only shows through `-e`.
  faust-rs prints `downsampling(...)`; the fixture stays in the corpus so the
  Rust-side checks cover it.
