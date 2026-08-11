# SourceFetcher C++ Baseline

Date: 2026-08-11

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`

Companion plan:
[`sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md`](sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md)

## 1. Purpose

Freeze the effective C++ transport contract before adding Rust production
network code. This baseline distinguishes behavior that can be differentially
matched from legacy implementation limitations that Rust intentionally adapts.

## 2. Source-derived contract

The pinned `sourcefetcher.hh/.cpp` defines:

| Property | Effective C++ behavior |
|---|---|
| Request model | blocking GET over HTTP/1.0 |
| TCP port | fixed port 80 |
| Explicit `host:port` | unsupported because the complete authority is passed to hostname resolution |
| TLS | absent |
| Accepted status range | 200 through 307, with 3xx handled as redirects |
| Redirect default | three |
| Read inactivity timeout | five seconds |
| Initial body allocation | 200 KiB, grown as necessary |
| Hard body limit | none |
| User agent | `HTTP Fetcher/0.2` by default |
| Referer | absent by default |
| Error state | process-global mutable state |

`sourcereader.cpp` recognizes both `http://` and `https://`. That lexical
recognition does not add TLS: an HTTPS locator still reaches the fixed-port raw
socket implementation. Rust therefore treats working HTTPS as an adapted
extension, not a behavior that can be byte-for-byte validated against this
reference.

## 3. Differential limitation

Hermetic CI servers bind an operating-system assigned loopback port. The C++
client cannot parse such a port and always connects to port 80. Binding the
fixture to port 80 would require platform-dependent privileges and would make
parallel tests conflict.

Consequently the maintained Rust fixture in
`crates/compiler/tests/support/http_server.rs` uses an ephemeral port and does
not invoke a locally installed Faust compiler. This follows the repository rule
that ordinary tests must be self-contained. C++ parity is established from the
pinned source for transport defaults and through functional compiler behavior
where a controlled port-80 environment is explicitly available outside CI.

## 4. Maintained route contract

`crates/compiler/tests/remote_source_baseline.rs` freezes reusable endpoints
for:

- a direct remote DSP;
- a nested relative library import;
- a relative redirect;
- arbitrary status and byte bodies through the shared fixture API;
- deterministic request recording.

Later phases extend this fixture with redirect ceilings, oversized bodies,
invalid UTF-8, cycles, and policy rejection. Tests never access the public
Internet.

## 5. Classified adaptations

The following Rust behavior is intentionally stronger than C++:

- real HTTPS certificate validation;
- explicit-port URL support;
- bounded response bodies;
- strict UTF-8 source validation;
- owned structured errors instead of process-global error strings;
- no network access unless compile-time and runtime policy both allow it;
- redirect-target policy validation;
- network-free `wasm32-unknown-unknown` compiler modules.

These adaptations must remain visible in the durable Rust-versus-C++
difference registry until the implementation is complete and `DIFF-GAP-003`
can be narrowed or removed.
