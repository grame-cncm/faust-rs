# Precedence-aware expression printing: only Cmajor has it

**Date**: 2026-08-05
**Status**: analysis complete, no code changed; follow-up task filed (see §6)
**Question asked**: given that `cpp`/`c` already share statement/loop emission
through `c_family` and avoid the "Fields vs Body branch duplicates the same
trailing logic" shape found on `asc` and `cmajor` during today's
documentation sweep, is there a *bigger* factoring opportunity across the
other backends rather than the small in-file helpers extracted so far?

## Summary

Yes, and it is more than a style nit. `crates/codegen/src/backends/textual.rs`
implements precedence-aware infix printing — a faithful Rust port of the C++
reference compiler's `TextInstVisitor::{leftArgNeedsParentheses,
rightArgNeedsParentheses}` — and its own module doc explicitly invites reuse:
*"A future textual backend with different precedence rules can therefore use
the same algorithm... Cmajor and other C-like targets can use
`c_like_fir_operator` directly."*

Only `cmajor` actually uses it. `c_family` (shared by `c`/`cpp`), `rust`,
`julia`, `asc`, and `codebox` each carry their own independent `emit_binop`/
`emit_binop_expr` that **always fully parenthesizes** every binary expression,
regardless of precedence. This is the exact defect class that was already
found and fixed for Cmajor: on the `bells.dsp` fixture, full parenthesization
produced expressions **111 levels deep**, and the Cmajor compiler (`cmaj`)
rejected the output outright. Switching to precedence-aware printing dropped
the depth to 3 and `bells` started compiling (`porting/journal/2026-08-04.md`,
"Share precedence-aware textual expression layout"; corroborated in
`porting/cmajor-backend-port-and-test-plan-2026-08-04-en.md` lines 673-678).

So this is not cosmetic duplication — it is a **fidelity gap against the
upstream C++ reference**, which the `c_family.rs` module doc itself confirms
uses one shared `Text.hh` module for precedence/parenthesization across *all*
of its textual backends: *"Upstream `/Users/letz/faust/compiler/generator/
Text.hh` plays the same role for every one of upstream's textual backends:
one shared literal/operator-formatting module instead of one copy per
backend."* The Rust port fragmented that single upstream module into: a
`c_family` core that unifies `c`+`cpp` but skips precedence-awareness, a
`textual.rs` module that has precedence-awareness but only one caller, and
three more backends (`rust`, `julia`, `asc`, `codebox`) with no sharing at
all.

Whether any of the non-Cmajor backends have an *active* problem (a real
target-toolchain rejection, the way Cmajor did) is **not established** — see
§5. What is established is that the same latent risk exists structurally in
five more emission paths.

## 1. How this was found

While reviewing `cpp`'s `mod.rs` during the ongoing doc/factorization sweep
across `crates/codegen/src/backends/*`, `DeclareVar`/`DeclareTable` emission
turned out to already be centralized in `c_family::emit_stmt_common`, so the
narrow "two branches duplicate the same trailing logic" shape fixed earlier
that day on `asc` (`emit_declare_var_init`) and `cmajor`
(`static_qualifier`) did not exist there. The user asked whether that implied
a *deeper* shared-core opportunity across the other backends, by analogy with
`c_family`. Tracing what `c_family` actually shares — and what its own module
doc says upstream shares — led to `textual.rs` and the discovery that it has
exactly one caller.

## 2. Evidence

Every non-Cmajor textual backend's binop renderer was located and checked for
a `use ... textual::` import:

| Backend | Binop renderer | Uses `textual.rs`? |
|---|---|---|
| `cmajor` | `emit_infix_operand` + `emit_value`'s `BinOp` arm | **Yes** — the only caller |
| `c_family` (shared by `c`, `cpp`) | `emit_binop_expr` (`c_family.rs:330`) | No — `format!("({lhs} {} {rhs})", ...)` unconditionally |
| `rust` | `emit_binop_expr` (`rust/mod.rs:~1807`) | No |
| `julia` | `emit_binop_expr` (`julia/mod.rs:~1328`) | No |
| `asc` | `FirMatch::BinOp` arm (`asc/mod.rs:~1042`) | No |
| `codebox` | `FirMatch::BinOp` arm (`codebox/mod.rs:~910`) | No (documented as a deliberate current scope limit in `eval.rs`'s own module doc: "arithmetic and comparison operators, always parenthesised") |
| `wasm` | n/a | Not applicable — WASM emits binary bytecode via an operand stack, not text; parenthesization is not a concept there |

`grep -rl "backends::textual\|super::textual" crates/codegen/src/backends`
returns exactly one file: `cmajor/mod.rs`.

## 3. How Cmajor's fix works (the reference shape)

`cmajor::emit_value`'s `BinOp` arm does **not** parenthesize its own result;
it defers that decision entirely to the caller:

```rust
FirMatch::BinOp { op, lhs, rhs, typ } => {
    let lhs = emit_infix_operand(store, options, op, lhs, OperandSide::Left)?;
    let rhs = emit_infix_operand(store, options, op, rhs, OperandSide::Right)?;
    let expression = format!("{lhs} {} {rhs}", emit_binop(op));
    // ... comparison-to-int32 wrapping, unrelated to parenthesization ...
}
```

`emit_infix_operand` renders the child normally, then inspects the child's
own FIR shape (not the rendered string) to decide whether the *parent*
operator requires wrapping it:

```rust
fn emit_infix_operand(..., parent_op: FirBinOp, operand: FirId, side: OperandSide)
    -> Result<String, CodegenError>
{
    let rendered = emit_value(store, options, operand)?;
    let FirMatch::BinOp { op: child_op, .. } = match_fir(store, operand) else {
        return Ok(rendered); // not a binop: never needs parens here
    };
    let needs_parentheses = infix_operand_needs_parentheses(
        c_like_fir_operator(parent_op),
        c_like_fir_operator(child_op),
        side,
        parent_op == child_op,
    );
    Ok(if needs_parentheses { format!("({rendered})") } else { rendered })
}
```

This is the shape any future port should mirror: **the child decides nothing
about its own wrapping; the parent decides, once, by comparing operators**.
Backends that instead have the child always self-wrap (every backend checked
in §2) cannot locally patch their way to this behavior — the self-wrapping
has to be removed from the child and the decision moved to the parent.

## 4. Why `rust` is not a drop-in port (the complication that stopped a live attempt)

The user asked to start a proof-of-concept on `rust`. Before writing code,
its actual binop emission turned out to differ from Cmajor's in two ways
that make a direct port of §3 unsafe without further design work:

**(a) Integer arithmetic is not infix.** Faust's C semantics require
wrapping overflow, so `rust::emit_binop_expr` routes `Add`/`Sub`/`Mul`/`Div`/
`Rem` on `Int32`/`Int64` through method calls instead of operators:

```rust
Ok(format!("({l}).{method}({r})"))  // e.g. "(a).wrapping_add(b)"
```

This has the *same* unbounded-depth problem as full parenthesization — each
nesting level adds one more `(...)` around the entire receiver
(`((a).wrapping_add(b)).wrapping_add(c)`, etc. — this was confirmed by
tracing FIR's left-associative `BinOp(Add, BinOp(Add, a, b), c)` shape
through the renderer) — but `infix_operand_needs_parentheses` does not apply
to it: there is no C-like infix operator here to compare precedence against.
Rust's method-call postfix `.` chains left-associatively without needing
parens around the receiver at all (`a.wrapping_add(b).wrapping_add(c)` is
valid and means the same thing), so the actual fix for *this* path is a
different, Rust-specific question — "is the receiver already a valid
postfix/primary expression?" — not a port of `textual.rs`.

**(b) Casts interleave with operand rendering.** `coerce_rendered` may wrap
an operand in an explicit `(x as T)` cast *before* it reaches the combining
`format!`, when the operand's own FIR type differs from the parent's target
type. A cast expression is already atomic/fully-parenthesized, so it never
needs a further precedence-based wrap — but that means a correct port cannot
just inspect the *raw* child `FirId`'s shape the way Cmajor's
`emit_infix_operand` does; it has to know whether the coercion step actually
inserted a cast for that specific operand, and only fall through to
`infix_operand_needs_parentheses` when it did not.

Only Rust's genuinely plain-infix paths — float `Add`/`Sub`/`Mul`/`Div`/
`Rem`, bitwise `And`/`Or`/`Xor`, shifts, and comparisons — are directly
analogous to Cmajor's shape. The integer wrapping-method path is a separate
problem wearing the same symptom.

## 5. What is *not* yet known

- Whether `julia`, `asc`, `codebox`, or `c`/`cpp` have an equivalent
  *practical* failure the way Cmajor did on `bells` — i.e. whether their
  target toolchains (`julia`, `asc`/AssemblyScript's compiler, RNBO's
  `codebox~` parser, a C/C++ compiler) actually reject or choke on deeply
  nested parenthesized expressions the way `cmaj` did. C/C++ compilers are
  generally far more tolerant of parenthesis depth than Cmajor's toolchain
  turned out to be; this has not been measured for any of them.
- Whether `bells.dsp` (or another deep-associative-chain fixture) currently
  renders pathologically deep output for these backends today. This would be
  the cheapest way to convert "structurally the same risk" into "confirmed
  active risk" for a given backend before investing in a fix.
- Whether `julia`/`asc`/`codebox` have the same kind of non-infix rendering
  complication found for `rust` in §4, or whether their binop paths are
  simple enough for a direct port of §3's shape. Not investigated per
  backend yet.

## 6. Disposition

Given the complication in §4 surfaced mid-implementation, the proof-of-concept
on `rust` was stopped rather than pushed through partially. The full finding
(this document's §2-§5) was instead filed as a follow-up task,
`task_37b2ce9f`, "Port precedence-aware expression printing to non-cmajor
textual backends", scoped to:

- treat each backend's plain-infix operators (float arithmetic, bitwise,
  shifts, comparisons) as the first-pass target, mirroring §3 exactly;
- explicitly exclude method-call-based paths (Rust's `wrapping_*`) from that
  first pass — they need their own design, not a copy of `textual.rs`'s
  algorithm;
- validate against each backend's existing C++ differential/parity test
  suite, the same safety net that validated the Cmajor fix;
- start with checking §5's open questions (does a deep-chain fixture actually
  misbehave for a given backend today?) before assuming every backend needs
  the change urgently.
