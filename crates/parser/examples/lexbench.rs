//! Where lexing time goes, and what a faster lexer could buy.
//!
//! Evidence for P2′ of
//! `porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`.
//! Run with `cargo run --release -p parser --example lexbench`.
//!
//! Reports three numbers: the cost of building the lexer definition, the
//! throughput of `lrlex` over the installed Faust library corpus, and the
//! throughput of a minimal hand-written scanner over the same bytes as an
//! order-of-magnitude reference.

use lrpar::Lexer;
use std::time::Instant;
fn main() {
    // 1. Cost of constructing the lexer definition (compiling 128 regexes).
    let t = Instant::now();
    for _ in 0..100 { let _d = parser::lexerdef(); }
    let build = t.elapsed().as_secs_f64() / 100.0;
    println!("lexerdef() build      : {:.3} ms", build * 1000.0);

    // 2. Cost of lexing, with the definition built once.
    let dir = std::path::Path::new("/usr/local/share/faust");
    let mut srcs = Vec::new();
    let mut bytes = 0usize;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map(|x| x == "lib").unwrap_or(false)
            && let Ok(s) = std::fs::read_to_string(&p) {
            bytes += s.len();
            srcs.push(s);
        }
    }
    let d = parser::lexerdef();
    let t = Instant::now();
    let mut toks = 0usize;
    for s in &srcs {
        let lx = d.lexer(s);
        for item in lx.iter() { if item.is_ok() { toks += 1; } }
    }
    let lex = t.elapsed().as_secs_f64();
    println!("lex {} .lib files ({:.1} KB, {toks} tokens): {:.1} ms  => {:.1} MB/s",
        srcs.len(), bytes as f64/1024.0, lex*1000.0, bytes as f64/1e6/lex);
    let t = Instant::now();
    let mut rn = 0usize;
    for s in &srcs { rn += reference_scan(s); }
    let rt = t.elapsed().as_secs_f64();
    println!("reference hand-written scan ({rn} tokens): {:.1} ms  => {:.1} MB/s",
        rt*1000.0, bytes as f64/1e6/rt);
    println!();
    println!("lrlex is {:.0}x slower than the reference scanner", lex/rt);
    println!("per token: lrlex {:.2} us, reference {:.3} us", lex*1e6/toks as f64, rt*1e6/rn as f64);
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
        if c.is_ascii_whitespace() { i += 1; continue; }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' { i += 1; }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') { i += 1; }
            i = (i + 2).min(b.len());
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
        } else if c.is_ascii_digit() || c == b'.' {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'e'
                || b[i] == b'f' || b[i] == b'+' || b[i] == b'-') { i += 1; }
        } else if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' { i += 1; }
            i += 1;
        } else {
            i += 1;
        }
        n += 1;
    }
    n
}
