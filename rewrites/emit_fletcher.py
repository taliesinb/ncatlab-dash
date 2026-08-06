#!/usr/bin/env python3
"""Stage 3: turn parsed cell grids into typst + fletcher diagram code.

One general rule covers every shape (squares, triangles, ladders, grids):
an arrow cell connects the nearest object cells along its axis — left/right
for `h`, up/down for `v`, and the diagonal neighbours for `d`. Object cells
become fletcher nodes at their (col, row) grid position (compressed so
arrow-only rows/columns don't leave gaps). Cell TeX is rendered through
mitex, so no TeX -> typst translation happens here at all.

Writes `typst` rows: hash, class (recomputed from the parsed grid), status
(ok | dangling | empty), and the complete .typ source.
"""

import argparse
import json
import re

import common

FLETCHER = "@preview/fletcher:0.5.8"
MITEX = "@preview/mitex:0.2.6"

# itex-isms mitex doesn't know; laps are spacing hacks with no meaning in
# a diagram node, so they reduce to their argument (or nothing).
TEX_FIXUPS = [
    (re.compile(r"\\math[lr]lap\s*"), ""),
    (re.compile(r"\\r?lap\s*"), ""),
    (re.compile(r"\\phantom\s*\{[^{}]*\}"), ""),
    (re.compile(r"\\hspace\s*\{[^{}]*\}"), r"\\;"),
    (re.compile(r"\\mathrm\b"), r"\\operatorname"),
    (re.compile(r"\s+"), " "),
]


def fix_tex(tex: str) -> str:
    for pat, rep in TEX_FIXUPS:
        tex = pat.sub(rep, tex)
    return tex.strip()

PREAMBLE = f"""#import "{FLETCHER}": diagram, node, edge
#import "{MITEX}": mi
#set page(width: auto, height: auto, margin: 4pt, fill: white)
#set text(size: 11pt)
"""

MARKS = {"r": "->", "l": "->", "lr": "<->", "u": "->", "d": "->",
         "se": "->", "sw": "->", "ne": "->", "nw": "->", "~": "-"}
TILDE_LABEL = {"simeq": r"\simeq", "cong": r"\cong", "equiv": r"\equiv",
               "=": "="}
HOOK = {"hookrightarrow": "hook->", "hookleftarrow": "hook->",
        "twoheadrightarrow": "->>", "twoheadleftarrow": "->>",
        "mapsto": "|->", "longmapsto": "|->",
        "Rightarrow": "=>", "Leftarrow": "=>"}

STEPS = {"se": (1, 1), "sw": (1, -1), "ne": (-1, 1), "nw": (-1, -1)}


def ts(tex: str) -> str:
    """A TeX fragment as a typst string literal for mi()."""
    tex = fix_tex(tex)
    return '"' + tex.replace("\\", "\\\\").replace('"', '\\"') + '"'


def nearest_object(grid, r, c, dr, dc):
    r, c = r + dr, c + dc
    while 0 <= r < len(grid) and 0 <= c < len(grid[r]):
        if grid[r][c]["k"] == "o":
            return r, c
        r, c = r + dr, c + dc
    return None


def emit(grid) -> tuple[str, str | None]:
    objects = {(r, c): cell for r, row in enumerate(grid)
               for c, cell in enumerate(row) if cell["k"] == "o"}
    if not objects:
        return "empty", None
    # Compress away rows/columns that hold no objects.
    rmap = {r: i for i, r in enumerate(sorted({r for r, _ in objects}))}
    cmap = {c: i for i, c in enumerate(sorted({c for _, c in objects}))}

    def coord(rc):
        return f"({cmap[rc[1]]}, {rmap[rc[0]]})"

    lines = [f"  node({coord(rc)}, mi({ts(cell['tex'])})),"
             for rc, cell in sorted(objects.items())]

    for r, row in enumerate(grid):
        for c, cell in enumerate(row):
            k = cell["k"]
            if k not in ("h", "v", "d"):
                continue
            if k == "h":
                a = nearest_object(grid, r, c, 0, -1)
                b = nearest_object(grid, r, c, 0, 1)
                if cell["dir"] == "l":
                    a, b = b, a
            elif k == "v":
                a = nearest_object(grid, r, c, -1, 0)
                b = nearest_object(grid, r, c, 1, 0)
                if cell["dir"] == "u":
                    a, b = b, a
            else:
                dr, dc = STEPS[cell["dir"]]
                a = nearest_object(grid, r, c, -dr, -dc)
                b = nearest_object(grid, r, c, dr, dc)
            if a is None or b is None:
                return "dangling", None

            mark = HOOK.get(cell.get("cmd", ""), MARKS[cell["dir"]])
            args = [coord(a), coord(b), f'"{mark}"']
            label = cell.get("above") or cell.get("below")
            if cell["dir"] == "~":
                label = TILDE_LABEL.get(cell.get("cmd", "="), "=")
            if label:
                args.append(f"label: mi({ts(label)})")
                if cell.get("below"):
                    args.append("label-side: right")
            lines.append(f"  edge({', '.join(args)}),")

    code = PREAMBLE + "#diagram(\n" + "\n".join(lines) + "\n)\n"
    return "ok", code


def classify(grid) -> str:
    kinds = ["".join(c["k"] if c["k"] != "e" else " " for c in row)
             for row in grid]
    flat = "".join(kinds)
    rows, cols = len(grid), max(len(r) for r in grid)
    if rows == 1:
        return "row"
    if cols == 1:
        return "column"
    if "d" in flat:
        return "triangle" if flat.count("o") == 3 else "diagonal-grid"
    n_obj_rows = sum(1 for k in kinds if "o" in k)
    if n_obj_rows == 2 and flat.count("o") == 4:
        return "square"
    return "ladder" if cols <= 3 else "grid"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()

    con = common.connect()
    common.stage_table(con, "typst", "class TEXT, status TEXT, code TEXT")

    n = 0
    for row in common.pending(
            con, "SELECT hash, grid FROM parsed WHERE status='ok'",
            "typst", args.force):
        grid = json.loads(row["grid"])
        status, code = emit(grid)
        con.execute(
            "INSERT OR REPLACE INTO typst(hash, class, status, code)"
            " VALUES (?,?,?,?)",
            (row["hash"], classify(grid), status, code))
        n += 1
    con.commit()
    print(f"emitted {n} new; totals by status / class:")
    common.report(con, "typst", "status")
    common.report(con, "typst", "class")
    con.close()


if __name__ == "__main__":
    main()
