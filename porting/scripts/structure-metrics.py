#!/usr/bin/env python3
"""Structural metrics for the faust-rs readability audit.

Deliberately syntactic: a brace scanner that skips line/block comments and
string literals. Not a parser; every number it prints is reproducible and
conservative (it undercounts rather than invents).
"""
import json, os, re, sys
from collections import defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

def rs_files(sub="crates"):
    for dirpath, dirnames, filenames in os.walk(os.path.join(ROOT, sub)):
        dirnames[:] = [d for d in dirnames if d != "target"]
        for f in filenames:
            if f.endswith(".rs"):
                yield os.path.join(dirpath, f)

def rel(p): return os.path.relpath(p, ROOT)

def crate_of(p):
    r = rel(p).split(os.sep)
    return r[1] if len(r) > 1 and r[0] == "crates" else "?"

def is_test_file(p):
    r = rel(p)
    return "/tests/" in r or r.endswith("/tests.rs") or r.endswith("_tests.rs")

def strip_noise(src):
    """Blank out strings/comments, preserving line structure and braces."""
    out = []
    i, n = 0, len(src)
    in_line_c = in_block_c = in_str = in_chr = False
    raw_hashes = -1
    while i < n:
        c = src[i]
        nxt = src[i+1] if i+1 < n else ""
        if in_line_c:
            if c == "\n": in_line_c = False; out.append(c)
            else: out.append(" ")
        elif in_block_c:
            if c == "*" and nxt == "/": in_block_c = False; out.append("  "); i += 2; continue
            out.append(c if c == "\n" else " ")
        elif in_str:
            if raw_hashes >= 0:
                if c == '"' and src[i+1:i+1+raw_hashes] == "#"*raw_hashes:
                    in_str = False; raw_hashes = -1
                    out.append(" "*(1+raw_hashes)); i += 1+raw_hashes; continue
                out.append(c if c == "\n" else " ")
            else:
                if c == "\\": out.append("  "); i += 2; continue
                if c == '"': in_str = False
                out.append(c if c == "\n" else " ")
        elif in_chr:
            if c == "\\": out.append("  "); i += 2; continue
            if c == "'": in_chr = False
            out.append(" ")
        else:
            if c == "/" and nxt == "/": in_line_c = True; out.append("  "); i += 2; continue
            if c == "/" and nxt == "*": in_block_c = True; out.append("  "); i += 2; continue
            if c == "r" and re.match(r'r#*"', src[i:i+8] or ""):
                m = re.match(r'r(#*)"', src[i:])
                raw_hashes = len(m.group(1)); in_str = True
                out.append(" "*m.end()); i += m.end(); continue
            if c == '"': in_str = True; out.append(" ")
            elif c == "'" and re.match(r"'([^'\\]|\\.)'", src[i:]): in_chr = True; out.append(" ")
            else: out.append(c)
        i += 1
    return "".join(out)

BLOCK_RE = re.compile(r'\b(fn|impl)\b')

def blocks(path):
    """Yield (kind, name, start_line, end_line) for fn/impl items."""
    src = open(path, encoding="utf-8", errors="replace").read()
    clean = strip_noise(src)
    lines_at = [0]
    for ch in clean:
        pass
    # map offset -> line
    line_of = []
    ln = 1
    for ch in clean:
        line_of.append(ln)
        if ch == "\n": ln += 1
    line_of.append(ln)
    res = []
    for m in BLOCK_RE.finditer(clean):
        kind = m.group(1)
        # find the opening brace of this item, bail at ';' (trait decls, etc.)
        j = m.end(); depth_paren = 0; opened = None
        while j < len(clean):
            c = clean[j]
            if c == "(": depth_paren += 1
            elif c == ")": depth_paren -= 1
            elif c == ";" and depth_paren <= 0: break
            elif c == "{" and depth_paren <= 0: opened = j; break
            j += 1
        if opened is None: continue
        depth = 0; k = opened
        while k < len(clean):
            if clean[k] == "{": depth += 1
            elif clean[k] == "}":
                depth -= 1
                if depth == 0: break
            k += 1
        if k >= len(clean): continue
        name_m = re.match(r'\s+([A-Za-z_][A-Za-z0-9_]*)', clean[m.end():])
        name = name_m.group(1) if name_m else "?"
        res.append((kind, name, line_of[m.start()], line_of[k]))
    return res

report = {}

# ---- 1. lines per crate / per file -----------------------------------------
crate_lines = defaultdict(lambda: [0, 0])  # prod, test
file_lines = {}
for p in rs_files():
    n = sum(1 for _ in open(p, encoding="utf-8", errors="replace"))
    file_lines[rel(p)] = n
    crate_lines[crate_of(p)][1 if is_test_file(p) else 0] += n
report["crate_lines"] = {k: {"prod": v[0], "test": v[1]} for k, v in sorted(crate_lines.items())}

# ---- 2. production files over 1500 lines -----------------------------------
big = sorted(((n, f) for f, n in file_lines.items()
              if n > 1500 and not is_test_file(os.path.join(ROOT, f))), reverse=True)
report["files_over_1500"] = [{"file": f, "lines": n} for n, f in big]

# ---- 3. long fns and impls --------------------------------------------------
long_fns, long_impls = [], []
for p in rs_files():
    if is_test_file(p): continue
    try: bl = blocks(p)
    except Exception as e: print("skip", rel(p), e, file=sys.stderr); continue
    for kind, name, a, b in bl:
        span = b - a + 1
        if kind == "fn" and span > 150: long_fns.append({"file": rel(p), "name": name, "line": a, "lines": span})
        if kind == "impl" and span > 500: long_impls.append({"file": rel(p), "line": a, "lines": span})
long_fns.sort(key=lambda d: -d["lines"]); long_impls.sort(key=lambda d: -d["lines"])
report["fns_over_150"] = long_fns
report["impls_over_500"] = long_impls

# ---- 4. module tree depth ---------------------------------------------------
depths = defaultdict(int)
for p in rs_files():
    parts = rel(p).split(os.sep)
    try: d = len(parts) - parts.index("src") - 2
    except ValueError: d = 0
    depths[crate_of(p)] = max(depths[crate_of(p)], d)
report["max_module_depth"] = dict(sorted(depths.items(), key=lambda kv: -kv[1]))

# ---- 5. modules without //! header -----------------------------------------
no_header = []
for p in rs_files():
    if is_test_file(p): continue
    head = "".join(open(p, encoding="utf-8", errors="replace").readlines()[:40])
    if "//!" not in head: no_header.append(rel(p))
report["modules_without_header"] = sorted(no_header)

# ---- 6. pub items without rustdoc ------------------------------------------
PUB_RE = re.compile(r'^\s*pub(?:\((?:crate|super)\))?\s+(fn|struct|enum|trait|type|const|mod|union)\s+([A-Za-z_][A-Za-z0-9_]*)')
undoc = defaultdict(list)
for p in rs_files():
    if is_test_file(p): continue
    lines = open(p, encoding="utf-8", errors="replace").read().splitlines()
    for i, line in enumerate(lines):
        m = PUB_RE.match(line)
        if not m: continue
        if "pub(crate)" in line or "pub(super)" in line: continue
        j = i - 1
        documented = False
        while j >= 0:
            s = lines[j].strip()
            if s.startswith("///") or s.startswith("/**"): documented = True; break
            if s.startswith("#[") or s.startswith("#!") or s == "": j -= 1; continue
            break
        if not documented: undoc[crate_of(p)].append(f"{rel(p)}:{i+1} {m.group(1)} {m.group(2)}")
report["undocumented_pub"] = {k: len(v) for k, v in sorted(undoc.items(), key=lambda kv: -len(kv[1]))}
report["undocumented_pub_examples"] = {k: v[:3] for k, v in list(sorted(undoc.items(), key=lambda kv: -len(kv[1])))[:6]}

# ---- 7. anonymous tuple returns with >= 3 fields ---------------------------
TUP_RE = re.compile(r'->\s*\(([^()]*(?:\([^()]*\)[^()]*)*)\)')
tuples = []
for p in rs_files():
    if is_test_file(p): continue
    for i, line in enumerate(open(p, encoding="utf-8", errors="replace").read().splitlines()):
        m = TUP_RE.search(line)
        if not m: continue
        inner = m.group(1)
        depth = 0; fields = 1
        for ch in inner:
            if ch in "<([": depth += 1
            elif ch in ">)]": depth -= 1
            elif ch == "," and depth == 0: fields += 1
        if inner.strip() and fields >= 3:
            tuples.append({"file": rel(p), "line": i+1, "fields": fields, "sig": line.strip()[:110]})
tuples.sort(key=lambda d: -d["fields"])
report["tuple_returns_ge3"] = tuples

print(json.dumps(report, indent=1))
