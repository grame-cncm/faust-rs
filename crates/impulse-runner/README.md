# impulse-runner

Command-line impulse-test runner backed by the Rust compiler and Interpreter
runtime. It is the faust-rs counterpart of the scalar pass in the C++
`tools/impulseinterp.cpp` / `controlTools.h::runDSP` workflow.

## Usage

From the workspace root:

```bash
cargo run -p impulse-runner -- path/to/process.dsp
```

The runner writes reference-style `.ir` text to standard output, so it can be
redirected directly:

```bash
cargo run -p impulse-runner -- process.dsp -n 1024 > process.ir
```

Supported options:

| Option | Meaning |
|---|---|
| `-single` / `-double` | Select `f32` (default) or `f64` DSP execution |
| `-n <frames>` | Number of output frames (default: `15000`) |
| `-I <dir>` | Add a repeatable Faust import directory |
| `-vec` | Request checked vector lowering |
| `-vs <size>` | Vector chunk size (default: `32`) |
| `-lv <variant>` | Vector loop variant (`0` or `1`) |
| `-ss <n>` / `--scheduling-strategy <n>` | Select scheduling strategy |
| `--table-init <const\|runtime>` | How `rdtable`/`rwtable` content is produced (default: `runtime`) |

Unknown options are rejected instead of being silently ignored.

## Reference protocol

- sample rate: 44100 Hz;
- compute block size: 64 frames;
- first frame of every input channel set to `1`, then silence;
- button zones held at `1` for the first block, then released;
- values with magnitude below `1e-6` normalized to zero before printing.

The runner currently emits only the scalar reference pass, not the C++
polyphonic/MIDI passes. The default 15000 frames therefore match the prefix
used by the impulse-test comparison workflow.

Compiler work runs on a dedicated thread with a 64 MiB stack. This matches the
main `faust-rs` CLI contract and prevents deeply nested standard-library
expansion from depending on the platform main-thread stack size.

## Validation

```bash
cargo test -p impulse-runner
```
