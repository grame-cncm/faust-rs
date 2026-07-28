# Phase 1 — Foundations

> **Crates**: `tlib`, `errors`, `utils`, `interval`, `algebra`, `graph`
> **Estimate**: 35–45 person days
> **Prerequisites**: none (initial phase)

---

## 1. C++ Inventory

### 1.1 tlib/ — 4,319 lines, 21 files

| File | Lines | Role |
|---------|--------|------|
| `tree.hh` / `tree.cpp` | ~900 | Hash-consing: `CTree`, `CTree::make()`, properties, serialization |
| `node.hh` / `node.cpp` | ~400 | Union tagged `Node` (`kIntNode`, `kInt64Node`, `kDoubleNode`, `kSymNode`, `kPointerNode`) |
| `symbol.hh` / `symbol.cpp` | ~300 | Global symbol table, `Symbol` with hash-consing by name |
| `list.hh` / `list.cpp` | ~400 | Cons lists (`cons`, `hd`, `tl`, `isNil`, `map`, `reverse`) via trees |
| `num.hh` | ~50 | `num` class for node arithmetic |
| `property.hh` | ~350 | Template `property<T>`: memoization by tree property |
| `occur.hh` / `occur.cpp` | ~200 | Occurrence counting (`Occur`) |
| `shlysis.hh` / `shlysis.cpp` | ~150 | Sharing analysis |
| `recursive-tree.cpp` | ~200 | De-Bruijn → symbolic recursive representation |
| `smartpointer.hh` | ~100 | Smart pointer `P<T>` with refcount |
| `compatibility.hh/.cpp` | ~80 | Portability stubs (timezone, etc.) |
| `dcond.hh/.cpp` | ~100 | Symbolic conditions (dead-code) |
| `garbageable.hh` | ~90 | Base class `Garbageable` (allocation pool, lightweight GC) |

### 1.2 errors/ — 412 lines, 5 files

| File | Lines | Role |
|---------|--------|------|
| `errormsg.hh/.cpp` | ~200 | `SigWarning()`, `SigError()`, error formatting with source localization |
| `exception.hh` | ~50 | `faustexception`: standard compiler exception |
| `timing.hh/.cpp` | ~160 | `startTiming()` / `endTiming()`: pass profiling |

### 1.3 utils/ — 805 lines, 8 files

| File | Lines | Role |
|---------|--------|------|
| `names.hh/.cpp` | ~250 | Generating unique names, manipulating UI paths |
| `files.hh/.cpp` | ~200 | I/O files, paths, embedding |
| `exepath.hh/.cpp` | ~100 | Executable path detection |
| `TMutex.h` | ~50 | Mutex for thread-safety (used by libfaust) |
| `tracer.hh` | ~30 | Optional tracing hook |

### 1.4 interval/ — 5,891 lines, 67 files

| File | Lines | Role |
|---------|--------|------|
| `interval_def.hh` | ~100 | `struct interval { double lo, hi; int lsb; }` + constructors |
| `interval_algebra.hh/.cpp` | ~400 | Class `interval_algebra`: dispatch to the 60+ operations |
| `intervalAdd.cpp` … `intervalXor.cpp` | ~80 each | 60 files, one per arithmetic/logic/trigo operation |
| `bitwiseOperations.hh/.cpp` | ~200 | Bitwise operations on intervals |
| `check.hh/.cpp` | ~200 | Interval Validation Tests |
| `precision_utils.hh` | ~50 | LSB/Precision Utilities |
| `utils.hh` | ~50 | Helpers (min/max on intervals) |

### 1.5 FaustAlgebra/ — 325 lines, 1 file

| File | Lines | Role |
|---------|--------|------|
| `FaustAlgebra.hh` | 325 | Abstract algebra: `Ring<T>` with +, -, ×, ÷ — template header-only |

### 1.6 DirectedGraph/ — 1,399 lines, 3 files

| File | Lines | Role |
|---------|--------|------|
| `DirectedGraph.hh` | ~600 | `digraph<N>` template: weighted directed graph, DFS, cycles |
| `DirectedGraphAlgorythm.hh` | ~500 | Topological sorting, strongly connected components (Tarjan), cycle graph |
| `Schedule.hh` | ~300 | `schedule<N>`: parallel/sequential sequence of nodes |

---

## 2. Mapping C++ → Rust

### 2.1 tlib

This is the fundamental crate. The entire representation of the trees (boxes, signals) is based on it.

#### 2.1.1 TreeArena (replaces CTree + static hash table)

```rust
/// Lightweight tree identifier in the arena. Copy + Eq + Hash + Ord.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TreeId(u32);

/// The different types of nodes (replaces enum NodeType + class Node)
#[derive(Clone, PartialEq)]
pub enum NodeValue {
    Int(i32),
    Int64(i64),
    Double(f64),            // note: custom PartialEq for NaN
    Sym(SymId),             // reference in the symbol table
    // Pointer removed: we use TreeId or concrete types instead
}

/// Internal node in the arena
struct TreeNode {
    value: NodeValue,
    branches: SmallVec<[TreeId; 4]>,  // most have ≤4 branches
    hash: u64,
    serial: u32,
    aperture: i32,
}

/// Arena with hash-consing
pub struct TreeArena {
    nodes: Vec<TreeNode>,
    intern: HashMap<u64, Vec<TreeId>>,  // hash → candidates
    symbols: SymbolTable,
}

impl TreeArena {
    pub fn make(&mut self, value: NodeValue, branches: &[TreeId]) -> TreeId;
    pub fn node(&self, id: TreeId) -> &NodeValue;
    pub fn arity(&self, id: TreeId) -> usize;
    pub fn branch(&self, id: TreeId, i: usize) -> TreeId;
    pub fn branches(&self, id: TreeId) -> &[TreeId];
}
```

**Key decisions**:
- `TreeId(u32)` is `Copy` → no lifetime, no refcount, pass by value
- Arena owns all data → no `Garbageable`, no GC
- `SmallVec<[TreeId; 4]>` to avoid heap allocation for small trees
- Hash-consing: same node + same branches → same `TreeId`

#### 2.1.2 Properties (replaces property<T> + fProperties)

```rust
/// Typed properties attached to trees (replaces property<T>)
pub struct TreeProperty<V> {
    data: HashMap<TreeId, V>,
}

impl<V> TreeProperty<V> {
    pub fn new() -> Self;
    pub fn get(&self, id: TreeId) -> Option<&V>;
    pub fn set(&mut self, id: TreeId, value: V);
    pub fn contains(&self, id: TreeId) -> bool;
    pub fn clear(&mut self);
}
```

Note: In C++ properties are stored *in* the CTree (via `fProperties`). In Rust we externalize them because it allows independent borrowing and avoids shared mutability.

#### 2.1.3 Lists (replaces list.hh)

```rust
/// Operations on cons lists (encoded as trees)
impl TreeArena {
    pub fn nil(&mut self) -> TreeId;
    pub fn cons(&mut self, head: TreeId, tail: TreeId) -> TreeId;
    pub fn hd(&self, list: TreeId) -> TreeId;
    pub fn tl(&self, list: TreeId) -> TreeId;
    pub fn is_nil(&self, id: TreeId) -> bool;
    pub fn list_len(&self, id: TreeId) -> usize;
    pub fn list_to_vec(&self, id: TreeId) -> Vec<TreeId>;
    pub fn vec_to_list(&mut self, v: &[TreeId]) -> TreeId;
    pub fn map_list(&mut self, id: TreeId, f: impl FnMut(&mut Self, TreeId) -> TreeId) -> TreeId;
    pub fn reverse_list(&mut self, id: TreeId) -> TreeId;
}
```

#### 2.1.4 Symbols (replaces symbol.hh)

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymId(u32);

pub struct SymbolTable {
    names: Vec<String>,
    lookup: HashMap<String, SymId>,
}

impl SymbolTable {
    pub fn intern(&mut self, name: &str) -> SymId;
    pub fn name(&self, id: SymId) -> &str;
}
```

#### 2.1.5 Recursive trees (replaces recursive-tree.cpp)

```rust
impl TreeArena {
    /// Convert de-Bruijn → symbolic recursive representation
    pub fn de_bruijn_to_sym(&mut self, t: TreeId) -> TreeId;
    /// Inverse
    pub fn sym_to_de_bruijn(&mut self, t: TreeId) -> TreeId;
}
```

#### 2.1.6 Occur, Shlysis, Dcond

```rust
pub struct OccurrenceCount { /* ... */ }
pub fn count_occurrences(arena: &TreeArena, root: TreeId) -> TreeProperty<OccurrenceCount>;
pub fn sharing_analysis(arena: &TreeArena, root: TreeId, barrier: impl Fn(TreeId) -> bool) -> TreeProperty<usize>;
pub fn dead_code_conditions(arena: &TreeArena, root: TreeId) -> TreeProperty<bool>;
```

### 2.2 errors

```rust
/// Source location (file + line/column range)
#[derive(Clone, Debug)]
pub struct SourceSpan {
    pub file: PathBuf,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// Severity level
#[derive(Clone, Copy, Debug)]
pub enum Severity { Error, Warning, Remark }

/// Compiler stage emitting a diagnostic
#[derive(Clone, Debug)]
pub enum Stage {
    SourceReader,
    Lexer,
    Parser,
    Eval,
    Propagate,
    Normalize,
    Transform,
    Fir,
    Codegen,
    Compiler,
}

/// Stable diagnostic code (for tests, CI, IDE)
#[derive(Clone, Copy, Debug)]
pub struct DiagnosticCode(&'static str);

/// One source label
#[derive(Clone, Debug)]
pub struct Label {
    pub span: SourceSpan,
    pub is_primary: bool,
    pub message: String,
}

/// One diagnostic entry
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub stage: Stage,
    pub code: DiagnosticCode,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub help: Vec<String>,
}

/// Aggregated diagnostics emitted by one phase/session
pub struct DiagnosticBundle {
    pub diagnostics: Vec<Diagnostic>,
}

/// Compiler error envelope
#[derive(Debug)]
pub struct FaustError {
    pub message: String,
    pub diagnostics: DiagnosticBundle,
}
impl std::error::Error for FaustError {}

/// Conversion contract for phase-local errors
pub trait ToDiagnostic {
    fn to_diagnostic(&self) -> Diagnostic;
}
```

Detailed architecture, code taxonomy, migration steps, and pass criteria:
- `porting/faust-rust-diagnostics-model-en.md`

### 2.3 uses

```rust
/// Unique name generation
pub struct NameGenerator {
    counters: HashMap<String, u32>,
}
impl NameGenerator {
    pub fn fresh(&mut self, prefix: &str) -> String;
}

/// UI paths
pub fn build_ui_path(group_labels: &[&str], widget_label: &str) -> String;
pub fn strip_url_and_tooltip(label: &str) -> (&str, Option<&str>, Option<&str>);

/// Files
pub fn search_file(name: &str, paths: &[PathBuf]) -> Option<PathBuf>;
pub fn include_file(path: &Path) -> io::Result<String>;
```

### 2.4 interval

```rust
/// Interval [lo, hi] with LSB precision
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
    pub lsb: i32,
}

impl Interval {
    pub const EMPTY: Interval = /* ... */;
    pub const FULL: Interval = /* ... */;
    pub fn new(lo: f64, hi: f64) -> Self;
    pub fn is_empty(&self) -> bool;
    pub fn contains(&self, x: f64) -> bool;
    pub fn intersect(self, other: Self) -> Self;
    pub fn union_(self, other: Self) -> Self;
}

/// Interval algebra (60+ operations)
pub struct IntervalAlgebra;

impl IntervalAlgebra {
    pub fn add(a: Interval, b: Interval) -> Interval;
    pub fn sub(a: Interval, b: Interval) -> Interval;
    pub fn mul(a: Interval, b: Interval) -> Interval;
    pub fn div(a: Interval, b: Interval) -> Interval;
    pub fn sin(a: Interval) -> Interval;
    pub fn cos(a: Interval) -> Interval;
    // ... 55+ additional operations
    pub fn delay(a: Interval, d: Interval) -> Interval;
    pub fn mem(a: Interval) -> Interval;
    pub fn button() -> Interval;
    pub fn hslider(lo: f64, hi: f64) -> Interval;
    // etc.
}
```

Note: Each `interval*.cpp` becomes a method. A single `algebra.rs` file with an `impl IntervalAlgebra` or an `IntervalOp` trait.

### 2.5 algebra

```rust
/// Abstract algebra Ring<T> (replaces FaustAlgebra.hh)
pub trait Ring: Sized + Clone {
    fn zero() -> Self;
    fn one() -> Self;
    fn add(self, other: Self) -> Self;
    fn sub(self, other: Self) -> Self;
    fn mul(self, other: Self) -> Self;
    fn div(self, other: Self) -> Self;
}

// Implementations for i32, f64, and TreeId (symbolic algebra)
impl Ring for i32 { /* ... */ }
impl Ring for f64 { /* ... */ }
```

### 2.6 graph

```rust
/// Weighted directed graph (replaces DirectedGraph.hh)
pub struct DiGraph<N: Hash + Eq + Clone, W = i32> {
    adj: HashMap<N, HashMap<N, W>>,
}

impl<N: Hash + Eq + Clone, W: Ord + Copy> DiGraph<N, W> {
    pub fn add_edge(&mut self, src: N, dst: N, weight: W);
    pub fn successors(&self, n: &N) -> impl Iterator<Item = (&N, &W)>;
    pub fn predecessors(&self, n: &N) -> impl Iterator<Item = (&N, &W)>;
    pub fn nodes(&self) -> impl Iterator<Item = &N>;
    pub fn topological_sort(&self) -> Result<Vec<N>, CycleError<N>>;
    pub fn tarjan_scc(&self) -> Vec<Vec<N>>;
    pub fn cycle_graph(&self) -> DiGraph<Vec<N>, W>;
}

/// Schedule (replaces Schedule.hh)
#[derive(Clone, Debug)]
pub enum Schedule<N> {
    Serial(Vec<Schedule<N>>),
    Parallel(Vec<Schedule<N>>),
    Atom(N),
}

impl<N> Schedule<N> {
    pub fn flatten_serial(&self) -> Vec<&N>;
    pub fn depth(&self) -> usize;
}
```

---

## 3. Dependencies between crates (Phase 1)

```
errors          (no internal dependency)
utils           → errors
algebra         (no internal dependency)
interval        → algebra
graph           (no internal dependency)
tlib            → errors
```

External dependencies:
- `smallvec` (for `SmallVec` in `TreeNode`)
- `hashbrown` or `std::collections::HashMap`
- `thiserror` (for `errors`)

---

## 4. Known pitfalls

### 4.1 Global state in CTree
C++ uses **static variables** in `CTree`:
- `gHashTable[400009]` — static hash-consing table
- `gSerialCounter` — global serial counter
- `gVisitTime` — global visit counter (non-reentrant!)

→ In Rust all this is in `TreeArena`, which belongs to `CompileSession`. No `static mut`.

### 4.2 In-tree vs. externalized properties
In C++, `CTree::fProperties` is an `map<Tree, Tree>` **in** each node. This creates a coupling between the tree and its annotations.

→ In Rust, we outsource with `TreeProperty<V>`. Advantage: you can have several simultaneous properties with independent loans. Disadvantage: the `t->getProperty(key)` pattern becomes `props.get(t)` — you have to pass the property maps explicitly.

### 4.3 Garbageable / allocation pool
In C++, `Garbageable` uses a global pool allocator which collects at the end of compilation.

→ In Rust, the `TreeArena` arena manages everything. `TreeId` are light clues (no pointer), liberation is done by dropping the arena. No need for GC.

### 4.4 Comparison of doubles (Node)
`Node` in C++ does `==` on doubles, which is a problem with NaN.

→ In Rust, implement `Eq` manually for `NodeValue::Double` using `f64::to_bits()` for hashing and bitwise equality.

### 4.5 void* in Node (kPointerNode)
In C++, `Node` can contain an `void*`. This is used to store `xtended*` (extended mathematical functions).

→ In Rust, replace with an `XtendedId(u16)` or a dedicated enum in `NodeValue`. No raw pointer.

---

## 5. Testing

### 5.1 tlib
- **Unit**: Create trees, check hash-consing (same node+branches → same TreeId)
- **Unit**: Properties — set/get/clear
- **Unit**: Cons lists — cons, hd, tl, map, reverse, nil
- **Unit**: de-Bruijn ↔ symbolic (round-trip)
- **Unit**: Serialization/ordering (`serial()`deterministic)
- **Bench**: criterion — creation of 100K trees, lookup in the hash table
- **Property**: proptest — for any tree t, `make(node(t), branches(t)) == t`

### 5.2 errors
- **Unit**: Collection of errors and warnings, counting, formatting
- **Unit**: Correct source location

### 5.3 interval
- **Unit**: Each operation (60+) with limiting cases (infinity, NaN, empty intervals)
- **Differential**: Compare with existing `check.cpp` test suite
- **Property**: For any binary op operation, if `x ∈ [a,b]` and `y ∈ [c,d]`, then `op(x,y) ∈ op([a,b],[c,d])`

### 5.4 graph
- **Unit**: Topological sorting (DAG), cycle detection
- **Unit**: Tarjan SCC on known examples
- **Unit**: Schedule serial/parallel

---

## 6. "Done" criteria

- [ ] All unit tests pass
- [ ] `TreeArena`: hash-consing verified (pointer identity = structural identity)
- [ ] Benchmark criterion: creation of 100K trees < 50ms
- [ ] `property<T>` migrated to `TreeProperty<V>` with documented API
- [ ] No `static mut` or `lazy_static` anywhere
- [ ] Every type and public function has a `///` Rustdoc
- [ ] `cargo clippy` without warning
- [ ] `cargo doc --no-deps` generates a clean doc
- [ ] All crates are `Send + Sync` (build tests)

---

## 7. Detailed Effort

| Crate | LOC C++ | Estimated LOC Rust | Person days |
|-------|---------|-----------------|----------------|
| tlib | 4,319 | 3,000–3,500 | 12–15 |
| errors | 412 | 300–400 | 2 |
| utils | 805 | 500–600 | 3 |
| interval | 5,891 | 3,500–4,000 | 10–12 |
| algebra | 325 | 200 | 1 |
| graph | 1,399 | 800–1000 | 5–7 |
| **Total Phase 1** | **13,151** | **8,300–9,700** | **33–40** |

The reduction in LOC comes from: no `Garbageable`, no separate header/impl, more concise Rust pattern matching, no `void*`.
