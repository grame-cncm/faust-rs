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
| `parse_program(input, source_file) -> ParseOutput` | Parse in-memory text, no import expansion (infallible) |
| `parse_program_with_options(input, source_file, &options) -> ParseOutput` | Same, with precision and shared metadata store from `ParseOptions` |
| `parse_program_with_imports(input, source_file, &options) -> Result<ParseOutput, SourceReaderError>` | Parse in-memory text and expand its imports (search paths, virtual sources, optional remote capability) |
| `parse_file(path, &options) -> Result<ParseOutput, SourceReaderError>` | Read a file and expand its imports |
| `parse_url(url, &options) -> Result<ParseOutput, SourceReaderError>` | Fetch an HTTP(S) entry through `ParseOptions::remote` and expand its imports |
| `parse_minimal(input) -> bool` | Whether `input` parses, for tests/tooling |
| `lex_tokens(input) -> Result<Vec<LexedToken>, String>` | Lex source text and return named tokens |
| `lex_stream(input, LexerImpl) -> LexOutcome` | Raw lexeme stream from either lexer implementation (corpus differential) |
| `lexerdef()` | A fresh compiled `lrlex` lexer definition |
| `import_candidates(name, search_paths, local_dir)` | Files an import may resolve to, in C++ `fopenSearch` order |

`ParseOptions::default()` is single precision with a fresh metadata store, no
search paths, no virtual sources and no networking; `with_float_size`,
`with_metadata_store`, `with_search_paths`, `with_virtual_sources` and
`with_remote` set each field.

### Types

| Item | Description |
|---|---|
| `ParseOptions` | Optional inputs shared by the entry points |
| `ParseOutput` | Root, raw errors, diagnostics bundle, metadata snapshot, used files/sources, parser state |
| `ParseState` | Parser state threaded through grammar actions (arena, `ParserCtx`, metadata store) |
| `ParserCtx` | Per-parse context replacing the C++ `gGlobal` parser subset |
| `LexedToken` | One lexed token: name, text, span, start line/col |
| `LexerImpl`, `LexOutcome`, `RawLexeme` | Lexer selection and raw lexeme stream of `lex_stream` |
| `PrimitiveOp` | Primitive operator family recognized by the parser |
| `SourceLocation` | Source position (file, line/col range) |
| `BoxProvenance`, `BoxOrigin`, `BoxOriginId`, `BoxOriginRole`, `LocatedBox` | Source origins recorded for parsed boxes |
| `WidgetDeclaration` | Widget declaration recorded by the parser |

Parser diagnostics are private pending records with a canonical `Severity` and
required `DiagnosticCode`; parse results expose them as a `DiagnosticBundle`.

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
| `VirtualSourceMap` | In-memory source bundle consulted before the filesystem |
| `ImportSite`, `ImportCycleEdge` | Import locations and the edges of a reported import cycle |
| `RemoteSourceFetcher` | Host-injected synchronous remote-source capability |
| `RemoteSourceCapability`, `RemoteFetchPolicy` | Fetcher plus immutable per-parse resource policy |
| `RemoteFetchRequest`, `FetchedSource`, `SourceFetchError`, `SourceFetchErrorKind` | Fetch request, result and error |
| `PrefetchedRemoteSourceBundle`, `PrefetchedRemoteSourceBundleError` | Immutable URL-keyed, I/O-free fetcher for browser/embedded hosts |
| `SourceReaderError` | Errors from local/remote loading, resolution, and import cycles |

## Position in the pipeline

Canonical pipeline (AGENTS.md): parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend.

The parser builds box trees with `boxes::BoxBuilder` and expands imports
structurally before `eval` runs.
