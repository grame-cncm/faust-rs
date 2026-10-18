# tlib

Foundation crate for hash-consed tree data used throughout the `faust-rs` compiler.

All compiler intermediate representations (`boxes`, `signals`, `fir`) are stored
as nodes in a shared `TreeArena`.  Structural hash-consing guarantees that
identical subtrees share the same `TreeId`, enabling cheap equality tests and
deduplication.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/tlib/tree.hh` / `tree.cpp` | Hash-consed tree core |
| `compiler/tlib/list.hh` / `list.cpp` | Linked-list primitives on trees |
| `compiler/tlib/property.hh` | Node-keyed property store |
| `compiler/tlib/node.hh` / `symbol.hh` | Node payload kinds and symbol table |

## Public API

| Item | Description |
|---|---|
| `TreeArena` | Owns all nodes; provides interning and child-list management |
| `TreeId` | Lightweight copyable node handle |
| `NodeKind` | Node payload: `Nil`, `Cons`, `Symbol`, `StringLiteral`, `Int(i64)`, `FloatBits(u64)`, or interned `Tag(u32)` |
| `PropertyStore` | Node-keyed side-channel storage (parser `def`/`use` props, …) |
| `de_bruijn_rec` / `de_bruijn_ref` | De Bruijn recursive-tree builders |
| `sym_rec` / `sym_ref` | Symbolic recursive-tree builders |
| `de_bruijn_to_sym` | Convert de Bruijn form to symbolic form |
| `lift_de_bruijn` / `lift_de_bruijn_n` | Lift free de Bruijn references |
| `validate_faust_list` | Check canonical `cons`/`nil` list shape |
| `validate_closed_de_bruijn_tree` | Check de Bruijn trees are closed and convertible |
| `validate_symbolic_recursion_tree` | Check symbolic recursion binders/refs are well formed |

## Parity invariants

- **Structural hash-consing**: `(NodeKind, children)` → unique `TreeId`.
- `nil` / `cons` list behavior follows Faust C++ conventions.
- Properties are node-keyed with fast interned-key lookup paths.

## Recursive trees (Phase 5)

Two recursive-tree encodings are supported:

| Encoding | C++ equivalent |
|---|---|
| De Bruijn (`de_bruijn_rec` / `de_bruijn_ref`) | `CTree` recursive form with index references |
| Symbolic (`sym_rec` / `sym_ref`) | Named-symbol form after `deBruijn2Sym` |

`de_bruijn_to_sym` ports C++ `deBruijn2Sym` with explicit `RecursionError` returns
instead of process-global assertions.

## Position in the pipeline

`tlib` is the foundation of the tree-encoded IR crates (`boxes`, `signals`,
`ui`, `fir`) and of the passes that walk them; crates that do not handle trees
(for example `diagnostics`, `interval`, `codegen`, `foreign-call`) do not
depend on it.
