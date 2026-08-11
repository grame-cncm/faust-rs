# Corpus runtime differential

`expected-differences.txt` is the checked classification used by
`cargo run -p xtask -- corpus-runtime-diff`.

- `mismatch` means both compiler outputs execute successfully but differ
  numerically. Its tracking field must be a stable `DIFF-GAP-*` entry.
- `oracle` means the pinned C++ Interp backend cannot provide a valid executable
  reference for that mutually accepted source. It is skipped before execution
  and never counted as a match.

The command rejects unlisted differences, malformed or duplicate entries,
unknown corpus cases, and `mismatch` entries whose configured scenarios now
match. Remove an entry in the same commit that closes its gap.
