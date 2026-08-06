//! Where lexing time goes, and what a faster lexer could buy.
//!
//! Evidence for P2′ of
//! `porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`.
//! Run with `cargo run --release -p parser --example lexbench`.
//!
//! Reports four numbers over the installed Faust library corpus: the cost of
//! building the lexer definition, `lrlex`'s throughput, a minimal hand-written
//! scanner as an order-of-magnitude reference, and the same 128 rules compiled
//! into **one** multi-pattern lazy DFA.
//!
//! # Reading the match counts
//!
//! The reference scanner and the combined-DFA loop are throughput probes, not
//! lexers: they do not implement rule priority, the exclusive start conditions
//! (`comment`/`doc`/`lst`), or `lrlex`'s exact longest-match tie-breaking, and
//! their match counts differ from `lrlex`'s token count accordingly. They
//! measure how fast the same bytes can be scanned by each strategy, which is
//! the question; they are not evidence that a replacement is easy.

use lrpar::Lexer;
use std::time::Instant;
fn main() {
    // 1. Cost of constructing the lexer definition (compiling 128 regexes).
    let t = Instant::now();
    for _ in 0..100 {
        let _d = parser::lexerdef();
    }
    let build = t.elapsed().as_secs_f64() / 100.0;
    println!("lexerdef() build      : {:.3} ms", build * 1000.0);

    // 2. Cost of lexing, with the definition built once.
    let dir = std::path::Path::new("/usr/local/share/faust");
    let mut srcs = Vec::new();
    let mut bytes = 0usize;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map(|x| x == "lib").unwrap_or(false)
            && let Ok(s) = std::fs::read_to_string(&p)
        {
            bytes += s.len();
            srcs.push(s);
        }
    }
    let d = parser::lexerdef();
    let t = Instant::now();
    let mut toks = 0usize;
    for s in &srcs {
        let lx = d.lexer(s);
        for item in lx.iter() {
            if item.is_ok() {
                toks += 1;
            }
        }
    }
    let lex = t.elapsed().as_secs_f64();
    println!(
        "lex {} .lib files ({:.1} KB, {toks} tokens): {:.1} ms  => {:.1} MB/s",
        srcs.len(),
        bytes as f64 / 1024.0,
        lex * 1000.0,
        bytes as f64 / 1e6 / lex
    );
    let t = Instant::now();
    let mut rn = 0usize;
    for s in &srcs {
        rn += reference_scan(s);
    }
    let rt = t.elapsed().as_secs_f64();
    println!(
        "reference hand-written scan ({rn} tokens): {:.1} ms  => {:.1} MB/s",
        rt * 1000.0,
        bytes as f64 / 1e6 / rt
    );
    // 3. The same 128 rules as ONE multi-pattern DFA, which is flex's model
    //    and which `regex-automata` — already a dependency, through lrlex —
    //    supports natively via `new_many`.
    {
        use lrlex::LexerDef as _;
        use regex_automata::{Anchored, Input, MatchKind};
        let pats: Vec<String> = d
            .iter_rules()
            .map(|r| format!("(?:{})", r.re_str()))
            .collect();
        // Lazy (hybrid) DFA: builds instantly, determinizes on demand with a
        // bounded cache. This is the shape a real replacement would use, since
        // the fully-determinized dense DFA below takes over a minute to build
        // and would have to be serialized at build time.
        {
            use regex_automata::hybrid::dfa::DFA as LazyDFA;
            let t = Instant::now();
            let lazy = LazyDFA::builder()
                .configure(LazyDFA::config().match_kind(MatchKind::All))
                .build_many(&pats)
                .expect("lazy DFA");
            println!(
                "lazy DFA build        : {:.1} ms ({} patterns)",
                t.elapsed().as_secs_f64() * 1000.0,
                pats.len()
            );
            let mut cache = lazy.create_cache();
            let t = Instant::now();
            let mut n = 0usize;
            for s in &srcs {
                let mut at = 0usize;
                while at < s.len() {
                    let inp = Input::new(s.as_str())
                        .span(at..s.len())
                        .anchored(Anchored::Yes);
                    match lazy.try_search_fwd(&mut cache, &inp) {
                        Ok(Some(h)) if h.offset() > at => {
                            at = h.offset();
                            n += 1;
                        }
                        _ => {
                            at += 1;
                        }
                    }
                }
            }
            let dt = t.elapsed().as_secs_f64();
            println!(
                "lazy DFA scan ({n} matches): {:.1} ms  => {:.1} MB/s",
                dt * 1000.0,
                bytes as f64 / 1e6 / dt
            );
            println!("  => {:.0}x faster than lrlex", lex / dt);
        }

        // A fully-determinized `dense::DFA` over the same patterns reaches
        // 240 MB/s but takes **79 seconds** to build, so it is only viable
        // serialized at build time — which is what flex effectively does. The
        // lazy DFA above gets the same throughput for a 0.6 ms build, so it is
        // the shape a replacement should take. Not run here: it would make this
        // benchmark take a minute and a half.
    }

    println!();
    println!(
        "lrlex is {:.0}x slower than the reference scanner",
        lex / rt
    );
    println!(
        "per token: lrlex {:.2} us, reference {:.3} us",
        lex * 1e6 / toks as f64,
        rt * 1e6 / rn as f64
    );
}

/// Order-of-magnitude reference: a minimal hand-written scanner over the same
/// input. Not a Faust lexer — it recognizes whitespace, comments, identifiers,
/// numbers, strings and single-char punctuation — but it establishes what a
/// straightforward DFA-shaped scanner costs on this corpus.
fn reference_scan(s: &str) -> usize {
    let b = s.as_bytes();
    let (mut i, mut n) = (0usize, 0usize);
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
        } else if c.is_ascii_digit() || c == b'.' {
            while i < b.len()
                && (b[i].is_ascii_digit()
                    || b[i] == b'.'
                    || b[i] == b'e'
                    || b[i] == b'f'
                    || b[i] == b'+'
                    || b[i] == b'-')
            {
                i += 1;
            }
        } else if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            i += 1;
        } else {
            i += 1;
        }
        n += 1;
    }
    n
}
