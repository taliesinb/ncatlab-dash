#!/usr/bin/env python3
"""Hand-verification gallery for the tikzcd -> fletcher converter.

Pairs each tikzcd block in the content mirror with its server-rendered
SVG from the HTML mirror (nth non-logo <svg> in the page = nth tikzcd
block) and with our fletcher render, and writes a numbered side-by-side
gallery to out/compare-tikzcd.html.

Rows are numbered by a stable corpus index: pages sorted by path, blocks
in page order. Use --sample N (seeded) for a random subset, --rows for
specific indices, --all for everything.
"""

import argparse
import hashlib
import pathlib
import re
import subprocess
import concurrent.futures

HERE = pathlib.Path(__file__).resolve().parent
BUILD = HERE.parent / "build"
BINARY = HERE.parent / "crates/nlab-typ/target/release/nlab-typ"
CACHE = HERE / "out/tikzcache"
OUT = HERE / "out/compare-tikzcd.html"

PRE = (
    '#import "@preview/fletcher:0.5.8": diagram, node, edge\n'
    '#import "@local/mitex:0.2.7": mi-itex, mitex-itex\n'
    "#set page(width: auto, height: auto, margin: 10pt, fill: white)\n"
    "#set text(size: 11pt)\n"
)

TIKZ_RE = re.compile(r"\\begin\{tikzcd\}.*?\\end\{tikzcd\}", re.S)
SVG_RE = re.compile(r"<svg[^>]*>.*?</svg>", re.S)


def corpus():
    """(index, page-path, tikzcd source) for every block, stable order."""
    blocks = []
    pages = sorted((BUILD / "nlab-content/pages").rglob("content.md"))
    for p in pages:
        src = p.read_text(errors="replace")
        for m in TIKZ_RE.finditer(src):
            blocks.append((len(blocks), p, m.group(0)))
    return blocks


def original_svgs(md_page: pathlib.Path):
    """Server-rendered tikzcd SVGs for a page, in document order."""
    rel = md_page.relative_to(BUILD / "nlab-content")
    html = BUILD / "nlab-content-html" / rel.with_name("content.html")
    if not html.exists():
        return []
    src = html.read_text(errors="replace")
    svgs = []
    for m in SVG_RE.finditer(src):
        before = src[max(0, m.start() - 300) : m.start()]
        if "pageName" in before or "float: left" in before:
            continue  # site logo
        if "<math" in before[-40:]:
            continue  # embedded in mathml (not a tikzcd render)
        svgs.append(m.group(0))
    return svgs


def convert_all(blocks):
    payload = "\x00".join(b for _, _, b in blocks) + "\x00"
    proc = subprocess.run(
        [str(BINARY), "tikzcds"],
        input=payload,
        capture_output=True,
        text=True,
        timeout=600,
    )
    outs = [o for o in proc.stdout.split("\x00") if o]
    res = []
    for o in outs:
        status, code, warns = (o.split("\x1f") + ["", ""])[:3]
        res.append((status, code, warns))
    return res


def render(code: str) -> pathlib.Path | None:
    CACHE.mkdir(parents=True, exist_ok=True)
    h = hashlib.sha1(code.encode()).hexdigest()[:16]
    svg = CACHE / f"{h}.svg"
    if svg.exists():
        return svg
    typ = CACHE / f"{h}.typ"
    typ.write_text(PRE + code.replace("mi(", "mi-itex("))
    r = subprocess.run(
        ["typst", "compile", "--format", "svg", str(typ), str(svg)],
        capture_output=True,
        text=True,
        timeout=60,
    )
    if r.returncode != 0:
        (CACHE / f"{h}.err").write_text(r.stderr)
        return None
    return svg


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=150)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--rows", help="comma-separated corpus indices")
    ap.add_argument("--all", action="store_true")
    args = ap.parse_args()

    blocks = corpus()
    converted = convert_all(blocks)
    assert len(converted) == len(blocks), (len(converted), len(blocks))

    if args.rows:
        pick = [int(x) for x in args.rows.split(",")]
    elif args.all:
        pick = list(range(len(blocks)))
    else:
        import random

        rng = random.Random(args.seed)
        pick = sorted(rng.sample(range(len(blocks)), min(args.sample, len(blocks))))

    page_svgs: dict[pathlib.Path, list[str]] = {}
    page_counts: dict[pathlib.Path, int] = {}
    for _, p, _ in blocks:
        page_counts[p] = page_counts.get(p, 0) + 1
    ordinal = {}
    seen: dict[pathlib.Path, int] = {}
    for i, p, _ in blocks:
        ordinal[i] = seen.get(p, 0)
        seen[p] = ordinal[i] + 1

    with concurrent.futures.ThreadPoolExecutor(8) as ex:
        ours = list(
            ex.map(
                lambda i: render(converted[i][1]) if converted[i][0] == "ok" else None,
                pick,
            )
        )

    rows = []
    n_ok = 0
    for i, svg in zip(pick, ours):
        idx, page, _src = blocks[i]
        name = (page.parent / "name").read_text().strip() if (page.parent / "name").exists() else str(page)
        if page not in page_svgs:
            page_svgs[page] = original_svgs(page)
        origs = page_svgs[page]
        # pair nth block with nth svg only when counts agree
        orig = (
            origs[ordinal[i]]
            if len(origs) == page_counts[page]
            else None
        )
        orig_html = orig if orig else "<em>no matched original</em>"
        if svg is not None:
            n_ok += 1
            mine = svg.read_text()
        else:
            mine = f"<em>FAILED: {converted[i][0]}</em>"
        warns = converted[i][2].replace("\x1e", ", ")
        warn_html = f'<div class="warn">ignored: {warns}</div>' if warns else ""
        rows.append(
            f'<tr id="row{idx}"><td class="n">{idx}</td>'
            f'<td class="page">{name}</td>'
            f"<td>{orig_html}</td><td>{mine}{warn_html}</td></tr>"
        )

    OUT.write_text(
        "<!doctype html><meta charset=utf-8><title>tikzcd compare</title>"
        "<style>body{font:14px sans-serif}table{border-collapse:collapse}"
        "td{border:1px solid #ccc;padding:6px;vertical-align:middle}"
        "td.n{font-weight:bold;color:#c00}td.page{max-width:9em;font-size:11px}"
        ".warn{color:#a60;font-size:11px;max-width:22em}"
        "svg{max-width:34em;height:auto}</style>"
        f"<h2>tikzcd &rarr; fletcher: {n_ok}/{len(pick)} rendered"
        f" (corpus {len(blocks)} blocks)</h2>"
        "<table><tr><th>#</th><th>page</th><th>nLab server render</th>"
        "<th>fletcher</th></tr>" + "\n".join(rows) + "</table>"
    )
    print(f"{OUT}: {len(pick)} rows, {n_ok} rendered ok")


if __name__ == "__main__":
    main()
