# parser

Production Faust parser — `lrpar`/`lrlex` grammar-generated crate.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/parser/faustparser.y` | Grammar rules |
| `compiler/parser/faustlexer.l` | Lexer rules |
| `compiler/errors/errormsg.hh` / `errormsg.cpp` | `setDefProp` / `setUseProp` |

## What this crate does

Tokenizes and parses Faust source text into a box tree (`boxes::BoxId`) stored
in a `TreeArena`. Handles `import("...")` expansion, top-level metadata
(`declare key "value";`), waveform accumulation, and definition/use property
tracking. Emits structured `diagnostics::Diagnostic` values for all parse
errors.

## Public API

### Entry points

| Function | Description |
|---|---|
| `parse_program(arena, ctx, src)` | Parse in-memory source text |
| `parse_program_with_metadata(arena, ctx, src)` | Parse + return metadata snapshot |
| `parse_file_with_imports(path, search_paths)` | Parse a file, recursively expanding imports |
| `parse_file_with_imports_and_metadata(path, search_paths)` | File parse + metadata |
| `parse_program_with_remote_imports_and_precision_and_metadata(...)` | Parse an in-memory root with an injected HTTP(S) source capability; an URL-named root is the base for relative imports |
| `parse_minimal(arena, ctx, src)` | Minimal parse for testing/tooling |
| `lex_tokens(src)` | Lex source text and return named tokens |
| `lexerdef()` | Returns the compiled `lrlex` lexer definition |
| `with_state(arena, ctx, f)` | Run a closure with a fresh `ParseState` |
| `set_use_prop_from_token(arena, token_id)` | Mark a symbol as used |

### Types

| Item | Description |
|---|---|
| `ParseOutput` | Full result of one parse: root + errors + diagnostics + metadata + state |
| `ParseState` | Mutable parser state (arena, context, cursor, metadata store) |
| `ParserCtx` | Per-parse context replacing the C++ `gGlobal` parser subset |
| `LexedToken` | One lexed token: name, text, span, start line/col |
| `PrimitiveOp` | Primitive operator family recognized by the parser |

### Context and diagnostics

| Item | Description |
|---|---|
| parser diagnostics | Private pending records with canonical `Severity` and required `DiagnosticCode`; exported parse results use `DiagnosticBundle` |
| `SourceLocation` | Source cursor position (file, line, col) |

### Metadata

| Item | Description |
|---|---|
| `CompilationMetadataStore` | Shared store for `declare key "value";` entries |
| `CompilationMetadataSnapshot` | Immutable snapshot of collected metadata |
| `CompilationMetadataKey` | Key: `Global { key }` or `Scoped { source_file, key }` |

### Source reader

| Item | Description |
|---|---|
| `SourceReader` | File, virtual, and URL import resolver with cycle detection and session caching |
| `ExpandedSource` | Expanded source text with line-origin tracking |
| `SourceLineOrigin` | Maps an expanded line back to its original file and line number |
| `SourceLocator` | Canonical file, virtual-path, or HTTP(S) source identity |
| `RemoteSourceFetcher` | Host-injected synchronous remote-source capability |
| `RemoteSourceCapability` | Fetcher plus immutable per-parse resource policy |
| `PrefetchedRemoteSourceBundle` | Immutable URL-keyed, I/O-free fetcher for browser/embedded hosts |
| `SourceReaderError` | Errors from local/remote loading, resolution, and import cycles |

## Position in the pipeline

```
source files  →  [parser]  →  boxes  →  eval  →  propagate
```
