# wasm-ffi

Raw WASM export surface for the Rust-backed `faustwasm` embedded-compiler path.

## Role

`wasm-ffi` is the thin binding crate that sits on top of the pure Rust compile
service in [`compiler`](../compiler/). It is intended to be compiled as a
standalone `wasm32-unknown-unknown` module and loaded from `faustwasm`.

The exported ABI is intentionally small:

- one compile request in
- one owned result handle out
- raw pointer/length accessors for `{ wasm, json, compile_options }`
- handle-based helper calls for `getInfos(...)`, `expandDSP(...)`, and
  `generateAuxFiles(...)`

## Build the compiler module

From the workspace root:

```bash
rustup target add wasm32-unknown-unknown
cargo run -p xtask -- build-faustwasm-compiler-module
```

This command:

- builds `wasm-ffi` for `wasm32-unknown-unknown`
- publishes Cargo's crate-normalized output as `libfaust-rs.wasm`
- verifies that the emitted module exports the raw ABI expected by the
  `faustwasm` Rust adapter
- prints the output path under `target/wasm32-unknown-unknown/`

Use `--debug` to build the non-release artifact:

```bash
cargo run -p xtask -- build-faustwasm-compiler-module --debug
```

If you want to force a specific standard-library root into the embedded bundle:

```bash
FAUST_RS_EMBEDDED_LIB_ROOT=/path/to/faust/libraries \
  cargo run -p xtask -- build-faustwasm-compiler-module
```

If a `faustwasm` source string depends on project-local `.lib` files, embed
both the local root and the standard-library root through `FAUST_LIB_PATH`:

```bash
FAUST_LIB_PATH=/path/to/project:/path/to/faust/libraries \
  cargo run -p xtask -- build-faustwasm-compiler-module
```

The default release artifact is:

```text
target/wasm32-unknown-unknown/release/libfaust-rs.wasm
```

## Raw ABI usage

The raw compiler-module ABI is explicitly handle-based:

1. allocate host-written UTF-8 buffers with `faust_wasm_alloc`
2. write `name`, `source`, and `args` bytes into those buffers
3. call `faust_wasm_compile_dsp`
4. inspect the returned handle with `faust_wasm_result_is_ok`
5. read payloads through:
   - `faust_wasm_result_wasm_ptr/len`
   - `faust_wasm_result_json_ptr/len`
   - `faust_wasm_result_compile_options_ptr/len`
   - or `faust_wasm_result_error_ptr/len`
6. optionally query structured diagnostics:
   - `faust_wasm_result_get_error_diagnostics` for a failed compiler request
   - `faust_wasm_result_get_diagnostics` for warnings/remarks retained by a
     successful request
7. inspect/copy the returned text-result handle, then release it with
   `faust_wasm_text_result_free`
8. copy compile payloads on the host side
9. release the compile result with `faust_wasm_result_free`
10. release the temporary request buffers with `faust_wasm_dealloc`

Helper text results (`faust_wasm_get_info`, `faust_wasm_expand_dsp`,
`faust_wasm_generate_aux_files_json`) carry structured diagnostics too: when
`faust_wasm_text_result_is_ok` returns `0`,
`faust_wasm_text_result_diagnostics_ptr/len` expose the complete
diagnostics-v2 report for compiler failures (null/0 for transport and
argument failures, and on modules predating these exports). The
compatibility message from `faust_wasm_text_result_ptr/len` is unchanged, so
hosts adopt the richer payload at their own pace. Host-adapter status:

- WebAssembly Music consumes these exports (structured errors reach its
  Faust editor, CLI, and LLM studio agent);
- the `faustwasm` TypeScript adapter (`RustLibFaust` / `FaustCompiler`) does
  not read them yet — its `readTextResult` frees the handle without querying
  diagnostics. Wiring `getErrorDiagnostics()`-style accessors for the helper
  surface there is tracked follow-up work.

Pointer validity rules:

- payload pointers returned by `faust_wasm_result_*_ptr` stay valid only until
  the matching `faust_wasm_result_free(handle)`
- structured diagnostic queries take no verbosity parameter and return the
  complete diagnostics-v2 report; hosts can filter typed fields locally
- diagnostic text-result handles own their JSON independently and remain valid
  after the originating compile-result handle is freed
- request buffers returned by `faust_wasm_alloc` stay valid only until the
  matching `faust_wasm_dealloc(ptr, len)`
- handles are process-global within one compiler-module instance and are not
  stable across module reinstantiation

Concurrency note:

- the module uses process-global mutex-protected registries for compile and
  helper results
- concurrent host calls are safe at the registry level, but the public contract
  is still “copy returned bytes promptly, then free the handle”

## Prefetched HTTP(S) source graphs

The compiler module never calls browser `fetch()` and does not link the native
HTTP transport. A browser or worker can nevertheless compile URL-addressed
graphs by fetching every source asynchronously before calling
`faust_wasm_compile_dsp`.

Pass the main source text normally and use its absolute URL as the `name`
argument. Add each imported response to the argument string with a repeated:

```text
--remote-source <absolute-http(s)-url> <base64-encoded-utf8-source>
```

For example, a root named `https://example.test/dsp/main.dsp` containing
`import("lib/identity.lib")` resolves that child as
`https://example.test/dsp/lib/identity.lib`; the latter URL and its encoded
source must be present in the bundle. Explicit absolute URL imports work from
both URL-named and ordinary in-memory roots.

The host owns asynchronous download, CORS/authentication, redirects, URL
authorization, and the aggregate graph-size policy. Redirects should be
resolved before injection and each final source stored under the URL used by
the import graph. The compiler normalizes URL fragments, rejects duplicate
normalized URLs, malformed base64, and non-UTF-8 source text, enforces its
per-response byte limit, and reports the canonical missing URL when a bundle
is incomplete. Base64 payloads are excluded from diagnostic option metadata.

This mechanism is intentionally separate from `--virtual-source`: virtual
sources have logical filesystem-like names, while prefetched remote sources
retain URL identity for relative resolution, cycle detection, provenance, and
diagnostics.

## Embedded Faust libraries

At build time, `wasm-ffi` can discover and embed a read-only bundle of Faust
`.lib` sources directly into the compiler-module. The current root discovery
order is:

- valid roots from `FAUST_RS_EMBEDDED_LIB_ROOT`
- valid roots from `FAUST_RS_FAUSTLIBRARIES_ROOT`
- all valid entries from `FAUST_LIB_PATH`
- `/usr/local/share/faust`
- `/usr/share/faust`

When several roots are embedded, logical `.lib` paths are merged in search
order and the first root providing a given logical path wins. This lets a
project-local root add files such as `ad.lib` while the next root still supplies
`stdfaust.lib` and the standard Faust libraries.

If none of these roots exist at build time, the compiler-module still builds,
but without an embedded standard-library bundle. In that case, source-string
compilation that depends on `import("stdfaust.lib")` will fail unless the host
provides equivalent imports through another path.

The embedded bundle is used for the Rust raw compiler path only. It allows:

- parser-side `import("stdfaust.lib")` from an in-memory DSP source
- evaluator-side `library("...")` / `component("...")` resolution against the
  same bundled logical sources

This keeps the `faustwasm` compiler-module self-contained for the standard
Faust libraries without recreating an Emscripten-style virtual filesystem.

Import precedence:

- user-supplied `-I` search paths are still parsed and forwarded into the typed
  compile request
- the embedded bundle provides the logical standard-library files directly to
  the parser/evaluator for source-string compilation
- `library_list` in the returned JSON reports the logical imported file names
  seen during compilation, not an Emscripten-style resolved filesystem path

## Current scope

| Surface | Current status |
| --- | --- |
| `compile_dsp` | implemented |
| `getInfos("version")` | implemented |
| `getInfos("help")` | implemented |
| `getInfos("libdir"\\|"includedir"\\|"archdir"\\|"dspdir"\\|"pathslist")` | supported; mirrors C++ Faust directory-info queries |
| `expandDSP(...)` | Implemented as Rust frontend validation plus source passthrough; invalid/unsupported requests return an explicit error |
| `generateAuxFiles(...)` | Implemented for `-cpp`, `-c`, `-wasm`, `-json`, `-svg`, and `-lang asc`; SVG artifacts are generated in memory through `draw::draw_schema_to_memory` |

That `.wasm` file is the compiler-module artifact that the `faustwasm`
embedded-compiler path is expected to load.
