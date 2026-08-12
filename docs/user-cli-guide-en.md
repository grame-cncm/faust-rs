# Faust-rs Compiler CLI Guide (User)

This guide documents the current user-facing options of the `compiler` binary.

## 1. Quick start

```bash
# Show scaffold version (no input file)
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

Only one mode can be selected at a time.

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

### `-e, --export-dsp`

Expand one DSP into a self-contained program: every `import`, every library
definition and every user abstraction is evaluated away, leaving a flat list of
`ID_<n>` definitions and a `process` binding.

```bash
cargo run -p compiler -- -e tests/corpus/rep_01_passthrough.dsp -o expanded.dsp
```

The result compiles with no library search path and produces the same DSP as
compiling the original. It is Faust source rather than generated code, so this
is a terminal mode: `-e --lang cpp` is rejected.

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

Run parse/eval/propagate and dump Signal IR text.

```bash
cargo run -p compiler -- --dump-sig tests/corpus/rep_01_passthrough.dsp
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

### `--dump-cranelift`

Compile through the experimental Cranelift backend and print the backend report.

```bash
cargo run -p compiler -- --dump-cranelift tests/corpus/rep_01_passthrough.dsp
```

### `--json`

Emit the strict Faust JSON description.

```bash
cargo run -p compiler -- --json tests/corpus/rep_01_passthrough.dsp
```

This can also be combined with `--lang <backend>` to emit a backend artifact
plus a companion `.json` file next to `-o <file>`.

### `--lang c|cpp|fir|interp|cranelift|julia|wasm|wast`

Faust-style backend language selector.

```bash
cargo run -p compiler -- --lang c tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang cpp tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang fir tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang interp tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang cranelift tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang julia tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- --lang wasm tests/corpus/rep_01_passthrough.dsp -o /tmp/out.wasm
cargo run -p compiler -- --lang wast tests/corpus/rep_01_passthrough.dsp
```

Legacy compatibility:

- `-lang c`, `-lang cpp`, `-lang fir`, `-lang interp`, `-lang cranelift`, `-lang julia`, `-lang wasm`, and `-lang wast` are accepted.
- `-lang -c` maps to `--lang c`.
- `-lang -cpp` maps to `--lang cpp`.
- `-lang -fir` maps to `--lang fir`.
- `-lang -interp` maps to `--lang interp`.

Installed binary examples:

```bash
faust-rs -lang c foo.dsp
faust-rs -lang cpp foo.dsp
faust-rs -lang fir foo.dsp
faust-rs -lang interp foo.dsp
faust-rs -lang cranelift foo.dsp
faust-rs -lang julia foo.dsp
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

Use double-precision internal DSP arithmetic (`-double` compatibility).

### `--mcd <n>` and `--dlt <n>`

Tune fast-lane delay lowering thresholds (`-mcd` / `-dlt` compatibility).

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
  `compute` is kept but emitted empty. Scalar mode only (`-os` with `-vec`
  is an error). Without `-ec`, control values are recomputed every frame.
- `-ec -os` combines both: one `control()` call followed by any number of
  `frame()` calls is the intended host schedule.

Supported backends: `c`, `cpp`, `rust`, and `fir` (the `-ec` FIR
combination is a faust-rs diagnostic extension). Other backends reject the
flags with a stable capability diagnostic (FRS-EXEC-*). Programs using the
foreign runtime variable `count` or block reverse-mode AD
(`BlockReverseAD`/`ReverseTimeRec` under `-os`) are rejected with typed
errors (FRS-SFIR-0009/0010). `-ec` also works in vector mode (`-ec -vec`),
where promoted control events are certificate-checked.

### `--no-fir-verify` and `--fir-verify-strict`

Control FIR verification before FIR dump / codegen.

### `--compilation-time` and `--timeout <secs>`

Print phase timings and set a global compilation timeout.

### `--fir-fixture <name>` and `--list-fir-fixtures`

Bypass DSP parsing and feed a built-in FIR fixture directly into FIR/backend
dump modes.

## 5. Diagnostics options

### `--error-format human|json`

- `human` (default): readable terminal diagnostics.
- `json`: structured diagnostics for tools/CI.

### `--error-verbosity standard|debug`

- `standard` (default): concise diagnostics.
- `debug`: includes low-level internal notes/fields.

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

- `--dump-cpp`
- `--dump-c`
- `--dump-fir`
- `--dump-fir-verify`
- `--dump-cranelift`
- `--json`
- `--lang c|cpp|fir|interp|cranelift|julia|wasm|wast`

Invalid with:

- `--parse`, `--dump-box`, `--dump-sig`, `--golden`, `-e`

Examples:

```bash
cargo run -p compiler -- --dump-cpp tests/corpus/rep_01_passthrough.dsp --signal-fir-lane fast
```

## 7. Mode rules and defaults

- With an input file and no explicit mode, default mode is C++ generation.
- Without input file and without mode, the command prints scaffold version.
- More than one mode at once is rejected.

## 8. Exit behavior

- Success: exit code `0`.
- Pipeline or I/O error: non-zero exit with diagnostics on stderr.
