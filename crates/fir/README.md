# fir crate

FIR (Faust Intermediate Representation) construction and matching helpers used by
the compiler/codegen pipeline.

## Type conventions

- `FirType::UI`, `FirType::Sound`, and `FirType::Meta` are API handle kinds
  that are already pointer-shaped in the FIR model.
- Canonical signatures therefore use these variants directly:
  - `buildUserInterface(UI)`
  - `metadata(Meta)`
- Use `FirType::Ptr(...)` only to add explicit pointer indirection beyond that
  base handle level.
- Examples:
  - `UI` maps to `UI*` (C++) / `UIGlue*` (C backend glue layer).
  - `Ptr(UI)` maps to `UI**` / `UIGlue**`.
  - `Ptr(Ptr(FaustFloat))` maps to `FAUSTFLOAT**` (used by `compute` I/O).

## Generated-table sub-modules

A `rdtable`/`rwtable` whose content is computed at initialization time carries
its generator as a `FirMatch::SubModule` — a nested program with its own state,
an `instanceInit<Sub>` and a `fill<Sub>(count, table)`. `Module` therefore has
an eighth field, `sub_modules`; it is an empty `Block` for every program without
a generated table, which is the common case.

- `fir::subcontainer` flattens sub-modules for backends that cannot express a
  nested container (`interp`, `wasm`, `cranelift`, `codebox`, `julia`), under
  one of two `SubModuleStatePolicy` choices for where the generator's state
  lands. It also carries two general-purpose normalizations used by backends
  with no shared static storage: `promote_static_tables_to_struct` and
  `qualify_sub_module_bodies`.
- `fir::inliner` provides the hygienic cloning the flattening rests on.

A backend that meets a `SubModule` it cannot emit must fail. Silence is a
wrong-answer bug: the table declaration it *does* emit would then be filled by
nothing and read as zeros. Rules `FIR-SM01`…`FIR-SM06` in `fir::checker` are
what make that unrepresentable rather than merely discouraged — SM01 requires
each sub-module's fill to be called from a lifecycle body, SM06 requires that
call to cover the table's whole declared length.

Design and phase history:
`porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`.

## Verifier notes

- `fir::checker` is diagnostic-first: it returns a full report instead of
  stopping on the first error.
- Phase 3 now explicitly rejects `Void`-typed expressions in material-value
  positions such as local initializers, `StoreVar`, `TeeVar`, `Return(Some(_))`,
  and `ValueArray` elements. This matches backend expectations without changing
  FIR construction APIs.
