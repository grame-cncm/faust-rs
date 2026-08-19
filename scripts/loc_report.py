#!/usr/bin/env python3
"""Report effective vs. test Rust lines of code for faust-rs.

A file counts as "test" code when:
  - it lives under a `tests/` directory (integration tests), or
  - it contains an inline `#[cfg(test)] mod ... { ... }` block, in which case
    only the lines inside that block count as test code; the rest of the
    file counts as effective code.

Line counts exclude blank lines and comment-only lines, using `cloc` under
the hood for the actual blank/comment classification (cloc is required on
PATH). Usage: `python3 scripts/loc_report.py [--by-crate]`.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = ROOT / "crates"

CFG_TEST_RE = re.compile(r"#\[cfg\(test\)\]")
MOD_OPEN_RE = re.compile(r"\bmod\s+\w+\s*\{")
CFG_TEST_MOD_DECL_RE = re.compile(
    r"#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*mod\s+(\w+)\s*;"
)
ALL_MOD_DECL_RE = re.compile(r"\bmod\s+(\w+)\s*;")


def resolve_mod_file(current: Path, name: str, existing: set[Path]) -> Path | None:
    """Resolve `mod NAME;` in `current` to its source file, Rust module rules."""
    directory = current.parent
    if current.stem in ("lib", "main", "mod"):
        candidates = [directory / f"{name}.rs", directory / name / "mod.rs"]
    else:
        subdir = directory / current.stem
        candidates = [subdir / f"{name}.rs", subdir / name / "mod.rs"]
    for candidate in candidates:
        if candidate in existing:
            return candidate
    return None


def find_external_test_files(rs_files: list[Path]) -> set[Path]:
    """Files pulled in wholly by `#[cfg(test)] mod NAME;`, transitively."""
    existing = set(rs_files)
    texts = {p: p.read_text(encoding="utf-8", errors="replace") for p in rs_files}

    external: set[Path] = set()
    frontier: list[Path] = []
    for path, text in texts.items():
        for m in CFG_TEST_MOD_DECL_RE.finditer(text):
            resolved = resolve_mod_file(path, m.group(1), existing)
            if resolved and resolved not in external:
                external.add(resolved)
                frontier.append(resolved)

    while frontier:
        path = frontier.pop()
        text = texts.get(path, "")
        for m in ALL_MOD_DECL_RE.finditer(text):
            resolved = resolve_mod_file(path, m.group(1), existing)
            if resolved and resolved not in external:
                external.add(resolved)
                frontier.append(resolved)
    return external


def is_test_dir_file(path: Path, external_test_files: set[Path]) -> bool:
    rel = path.relative_to(ROOT)
    if "tests" in rel.parts[:-1]:
        return True
    # `mod tests;` commonly points at a sibling `tests.rs` file rather than
    # an inline block; treat those (and `test.rs`) as wholly test code too.
    if path.stem in ("tests", "test"):
        return True
    return path in external_test_files


def crate_of(path: Path) -> str:
    rel = path.relative_to(CRATES_DIR)
    return rel.parts[0]


def split_inline_test_blocks(text: str) -> tuple[str, str]:
    """Return (effective_text, test_text) for a single source file's contents.

    Lines inside `#[cfg(test)] mod ... { ... }` blocks go to test_text (with
    all other lines blanked out); everything else goes to effective_text
    (with the test block's lines blanked out). Blanking preserves line
    numbers so cloc's blank/comment detection stays accurate line-by-line.
    """
    lines = text.splitlines(keepends=True)
    is_test_line = [False] * len(lines)

    i = 0
    while i < len(lines):
        if CFG_TEST_RE.search(lines[i]):
            # Find the `mod ... {` line (usually the next non-blank line).
            j = i + 1
            while j < len(lines) and not MOD_OPEN_RE.search(lines[j]):
                if lines[j].strip():
                    break
                j += 1
            if j < len(lines) and MOD_OPEN_RE.search(lines[j]):
                depth = lines[j].count("{") - lines[j].count("}")
                start = i
                end = j
                k = j + 1
                while k < len(lines) and depth > 0:
                    depth += lines[k].count("{") - lines[k].count("}")
                    end = k
                    k += 1
                for idx in range(start, end + 1):
                    is_test_line[idx] = True
                i = end + 1
                continue
        i += 1

    eff_lines = [line if not flag else "\n" for line, flag in zip(lines, is_test_line)]
    test_lines = [line if flag else "\n" for line, flag in zip(lines, is_test_line)]
    return "".join(eff_lines), "".join(test_lines)


def run_cloc(dir_path: Path) -> int:
    """Sum of Rust 'code' lines reported by cloc for a directory tree."""
    result = subprocess.run(
        ["cloc", "--include-lang=Rust", "--json", str(dir_path)],
        capture_output=True,
        text=True,
        check=True,
    )
    if not result.stdout.strip():
        return 0
    data = json.loads(result.stdout)
    return int(data.get("Rust", {}).get("code", 0))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--by-crate", action="store_true", help="Print a per-crate breakdown table."
    )
    args = parser.parse_args()

    rs_files = [p for p in CRATES_DIR.rglob("*.rs") if "target" not in p.parts]
    external_test_files = find_external_test_files(rs_files)

    per_crate_eff: dict[str, int] = defaultdict(int)
    per_crate_test: dict[str, int] = defaultdict(int)

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        eff_dir = tmp_path / "effective"
        test_dir = tmp_path / "test"
        eff_dir.mkdir()
        test_dir.mkdir()

        for idx, path in enumerate(rs_files):
            text = path.read_text(encoding="utf-8", errors="replace")
            crate = crate_of(path)
            if is_test_dir_file(path, external_test_files):
                eff_text, test_text = "", text
            else:
                eff_text, test_text = split_inline_test_blocks(text)

            (eff_dir / f"{idx}_{path.name}").write_text(eff_text, encoding="utf-8")
            (test_dir / f"{idx}_{path.name}").write_text(test_text, encoding="utf-8")

            # Per-crate breakdown computed the same way, per-file, below.

        total_eff = run_cloc(eff_dir)
        total_test = run_cloc(test_dir)

        if args.by_crate:
            # Re-run per crate for the breakdown table (separate temp trees).
            crates = sorted({crate_of(p) for p in rs_files})
            for crate in crates:
                with tempfile.TemporaryDirectory() as ctmp:
                    ctmp_path = Path(ctmp)
                    ceff = ctmp_path / "effective"
                    ctest = ctmp_path / "test"
                    ceff.mkdir()
                    ctest.mkdir()
                    crate_files = [p for p in rs_files if crate_of(p) == crate]
                    for idx, path in enumerate(crate_files):
                        text = path.read_text(encoding="utf-8", errors="replace")
                        if is_test_dir_file(path, external_test_files):
                            eff_text, test_text = "", text
                        else:
                            eff_text, test_text = split_inline_test_blocks(text)
                        (ceff / f"{idx}_{path.name}").write_text(eff_text, encoding="utf-8")
                        (ctest / f"{idx}_{path.name}").write_text(test_text, encoding="utf-8")
                    per_crate_eff[crate] = run_cloc(ceff)
                    per_crate_test[crate] = run_cloc(ctest)

    print(f"Effective (non-test) Rust LOC: {total_eff:>8}")
    print(f"Test Rust LOC:                 {total_test:>8}")
    print(f"Total Rust LOC:                {total_eff + total_test:>8}")

    if args.by_crate:
        print()
        print(f"{'crate':<20} {'effective':>10} {'test':>10} {'total':>10}")
        for crate in sorted(per_crate_eff):
            e = per_crate_eff[crate]
            t = per_crate_test[crate]
            print(f"{crate:<20} {e:>10} {t:>10} {e + t:>10}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
