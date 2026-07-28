# Phase 2 — Block Diagrams (Boxes)

> **Crate**: `boxes`
> **Estimate**: 12–16 person days
> **Prerequisites**: Phase 1 (`tlib`, `diagnostics`)

---

## 1. C++ Inventory

### 1.1 boxes/ — 3,231 lines, 7 files

| File | Lines | Role |
|---------|--------|------|
| `boxes.hh` | ~600 | **180 constructors/destructors**: `boxInt`, `boxReal`, `boxIdent`, `boxSeq`, `boxPar`, `boxSplit`, `boxMerge`, `boxRec`, `boxWire`, `boxCut`, `boxSlot`, `boxRoute`, `boxWaveform`, `boxSoundfile`, `boxButton`, `boxHSlider`, `boxVSlider`, `boxNumEntry`, `boxHBargraph`, `boxVBargraph`, `boxHGroup`, `boxVGroup`, `boxTGroup`, `boxPrim0`…`boxPrim5`, `boxFFun`, `boxFConst`, `boxFVar`, `boxModulation`, `boxOnDemand`, `boxDownSampling`, `boxUpSampling`, `boxEnvironment`, `boxComponent`, `boxLibrary`, `boxImport`, `boxAccess`, `boxWithLocalDef`, `boxError`, etc. Each constructor has a corresponding `isBox*` |
| `boxes.cpp` | ~1,200 | Implementation: creation of symbols (`gGlobal->BOXIDENT`, etc.), constructors `tree(sym, ...)`, pattern matching `isTree(t, sym, &a, &b, ...)` |
| `boxtype.cpp` | ~400 | Box type inference: `getBoxType(box, &inputs, &outputs)` — determines the number of inputs/outputs |
| `boxcomplexity.hh/.cpp` | ~200 | Complexity calculation (block diagram size metrics) |
| `ppbox.hh/.cpp` | ~500 | Pretty-printing: `boxpp(box)` for human display of boxes |

---

## 2. Structure analysis

### 2.1 How boxes are encoded in C++

Each box constructor uses a **unique symbol** (created once in `gGlobal`) and builds a tree:

```cpp
// In global.cpp: gGlobal->BOXSEQ = symbol("BoxSeq");
// In boxes.cpp:
Tree boxSeq(Tree x, Tree y) { return tree(gGlobal->BOXSEQ, x, y); }
bool isBoxSeq(Tree t, Tree& x, Tree& y) { return isTree(t, gGlobal->BOXSEQ, x, y); }
```

→ All boxes are generic trees with a discriminating symbol. There is no static typing.

### 2.2 Rust strategy: Typed Enum vs generic trees

**Option A — Typed Enum** (recommended):
```rust
pub enum BoxKind {
    Int(i32),
    Real(f64),
    Ident(String),
    Wire,                        // _
    Cut,                         // !
    Seq(TreeId, TreeId),         // a : b
    Par(TreeId, TreeId),         // a , b
    Split(TreeId, TreeId),       // a <: b
    Merge(TreeId, TreeId),       // a :> b
    Rec(TreeId, TreeId),         // a ~ b
    Route(TreeId, TreeId, TreeId),
    Slot(i32),
    Prim0(Prim0Op),
    Prim1(Prim1Op),
    Prim2(Prim2Op),
    Prim3(Prim3Op),
    Prim4(Prim4Op),
    Prim5(Prim5Op),
    FFun(FFunDesc),
    FConst(FConstDesc),
    FVar(FVarDesc),
    Button(TreeId),              // label
    Checkbox(TreeId),
    HSlider(TreeId, TreeId, TreeId, TreeId, TreeId),
    VSlider(TreeId, TreeId, TreeId, TreeId, TreeId),
    NumEntry(TreeId, TreeId, TreeId, TreeId, TreeId),
    HBargraph(TreeId, TreeId, TreeId),
    VBargraph(TreeId, TreeId, TreeId),
    Soundfile(TreeId, TreeId),
    HGroup(TreeId, TreeId),
    VGroup(TreeId, TreeId),
    TGroup(TreeId, TreeId),
    Waveform(Vec<TreeId>),
    Modulation(TreeId, TreeId),
    OnDemand(TreeId),
    DownSampling(TreeId, TreeId),
    UpSampling(TreeId, TreeId),
    Environment(TreeId),
    Component(TreeId),
    Library(TreeId),
    Import(TreeId),
    Access(TreeId, TreeId),
    WithLocalDef(TreeId, TreeId),
    PatternMatcher(PatternMatcherRef),
    BoxError,
    // ... ~10 additional constructors
}
```

**Advantages of enum**:
- Exhaustive pattern matching guaranteed by the Rust compiler
- Static typing: impossible to confuse an `Seq` box with an `Par` box
- Performance: no symbol lookup, discrimination by tag

**Inconvenience** :
- The arena should store `BoxKind` rather than a generic `Node`
- Possibility: we keep the generic arena (Node/TreeId) but we provide a typed API layer on top

**Decision: Hybrid approach** — We keep the generic `TreeArena` (Phase 1) but we provide a **typed API layer** for the boxes:

```rust
/// Typed layer above TreeArena for boxes
pub struct BoxBuilder<'a> {
    arena: &'a mut TreeArena,
}

impl<'a> BoxBuilder<'a> {
    pub fn int(&mut self, n: i32) -> TreeId;
    pub fn real(&mut self, r: f64) -> TreeId;
    pub fn wire(&mut self) -> TreeId;
    pub fn cut(&mut self) -> TreeId;
    pub fn seq(&mut self, a: TreeId, b: TreeId) -> TreeId;
    pub fn par(&mut self, a: TreeId, b: TreeId) -> TreeId;
    // ... etc.
}

/// Pattern matching: constructor extraction
pub enum BoxMatch<'a> {
    Int(i32),
    Real(f64),
    Wire,
    Cut,
    Seq(TreeId, TreeId),
    Par(TreeId, TreeId),
    // ...
    Unknown,
}

pub fn match_box(arena: &TreeArena, id: TreeId) -> BoxMatch<'_>;
```

This approach preserves compatibility with the generic `TreeArena` while providing type certainty.

### 2.3 Canonical API rollout (`BoxBuilder` + `match_box`)

Decision: standardize box construction/traversal around:
- `BoxBuilder<'a>` for construction,
- `match_box(arena, id) -> BoxMatch` for canonical dispatch.

Status: migration completed for public `boxes` API.
- Public construction/inspection is now `BoxBuilder` + `match_box`.
- Legacy `box_*` / `is_box_*` remain internal-only implementation helpers in `crates/boxes`.

Implementation steps:
1. Introduce a non-breaking `BoxBuilder` facade in `crates/boxes`.
- Provide `BoxBuilder::new(&mut TreeArena)` and constructor methods grouped by family (composition, primitives, UI, scope/module, foreign/case, wrappers).
- Keep signatures parity-friendly with C++ constructor families.

2. Introduce canonical matcher API.
- Add `BoxMatch` + `match_box(...)` as the single entry point for read-side dispatch.
- Ensure `BoxMatch` covers all constructors currently needed by parser/eval/propagate scopes.

3. Migrate callers to canonical APIs and remove public legacy wrappers.
- Parser/actions use `BoxBuilder` methods.
- Read-side checks use `match_box` decoding.

4. Migrate consumers incrementally.
- New passes (`eval`/`propagate`/future transforms) must use `match_box` directly to avoid duplicated `is_box_*` ladders.

Deliverables:
- `crates/boxes`: `BoxBuilder`, `BoxMatch`, `match_box`.
- Mapping note in Rustdoc: constructor/matcher status (`1:1`, `adapted`, `deferred`) for touched families.
- Tests proving canonical matcher coverage.

Pass criteria:
- No behavior drift on existing parser corpus and differential checks.
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --all-targets`
- No new consumer module introduces ad-hoc box dispatch outside `match_box`.

---

## 3. Detailed mapping

### 3.1 Box builders (~180)

Categorization:

| Category | Number | Examples |
|-----------|--------|----------|
| Values | 3 | `boxInt`, `boxReal`, `boxIdent` |
| Composition | 5 | `boxSeq`, `boxPar`, `boxSplit`, `boxMerge`, `boxRec` |
| Routing | 3 | `boxWire`, `boxCut`, `boxRoute` |
| Primitives | ~20 | `boxPrim0`…`boxPrim5`, `boxFFun`, `boxFConst`, `boxFVar` |
| UI Widgets | ~12 | `boxButton`, `boxCheckbox`, `boxHSlider`, `boxVSlider`, `boxNumEntry`, `boxHBargraph`, `boxVBargraph`, `boxSoundfile` |
| UI Groups | 3 | `boxHGroup`, `boxVGroup`, `boxTGroup` |
| Modularity | ~8 | `boxEnvironment`, `boxComponent`, `boxLibrary`, `boxImport`, `boxAccess`, `boxWithLocalDef`, `boxSlot` |
| Waveform | 1 | `boxWaveform` |
| Multi-rate | 3 | `boxOnDemand`, `boxDownSampling`, `boxUpSampling` |
| Modulation | 1 | `boxModulation` |
| Pattern | 2 | `boxPatternMatcher`, `boxCase` |
| Error | 1 | `boxError` |
| Metadata | ~5 | `boxMetadata`, `boxIPar`, `boxISeq`, `boxISum`, `boxIProd` |

### 3.2 Type inference (boxtype.cpp)

```rust
/// Result of box type inference
#[derive(Clone, Copy, Debug)]
pub struct BoxType {
    pub inputs: usize,
    pub outputs: usize,
}

/// Infers the number of inputs/outputs of a box
pub fn get_box_type(
    arena: &TreeArena,
    box_id: TreeId,
    env: &BoxTypeEnv,
) -> Result<BoxType, BoxTypeError>;
```

In C++, `getBoxType` is recursive with memoization via `gGlobal->gBoxTypeTable`. In Rust, we use a `TreeProperty<BoxType>` for the cache.

### 3.3 Pretty-printing (ppbox.cpp)

```rust
/// Pretty-printer for boxes
pub struct BoxPrinter<'a> {
    arena: &'a TreeArena,
    shared: bool,  // mode ppboxShared vs ppbox
}

impl<'a> BoxPrinter<'a> {
    pub fn print(&self, id: TreeId) -> String;
}

impl std::fmt::Display for BoxDisplay<'_> { /* ... */ }
```

### 3.4 Complexity (boxcomplexity.cpp)

```rust
pub fn box_complexity(arena: &TreeArena, id: TreeId) -> usize;
```

---

## 4. Global symbols → constants

In C++, `boxes.cpp` creates ~100 symbols in `gGlobal`:
```cpp
gGlobal->BOXIDENT = symbol("BoxIdent");
gGlobal->BOXSEQ   = symbol("BoxSeq");
// ...
```

In Rust, these symbols become constants in the `TreeArena`:

```rust
impl TreeArena {
    /// Pre-registered symbols for boxes
    pub(crate) fn init_box_symbols(&mut self) -> BoxSymbols {
        BoxSymbols {
            box_ident: self.symbols.intern("BoxIdent"),
            box_seq:   self.symbols.intern("BoxSeq"),
            box_par:   self.symbols.intern("BoxPar"),
            // ...
        }
    }
}

pub(crate) struct BoxSymbols {
    pub box_ident: SymId,
    pub box_seq: SymId,
    pub box_par: SymId,
    // ... ~100 fields
}
```

Alternatively, if we use the enum approach, symbols are no longer necessary — the enum tag is enough to discriminate.

---

## 5. Dependencies

```
boxes → tlib, errors
```

No dependency on `interval`, `signals` or `graph` at this point.

---

## 6. Known pitfalls

### 6.1 Symbols duplicated via gGlobal
In C++, box symbols are in `gGlobal`. Each compilation recreates these symbols. In Rust, they are part of the `TreeArena` (or are static constants if we keep the enum approach).

### 6.2 boxPrim* and the link to the operations
`boxPrim2(sigAdd)` links a box to a signal operation. In C++, it's a `Tree (*)(Tree, Tree)` function pointer. In Rust, we will use a `enum Prim2Op { Add, Sub, Mul, ... }` which will be resolved later during propagation.

### 6.3 Pattern matching and evaluation
`boxPatternMatcher` contains a compiled automaton (from `patternmatcher/`). In phase 2, we store it as an opaque reference — the full implementation will come in phase 4 (evaluation).

### 6.4 boxRoute and combinatorial complexity
`boxRoute(n, m, r)` defines explicit routing from n inputs to m outputs. The evaluation of `r` produces a list of pairs, but this is done at evaluation (phase 4), not here.

---

## 7. Testing

- **Unit**: Create each type of box, check the pattern matching round-trip
- **Unit**: `get_box_type` on known compositions (seq, par, split, merge, rec)
- **Unit**: `box_complexity` on examples
- **Unit**: Pretty-printing vs expected strings
- **Completeness**: Check that each `boxes.hh` constructor has a Rust equivalent
- **Differential**: Parse a simple Faust file with C++, serialize the box tree, compare with the Rust version

---

## 8. "Done" criteria

- [ ] `boxes.hh`'s ~180 constructors and destructors have a Rust equivalent
- [ ] `get_box_type` passes the composition tests (seq, par, split, merge, rec)
- [ ] Pretty-functional printing
- [ ] Hash-consing: `boxSeq(a, b) == boxSeq(a, b)` (same TreeId)
- [ ] Canonical box API in place (`BoxBuilder` + `match_box`) and public legacy wrappers removed
- [ ] No dependency on `gGlobal`
- [ ] Complete Rustdoc
- [ ] `Send + Sync` verified

---

## 9. Detailed Effort

| Sub-module | LOC C++ | Estimated LOC Rust | Days |
|-------------|---------|-----------------|-------|
| Constructors/destructors (boxes.hh/.cpp) | 1,800 | 1200–1500 | 6–8 |
| Type inference (boxtype.cpp) | 400 | 300 | 2 |
| Pretty-printing (ppbox.hh/.cpp) | 500 | 400 | 2 |
| Complexity (boxcomplexity.hh/.cpp) | 200 | 150 | 1 |
| Tests + docs | — | 500 | 2–3 |
| **Total** | **3,231** | **2,550–2,850** | **13–16** |
