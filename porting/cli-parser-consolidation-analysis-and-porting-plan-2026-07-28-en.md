# CLI Parser Consolidation Analysis and Porting Plan

Date: 2026-07-28

Status: proposed

Scope: Cargo binaries, examples, benchmarks, and `xtask` commands that parse
process command-line arguments

## 1. Motivation

A code review observed that the workspace contains "multiple custom CLI
argument parsers." The observation is correct and broader than a few isolated
helpers.

The production `faust-rs` binary already uses `clap` in
`crates/compiler/src/cli/args.rs`. It normalizes legacy Faust spellings before
calling `CliArgs::parse_from`, which is an appropriate separation between
compatibility translation and argument validation.

Other executable targets still:

- iterate over `std::env::args` directly;
- recognize options with handwritten `match` statements;
- consume option values manually;
- maintain usage strings independently from accepted arguments;
- choose inconsistent diagnostics and exit statuses;
- parse paths through UTF-8 `String` even when `PathBuf`/`OsString` is more
  appropriate.

This conflicts with the repository rule that `clap` is the default parser for
user-facing binaries unless an alternative has an explicit documented reason.
It also creates real maintenance cost: the Interpreter and Cranelift impulse
runners already duplicate nearly the same option grammar, while `xtask`
contains a top-level dispatcher plus many command-specific parsers.

The goal is not to centralize the handwritten logic into a larger custom
parser. The goal is to make every ordinary process CLI declarative and typed,
while keeping protocol parsers outside the migration.

## 2. Current inventory

### 2.1 Compliant production CLI

| Target | Current mechanism | Decision |
|---|---|---|
| `faust-rs` | `normalize_legacy_args(std::env::args())` followed by `clap::Parser` | Keep; treat the normalizer as compatibility translation, not parsing |

The compiler CLI is the reference pattern: legacy spellings are translated,
then `clap` owns arity, type validation, conflicts, help, and diagnostics.

### 2.2 Cargo binaries with handwritten parsers

| Target | Source | Current grammar |
|---|---|---|
| `impulse-runner` | `crates/impulse-runner/src/main.rs` | DSP path, `-double`, `-single`, `-n`, `-I`, `-vec`, `-vs`, `-lv`, `-ss` |
| `impulse_cranelift` | `crates/cranelift-ffi/src/bin/impulse_cranelift.rs` | Near-duplicate of the Interpreter runner, forwarding compiler options to the C API |
| `xtask` | `crates/xtask/src/main.rs` and command modules | Top-level command dispatch plus command-specific option iterators |
| `treearena_bench` | `crates/tlib/src/bin/treearena_bench.rs` | Optional size and `--prealloc` |

The impulse parsers reject unknown flags, but duplicate option recognition,
missing-value checks, numeric parsing, and positional-argument validation.
Their output types legitimately differ: the Interpreter runner constructs
`ComputeMode` and `SchedulingStrategy`, whereas the Cranelift runner forwards
canonical compiler arguments to its FFI factory. That difference does not
justify two parsing implementations.

### 2.3 Cargo examples and developer tools with handwritten parsing

| Target | Source | Current grammar |
|---|---|---|
| `interp_baseline` | `crates/codegen/examples/interp_baseline.rs` | Default fixtures, `--fixture NAME`, or `--fbc PATH` |
| `count_vector_corpus` | `crates/compiler/examples/count_vector_corpus.rs` | Positional loop/strategy values plus precision, JSON, filter, shard, and comparison options |
| `corpus_scan_cranelift` | `crates/compiler/examples/corpus_scan_cranelift.rs` | Free positional filters |
| `compute_bench` | `crates/cranelift-ffi/examples/compute_bench.rs` | DSP path or `--fixture NAME` |
| `export_clif` | `crates/cranelift-ffi/examples/export_clif.rs` | Input and output paths |

These tools are not distributed compiler APIs, but they are documented or
repeatable developer interfaces. They benefit from the same typed parsing and
testability as production binaries.

### 2.4 `xtask` concentration

`xtask` is the largest source of custom parsing. Its main module matches a
command string and forwards the remaining iterator. Handwritten option loops
then appear in:

- four runtime-trace parsers;
- FIR dump scanning;
- backend smoke and nightly alignment;
- corpus status queries;
- code graph generation;
- Wasm module building;
- vector coverage merge/check;
- vector compile budget checking;
- emission determinism;
- libfaust API matrix generation;
- libfaust export/build commands;
- P7 matrix reporting.

There are at least eighteen command-option loops in addition to the top-level
dispatcher. The current module header explicitly says local parsing avoids a
CLI dependency, but `xtask` is itself a CLI and not a runtime dependency of the
compiler. The rationale does not offset the duplication and does not satisfy
the repository rule requiring a documented exception.

## 3. Out-of-scope parsers

The migration must distinguish a process CLI from an embedded argument
protocol.

### 3.1 FFI compiler argument vectors

`ffi_common::parse_ffi_compile_args` parses `argc`/`argv` supplied through C
APIs. Those values are part of the external Faust compatibility protocol, not
the process command line. They must preserve the accepted upstream-style
tokens and must not invoke `clap`.

The same exclusion applies to:

- FFI factory wrappers that decode C argument arrays;
- compiler helpers that inspect already-received options for JSON naming or
  auxiliary-file behavior;
- parsers for persisted reports, traces, FIR, FBC, CLIF, JSON, or other data
  formats.

### 3.2 Standalone architecture source

`tests/impulse-tests/archs/impulserust.rs` is compiled as a standalone
architecture artifact and intentionally cannot assume a Cargo dependency on
`clap`. Its small `-n` parser remains an explicit dependency-free exception.
The exception must be recorded next to the source and in the CLI policy check.

### 3.3 Faust legacy-token normalization

A function that only maps compatibility aliases such as `-double` to
`--double` is not an alternative parser if:

- it does not validate values;
- it does not resolve conflicts;
- it does not apply defaults;
- its output is always parsed by `clap`.

This narrow normalizer remains allowed for interfaces that must accept
historical Faust single-dash multi-character options.

## 4. Target architecture

### 4.1 Workspace dependency

Move the existing dependency declaration to the workspace:

```toml
[workspace.dependencies]
clap = { version = "4", features = ["derive"] }
```

Each participating package then declares:

```toml
clap.workspace = true
```

The compiler keeps its existing version and behavior; this is dependency
declaration consolidation, not a library upgrade.

### 4.2 `xtask` command model

Add a typed CLI root, preferably in `crates/xtask/src/cli.rs`:

```rust
#[derive(clap::Parser)]
pub(crate) struct XtaskCli {
    #[command(subcommand)]
    pub(crate) command: XtaskCommand,
}

#[derive(clap::Subcommand)]
pub(crate) enum XtaskCommand {
    GoldenCheck,
    InterpTraceDump(InterpTraceDumpArgs),
    CorpusStatusQuery(CorpusStatusQueryArgs),
    LibfaustExportCheck(LibfaustExportCheckArgs),
    // ...
}
```

Command modules should own their `#[derive(clap::Args)]` structures and
value-domain enums. Existing execution functions should continue to accept
typed option structures so parsing remains separate from work.

The handwritten `USAGE` constant must stop being the source of truth.
Generated `clap` help becomes authoritative; `crates/xtask/README.md` remains
the longer workflow guide.

`golden-gen-cpp -- <extra arguments>` is a required special case. It must use
`OsString` passthrough with `last`/`trailing_var_arg` semantics so arbitrary
non-UTF-8 paths and hyphen-prefixed arguments continue to reach the C++ Faust
process unchanged.

### 4.3 Impulse runners

Each runner should define a small declarative `RunnerArgs` structure. A new
shared crate is not justified: it would add a workspace boundary solely to
share a few field declarations, and making an FFI adapter depend on a tool
crate would invert the intended dependency direction.

The two local structures may be similar, but they must contain no handwritten
token-consumption loop. Their post-parse conversions can remain
backend-specific:

- Interpreter: `RunnerArgs -> ComputeMode + SchedulingStrategy`;
- Cranelift: `RunnerArgs -> canonical FFI compiler argv`.

Legacy spellings must remain accepted:

| Existing spelling | Canonical Clap spelling |
|---|---|
| `-double` | `--double` |
| `-single` | `--single` |
| `-vec` | `--vectorize` or the selected canonical long name |
| `-vs N` | `--vector-size N` |
| `-lv N` | `--loop-variant N` |
| `-ss N` | `--scheduling-strategy N` |
| `-I DIR` | `--import-dir DIR` |
| `-n N` | `-n N` / `--frames N` |

A narrow pre-parser normalizer may translate the legacy forms before
`RunnerArgs::try_parse_from`. `clap` must then perform required-value,
numeric-type, unknown-option, conflict, and positional-arity validation.

The current last-option-wins behavior of `-double`/`-single` must either be
preserved with explicit overrides or deliberately changed and recorded during
the baseline phase.

### 4.4 Secondary tools

Developer tools should use the smallest suitable declarative form:

- typed positional `PathBuf` for input/output paths;
- `ValueEnum` for fixture, precision, and output-format choices;
- bounded value parsers for loop variants, shard components, and frame counts;
- `Vec<String>` only for intentionally free-form filters;
- mutually exclusive groups for DSP-path versus fixture/FBC modes.

Examples that no longer serve a documented or tested workflow may be removed
instead of migrated, but removal must be decided per target and recorded.

## 5. Compatibility contract

### 5.1 Preserved behavior

The migration must preserve:

- all successful invocations used by CI, Makefiles, README files, and scripts;
- Faust-compatible legacy spellings used by impulse tests;
- default values and accepted numeric domains;
- repeated options such as `-I` and repeated `--case`;
- `--` passthrough for C++ Faust;
- output payloads produced by successful commands;
- repository-relative default paths;
- non-UTF-8-capable path handling where the command delegates to another
  process or filesystem API.

### 5.2 Adapted behavior

Invalid command lines should adopt normal `clap` behavior:

- generated error plus relevant usage;
- exit status `2` for syntax/validation failures;
- exit status `0` for `--help` and `--version`;
- unknown `xtask` commands rejected instead of printing usage and returning
  success.

This is an intentional user-visible adaptation. CI must confirm that no script
depends on the current unknown-command success status before the change lands.
Exact whitespace and line wrapping of help text are not stable contracts;
accepted tokens, defaults, exit status, and semantic diagnostics are.

### 5.3 Public API mapping

| Surface | Mapping |
|---|---|
| `faust-rs` successful CLI | `1:1`; existing Clap surface retained |
| Impulse-runner successful invocations | `1:1`; parser implementation adapted |
| `xtask` successful workflows | `1:1`; dispatcher and parser implementation adapted |
| Invalid CLI diagnostics/status | `adapted` to standard Clap behavior |
| FFI compiler argv protocol | `1:1`; explicitly unchanged |
| Standalone impulse architecture | `deferred/exempt`; dependency-free parser retained |

There is no compiler IR, generated-code, C ABI, or runtime DSP behavior change.

## 6. Testing strategy

### 6.1 Baseline matrix

Before migration, record for every affected target:

- documented invocations;
- CI and script call sites;
- successful defaults and option combinations;
- missing-value behavior;
- invalid numeric values;
- unknown options and extra positionals;
- help/version availability and exit status;
- repeated option behavior;
- passthrough arguments after `--`.

The baseline should test semantics, not freeze incidental formatting from the
handwritten usage strings.

### 6.2 Parser tests

Every `Parser`, `Args`, and `Subcommand` family should include:

- `CommandFactory::command().debug_assert()`;
- `try_parse_from` tests for defaults and representative combinations;
- compatibility tests for legacy Faust spellings after normalization;
- invalid-value, missing-value, conflict, and extra-positional tests;
- tests that repeated path/case options retain order;
- a non-UTF-8 passthrough test on Unix where applicable.

Business-logic tests should construct typed options directly. They should not
reparse strings unless the parsing contract itself is under test.

### 6.3 End-to-end command tests

Run smoke commands for:

- every `xtask` subcommand used by CI;
- both impulse runners on a compact inline or repository corpus fixture;
- each migrated benchmark/example help surface;
- `golden-gen-cpp --` passthrough with a fake executable harness;
- invalid top-level and subcommand invocations with asserted exit status.

## 7. Enforcement

Add `cargo run -p xtask -- cli-parser-check` and run it in CI.

The check should inspect Cargo metadata and the source files of binary/example
targets. It should reject:

- new direct `std::env::args`/`args_os` consumers outside an explicit
  allowlist;
- reintroduction of the known `xtask` `parse_*_options` iterator functions;
- manual `"missing value after"` / `"unknown option"` CLI diagnostics in
  ordinary Cargo targets.

The allowlist must be small and auditable:

- the compiler's legacy-normalization entry point, because it immediately
  invokes `CliArgs::parse_from`;
- any generated or standalone architecture source that cannot depend on
  Cargo packages.

The check is an architectural regression guard, not a proof that all parsing
uses `clap`. Its error should identify the file, matched construct, and the
documented exception process.

## 8. Porting phases

### Phase C0 — Baseline and decision gate

Deliverables:

- complete executable-target and call-site inventory;
- semantic command matrix for all affected targets;
- explicit confirmation of invalid-invocation exit-status adaptation;
- exception list for embedded protocols and standalone architectures;
- `clap` version pinned as the existing compiler version.

Pass criteria:

- every current manual process parser is classified;
- every documented/CI invocation is represented by a test or inventory row;
- no implementation begins with unresolved legacy-option or passthrough
  semantics.

### Phase C1 — Impulse runners

Deliverables:

- workspace-level `clap` dependency declaration;
- typed Parser structures for both impulse runners;
- legacy-token normalization followed by Clap validation;
- removal of `parse_args_from` token loops;
- compatibility tests for scalar/vector, precision, import, scheduling, and
  frame options.

Pass criteria:

- existing impulse Makefile invocations remain accepted;
- runner output for representative DSPs is byte-identical;
- malformed options exit through the documented Clap path;
- no FFI dependency boundary is inverted.

### Phase C2 — `xtask`

Deliverables:

- typed `XtaskCli` and `XtaskCommand`;
- module-owned `Args` types for all parameterized subcommands;
- removal of the dispatcher and command-specific token loops;
- generated top-level and per-command help;
- preserved `OsString` passthrough for `golden-gen-cpp`.

Pass criteria:

- all CI-invoked commands pass;
- all existing parser unit semantics have equivalent `try_parse_from` tests;
- unknown commands and invalid arguments use the accepted status contract;
- `crates/xtask/README.md` matches generated command names and options.

This is the largest phase and should remain one coherent commit only if the
review diff stays tractable. Otherwise split it by command families while
keeping every intermediate commit buildable.

### Phase C3 — Secondary binaries, examples, and benchmarks

Deliverables:

- migrate `treearena_bench`, `interp_baseline`, `count_vector_corpus`,
  `corpus_scan_cranelift`, `compute_bench`, and `export_clif`;
- remove obsolete examples instead where explicitly justified;
- replace panic-based usage handling with typed diagnostics;
- document every retained non-Clap exception.

Pass criteria:

- all examples compile under `--all-targets`;
- documented invocations remain accepted;
- path arguments use `PathBuf`/`OsString` where appropriate;
- no new dependency is added to standalone generated architecture code.

### Phase C4 — Policy enforcement and cleanup

Deliverables:

- `xtask cli-parser-check`;
- CI integration;
- removal of stale handwritten usage/parser comments;
- final CLI ownership inventory in the relevant READMEs;
- journal entry and public API mapping for each touched target.

Pass criteria:

- the policy check passes with only documented exceptions;
- repository search finds no unclassified process parser;
- full workspace gates pass.

## 9. Validation gates

Each implementation phase must run:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -q -p xtask -- cli-parser-check   # once available
cargo run -q -p xtask -- golden-check
git diff --check
```

Phases touching impulse workflows must also execute both runners on a compact
representative corpus subset and compare the emitted impulse text with their
pre-migration baselines.

## 10. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Legacy `-double`/`-ss` scripts break | Baseline call sites; normalize aliases before Clap; test every legacy form |
| `xtask` scripts depend on exit status `0` for unknown commands | Search scripts and CI in C0; explicitly approve status adaptation |
| Passthrough arguments are interpreted by Clap | Use final/trailing `OsString` capture and fake-process tests |
| Help snapshots become brittle across Clap versions | Assert semantic tokens/defaults; do not freeze wrapping |
| A shared CLI helper creates new dependency coupling | Keep small declarative structures local; share only existing domain types |
| Protocol parsers are mistakenly migrated | Maintain explicit scope classification and policy allowlist |
| Paths lose non-UTF-8 support | Prefer `PathBuf`/`OsString`; add Unix-specific test |

## 11. Rejected alternatives

### Keep the current parsers because they are internal

Rejected. `xtask` and the runners are repeatable developer/CI interfaces, and
their current duplication already causes inconsistent validation and
documentation.

### Create one generic handwritten parser

Rejected. It would preserve the original problem behind another abstraction
and recreate functionality already supplied and tested by `clap`.

### Create a new `cli-common` crate

Rejected for the current scope. The commonality is mostly declarative field
shape, while conversion and dependency ownership differ. A new crate would add
architectural cost and could invert tool/FFI dependency direction.

### Apply Clap to FFI `argc`/`argv`

Rejected. Those vectors are an external embedded compatibility protocol, not
a process CLI, and must retain their exact Faust-specific semantics.

## 12. Completion criteria

The review concern is closed when:

1. every Cargo binary/example process CLI is Clap-based or explicitly exempt;
2. `xtask` has one typed subcommand tree and no command-specific token loops;
3. both impulse runners preserve their existing accepted invocation surface;
4. embedded FFI parsers are clearly identified as protocols rather than CLIs;
5. a CI check prevents unreviewed manual parsers from returning;
6. workspace, golden, and impulse validation gates are green.
