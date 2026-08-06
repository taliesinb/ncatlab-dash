#!/usr/bin/env python3
"""Bulk-extract MathML <mtable> commutative-diagram markup from the nLab
HTML mirror into a SQLite database, with provenance and a shape
classification.

Usage: extract_mtables.py [--html DIR] [--db FILE]

Background: old nLab pages draw commutative diagrams as itex `\\array{...}`
blocks, which the wiki compiles into MathML <mtable>s full of arrow
characters — these render poorly everywhere. Newer pages use tikzcd, which
the wiki compiles server-side to clean inline SVG. The long-term plan is to
recognize the common mtable shapes (squares, triangles, ladders, composite
rows) and re-render them as real diagrams (e.g. via typst + fletcher); this
script does the extraction and triage.

Each extracted diagram row records the page it came from (id + name), its
sequence number among that page's diagrams, its byte offset in the mirror's
content.html, grid metrics, the arrow characters used, a shape class, and
the raw <math> markup.

Classes:
  square    3x3 grid: objects in the corners, horizontal arrows between
            top/bottom pairs, vertical arrows between left/right pairs
  triangle  2x3 or 3x3 grid with three objects and a diagonal arrow
  row       single-row composite (A -> B -> C)
  column    single-column tower
  ladder    Nx3 stack of squares (alternating object/arrow rows)
  grid      other rectangular arrangements with row+column arrows
  other     anything else
"""

import argparse
import re
import sqlite3
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

MATHML_NS = "{http://www.w3.org/1998/Math/MathML}"

H_ARROWS = set("→⟶←⟵↠↪↣↦⇀⇌⇒⇐⇔⟹⟸⟺⤳⇢⇠↔⟷")
V_ARROWS = set("↓↑⇓⇑↡↟")
D_ARROWS = set("↘↙↗↖⇘⇙⇗⇖")
ALL_ARROWS = H_ARROWS | V_ARROWS | D_ARROWS

MATH_RE = re.compile(r'<math [^>]*display="block"[^>]*>.*?</math>', re.S)


def cell_kind(text: str) -> str:
    chars = set(text)
    if chars & D_ARROWS:
        return "d"
    if chars & V_ARROWS:
        return "v"
    if chars & H_ARROWS:
        # An object mentioning an arrow inside (e.g. a hom-set [A -> B])
        # still counts as an arrow cell only if it is mostly arrow-ish;
        # heuristically: short content.
        return "h" if len(text.strip()) <= 12 else "o"
    if not text.strip():
        return " "
    return "o"


def grid_of(math_xml: str):
    """Return the diagram's cell-kind grid, or None if unparseable."""
    try:
        root = ET.fromstring(math_xml)
    except ET.ParseError:
        return None
    mtable = root.find(f".//{MATHML_NS}mtable")
    if mtable is None:
        return None
    grid = []
    for mtr in mtable.findall(f"{MATHML_NS}mtr"):
        row = []
        for mtd in mtr.findall(f"{MATHML_NS}mtd"):
            row.append(cell_kind("".join(mtd.itertext())))
        grid.append(row)
    width = max((len(r) for r in grid), default=0)
    return [r + [" "] * (width - len(r)) for r in grid]


def classify(grid) -> str:
    if not grid:
        return "other"
    rows, cols = len(grid), len(grid[0])
    flat = "".join("".join(r) for r in grid)
    if rows == 1:
        return "row" if "h" in flat else "other"
    if cols == 1:
        return "column" if "v" in flat else "other"
    if "d" in flat:
        if rows <= 3 and flat.count("o") == 3:
            return "triangle"
        return "other" if rows <= 2 else "grid"
    # Object rows alternate with vertical-arrow rows?
    kinds = ["v" if "v" in "".join(r) and "o" not in "".join(r) else "o"
             for r in grid]
    if all(k == ("o" if i % 2 == 0 else "v") for i, k in enumerate(kinds)):
        n_obj_rows = (rows + 1) // 2
        if n_obj_rows == 2 and cols == 3:
            return "square"
        if cols == 3:
            return "ladder"
        return "grid"
    return "other"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--html", type=Path,
                    default=Path(__file__).resolve().parent.parent
                    / "build" / "nlab-content-html")
    ap.add_argument("--db", type=Path,
                    default=Path(__file__).resolve().parent / "mtables.db")
    args = ap.parse_args()
    if not (args.html / "pages").is_dir():
        sys.exit(f"error: {args.html} is not a nlab-content-html checkout")

    args.db.unlink(missing_ok=True)
    con = sqlite3.connect(args.db)
    con.execute("""
        CREATE TABLE mtables(
            id INTEGER PRIMARY KEY,
            page_id INTEGER,      -- nLab page id (pages/<id>.html in docset)
            page_name TEXT,
            seq INTEGER,          -- 0-based diagram index within the page
            offset INTEGER,       -- byte offset in mirror content.html
            rows INTEGER,
            cols INTEGER,
            class TEXT,
            arrows TEXT,          -- distinct arrow characters used
            grid TEXT,            -- cell-kind rows joined by '/'
            mathml TEXT           -- raw <math> markup
        )""")

    n_pages = n_diagrams = 0
    for name_file in sorted((args.html / "pages").glob("*/*/*/*/*/name")):
        page_dir = name_file.parent
        html = page_dir / "content.html"
        if not html.is_file():
            continue
        text = html.read_text(encoding="utf-8", errors="surrogateescape")
        seq = 0
        page_name = None
        for m in MATH_RE.finditer(text):
            xml = m.group(0)
            if "<mtable" not in xml or not (set(xml) & ALL_ARROWS):
                continue
            if page_name is None:
                page_name = name_file.read_text(encoding="utf-8").strip()
            grid = grid_of(xml)
            cls = classify(grid)
            con.execute(
                "INSERT INTO mtables(page_id, page_name, seq, offset, rows,"
                " cols, class, arrows, grid, mathml)"
                " VALUES (?,?,?,?,?,?,?,?,?,?)",
                (int(page_dir.name), page_name, seq, m.start(),
                 len(grid) if grid else 0,
                 len(grid[0]) if grid else 0, cls,
                 "".join(sorted(set(xml) & ALL_ARROWS)),
                 "/".join("".join(r) for r in grid) if grid else None, xml))
            seq += 1
            n_diagrams += 1
        if seq:
            n_pages += 1
    con.commit()

    print(f"{n_diagrams} mtable diagrams from {n_pages} pages -> {args.db}")
    for cls, n in con.execute(
            "SELECT class, count(*) FROM mtables GROUP BY class"
            " ORDER BY 2 DESC"):
        print(f"  {cls:10} {n}")
    con.close()


if __name__ == "__main__":
    main()
