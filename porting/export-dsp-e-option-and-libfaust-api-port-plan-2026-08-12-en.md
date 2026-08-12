# `-e` Export-DSP Option and `libfaust.h` API — Analysis and Porting Plan

Date: 2026-08-12

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`
(cross-checked against the installed `faust 2.87.4` binary for observed output)

Status: planned — no implementation yet

## 1. Objective

Two coupled deliverables:

1. **`-e` / `--export-dsp`**: expand a Faust program into a *self-contained*
   `.dsp` source in which every `import`, every library definition, and every
   user abstraction has been inlined into a flat, evaluated block-diagram
   expression. The result must be re-compilable by both `faust` and `faust-rs`
   and must produce the same generated DSP code as compiling the original.
2. **`libfaust.h` in the FFI model**: add the backend-agnostic top-level
   libfaust API surface (`generateSHA1`, `expandDSPFrom{File,String}`,
   `generateAuxFilesFrom{File,String}[2]`) to the faust-rs C/C++ header
   distribution, alongside the existing `libfaust-box.h`,
   `libfaust-signal.h`, `interpreter-dsp.h`, and `cranelift-dsp.h`.

The two are coupled because `expandDSP*` — the header's central entry point —
is exactly the `-e` code path, and because faust-rs already ships a *stub*
`Compiler::expand_dsp` that returns the input verbatim
([crates/compiler/src/service.rs:49](crates/compiler/src/service.rs:49)).
Closing the option also closes the registered behavioral difference
`DIFF-BEH-006`.

This is an `adapted` port: the produced text is the parity target, the
implementation is idiomatic Rust with no process-global printer state.

## 2. C++ Reference Contract

### 2.1 Option plumbing

| Element | Location |
|---|---|
| `-e` / `--export-dsp` parsing | `compiler/global.cpp:1681` |
| `gExportDSP` flag | `compiler/global.hh:263`, init `compiler/global.cpp:637` |
| CLI branch (step "3.1") | `compiler/libcode.cpp:1378` |
| Document assembly | `expandDSPInternalAux`, `compiler/libcode.cpp:1192` |
| Library entry point | `expandDSPInternal`, `compiler/libcode.cpp:1211` |
| Public `expandDSP` | `compiler/libcode.cpp:1522` |
| C++ / C facades | `compiler/generator/dsp_aux.cpp:225,236,347,360` |
| Header being ported | `compiler/generator/libfaust.h` (C++), `architecture/faust/dsp/libfaust-c.h` (C) |

The CLI branch sits **after** parsing, evaluation, and the `numOutputs == 0`
rejection, and **before** propagation. So `-e` performs parse + eval only, and
a program with no output signal is still rejected.

### 2.2 Document layout

`expandDSPInternalAux` writes, in order:

1. `declare version "<FAUSTVERSION>";`
2. `declare compile_options "<reorganizeCompilationOptions(argc, argv)>";`
3. `declare library_path<i> "<path>";` for every source file returned by
   `SourceReader::listSrcFiles()` **except the first** (the DSP file itself) —
   `compiler/parser/sourcereader.cpp:387`;
4. `global::printDeclareHeader(out)` — the whole top-level metadata set
   (`compiler/global.cpp:2154`);
5. `boxppShared(process_tree, out)` — the shared box serialization
   (`compiler/boxes/ppbox.hh`, `compiler/boxes/ppbox.cpp:505`).

`printDeclareHeader` rules:

- keys are emitted with `.`, `:` and `/` replaced by `_`
  (`basics.lib/name` → `basics_lib_name`);
- the `author` key is special: the first value stays `author`, every further
  value is re-emitted as `contributor`;
- values are printed as quoted Faust strings.

### 2.3 `boxppShared`

`boxppShared` is a *sharing-aware* box printer. Every composite node is
memoized in `gGlobal->gBoxTable` (node → `(id, text)`) and appended to
`gGlobal->gBoxTrace` as `ID_<n> = <text>;`. The traversal then prints
`ID_<n>` at each use site. The constructor
`boxppShared(Tree, std::ostream&)` renders the root into a string first, then
flushes the whole `ID_` trace, then writes `process = <root>;`.

Consequences to preserve:

- **DAG, not tree**: shared sub-diagrams appear once, so the output is linear
  in the DAG size, not exponential in the tree size. This is the whole point of
  the printer and the reason a naive recursive printer is not an acceptable
  substitute.
- **Definition order**: `ID_` numbering follows first-completion order of the
  post-order traversal, so every `ID_n` is defined before its first use, and
  the emitted `.dsp` is a valid top-level definition list.
- **Non-shared leaves**: primitives, ints, reals, `_`, `!`, idents, `()`,
  `environment`, `component(...)`, `import(...)`, slots, `case{...}`, and
  pattern nodes print inline without an `ID_`.
- **Two nodes deliberately fall back to non-shared `boxpp`**: `BoxAbstr`
  (`\x.(body)`) and `BoxSymbolic` (`\(slot).(body)`), because the bound
  variable would become free if the body were hoisted into a top-level
  `ID_` definition (`compiler/boxes/ppbox.cpp:550,717`).
- Slots print as `x<id>` (`compiler/boxes/ppbox.cpp:425,715`).

### 2.4 Number formatting

Reals go through `T(double)` → `TAux` + `ensureFloat` + `inumix`
(`compiler/generator/Text.cpp:227-300`, `compiler/generator/floats.cpp:287`):

1. print with `%.*g`, precision starting at 1 and increasing until the value
   round-trips through `strtof`/`strtod` (per `gFloatSize`), capped at 32;
2. append `.0` if the result contains neither `.` nor `e`;
3. append the numeric suffix for the float size (`f` for single, none for
   double, `L`/`q` for the wider modes).

This is *not* Rust's `{}` shortest-round-trip formatting: C++ emits
`4.4e+02f` where Rust `Display` emits `440`. Byte parity requires a `%g`-shaped
formatter.

### 2.5 `expandDSPFromString` short-circuit and its quirk

`expandDSPFromString` (`compiler/generator/dsp_aux.cpp:236`) first tests
`startWith(dsp_content, "declare compile_options ")`. If the incoming text is
*already* expanded with the same normalized options, it is returned unchanged
with a SHA key computed over it; if the options differ, a new
`declare compile_options ...;` line is prepended.

Observed quirk: `expandDSPInternalAux` writes `declare version` **before**
`declare compile_options`, so real `-e` output never satisfies that
`startWith` test and is always re-expanded. The comment at
`compiler/libcode.cpp:1194` ("has to be located first in the string") documents
an invariant the code no longer holds. The Rust port will implement the same
test (so hand-written option-prefixed sources behave identically) and record
the quirk instead of silently "fixing" it.

### 2.6 Option normalization

`reorganizeCompilationOptions` (`compiler/generator/dsp_aux.cpp:203`) rebuilds
argv in a canonical order (`-single`/`-double`, then vectorization-implying
options, then `-vec`-dependent or `-scal`-dependent options, then
`-mcd`/`-cn`/`-ftz`), then appends every remaining argument verbatim except a
leading `faust`. Because the remainder is appended verbatim, the CLI form
leaks the input and output filenames into the string, as observed below.

### 2.7 Observed reference output

`faust -e e1.dsp -o out.dsp` on

```faust
import("stdfaust.lib");
declare name "E1";
freq = hslider("freq", 440, 20, 20000, 1);
process = os.osc(freq) <: _,_;
```

produces (abridged):

```
declare version "2.87.4";
declare compile_options "-single -scal -e e1.dsp -o out.dsp";
declare library_path0 "/usr/local/share/faust/stdfaust.lib";
...
declare basics_lib_name "Faust Basic Element Library";
declare filename "e1.dsp";
declare name "E1";
...
ID_0 = (65536 : int);
ID_1 = _, ID_0;
ID_2 = (ID_1 : %);
...
ID_18 = hslider("freq", 4.4e+02f, 2e+01f, 2e+04f, 1.0f);
ID_19 = fconstant(int fSamplingFreq, <math.h>);
...
ID_31 = (ID_30 : \(x1).(x1,(x1 : floor) : -));
ID_41 = ID_39 <: ID_40;
process = ID_41;
```

Two facts established by experiment and used as acceptance criteria below:

- the expanded file **re-compiles** (`faust out.dsp -o rt.cpp` succeeds);
- the round-tripped C++ is **identical to the direct compilation** except for
  metadata (mangled keys, added `library_path*`, `version`, and the different
  `compile_options` string). The DSP algorithm code is byte-identical.

A second sample confirms the `author`/`contributor` rule, the auto-declared
`filename` and `name` entries, the absence of `library_path*` lines when the
program has no imports, and `\(x1).(...)` printing for user abstractions.

## 3. Current Rust Boundary

| C++ element | Rust today | Gap |
|---|---|---|
| `gExportDSP` / `-e` | — | no CLI flag; `-e` is unused in [crates/compiler/src/cli/args.rs](crates/compiler/src/cli/args.rs) |
| `boxppShared` | none | [crates/compiler/src/box_preview.rs:44](crates/compiler/src/box_preview.rs:44) `render_human_box_expr` is a *lossy, depth-capped, non-sharing* diagnostics renderer; it must not be reused as the serializer |
| `boxpp` | none | same |
| `BoxMatch` coverage | complete, [crates/boxes/src/matcher.rs](crates/boxes/src/matcher.rs) | ready substrate for the printer (all ~110 variants incl. `Symbolic`, `Slot`, `Closure`, `PatternMatcher`, `FFun`, `Waveform`, `Route`, `Ondemand`, `ForwardAD`/`ReverseAD`) |
| `printDeclareHeader` | partial | [crates/compiler/src/json_naming.rs:314](crates/compiler/src/json_naming.rs:314) has the identical author/contributor fold for JSON; no `.`→`_` mangling, no `declare` syntax |
| `listSrcFiles()` | `ParseOutput::used_files` + `SignalCompileOutput::loaded_files` | same deterministic recursive-import order; needs the "drop the first entry" rule |
| metadata set | `CompilationMetadataSnapshot` | `filename` / `name` are *derived* in the JSON path, not stored in the snapshot ([crates/compiler/src/json_naming.rs:373](crates/compiler/src/json_naming.rs:373)) — the expander must synthesize them |
| `reorganizeCompilationOptions` | none | [crates/compiler/src/json_naming.rs:239](crates/compiler/src/json_naming.rs:239) `compile_options_json_string` is a much narrower JSON-oriented string |
| `generateSHA1` | **fake** | [crates/interp-ffi/src/factory.rs:966](crates/interp-ffi/src/factory.rs:966) is an FNV-1a value repeated 4× to fill 64 hex chars |
| `T(double)` | different | [crates/codegen/src/backends/c_family.rs:427](crates/codegen/src/backends/c_family.rs:427) `trim_float` is Rust `Display`-shaped, not `%g` |
| eval-only pipeline | none | [crates/compiler/src/lib.rs:1211](crates/compiler/src/lib.rs:1211) `pipeline_to_signals` always continues into propagation |
| `expandDSP*` facade | stub | [crates/compiler/src/service.rs:49](crates/compiler/src/service.rs:49) returns `request.source` unchanged |
| `expandC*DSPFrom*` | per-backend only | `expandCInterpreterDSPFrom*` ([crates/interp-ffi/src/factory.rs:725,786](crates/interp-ffi/src/factory.rs:725)), `expandCCraneliftDSPFrom*` ([crates/cranelift-ffi/src/factory.rs:1292](crates/cranelift-ffi/src/factory.rs:1292)); no backend-agnostic `expandCDSPFrom*` |
| `libfaust.h` / `libfaust-c.h` | absent | headers present: `box-ffi`, `signal-ffi`, `interp-ffi`, `cranelift-ffi` `include/` dirs |
| header/export gate | exists | [crates/xtask/src/libfaust_export_check.rs:226](crates/xtask/src/libfaust_export_check.rs:226) parses headers, diffs against `porting/generated/libfaust-rs-exported-symbols.txt`, and syntax-checks C and C++ clients |

## 4. Target Architecture

### 4.1 `boxes::print` — the serializer (new module)

New file `crates/boxes/src/print.rs`, exported from
[crates/boxes/src/lib.rs](crates/boxes/src/lib.rs).

```rust
pub struct BoxPrinter<'a> {
    arena: &'a TreeArena,
    table: HashMap<BoxId, u32>,   // node -> ID_n
    trace: Vec<String>,           // "ID_n = <text>;"
    float_size: FloatSize,
}

pub struct SharedBoxProgram { pub definitions: Vec<String>, pub root: String }

pub fn box_pp(arena: &TreeArena, node: BoxId, float_size: FloatSize) -> String;
pub fn box_pp_shared(arena: &TreeArena, node: BoxId, float_size: FloatSize)
    -> Result<SharedBoxProgram, BoxPrintError>;
```

Design decisions, all deliberate deviations from C++ that do not change output:

- **no global state**: the memo table and trace live in `BoxPrinter`, not in a
  process-global. Two concurrent expansions cannot interleave `ID_` numbering.
- **explicit worklist, not recursion**: the C++ printer recurses through
  `operator<<`; deep block diagrams would blow the Rust stack. Traversal is an
  explicit post-order stack, so depth is heap-bounded.
- **`Result`, not exceptions**: `BoxPrintError::NotAValidBox { node, kind }`
  replaces `throw faustexception("... is not a valid box")`.
- **`BoxMatch::Unknown` is an error, never a silent `?`**: a printer that
  degrades unknown nodes produces text that compiles to something else. This is
  the same failure class recorded in the scheduling stream ("typed FIR walkers
  silently skipping unknown node kinds") and is closed here by construction.
- `Abstr` and `Symbolic` bodies are printed with the *non-shared* `box_pp`
  path, mirroring `compiler/boxes/ppbox.cpp:550,717`, and their sub-nodes must
  not be entered into the shared table (a node reachable both inside and
  outside an abstraction must be printed inline inside it).

Operator precedence and parenthesization follow `streambinopShared`
(`compiler/boxes/ppbox.cpp:187`): priorities `:` = 1, `,` = 2, `<:`/`:>` = 1,
`~` = 4, with the parent priority threaded through.

### 4.2 `boxes::print::format_real` — `%g` parity

A faithful `TAux` port: increase precision `p` from 1 until
`format!("{:.*e}", ...)`-equivalent `%g` output round-trips to the same
`f32`/`f64`, cap at 32, then `ensure_float`, then suffix. Since Rust has no
`%g`, the module implements it: choose `%e` or `%f` style per the C rule
(`exponent < -4 || exponent >= precision` → `%e`), strip trailing zeros of the
significand, and format the exponent with a sign and at least two digits
(`4.4e+02`). This lives beside the printer, is unit-tested against a table of
values captured from the C++ binary, and is **not** shared with
`c_family::trim_float`, whose contract is C literal emission, not `-e` parity.

### 4.3 Eval-only pipeline split

[crates/compiler/src/lib.rs:1211](crates/compiler/src/lib.rs:1211)
`pipeline_to_signals` is split at the eval boundary:

```rust
struct BoxCompileOutput {
    parse: ParseOutput,                  // owns the arena
    compilation_metadata: CompilationMetadataSnapshot,
    loaded_files: Vec<PathBuf>,
    process_box: BoxId,
    definitions_root: BoxId,
    entrypoint_name: Box<str>,
}

fn pipeline_to_boxes(...) -> Result<BoxCompileOutput, CompilerError>;
fn pipeline_to_signals(...) -> Result<SignalCompileOutput, CompilerError>; // calls the above
```

All existing diagnostic enrichment (eval error node, owner definition, source
labels, guidance) stays in `pipeline_to_boxes` so `-e` reports eval failures
exactly like a normal compilation. `SignalCompileOutput` keeps its current
public shape.

Note on the C++ `numOutputs == 0` rejection at `compiler/libcode.cpp:1370`:
arity comes from `propagate::box_arity_typed`, which runs *before* propagation
proper in the Rust pipeline. `pipeline_to_boxes` therefore also returns
`process_arity`, and the `-e` path applies the same rejection.

### 4.4 `compiler::expand` — document assembly (new module)

New file `crates/compiler/src/expand.rs`:

```rust
pub struct ExpandedDsp { pub text: String, pub sha_key: String }

impl Compiler {
    pub fn expand_dsp_document(
        &self,
        source_name: &str,
        source: &str,
        search_paths: &[PathBuf],
        options: &ExpandOptions,   // carries the normalized compile_options string
    ) -> Result<ExpandedDsp, CompilerError>;
}
```

Assembly order is §2.2 verbatim. Sub-parts:

- `declare version` uses `Compiler::version()`
  ([crates/compiler/src/lib.rs:887](crates/compiler/src/lib.rs:887)), i.e. the
  faust-rs version, not `FAUSTVERSION`. Deliberate: the string identifies the
  compiler that produced the file.
- `declare compile_options` uses a new
  `expand::reorganize_compilation_options(argv)` — a direct port of
  `reorganizeCompilationOptionsAux` including the `-vec`-implying folds and the
  verbatim tail.
- `library_path<i>` comes from `used_files` (skipping index 0) followed by
  `loaded_files`, deduplicated, preserving order.
- the metadata header reuses the author/contributor fold already implemented at
  [crates/compiler/src/json_naming.rs:314](crates/compiler/src/json_naming.rs:314),
  factored into a shared helper, plus the `.`/`:`/`/` → `_` mangling, plus the
  synthesized `filename` (from `source_name_to_filename`) and `name` (from
  `resolve_ui_root_label`) entries when the snapshot does not carry them.
- values are re-quoted with Faust string escaping.

`Compiler::expand_dsp` ([crates/compiler/src/service.rs:49](crates/compiler/src/service.rs:49))
is rewritten on top of this, keeping the `startWith(compile_options)`
short-circuit of §2.5, and its doc comment's "returns the input verbatim" note
is removed.

### 4.5 Real SHA-1

New `crates/ffi-common/src/sha1.rs` (or a vetted `sha1` crate dependency — the
workspace already vendors `mimalloc`, so one small hashing dependency is
acceptable; decide at implementation time and record the choice). It replaces
the FNV stand-in at
[crates/interp-ffi/src/factory.rs:966](crates/interp-ffi/src/factory.rs:966)
and is shared by every `expandC*` entry point, so a faust-rs SHA key equals the
C++ SHA key for the same expanded text. This matters: the key is a cache
identity in `faustwasm` and in host factory caches, so a fake key is a
correctness hazard the moment two hosts compare keys.

### 4.6 CLI surface

- new field `pub export_dsp: bool` with `#[arg(short = 'e', long = "export-dsp")]`
  in [crates/compiler/src/cli/args.rs](crates/compiler/src/cli/args.rs);
- `-e` added to `normalize_legacy_args` is unnecessary (clap already accepts
  `-e`), but `--export-dsp` is registered for symmetry with the C++ long form;
- new branch in [crates/compiler/src/cli/source_mode.rs](crates/compiler/src/cli/source_mode.rs)
  placed *before* every backend branch, next to `cli.dump_box`, using
  `emit_output(&text, cli.output.as_ref())`;
- `-e` must be counted in the CLI's `mode_count` mutual-exclusion check so
  `-e -lang cpp` is rejected rather than silently ignoring one of them.

**Deliberate deviation**: C++ writes to `ofstream(gOutpath)` and, with no
`-o`, silently writes nowhere — `faust -e e1.dsp` produces no output and exits
0 (verified). faust-rs prints to stdout in that case, like every other
faust-rs dump mode. Recorded in the differences registry.

### 4.7 FFI surface

New crate `crates/libfaust-ffi/` (rlib, `crate-type = ["rlib"]`, depending on
`compiler` and `ffi-common`), linked into
[crates/faust-ffi/src/lib.rs](crates/faust-ffi/src/lib.rs) like the other
`*-ffi` crates. Rationale: `faust-ffi` is a cdylib/staticlib aggregator with no
Rust code of its own beyond re-exports, and a separate rlib keeps the
backend-agnostic surface testable in isolation.

Exports (C names, matching `architecture/faust/dsp/libfaust-c.h`):

| Symbol | Backed by |
|---|---|
| `generateCSHA1` | §4.5 |
| `expandCDSPFromFile` / `expandCDSPFromString` | `Compiler::expand_dsp` |
| `generateCAuxFilesFromFile` / `...FromString` | `Compiler::generate_aux_files` + `write_aux_artifacts_to_disk` |
| `generateCAuxFilesFromFile2` / `...FromString2` | same, returning the single artifact as a string |

`freeCMemory` is **not** re-declared: it is already exported unconditionally by
[crates/interp-ffi/src/factory.rs:432](crates/interp-ffi/src/factory.rs:432)
(`box-ffi` and `cranelift-ffi` gate theirs behind `standalone-capi-globals`).
The new crate documents that its returned strings are freed through that
symbol, and does not define its own.

Headers, in `crates/libfaust-ffi/include/`:

- **`libfaust-c.h`** — the C declarations above, in the house style of
  [crates/box-ffi/include/libfaust-box-c.h](crates/box-ffi/include/libfaust-box-c.h)
  (no `faust/export.h`, no `LIBFAUST_API`, `#ifdef __cplusplus extern "C"`).
- **`libfaust.h`** — the requested header. C++ `std::string`-based signatures
  cannot be exported from Rust (mangling + ABI), so, exactly like
  [crates/box-ffi/include/libfaust-box.h](crates/box-ffi/include/libfaust-box.h),
  it is a **header-only inline wrapper over the C ABI**:

```cpp
inline std::string generateSHA1(const std::string& data) {
    char key[64]; generateCSHA1(data.c_str(), key); return std::string(key);
}
inline std::string expandDSPFromString(const std::string& name_app,
                                       const std::string& dsp_content, int argc,
                                       const char* argv[], std::string& sha_key,
                                       std::string& error_msg) { /* ... freeCMemory */ }
```

Every wrapper owns the returned `const char*` and releases it through
`freeCMemory` before returning, so the C++ signature of
`compiler/generator/libfaust.h` is reproduced with no leak and no allocator
crossing. The `sha_key` buffer contract (64 chars) and `error_msg` contract
(4096 chars) are honored inside the wrappers, so C++ callers see only
`std::string`.

The header pair is added to `expected_header_symbols`
([crates/xtask/src/libfaust_export_check.rs:226](crates/xtask/src/libfaust_export_check.rs:226))
and to the C/C++ smoke clients in `syntax_check_headers`; the exported-symbol
baseline `porting/generated/libfaust-rs-exported-symbols.txt` is re-blessed.

## 5. Parity Contract

Byte parity with C++ `-e` output is **not** promised, and the reasons are
enumerated so a diff is never mistaken for a bug:

| Divergence | Reason |
|---|---|
| `declare version` value | faust-rs version vs `FAUSTVERSION` |
| `compile_options` tail | faust-rs argv spelling differs (`--lang cpp`, `--table-init`, ...) |
| absolute library paths | installation-dependent on both sides |
| `ID_` numbering | equal only if the evaluated DAG and traversal order match; asserted per fixture, not assumed |

What **is** promised, and is the acceptance criterion:

1. `faust-rs -e prog.dsp -o exp.dsp` produces a file that `faust-rs` and
   `faust` both accept;
2. compiling `exp.dsp` and compiling `prog.dsp` with the same backend and
   options yields **identical generated DSP code** modulo the metadata block
   (the same modulo established for C++ in §2.7);
3. the expanded file is *self-contained*: compiling it with an empty library
   search path succeeds;
4. `expand(expand(prog)) == expand(prog)` up to the `compile_options` line.

## 6. Implementation Phases

Each phase carries a producer, an *independent* checker (not the producer's own
assertions), and mutations that must make the checker fail.

### Phase E0 — Reference corpus and oracle

- Producer: `tests/expand/` fixture set (≈25 `.dsp` covering: no imports;
  `stdfaust.lib`; user abstractions → `\(x1)`; `with{}`; pattern matching;
  `route`; `waveform`; `soundfile`; `ffunction`/`fconstant`/`fvariable`;
  `rdtable`/`rwtable`; all UI widgets and groups; metadata boxes; `ondemand` /
  `upsampling` / `downsampling`; `fad`/`rad`; deeply shared sub-diagrams;
  a 5000-node generated diagram for the stack-depth case).
- Producer: `xtask expand-oracle` capturing `faust -e` output per fixture into
  `tests/expand/oracle/` when a C++ `faust` is on `PATH`.
- Checker: the oracle capture is re-run and must be byte-stable; every fixture
  must round-trip through C++ `faust` itself (guards against fixtures whose
  C++ expansion is already broken).
- Mutation: delete a fixture's `process` → capture must fail, not record empty.

### Phase E1 — `boxes::print`

- Producer: `box_pp` + `box_pp_shared` per §4.1, `format_real` per §4.2.
- Checker: an independent `printed_dag_is_wellformed` checker that re-parses
  the emitted `ID_` list with the faust-rs parser and asserts (a) every `ID_n`
  is defined before use, (b) no `ID_` is defined twice, (c) no `ID_` is unused,
  (d) the re-parsed, re-evaluated box DAG is structurally equal to the input
  DAG (`dump_box` equality).
- Checker: `format_real` differential against the values captured from the C++
  binary in E0.
- Mutations that must fail the checker: emit `ID_` uses before definitions;
  drop the memo table (tree expansion) — caught by (b)/(c) plus a size bound;
  hoist an `Abstr` body into a shared `ID_` — caught by (d) and by a
  free-variable check; return `_` for `BoxMatch::Unknown`.

### Phase E2 — Eval-only pipeline split

- Producer: `pipeline_to_boxes` / `pipeline_to_signals` split per §4.3.
- Checker: the existing compiler and CLI suites must pass unchanged; plus an
  equality test asserting `pipeline_to_signals` output is bit-identical before
  and after the split for the whole `tests/corpus`.
- Mutation: skip metadata aggregation in the box path → the `-e` header tests
  of E3 must fail.

### Phase E3 — Document assembly

- Producer: `compiler::expand` per §4.4, including
  `reorganize_compilation_options`.
- Checker: header-block tests driven by the E0 oracle for the *ordering* and
  *key-mangling* rules; `author`/`contributor` fold; `library_path*` presence
  and absence.
- Mutations: reorder `version`/`compile_options`; drop the `.`→`_` mangling;
  keep the first source file in `library_path*`.

### Phase E4 — CLI `-e`

- Producer: the flag, the branch, `mode_count` participation.
- Checker: `tests/cli-transcripts/` entries for `-e` success, `-e` with no
  output signal (must be rejected), and `-e -lang cpp` (must be rejected as a
  mode conflict); plus the §5 round-trip criterion run over the whole
  `tests/expand/` corpus with both compilers.
- Mutation: place the `-e` branch after the backend dispatch → the conflict
  transcript must fail.

### Phase E5 — SHA-1

- Producer: real SHA-1, replacing the FNV stand-in.
- Checker: RFC 3174 test vectors plus a differential against C++
  `generateSHA1` for every expanded fixture.
- Mutation: truncate the digest to 32 chars → vectors fail.

### Phase E6 — FFI crate and headers

- Producer: `crates/libfaust-ffi/` + `include/libfaust-c.h` +
  `include/libfaust.h`; `faust-ffi` wiring; baseline re-bless.
- Checker: `cargo run -p xtask -- libfaust-export-check` (header symbols ⊆
  exports, baseline diff, C and C++ smoke clients compile); plus a C++ smoke
  client that actually calls `expandDSPFromString` on a fixture and compares the
  result with the CLI's `-e` output for the same source.
- Mutations: declare a symbol in the header that is not exported; leak the
  returned `const char*` in a wrapper (checked under a leak-detecting build);
  redefine `freeCMemory` in the new crate (must fail to link the cdylib).

### Phase E7 — Documentation and registry

- `porting/faust-rs-vs-faust-cpp-differences-en.md`: `DIFF-BEH-006` moves from
  "returns the original source string" to `adapted` with the §5 table as its
  evidence; add the stdout-vs-silent-drop deviation of §4.6.
- `docs/`: document `-e` in the CLI reference alongside the other dump modes.
- `porting/journal/2026-08-12.md` + `JOURNAL.md` (English) entries.
- `porting/HANDOFF.md` if the work spans sessions.

## 7. Validation Matrix

| Property | Mechanism | Phase |
|---|---|---|
| DAG sharing preserved (no exponential blowup) | emitted-size bound vs node count | E1 |
| Emitted program is well-formed | independent re-parse checker | E1 |
| Structural identity after round-trip | `dump_box` equality | E1 |
| Real literal parity | differential vs C++ `T()` | E1 |
| No stack overflow on deep diagrams | 5000-node fixture | E1 |
| Existing pipeline unchanged | full corpus bit-comparison | E2 |
| Header block parity | oracle-driven tests | E3 |
| Self-containedness | recompile with empty search path | E4 |
| Codegen equality after round-trip | backend diff modulo metadata | E4 |
| Idempotence | `expand(expand(x))` | E4 |
| SHA key parity | RFC vectors + C++ differential | E5 |
| Header/export coherence | `xtask libfaust-export-check` | E6 |
| C++ wrapper behavior | calling smoke client | E6 |

## 8. Non-goals

- No `-mdoc`, `-xml`, `-svg` interaction changes; `-e` remains an independent
  terminal mode as in C++.
- No pretty-printing/beautification of the expanded output: the `ID_` form is
  the contract, not an aesthetic choice.
- No `boxppShared` support for nodes that cannot appear in an evaluated
  `process` (`Closure`, `PatternMatcher`, `PatternVar`) beyond the C++
  behavior of printing them for debugging; they remain reachable only through
  `box_pp`.
- No LLVM/`generateAuxFiles` backend expansion beyond what
  `Compiler::generate_aux_files` already supports; the new C entry points are
  a surface, not new backends.
- No change to `wasm-ffi`'s `faust_wasm_expand_dsp`
  ([crates/wasm-ffi/src/lib.rs:903](crates/wasm-ffi/src/lib.rs:903)) beyond
  inheriting the real implementation through `Compiler::expand_dsp`.

## 9. Completion Definition

1. `faust-rs -e` produces self-contained, re-compilable, idempotent DSP for the
   whole `tests/expand/` corpus, with codegen equality after round-trip.
2. `Compiler::expand_dsp` no longer returns its input, and `DIFF-BEH-006` is
   updated with the enumerated residual divergences.
3. `crates/libfaust-ffi/include/libfaust.h` and `libfaust-c.h` are shipped,
   covered by `xtask libfaust-export-check`, and exercised by a calling C++
   smoke client.
4. SHA keys are real SHA-1 and match C++ for identical expanded text.
5. Journal and differences registry updated in English.
