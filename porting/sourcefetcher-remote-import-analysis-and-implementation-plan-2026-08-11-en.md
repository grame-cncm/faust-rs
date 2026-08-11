# SourceFetcher Remote-Import Analysis and Implementation Plan

Date: 2026-08-11

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`

Status: design complete; implementation not started

## 1. Objective

Close `DIFF-GAP-003` by supporting Faust sources addressed through `http://`
and `https://`, without making ordinary compilation depend on network access.
The implementation must preserve the existing local-file and embedded-library
paths, remain deterministic when networking is disabled, and expose an
explicit security policy to native compiler hosts.

This is an `adapted` port, not a line-by-line translation of the legacy C++
socket client. The user-visible import behavior is the parity target; the
transport implementation is replaced with a maintained Rust HTTP client.
The project explicitly permits a more general and flexible Rust design where
that improves safety, embedding, or cross-platform behavior. C++ compatibility
is therefore one policy profile, not a restriction on the internal API.

## 2. C++ Reference Contract

Primary sources:

- `compiler/parser/sourcefetcher.hh`
- `compiler/parser/sourcefetcher.cpp`
- `compiler/parser/sourcereader.cpp`
- `compiler/parser/enrobage.cpp`

The C++ surface provides:

- `http_fetch(url, buffer)` for a blocking GET;
- configurable user-agent and referer headers;
- a configurable read timeout, defaulting to five seconds;
- a configurable redirect count, defaulting to three;
- filename extraction from an URL;
- process-global last-error reporting;
- integration in `SourceReader::parseFile(...)` and `checkURL(...)`.

The implementation is a 2001-era HTTP/1.0 socket client. It always connects to
port 80, grows an initial 200 KiB response buffer as needed, and has no TLS
transport. `sourcereader.cpp` recognizes both `http://` and `https://`, but the
low-level fetcher does not implement a real HTTPS handshake. Therefore true
HTTPS support in Rust is a compatibility improvement over the effective C++
implementation, while HTTP imports remain the direct differential baseline.

The C++ redirect implementation deliberately hides the final URL from its
caller. Rust should preserve requested-source identity in diagnostics, but may
also retain the final normalized URL internally for cache and audit purposes.

## 3. Current Rust Boundary

`crates/parser/src/source_reader.rs` currently owns:

- local search-path resolution;
- immutable virtual sources used by `wasm-ffi`;
- recursive import expansion;
- per-read caching and cycle detection;
- deterministic `used_files` ordering;
- structured source diagnostics.

Its source identity, caches, origin records, cycle edges, and public usage list
are all expressed as `PathBuf`. A remote fetch function alone is consequently
insufficient: URL identity and relative URL resolution must become first-class
without treating URL strings as filesystem paths.

The existing frozen policy is local-file only. No network dependency is
currently present in the workspace, and `wasm-ffi` resolves standard libraries
from its build-time virtual-source bundle.

## 4. Rust Crate Assessment

The assessment below reflects the maintained crate documentation available on
2026-08-11.

| Candidate | Strengths | Costs / mismatch | Decision |
|---|---|---|---|
| [`ureq` 3.x](https://docs.rs/ureq/latest/ureq/) | Blocking API; pure Rust; HTTP/HTTPS; Rustls or native TLS; redirects, proxies and detailed timeouts; bounded body readers | Native socket transport is not a browser-WASM solution | **Selected for native transport** |
| [`url` 2.x](https://docs.rs/url/latest/url/) | WHATWG URL parsing, normalization, and correct relative-reference joining | No transport | **Selected for URL identity/resolution** |
| [`reqwest`](https://docs.rs/reqwest/latest/reqwest/) | Mature and highly configurable; async and blocking clients | Larger dependency/runtime surface than this synchronous compiler path needs; blocking client is unavailable on `wasm32-unknown-unknown` | Rejected for this slice |
| [`curl`](https://docs.rs/curl/latest/curl/) | Very mature protocol and platform behavior | Reintroduces a C/libcurl build and linking dependency into the Rust port | Rejected |
| [`minreq`](https://docs.rs/minreq/latest/minreq/) | Small blocking client | Its own documentation recommends a more robust client for uncontrolled servers; TLS/features require more careful assembly | Rejected |
| [`gloo-net`](https://docs.rs/gloo-net/latest/gloo_net/) | Idiomatic browser Fetch bindings | Browser-only and asynchronous; cannot be inserted into the current synchronous parser contract | Deferred host-side option |

`ureq` is the closest match to the compiler's blocking read model. Its agent
configuration supports an explicit redirect ceiling and granular timeouts, and
its body reader supports an explicit size limit. `url::Url::join` supplies the
remote equivalent of resolving a local import against the importing file's
parent directory.

## 5. Target Architecture

### 5.1 Source identity

Introduce a canonical source locator instead of encoding every source as a
filesystem path:

```rust
pub enum SourceLocator {
    File(PathBuf),
    Url(url::Url),
    Virtual(PathBuf),
}
```

Required invariants:

- file identities remain canonical `PathBuf` values;
- URL identities allow only `http` and `https` schemes;
- URL fragments are removed before lookup because they are not part of the
  HTTP resource request;
- relative imports in a remote source use `Url::join` against that source;
- virtual sources keep normalized logical paths and never fall through to the
  network;
- equality, hashing, cache keys, cycle edges, and origin records use the same
  normalized locator representation;
- local public APIs remain source-compatible where possible: `used_files()`
  continues to return only filesystem sources, while a new `used_sources()`
  reports the complete ordered locator list.

This is an `adapted` public-API mapping. It avoids fake paths such as
`PathBuf::from("https://...")`, whose parsing and display are platform
dependent and incorrect on Windows.

### 5.2 Dependency inversion

The parser owns source-resolution semantics but must not own a concrete network
stack. Define a synchronous injected interface at the parser boundary:

```rust
pub trait RemoteSourceFetcher: Send + Sync {
    fn fetch(&self, request: &FetchRequest)
        -> Result<FetchedSource, SourceFetchError>;
}
```

`FetchRequest` carries the normalized requested URL and the immutable policy
limits. `FetchedSource` carries bytes, requested URL, final URL, and optional
response metadata needed for diagnostics. The parser receives either no
fetcher (network disabled) or an injected implementation.

The concrete `UreqSourceFetcher` belongs in `crates/compiler`, above `parser`
in the dependency graph. This keeps `ureq` out of parser-core and lets tests
inject a deterministic fake transport. `url` remains in `parser` because URL
joining and canonical identity are resolution semantics, not HTTP transport.

### 5.3 Feature and runtime policy

Network imports must require two independent opt-ins:

1. a Cargo feature such as `network-imports`, disabled by default;
2. an explicit compiler/runtime option such as `--allow-network-imports`.

Feature-off builds retain no HTTP client dependency. Feature-on builds still
perform no request unless the runtime option is enabled. A URL encountered
without permission produces a stable structured diagnostic rather than being
reported as an ordinary missing local file.

The native transport is target-gated away from `wasm32-unknown-unknown`.
`wasm-ffi` continues to use immutable embedded sources; it must not gain hidden
network access. A future browser host can prefetch sources and inject them as
virtual sources, or a separate async compile API can be designed, but neither
is part of this plan.

### 5.4 Default fetch policy

Freeze the following initial defaults:

- methods: GET only;
- schemes: HTTP and HTTPS only;
- redirect ceiling: three, matching C++;
- timeout: five seconds for connection/response inactivity, matching the
  visible C++ default, plus a finite whole-request ceiling;
- response size: explicit 10 MiB maximum, configurable through the library API
  but not initially exposed as a CLI flag;
- accepted payload: strict UTF-8 after a bounded byte read;
- cookies: disabled;
- credentials: user-info URLs rejected initially;
- authorization forwarding across redirects: disabled;
- cache: compilation-session memory only; no implicit disk or cross-run cache;
- user agent: a stable `faust-rs/<version>` value;
- referer: absent by default;
- retries: none; a compiler invocation must not silently multiply requests;
- proxy behavior: no new Faust-specific proxy API in the first slice; any
  inherited client behavior must be made explicit and tested before release.

The 10 MiB bound is a safety adaptation. The legacy C++ buffer grows without a
hard ceiling, which permits unbounded memory consumption. A distinct
`ResponseTooLarge` diagnostic documents the compatibility difference.

### 5.5 Host security boundary

Remote imports are an SSRF-capable facility when a compiler is embedded in a
server. The fetch policy must therefore support an optional host predicate or
allowlist before the feature is exposed through a server-facing API.

The native CLI may allow arbitrary HTTP(S) hosts after the user's explicit
runtime opt-in, including loopback hosts useful for local development. Embedded
and multi-user services must keep networking disabled unless the host injects
an allowlist policy. Redirect destinations must be checked by the same policy
as the original URL.

Diagnostics and logs must redact URL passwords and must not copy response
bodies into error messages. Query strings are retained for request identity but
should be omitted from ordinary progress logs because they may contain tokens.

## 6. Resolution and Precedence Contract

Resolution order must stay deterministic:

1. immutable virtual source with an exact logical name;
2. configured `-I` search roots, in their existing order;
3. the importing local file's directory;
4. for a remote importer, URL joining against the importer's URL;
5. configured remote `-I` roots, if and only if remote search roots are later
   admitted explicitly.

An explicit absolute HTTP(S) import bypasses local-path probing but still
requires network permission. `file://` remains a local filesystem locator and
must be converted through `Url::to_file_path`, not string slicing.

The first implementation should not reinterpret every unresolved local import
as a network request. Network access occurs only for an explicit URL or a
relative import whose parent source is already remote.

Redirect aliases must not defeat cycle detection. The reader records the
requested normalized locator for source provenance and associates the final
normalized URL as an alias in the session cache. A redirect back to an active
requested or final locator is an import cycle.

## 7. Diagnostics Contract

Add stable source-reader diagnostics for at least:

- invalid or unsupported URL;
- network imports disabled;
- host rejected by policy;
- DNS/connect/TLS/timeout failure;
- non-success HTTP status;
- redirect limit or rejected redirect target;
- response body too large;
- non-UTF-8 Faust source.

Diagnostics retain the importing source and import-site span when available,
the sanitized requested URL, and a compact transport reason. Concrete `ureq`
error formatting must not become the stable public contract; transport errors
are mapped into Rust-owned categories first.

## 8. Implementation Phases

### Phase 0 — Contract fixtures and baseline

Status: complete (`6a8cdcf7`).

1. Add self-contained local HTTP-server fixtures that the Rust and pinned C++
   compilers can both consume over plain HTTP.
2. Record C++ behavior for direct URL input, URL imports, redirects, status
   failures, relative nested imports, and `checkURL` architecture lookup.
3. Confirm which public compiler and FFI entry points are allowed to enable
   network access.

Pass criteria: the tested C++ contract and intentional security adaptations are
written down before production transport code lands.

### Phase 1 — Locator model without networking

Status: transport-independent boundary complete; production import expansion
is wired to it in Phase 3.

1. Introduce `SourceLocator` and migrate cache, cycle, origin, and usage
   tracking.
2. Preserve `Path`/`PathBuf` entry points and local behavior.
3. Add URL parsing/joining unit tests using only in-memory fake sources.
4. Add `RemoteSourceFetcher` injection with a disabled implementation.

Pass criteria: all existing tests and corpus gates remain green; feature-off
URL inputs fail with the new stable disabled-network diagnostic; no HTTP crate
is linked.

### Phase 2 — Native `ureq` transport

Status: complete for the native transport; compiler import wiring follows in
Phase 3.

1. Add target-gated optional `ureq` dependency in `compiler`.
2. Implement the frozen timeout, redirect, body-size, UTF-8, header, and error
   mapping policy.
3. Enforce the host policy on the initial URL and every redirect.
4. Add deterministic tests through a loopback server and transport fakes;
   tests never access the public Internet.

Pass criteria: success, failure, redirect, timeout classification, oversized
body, invalid UTF-8, and policy rejection are covered on Linux, macOS, and
Windows.

### Phase 3 — Compiler and CLI integration

Status: complete for structural `import(...)` graphs; evaluator-driven remote
`component(...)`/`library(...)` loading remains an explicit Phase 4 API-surface
item.

1. Thread the fetch policy through `Compiler` options without global state.
2. Add the explicit runtime CLI option and truthful `compile_options`
   propagation where applicable.
3. Support direct remote entry sources and nested relative URL imports.
4. Preserve deterministic source/library lists and sanitized metadata.

Pass criteria: native CLI differential fixtures match C++ for the shared HTTP
contract; feature-off and runtime-off behavior is tested separately.

### Phase 4 — Enrobage and public API surface

1. Route remote architecture-file `checkURL` behavior through the same policy
   and fetcher rather than a second client.
2. Map affected Rust, C, and C++ facade APIs as `adapted` or `deferred`.
3. Keep embedded/server-facing APIs network-disabled until an explicit host
   allowlist can be injected.

Pass criteria: no public entry point silently enables network access and no
second fetch implementation exists.

### Phase 5 — Closure and maintenance gates

1. Add a compact remote-import corpus fixture or a dedicated hermetic fixture
   suite; versioned corpus sources themselves must remain local.
2. Update `DIFF-GAP-003`, supported-subset documentation, CLI help, and API
   documentation.
3. Run the mandatory parser/compiler compilation-cost gate.

Pass criteria: feature-on/off CI is green, the local HTTP differential has no
unclassified mismatch, and `DIFF-GAP-003` is removed or narrowed to explicitly
deferred browser/server behavior.

## 9. Validation Matrix

Every implementation commit must run the normal repository gates. The final
slice additionally requires:

| Dimension | Required cases |
|---|---|
| Build policy | feature absent; feature present/runtime denied; feature present/runtime allowed |
| Transport | HTTP success; HTTPS success with a local trusted test certificate or transport fake; DNS/connect/TLS failure |
| Redirect | none; one relative redirect; three redirects; fourth rejected; redirect target denied |
| Body | empty; normal UTF-8; chunked; oversized; invalid UTF-8 |
| Import graph | direct URL; remote relative child; duplicate import; redirect alias; remote cycle; mixed remote/virtual/local |
| Diagnostics | human, JSON, and diagnostics-v2 stable category checks |
| Platforms | Linux, macOS, Windows; `wasm32-unknown-unknown` remains network-free |
| Regression | parser unit tests; compiler integration tests; golden gates; corpus numerical differential where applicable |

Implementation touches to `parser` or `compiler` also require:

```text
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- golden-check
cargo run --release -p xtask -- compile-budget-check
```

## 10. Explicit Non-goals

The initial implementation does not include:

- asynchronous compilation;
- browser-side fetching from inside `wasm-ffi`;
- FTP, data URLs, Git repositories, package registries, or arbitrary URI
  schemes;
- authentication, cookies, or credential forwarding;
- persistent HTTP or source caches;
- remote source discovery after an ordinary local lookup fails;
- byte-for-byte reproduction of legacy `http_strerror()` messages;
- preservation of the unsafe process-global C++ setter API.

## 11. Completion Definition

This plan is complete when native Faust compilation can consume explicit
HTTP(S) sources and their relative remote imports under an explicit policy,
with hermetic tests, bounded resource use, deterministic disabled behavior,
and documented API adaptation. Merely adding `ureq::get(...)` at an unresolved
import call site does not satisfy the source-identity, cycle, diagnostics,
security, or cross-target contracts above.
