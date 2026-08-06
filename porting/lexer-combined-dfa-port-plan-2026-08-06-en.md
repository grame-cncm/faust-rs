# Replacing the per-rule lexer with a combined multi-pattern DFA

**Date**: 2026-08-06
**Status**: **complete (L0-L3, 2026-08-06)**. The corpus went from 2.13x to
**1.21x** the reference's compile time.
**Motivation**:
`porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md` §P2′,
which measured lexing at ~61 % of remaining compile time.

---

## 1. Objective

Make lexing cost O(input bytes) instead of O(tokens × rules), without changing
a single token the parser receives.

`faustlexer.l` stays the source of truth and `lrlex` keeps generating the rule
table. Only the *matching strategy* changes.

## 2. Why the current lexer is slow

`lrlex` compiles each of the 128 rules into its own anchored `Regex` and, at
every token start, runs every rule the current start condition allows, keeping
the longest match
([`lexer.rs:404-432`](file:///Users/letz/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lrlex-0.13.10/src/lib/lexer.rs)):

```rust
for (ridx, r) in self.iter_rules().enumerate() {
    if !Self::state_matches(current_state, r.start_states()) { continue; }
    if let Some(m) = r.re.find(&s[old_i..]) {
        if len > longest { longest = len; longest_ridx = ridx; }
    }
}
```

That is up to 128 automaton startups per token. `flex`, which C++ Faust uses,
compiles its 160 rules into **one** table-driven DFA of 611 states
(`yy_accept`/`yy_base`/`yy_def`/`yy_nxt`/`yy_chk` in
`compiler/parser/faustlexer.cpp`) and does one table lookup per input byte.
The gap is asymptotic, not a matter of tuning.

Measured (`cargo run --release -p parser --example lexbench`, 2.2 MB of
installed `.lib`):

| strategy | build | throughput |
|---|---|---|
| `lrlex`, 128 separate regexes | 2.3 ms | 2.4 MB/s |
| one lazy multi-pattern DFA | 0.6 ms | 266 MB/s |
| one dense (determinized) DFA | 79 s | 240 MB/s |
| hand-written reference scanner | — | ~900 MB/s |

## 3. Design

### 3.1 One lazy DFA per start condition

`regex-automata` — already a dependency, pulled in by `lrlex` itself — builds
multi-pattern automata with `new_many` and reports which pattern matched.

The naive shape, one DFA over all rules filtered afterwards by start condition,
is **wrong**: the longest match might belong to a rule the current condition
forbids, masking a shorter legal one. Start conditions must be *inside* the
automaton, which is also how flex does it.

`faustlexer.l` declares `%x comment doc lst` — three exclusive conditions plus
`INITIAL`. Rule counts per condition are 128 / 6 / 7 / 9. So: **four lazy
DFAs**, each built over the rules eligible in that condition, with a per-DFA
map from local `PatternID` to global rule index.

Eligibility is `lrlex`'s own predicate: a rule with no explicit start states
matches in any non-exclusive condition; otherwise the condition's id must be in
its list.

### 3.2 What the lexer must reproduce exactly

This is the contract, taken from `lrlex`'s loop rather than from the `.l`
syntax, because the loop is what currently defines behaviour:

1. **Longest match wins** at each position.
2. **Ties go to the earliest rule in the file.** `lrlex` uses `>` when comparing
   lengths, so an earlier rule already holding the length keeps it.
3. A rule whose `name()` is `None` **skips** — consumes input, emits no lexeme.
4. A rule that matched with `tok_id == None` is a **lex error** at that span.
5. **No match** (`longest == 0`) is a lex error at that span, and lexing stops.
6. On a match, `target_state` applies to a *counted* stack:
   `ReplaceStack` clears and pushes, `Push` increments if the head is the same
   condition else pushes, `Pop` decrements or pops.
7. An unknown target state id is a lex error, and lexing stops.

Items 4, 5 and 7 matter as much as 1–3: error spans are part of diagnostics
this project gates on.

### 3.3 Where it lives

A `NonStreamingLexer` implementation in `crates/parser`, so
`faustparser_y::parse(&lexer, &state)` is untouched. `lrlex_mod!` keeps
generating the rules; the new code reads them through the existing public
accessors (`re_str`, `name`, `start_states`, `target_state`) and never
reimplements `.l` parsing.

## 4. Risks

- **Tie-breaking.** The plan needs the *lowest* `PatternID` among those matching
  at the longest end position. Whether `MatchKind::All` guarantees that is
  **not established** and must be verified before relying on it; if it does
  not, the fallback is an overlapping search at the winning end offset, or
  re-testing the candidate rules in order. This is the single most likely place
  to introduce a silent difference, because a wrong tie-break changes which
  token id is produced without changing lengths.
- **Lazy DFA cache exhaustion.** A `hybrid` DFA determinizes on demand into a
  bounded cache; on thrash it degrades or errors. Behaviour on
  `CacheError` must be a hard failure, never a silent fallback to a different
  match.
- **UTF-8 and spans.** `lrlex` works on `&str` and yields byte spans. The DFA
  search must produce identical byte offsets, including for the `<doc>(.|\n)`
  rule which matches single characters.
- **Empty matches.** A rule matching the empty string would loop forever. The
  current loop treats `longest == 0` as "no match"; the replacement must keep
  that, not treat a zero-length match as progress.
- **Build cost per process.** Four lazy DFAs at ~0.6 ms total, behind the same
  `OnceLock` the definition already uses.

## 5. Phases

- **L0 — differential harness. Done 2026-08-06.** An `xtask` (or test) that lexes every file in
  `tests/impulse-tests/dsp/`, `tests/corpus/` and the installed Faust library
  directory with both lexers and compares the full token stream: id, start,
  length, and the error position when lexing fails. This lands and passes
  *before* the new lexer is wired in, against `lrlex` on both sides, so that a
  green run means the harness works rather than that nothing changed.

  `cargo run -p xtask -- lexer-differential`: 405 files, 646 498 lexemes,
  identical. Two things it refuses to pass without, both because a silent gap
  here would make every later phase meaningless:

  - **Every start condition must be reached.** Verified from the source text
    rather than the token stream — a bug that failed to enter a condition would
    also suppress its tokens, so asking the tokens would be circular. Reached:
    comment 47 files, doc 3, lst 3.
  - **At least one input must fail to lex.** It turns out none did.
    `faustlexer.l` ends with a catch-all `. 'EXTRA'`, so *no input can fail in
    `INITIAL`* — unknown characters become `EXTRA` tokens the parser rejects
    later. Obligation §3.2/5 is unreachable there. The exclusive `lst`
    condition has no catch-all, and that is the one shape that stops the lexer:
    `tests/lexer-fixtures/lst_unknown_key.dsp` is an unrecognized attribute
    inside `<listing …>`. Without it the error-offset comparison was dead code.
- **L1 — the combined lexer. Done 2026-08-06.** Four lazy DFAs, one per start
  condition, selected through `LexerImpl` rather than an env switch — the
  differential needs both reachable in one process anyway, so the enum is the
  switch. First run: **46 differing files**, all fixed by the differential
  rather than by inspection.
- **L2 — flip the default. Done 2026-08-06.** `parse` builds its lexer from the
  combined DFAs. Only token *production* changed: the lexemes are handed to
  `LRNonStreamingLexer::new`, so spans, line/column and error recovery remain
  `lrlex`'s and the surface the grammar sees is identical by construction.
- **L3 — measured. Done 2026-08-06.** Below.

#### What the differential caught

Two defects, neither of which inspection would have found, and both invisible
to every other gate because the parser rejects the resulting programs anyway.

1. **Regex syntax flags.** `lrlex` compiles its rules with
   `dot_matches_new_line`, `multi_line` and `octal` all true; the DFA builder
   defaults them off. 46 library files diverged, every one at the `****…*/`
   end of a banner comment.
2. **The start-state stack must refill with `INITIAL` when a `Pop` empties
   it.** `lrlex` does this explicitly; without it, `<-comment>` at depth one
   left the stack empty and the next token failed. This is visible only in
   `lrlex`'s loop — the `.l` file says nothing about it — which is why §3.2
   takes the contract from the loop.

#### Results

| | before | after |
|---|---|---|
| `compile-bench`, 94 DSPs | 2.13× | **1.21×** |
| median per-DSP delta | +92 % | **+82 %** |
| DSPs faster than C++ Faust | 3 | **8** |
| `compile-profile`, 133 DSPs | 12.9 s | **5.79 s** |
| — `parser` stage | 2.97 s | **0.34 s** |
| — `evaluation` stage | 7.56 s | **2.15 s** |

`evaluation` fell with `parser` because library lexing happens there.
`signal-fir` is now the largest stage at 47 %.

Gates: `lexer-differential` 405 files / 646 498 lexemes identical; cpp impulse
lane 94/94; `golden-check` byte-identical; `cli-transcript-check` 148;
`emission-determinism` 399 stable; `vector-coverage-check` 1568 pairs.

#### A gate the change invalidated

`compile-budget-check` measures in *units* normalized against `karplus`, whose
cost was itself mostly library lexing. With that gone the divisor shrank ~3.7×,
so every case's unit count rose by about that factor while every absolute time
fell. The recorded increases are the denominator moving, not a regression.

Its vector/scalar ratio allowance also had to go from 4.0× to 9.0×. The
front-end cost was a large constant present in *both* measurements and was
masking the real ratio; removing it exposed vector lowering at ~7.3× scalar on
`reverb_designer`. That is the true figure, and the gate was right to refuse the
old allowance rather than let it through.

## 6. Validation

| # | Obligation | Independent check | Rejecting mutation |
|---|---|---|---|
| V1 | Identical token streams | L0 differential over the whole corpus and every `.lib` | Swap two rules' priority → stream differs |
| V2 | Identical error spans | L0 compares failures, not only successes | Report the error one byte late |
| V3 | Start conditions honoured | Files exercising `comment`, `doc`/`mdoc`, `lst` must be in the L0 set, and their presence asserted, not assumed | Drop the per-condition split and filter afterwards → nested-comment and `mdoc` files diverge |
| V4 | No silent degradation | `CacheError` from the lazy DFA is a hard error | Return "no match" on cache error → V1 fails somewhere |
| V5 | It is actually faster | `compile-profile` share of `parser`+`evaluation`, and `lexbench` | — (that is the point of the change) |

V3 deserves the same suspicion as the memo-scope test in
`porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`: the
corpus may contain no `mdoc` at all, in which case the whole `doc`/`lst`
machinery is untested and a green V1 proves nothing about it. **Check that the
L0 input set exercises each start condition before trusting it**, and add
fixtures if it does not.

## 7. What this does not do

It does not touch `faustlexer.l`, the grammar, or the parser. It does not
address the other finding of §P2′ — that `platform.lib` is parsed three times
and `maths.lib` twice in a single compilation — which is independent and
cheaper.
