#!/usr/bin/env python3
"""Stage 7: compile verified typst diagrams to SVG for the docset.

PNGs (render_typst.py) exist for gallery comparison; the docset wants
SVGs, which scale cleanly and — spliced inline — invert properly in
Dash's dark mode like the nLab's own tikzcd diagrams. Only hashes whose
PNG render succeeded are compiled. Output: out/svg/<hash>.svg, which
make_docset.py --diagrams consumes.
"""

import argparse
import subprocess
from concurrent.futures import ThreadPoolExecutor

import common


def compile_one(hash_: str, code: str) -> tuple[str, bool, str]:
    typ = common.OUT / "svg" / f"{hash_}.typ"
    svg = common.OUT / "svg" / f"{hash_}.svg"
    # Transparent background: the docset splices these inline, and Dash's
    # dark mode inverts them like the nLab's own tikzcd SVGs.
    typ.write_text(code.replace("fill: white", "fill: none"),
                   encoding="utf-8")
    proc = subprocess.run(
        ["typst", "compile", "--format", "svg", str(typ), str(svg)],
        capture_output=True, text=True, timeout=60)
    return hash_, proc.returncode == 0 and svg.is_file(), \
        proc.stderr.strip()[:200]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--jobs", type=int, default=10)
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()

    (common.OUT / "svg").mkdir(parents=True, exist_ok=True)
    con = common.connect()
    todo = []
    for r in con.execute(
            "SELECT t.hash hash, t.code code FROM typst t"
            " JOIN renders r ON r.hash = t.hash AND r.kind='typst'"
            " AND r.status='ok' WHERE t.status='ok'"):
        svg = common.OUT / "svg" / f"{r['hash']}.svg"
        if args.force or not svg.is_file():
            todo.append((r["hash"], r["code"]))
    ok = err = 0
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for hash_, success, error in pool.map(
                lambda t: compile_one(*t), todo):
            ok += success
            err += not success
    total = len(list((common.OUT / "svg").glob("*.svg")))
    print(f"svg renders: {ok} new ok, {err} errors; {total} total on disk")
    con.close()


if __name__ == "__main__":
    main()
