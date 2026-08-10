#!/usr/bin/env python3
"""Visual review sheet for batch-converted prose pages.

Converts a sample of pages with `nlab-typ page`, compiles each to PDF
and PNGs (first N pages), and writes out/review-pages.html: a numbered
contact sheet with links to the full PDFs (out/pages/<id>.pdf).

Usage: review_pages.py [--sample N] [--seed S] [--ids 395,17488,...]
"""

import argparse
import pathlib
import random
import subprocess
import concurrent.futures

HERE = pathlib.Path(__file__).resolve().parent
BUILD = HERE.parent / "build/nlab-content/pages"
BIN = HERE.parent / "crates/nlab-typ/target/release/nlab-typ"
OUTDIR = HERE / "out/pages"
OUT = HERE / "out/review-pages.html"
PREVIEW_PAGES = 3


def convert(md: pathlib.Path):
    pid = md.parent.name
    name = (md.parent / "name").read_text().strip() if (md.parent / "name").exists() else pid
    try:
        r = subprocess.run(
            [str(BIN), "page", name],
            input=md.read_text(errors="replace"),
            capture_output=True,
            text=True,
            timeout=120,
        )
    except subprocess.TimeoutExpired:
        return (pid, name, "convert-timeout")
    if r.returncode != 0:
        return (pid, name, "convert-fail")
    typ = OUTDIR / f"{pid}.typ"
    typ.write_text(r.stdout)
    c = subprocess.run(
        ["typst", "compile", "--format", "pdf", str(typ), str(typ.with_suffix(".pdf"))],
        capture_output=True,
        text=True,
        timeout=180,
    )
    if c.returncode != 0:
        return (pid, name, "compile-fail: " + c.stderr.splitlines()[0][:120])
    subprocess.run(
        ["typst", "compile", "--format", "png", "--ppi", "96",
         "--pages", f"1-{PREVIEW_PAGES}", str(typ),
         str(OUTDIR / f"{pid}-{{n}}.png")],
        capture_output=True,
        timeout=180,
    )
    return (pid, name, "ok")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=24)
    ap.add_argument("--seed", type=int, default=5)
    ap.add_argument("--ids", help="comma-separated page ids")
    args = ap.parse_args()

    pages = sorted(BUILD.rglob("content.md"))
    if args.ids:
        want = set(args.ids.split(","))
        picked = [p for p in pages if p.parent.name in want]
    else:
        # bias toward substantial pages: weight by size
        rng = random.Random(args.seed)
        big = [p for p in pages if p.stat().st_size > 4000]
        picked = rng.sample(big, min(args.sample, len(big)))

    OUTDIR.mkdir(parents=True, exist_ok=True)
    with concurrent.futures.ThreadPoolExecutor(6) as ex:
        results = list(ex.map(convert, picked))

    rows = []
    for pid, name, status in results:
        imgs = "".join(
            f'<img src="pages/{pid}-{i}.png">'
            for i in range(1, PREVIEW_PAGES + 1)
            if (OUTDIR / f"{pid}-{i}.png").exists()
        )
        body = imgs if status == "ok" else f"<em>{status}</em>"
        rows.append(
            f'<tr id="p{pid}"><td class="n">{pid}</td>'
            f'<td class="page"><a href="pages/{pid}.pdf">{name}</a></td>'
            f'<td class="sheet">{body}</td></tr>'
        )

    n_ok = sum(1 for r in results if r[2] == "ok")
    OUT.write_text(
        "<!doctype html><meta charset=utf-8><title>page review</title>"
        "<style>body{font:14px sans-serif}table{border-collapse:collapse}"
        "td{border:1px solid #ccc;padding:6px;vertical-align:top}"
        "td.n{font-weight:bold;color:#c00}td.page{max-width:10em}"
        ".sheet img{width:24em;border:1px solid #eee;margin-right:4px;"
        "vertical-align:top}</style>"
        f"<h2>converted pages: {n_ok}/{len(results)} ok"
        " &mdash; first pages shown, click title for full PDF</h2>"
        "<table>" + "\n".join(rows) + "</table>"
    )
    print(f"{OUT}: {len(results)} pages, {n_ok} ok")


if __name__ == "__main__":
    main()
