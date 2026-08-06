# Replacing the per-rule lexer with a combined multi-pattern DFA

**Date**: 2026-08-06
**Status**: plan; not implemented
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
- **L1 — the combined lexer, behind an env switch.** Implement it, default off,
  and make L0 compare old against new.
- **L2 — flip the default**, keeping the switch for bisection.
- **L3 — measure and record**: `compile-profile`, `compile-bench`, and the
  `compile-budget` baselines retightened.

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
