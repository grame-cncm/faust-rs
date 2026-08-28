# Faust-rs Compiler CLI Guide (User)

This guide documents the current user-facing options of the `compiler` binary
(`faust-rs`).

## 1. Quick start

```bash
# Show scaffold version (no input file, no mode)
cargo run -p compiler

# Default compile mode with input file: C++ output on stdout
cargo run -p compiler -- tests/corpus/rep_01_passthrough.dsp
```

Installed binary usage:

```bash
# Install the CLI binary
cargo install --path crates/compiler

# Then use the installed command directly
faust-rs -lang c foo.dsp
faust-rs -lang cpp foo.dsp
faust-rs -lang fir foo.dsp
```

## 2. Main command form

```bash
cargo run -p compiler -- [MODE] <input.dsp> [OPTIONS]
```

Only one mode can be selected at a time, with one deliberate exception: `-e`
and `--json` do not emit backend code (one emits Faust source, the other a
JSON description), so pairing either with `-lang <backend>` is accepted — the
backend selection tags the output rather than conflicting with it. Pairing
`-e` with an actual code emitter (`--dump-cpp`, etc.) is still rejected.

## 3. Modes

### `--golden`

Generate golden snapshot text for one DSP file.

```bash
cargo run -p compiler -- --golden tests/corpus/rep_01_passthrough.dsp
```

Notes:

- `--import-dir` is not supported in this mode.

### `--parse`

Parse one DSP file and print parser status.

```bash
cargo run -p compiler -- --parse tests/corpus/rep_01_passthrough.dsp
```

### `--check`

Run the full front end (parse → eval → propagate → type) plus FIR
verification, with no codegen. Exits `0` with no diagnostics, `1` otherwise.

```bash
cargo run -p compiler -- --check tests/corpus/rep_01_passthrough.dsp
```

Unlike `--dump-fir-verify`, this mode prints no dump text: under
`--error-format json` it always emits a diagnostics payload — an empty
`diagnostics` array on success — so success and failure share one schema.
Under `--error-format human`, success prints `Check OK: 0 diagnostics`.
Incompatible with `--no-fir-verify` (verification always runs here).

### `-e, --export-dsp`

Expand one DSP into a self-contained program: every `import`, every library
definition and every user abstraction is evaluated away, leaving a flat list of
`ID_<n>` definitions and a `process` binding.

```bash
cargo run -p compiler -- -e tests/corpus/rep_01_passthrough.dsp -o expanded.dsp
```

The result compiles with no library search path and produces the same DSP as
compiling the original.

`-lang` is accepted alongside `-e` and recorded in `compile_options`, as in
C++: it selects a backend the expansion does not use. Two emitters do conflict,
so `-e --dump-cpp` is rejected.

Two lines head every expansion — `declare version` then
`declare compile_options` — followed by one `declare library_path<i>` per
library the program used, the whole `declare` metadata set, and the serialized
program.

Differences from C++ `faust -e`, both deliberate:

- with no `-o`, faust-rs prints to standard output; C++ opens an empty path and
  produces nothing;
- the version string, the option spelling, and the absolute library paths
  differ by construction. The serialized program does not.

### `--dump-box`

Parse and dump Box IR text.

```bash
cargo run -p compiler -- --dump-box tests/corpus/rep_01_passthrough.dsp
```

### `--dump-sig`

Run parse/eval/propagate and dump Signal IR text, one tree per output.

```bash
cargo run -p compiler -- --dump-sig tests/corpus/rep_01_passthrough.dsp
```

### `--dump-sig-dag`

Same information as `--dump-sig`, printed as one binding per interior node
instead of one tree per output, so shared structure appears once and node
identity is readable off the text.

Both `--dump-sig` and `--dump-sig-dag` show the *propagated* forest, before
normal-form staging: promotion casts, algebraic simplification, and the
`-ct` table clamps are not yet applied. Use `--dump-sig-dag-prepared` to see
those.

```bash
cargo run -p compiler -- --dump-sig-dag tests/corpus/rep_01_passthrough.dsp
```

### `--dump-sig-dag-prepared`

Like `--dump-sig-dag`, but after the signal-preparation staging pipeline:
symbolic recursion (`SYMREC`/`SYMREF`), promotion casts, algebraic
simplification, and — under the default `--check-table 1` — the table
clamps inserted by the check-table pass (`SIGMAX(0, SIGMIN(idx, size-1))`).
This is the closest textual view of what FIR lowering actually consumes.

```bash
cargo run -p compiler -- --dump-sig-dag-prepared tests/corpus/rep_01_passthrough.dsp
```

### `--dump-fir`

Run parse/eval/propagate + signal->FIR lowering and dump FIR IR text.

```bash
cargo run -p compiler -- --dump-fir tests/corpus/rep_01_passthrough.dsp
```

### `--dump-cpp`

Generate C++ backend output text.

```bash
cargo run -p compiler -- --dump-cpp tests/corpus/rep_01_passthrough.dsp
```

### `--dump-c`

Generate C backend output text.

```bash
cargo run -p compiler -- --dump-c tests/corpus/rep_01_passthrough.dsp
```

### `--dump-interp`

Compile to interpreter bytecode and print `.fbc` text. Equivalent to
`-lang interp`.

```bash
cargo run -p compiler -- --dump-interp tests/corpus/rep_01_passthrough.dsp
```

### `--dump-cranelift`

Compile through the experimental Cranelift backend and print the backend report.

```bash
cargo run -p compiler -- --dump-cranelift tests/corpus/rep_01_passthrough.dsp
```

### `--dump-fir-verify`

Run the FIR verifier and print the verification report without backend codegen.

```bash
cargo run -p compiler -- --dump-fir-verify tests/corpus/rep_01_passthrough.dsp
```

### `--dump-cpp-from-fbc`

Read interpreter `.fbc` text and emit self-contained native C++.

```bash
cargo run -p compiler -- --dump-cpp-from-fbc foo.fbc --cpp-class-name MyInterpDsp
```

The input file must have a `.fbc` extension. Incompatible with
`--signal-fir-lane`, `--import-dir`, `-a`/`-A`/`-i`, `--fir-fixture`, and
`--super-class-name`. `--cpp-class-name` is only valid with this mode (use
`-cn`/`--class-name` for ordinary DSP-to-C++ generation).

### `--json`

Emit the strict Faust JSON description.

```bash
cargo run -p compiler -- --json tests/corpus/rep_01_passthrough.dsp
```

This can also be combined with `--lang <backend>` to emit a backend artifact
plus a companion `.json` file next to `-o <file>` (required in that
combination). It combines the same way with `--dump-fir`, `--dump-interp`,
and `--dump-cranelift`.

### `--svg`

Render the block diagram of `process` as SVG files into `<name>-svg/`, where
`<name>` is the input file's stem.

```bash
cargo run -p compiler -- --svg tests/corpus/rep_01_passthrough.dsp
```

See [SVG diagram options](#7-svg-block-diagram-generation) for the layout flags.

### `--lang asc|c|cmajor|codebox|codebox-test|cpp|cranelift|fir|interp|julia|rust|wasm|wast`

Faust-style backend language selector.

```bash
cargo run -p compiler -- --lang asc tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang c tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang cmajor tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang codebox tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang codebox-test tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang cpp tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang cranelift tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang fir tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang interp tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang julia tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang rust tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang wasm tests/corpus/rep_01_passthrough.dsp -o /tmp/out.wasm
cargo run -p compiler -- --lang wast tests/corpus/rep_01_passthrough.dsp
```

For the backends that also have a dedicated flag, `-lang c`, `-lang cpp`,
`-lang fir`, `-lang interp` and `-lang cranelift` are equivalent to
`--dump-c`, `--dump-cpp`, `--dump-fir`, `--dump-interp` and
`--dump-cranelift`. The rest (`asc`, `cmajor`, `codebox`, `codebox-test`,
`julia`, `rust`, `wasm`, `wast`) are reachable only through `-lang`.

`codebox-test` is a second spelling of the `codebox` backend (RNBO codebox)
that prefixes generated parameter names with `RB_`; it changes only naming,
not behavior.

`clap` also accepts a few short value aliases: `c99` for `c`, `cxx`/`c++` for
`cpp`, `clif` for `cranelift`, `interp-fbc` for `interp`, `jl` for `julia`,
`rs` for `rust`, `wat` for `wast`.

Legacy compatibility (raw process arguments, before `clap` parsing):

- `-lang asc`, `-lang c`, `-lang cmajor`, `-lang codebox`, `-lang codebox-test`, `-lang cpp`, `-lang cranelift`, `-lang fir`, `-lang interp`, `-lang julia`, `-lang rust`, `-lang wasm`, and `-lang wast` are accepted.
- `-lang -c` maps to `--lang c`.
- `-lang -cpp` maps to `--lang cpp`.
- `-lang -fir` maps to `--lang fir`.
- `-lang -interp` maps to `--lang interp`.

Installed binary examples:

```bash
faust-rs -lang c foo.dsp
faust-rs -lang cpp foo.dsp
faust-rs -lang cranelift foo.dsp
faust-rs -lang fir foo.dsp
faust-rs -lang interp foo.dsp
faust-rs -lang julia foo.dsp
faust-rs -lang rust foo.dsp
faust-rs -lang wasm foo.dsp -o foo.wasm
faust-rs -lang wast foo.dsp
```

If your command is named `faust` (symlink/wrapper), the same commands work:

```bash
faust -lang c foo.dsp
faust -lang cpp foo.dsp
faust -lang fir foo.dsp
faust -lang interp foo.dsp
```

## 4. Common options

### `-o, --output <file>`

Write text output to a file instead of stdout.

```bash
cargo run -p compiler -- --dump-cpp tests/corpus/rep_01_passthrough.dsp -o /tmp/out.cpp
```

For `--lang wasm`, `-o` writes the `.wasm` file and also writes the companion
JSON file next to it with the same stem.

### `-I, --import-dir <dir>`

Add import search directories. Can be repeated.

```bash
cargo run -p compiler -- --dump-sig main.dsp -I ./lib -I ./third_party/faust
```

### `-cn, --class-name <name>`

Specify the DSP class name used instead of `mydsp`.

### `-scn, --super-class-name <name>`

Specify the DSP superclass name used instead of `dsp`. Only meaningful for
C++ output or architecture wrapping.

### `--cpp-class-name <name>`

Override the generated C++ class name for `--dump-cpp-from-fbc`. Distinct
from `-cn`/`--class-name`, which applies to ordinary DSP-to-code generation;
using `--cpp-class-name` outside `--dump-cpp-from-fbc` is rejected.

### `-pn, --process-name <name>`

Specify the top-level DSP entry-point name instead of `process`.

### `-a, --architecture <file>`

Wrapper architecture file. Currently supported only for C/C++/Cmajor/Julia
output (rejected for FIR, JSON, interp, Cranelift, WASM, WAST, AssemblyScript,
and Rust output).

```bash
cargo run -p compiler -- -lang cpp foo.dsp -a myarch.cpp
```

### `-A, --architecture-dir <dir>`

Additional architecture search directories. Can be repeated. Requires
`-a`/`--architecture`.

### `-i, --inline-architecture-files`

Inline `#include <faust/...>` architecture files. Requires
`-a`/`--architecture`.

### `--allow-network-imports`

Permit explicit HTTP(S) entry sources, structural imports, and main
architecture templates for this invocation. Networking has two independent
gates: the binary must be built with the default-off `network-imports` Cargo
feature, and this runtime option must be present. There is no network fallback
after an ordinary local import miss.

```bash
cargo run -p compiler --features network-imports -- \
  --allow-network-imports -lang cpp https://example.test/main.dsp

cargo run -p compiler --features network-imports -- \
  --allow-network-imports -lang cpp local.dsp \
  -a https://example.test/architecture.cpp
```

The native CLI permits any HTTP(S) host after explicit opt-in. Server or
multi-user embeddings should instead use the Rust `Compiler` API with a
restricted `RemoteUrlPolicy`. Browser-WASM performs no network I/O internally,
but its raw ABI accepts host-prefetched canonical URL/source bundles for
structural remote import graphs. C/C++ compatibility facades, remote
`component(...)`/`library(...)`, and remote inline architecture sub-includes
remain network-disabled.

### `--double`

Use double-precision internal DSP arithmetic (`-double` compatibility). The
external DSP interface (`FAUSTFLOAT` audio buffers and UI zones) always stays
at the type declared by the architecture file; only internal calculations
switch to `double`.

### `--memory-manager` (`-mem` / `-mem0` compatibility)

Use the host custom memory manager for eligible native DSP state. The four
spellings (`--memory-manager`, `--memory-manager0`, `-mem`, `-mem0`) select
the same typed `mem0` mode. Only mode zero is implemented: scalar-only
(rejected with `-vec`), and restricted to the `c`, `cpp`, and `cranelift`
backends (or the default C++ mode when no backend is selected); `mem1`
through `mem3` are deliberately rejected.

### `--mcd <n>`

Maximum delay (in samples) below which the shift/copy strategy is used
instead of a circular ring buffer (`-mcd` compatibility). Delays ≤ `mcd` use
a statically-shifted array (no `fIOTA`). Default: `16`.

### `--dlt <n>`

Delay-line threshold above which the if-based wrapping strategy is used
instead of the default power-of-two circular buffer (`-dlt` compatibility).
Delays > `dlt` use an exact-size buffer with a per-line counter variable.
Default: disabled (all delays above `mcd` use circular-pow2).

### `--check-table <0|1>`

Check table index ranges and generate safe accesses (`-ct` compatibility).
With `1` (the default, matching the reference compiler), every
`rdtable`/`rwtable` index the interval analysis cannot prove in-bounds is
clamped at the signal level to `max(0, min(index, size-1))`, before FIR
lowering — the clamp is visible in `--dump-sig-dag-prepared` and in the
generated code. With `0`, unprovable accesses are generated raw and an
out-of-range index is undefined behavior, exactly the reference `-ct 0`
contract. Under `--warn`, each clamped site is reported as a
`WARNING : RDTbl read index [lo:hi] is outside of table size (N)` line.

### `--table-init runtime|const`

How the initial content of `rdtable`/`rwtable` tables is produced (`-table-init`
compatibility).

- `runtime` (default): compiles each table generator into a sub-module whose
  `fill` function computes the content at initialization time, as the C++
  reference does. The only mode that can express content depending on the
  sample rate or on a foreign function.
- `const`: evaluates the generator at compile time and emits a literal
  initializer list. A generator reading `ma.SR` also requires
  `--table-init-sample-rate HZ`.

### `--table-init-sample-rate <hz>`

Sample rate embedded when `--table-init const` folds a generated table that
reads `ma.SR`. Required for that dependency; ignored otherwise.

### `--vec`, `--vs <n>`, `--lv 0|1`

Vector mode (`-vec`/`-vs`/`-lv` compatibility): restructure `compute()` into
an outer chunk loop so the C compiler can auto-vectorize the inner loops
(SIMD).

- `--vec`: enable vector mode. Selection is checked — a program shape the
  vector pipeline cannot certify falls back to scalar lowering instead of
  emitting unverified code, and certified vector output is bit-exact against
  scalar output for the same program.
- `--vs <n>`: vector size. Default: `32`.
- `--lv 0|1`: vector loop variant, as Faust C++. `0` (default, fastest) is a
  constant-trip main loop over `count - count % vs` plus a scalar remainder,
  the autovectorization-friendly form; `1` (simple) is a single loop with a
  runtime `min(vindex + vs, count)` bound.

`-vec` is rejected for the `codebox` and `cmajor` backends (their output is
inherently per-sample).

### `--scheduling-strategy <n>` (`-ss` compatibility)

Signal/loop dependency scheduling strategy, as Faust C++: `0` = depth-first
(default), `1` = breadth-first, `2` = special (interleaved), `n >= 3` =
reverse breadth-first. Independent of `-vec`/`-vs`/`-lv`: it drives the
scalar control/signal schedule and the checked vector loop schedule. Unlike
C++'s `atoi`, a missing value, a non-integer value, or a negative value is a
hard parse error here rather than a silent fallback to `0`.

### `-ec, --external-control` and `-os, --one-sample`

Execution options ported from C++ Faust (the legacy `--ext-control`
spelling is also accepted):

- `-ec` moves control-rate computations out of the block entry point into a
  separate `control` function that the host schedules explicitly. Slow
  values are promoted to DSP state; neither initialization, `compute`, nor
  `frame` calls `control` implicitly, and previously stored values stay
  unchanged until the host calls it.
- `-os` emits a one-sample `frame(inputs, outputs)` entry point over flat
  channel arrays — no block count, no sample loop. The canonical block
  `compute` is kept but emitted empty (except for `codebox`/`cmajor`, which
  have no block `compute` to keep). Scalar mode only (`-os` with `-vec`
  is an error). Without `-ec`, control values are recomputed every frame.
- `-ec -os` combines both: one `control()` call followed by any number of
  `frame()` calls is the intended host schedule.

Support by backend:

- `c`, `cpp`, `rust`, `fir`, `asc`: explicit — `-ec`/`-os` change the emitted
  output shape (the `-ec` FIR combination is a faust-rs diagnostic
  extension).
- `codebox`, `cmajor`: intrinsic — the target's own contract already ticks
  one sample at a time with external controls, so passing `-ec`/`-os` or
  omitting them produces identical output; `codebox` also rejects `-vec`
  outright since it has no block loop.
- `interp`, `cranelift`, `wasm`, `wast`, `julia`: unsupported — rejected with
  a stable capability diagnostic (`FRS-EXEC-*`).

Programs using the foreign runtime variable `count` or block reverse-mode AD
(`BlockReverseAD`/`ReverseTimeRec`) under `-os` are rejected with typed
errors (FRS-SFIR-0009/0010). `-ec` also works in vector mode (`-ec -vec`),
where promoted control events are certificate-checked.

### `--no-fir-verify` and `--fir-verify-strict`

Control FIR verification before FIR dump / codegen. `--no-fir-verify` is
incompatible with `--dump-fir-verify` and `--check` (both require
verification to run).

### `--warn`

Report non-blocking semantic warnings, such as a math operation whose operand
may leave its domain at run time — the class the reference compiler reports
under `-wall`/`-me`. Off by default. Warnings go to stderr in the selected
`--error-format` and never change the exit status.

Also reports each table access clamped by the check-table pass
(`--check-table`, on by default) as a plain
`WARNING : RDTbl read index [lo:hi] is outside of table size (N)` stderr
line — the report C++ prints under `-wall` from its own table promotion.

### `--compilation-time` and `--timeout <secs>`

`--compilation-time` (`-time` compatibility) prints per-phase timing lines to
stderr. `--timeout` sets a global compilation timeout in seconds (default:
`120`; `0` disables the watchdog).

### `--fir-fixture <name>` and `--list-fir-fixtures`

Bypass DSP parsing and feed a built-in FIR fixture directly into FIR/backend
dump modes (`fir`, `c`, `cpp`, `interp`, `cranelift`, `wasm`, `wast`, `json`).
Intended for backend debugging / bring-up.

```bash
cargo run -p compiler -- --list-fir-fixtures
cargo run -p compiler -- --fir-fixture <name> -lang cpp
```

`--fir-fixture` is incompatible with a DSP input file, `--golden`, `--parse`,
`--dump-box`, `--dump-sig`, `--dump-sig-dag`, `--dump-sig-dag-prepared`,
`--check`, `--signal-fir-lane`, and `--import-dir`. `--list-fir-fixtures` does not accept `--fir-fixture` or
an input file.

## 5. Diagnostics options

### `--error-format human|json`

- `human` (default): readable terminal diagnostics.
- `json`: structured diagnostics for tools/CI.

### `--error-verbosity concise|standard|debug|full`

A ladder of progressive disclosure; each level shows everything the previous
one did, plus more.

- `concise`: header, primary location, and the shortest safe fix.
- `standard` (default): everything needed to act — all relevant labels, rule
  and computed facts, traces, and fixes.
- `debug`: standard plus internal ids and typed debug context.
- `full`: debug plus untruncated traces and related diagnostics.

### `--diagnostic-paths absolute|relative|basename`

How source paths are spelled in rendered human diagnostics.

- `absolute` (default): the path exactly as the compiler recorded it.
- `relative`: relative to the working directory when that is shorter — keeps
  CI logs and shared transcripts readable without hiding which file is meant.
- `basename`: file name only, for sharing a diagnostic without disclosing
  directory structure.

Presentation only: the JSON diagnostics channel always reports the compiled
source path verbatim, because a tool resolving a range needs the path the
compiler actually used.

### `--help-error-format`

Print a dedicated summary for diagnostics options and exit.

```bash
cargo run -p compiler -- --help-error-format
```

For interpretation details, see `docs/user-diagnostics-guide-en.md`.

## 6. Signal->FIR lane selection

### `--signal-fir-lane fast`

Select the lowering lane used before FIR-backed outputs.

- `fast`: transform fast-lane.

Default in the CLI: `fast` when option is omitted.

Valid with:

- `--check`
- `--dump-cpp`
- `--dump-c`
- `--dump-fir`
- `--dump-fir-verify`
- `--dump-interp`
- `--dump-cranelift`
- `--json`
- `--lang asc|c|cmajor|codebox|codebox-test|cpp|cranelift|fir|interp|julia|rust|wasm|wast`

Invalid with:

- `--parse`, `--dump-box`, `--dump-sig`, `--dump-sig-dag`,
  `--dump-sig-dag-prepared`, `--golden`, `-e`
- `--fir-fixture` (the input is already FIR)

Examples:

```bash
cargo run -p compiler -- --dump-cpp tests/corpus/rep_01_passthrough.dsp --signal-fir-lane fast
```

## 7. SVG block-diagram generation

`--svg` (see [Modes](#--svg)) accepts these layout options, ported from
C++ Faust:

- `--shadow-blur` (`-blur`): add a Gaussian drop-shadow to boxes.
- `--scaled-svg` (`-sc`): emit a viewBox-only (responsive) header instead of a
  fixed mm size.
- `--draw-route-frame` (`-drf`): draw a visible frame around route boxes.
- `--max-name-size <n>` (`-mns`): maximum label length before truncation.
  Default: `40`.
- `--fold <n>` (`-f`): fold diagrams with complexity above `n` into separate
  files. `0` disables folding. Default: `25`.
- `--fold-complexity <n>` (`-fc`): minimum per-expression complexity to
  trigger folding. Default: `2`.

These flags are only meaningful together with `--svg`.

## 8. Informational flags

These print one line and exit before any compilation.

- `-v, --version` (`-version` compatibility): print `faust-rs <version>` plus
  copyright text.
- `--libdir` (`-libdir`): print the directory containing libfaust libraries.
- `--includedir` (`-includedir`): print the directory containing Faust
  headers.
- `--archdir` (`-archdir`): print the directory containing Faust architecture
  files.
- `--dspdir` (`-dspdir`): print the directory containing Faust DSP libraries.
- `--pathslist` (`-pathslist`): print architecture and DSP library search
  paths.

When more than one is given, `--libdir` takes precedence, then
`--includedir`, `--archdir`, `--dspdir`, `--pathslist` — matching C++ Faust's
`global::printDirectories()` order.

## 9. Mode rules and defaults

- With an input file and no explicit mode, default mode is C++ generation.
- Without input file and without mode, the command prints scaffold version.
- More than one mode at once is rejected, except `-e`/`--json` combined with
  `-lang <backend>` (see [Main command form](#2-main-command-form)).

## 10. Legacy flag compatibility

Raw process arguments are normalized to their `clap`-based equivalents before
parsing, so these historical Faust spellings keep working:

| Legacy | Maps to |
| --- | --- |
| `-lang <v>` | `--lang <v>` (`-c`/`-cpp`/`-fir`/`-interp` values map to `c`/`cpp`/`fir`/`interp`) |
| `-pn <name>` | `--process-name <name>` |
| `-cn <name>` | `--class-name <name>` |
| `-scn <name>` | `--super-class-name <name>` |
| `-double` | `--double` |
| `-json` | `--json` |
| `-mem`, `-mem0` | `--memory-manager` |
| `-version` | `--version` |
| `-libdir` | `--libdir` |
| `-includedir` | `--includedir` |
| `-archdir` | `--archdir` |
| `-dspdir` | `--dspdir` |
| `-pathslist` | `--pathslist` |
| `-mcd <n>` | `--mcd <n>` |
| `-dlt <n>` | `--dlt <n>` |
| `-ct <0\|1>` | `--check-table <0\|1>` |
| `-table-init <v>` | `--table-init <v>` |
| `-vec` | `--vec` |
| `-vs <n>` | `--vs <n>` |
| `-lv <n>` | `--lv <n>` |
| `-ss <n>` | `--scheduling-strategy <n>` |
| `-ec`, `--ext-control` | `--ec` (`--external-control`) |
| `-os` | `--os` (`--one-sample`) |
| `-time` | `--compilation-time` |
| `-svg` | `--svg` |
| `-blur` | `--shadow-blur` |
| `-sc` | `--scaled-svg` |
| `-drf` | `--draw-route-frame` |
| `-mns <n>` | `--max-name-size <n>` |
| `-f <n>` | `--fold <n>` |
| `-fc <n>` | `--fold-complexity <n>` |
| `-timeout <secs>` | `--timeout <secs>` |

## 11. Exit behavior

- Success: exit code `0`.
- Pipeline or I/O error: non-zero exit with diagnostics on stderr.
- Invalid command line (unusable flag combination, more than one mode, etc.):
  exit code `2`.
