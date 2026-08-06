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
import html
import json
import re

import common
import parse_arrays

FLETCHER = "@preview/fletcher:0.5.8"
MITEX = "@preview/mitex:0.2.6"

# itex-isms mitex doesn't know; laps are spacing hacks with no meaning in
# a diagram node, so they reduce to their argument (or nothing).
TEX_FIXUPS = [
    (re.compile(r"\\math[lr]lap\s*"), ""),
    (re.compile(r"\\r?lap\s*"), ""),
    (re.compile(r"\\phantom\s*\{[^{}]*\}"), ""),
    (re.compile(r"\\hspace\s*\{[^{}]*\}"), r"\\;"),
    # itex sets | as an ordinary symbol; mitex spaces it as a relation —
    # except a restriction bar |_c, whose subscript renders fine raw.
    (re.compile(r"(?<![\\{])\|(?!_)"), r"{\\mid}"),
    (re.compile(r"\\mathscr\b"), r"\\mathcal"),  # mitex lacks \mathscr
    # typst math italicizes sans; \textsf gives the upright sans words
    # the nLab uses for category names.
    (re.compile(r"\\mathsf\b"), r"\\textsf"),
    (re.compile(r"\s+"), " "),
]

# itex groups a run of letters into one upright identifier (Set, Tmf, op,
# coim); LaTeX/mitex would typeset them as a product of italic variables.
# Arguments of text-like commands are already words and must not be
# rewrapped (\text{\mathrm{field}} renders literally).
WORD_RE = re.compile(r"(\\[a-zA-Z]+)|([a-zA-Z]{2,})")
PROTECTED_RE = re.compile(
    r"\\(?:text|textsf|textrm|texttt|mathrm|mathit|mathbf|mathsf|mathtt"
    r"|operatorname|mathcal|mathfrak|mathscr|mathbb|begin|end)\s*\{[^{}]*\}")


# mitex 0.2.6 emits symbol modifiers typst 0.15 removed for the circled
# operators; the raw unicode characters pass through fine.
CIRCLED = {"bigotimes": "⨂", "bigoplus": "⨁", "bigodot": "⨀",
           "otimes": "⊗", "oplus": "⊕", "ominus": "⊖", "odot": "⊙",
           "oslash": "⊘", "circledast": "⊛", "circledcirc": "⊚",
           # mitex emits typst's pre-0.13 names (sect) for these
           "cap": "∩", "cup": "∪", "bigcap": "⋂", "bigcup": "⋃",
           "setminus": "∖"}
CIRCLED_RE = re.compile(r"\\(%s)\b" % "|".join(CIRCLED))


STACK_LONG = {"to": "longrightarrow", "rightarrow": "longrightarrow",
              "leftarrow": "longleftarrow", "mapsto": "longmapsto"}


def translate_stacked_pairs(tex: str) -> str:
    """\\stackrel{\\stackrel{A}{ar1}}{\\stackrel{B}{ar2}} — the adjoint-pair
    notation — becomes the classic display: A over a long ar1, stacked on
    ar2 with B under it."""
    while True:
        m = re.search(r"\\stackrel\s*\{\s*\\stackrel\s*", tex)
        if not m:
            return tex
        a, rest = parse_arrays.read_group(tex[m.end():])
        ar1, rest = parse_arrays.read_group(rest)
        rest = rest.lstrip()
        if not rest.startswith("}"):
            return tex
        inner, rest2 = parse_arrays.read_group(rest[1:])
        m2 = re.match(r"\\stackrel\s*", inner)
        if not m2:
            return tex
        b, r = parse_arrays.read_group(inner[m2.end():])
        ar2, r = parse_arrays.read_group(r)
        if r.strip():
            return tex

        def longen(ar):
            cmd = ar.strip().lstrip("\\")
            return "\\" + STACK_LONG.get(cmd, cmd)

        tex = (tex[:m.start()]
               + "\\underset{\\underset{%s}{%s}}{\\overset{%s}{%s}}"
               % (b, longen(ar2), a, longen(ar1)) + rest2)


def translate_underoverset(tex: str) -> str:
    """itex \\underoverset{below}{above}{base} -> nested over/underset."""
    while "\\underoverset" in tex:
        i = tex.find("\\underoverset")
        below, rest = parse_arrays.read_group(tex[i + len("\\underoverset"):])
        above, rest = parse_arrays.read_group(rest)
        base, rest = parse_arrays.read_group(rest)
        tex = (tex[:i] + "\\overset{%s}{\\underset{%s}{%s}}"
               % (above, below, base) + rest)
    return tex


def fix_tex(tex: str) -> str:
    tex = translate_stacked_pairs(tex)
    tex = translate_underoverset(tex)
    tex = CIRCLED_RE.sub(lambda m: CIRCLED[m.group(1)], tex)
    for pat, rep in TEX_FIXUPS:
        tex = pat.sub(rep, tex)
    saved: list[str] = []

    def stash(m):
        saved.append(m.group(0))
        return f"\x00{len(saved) - 1}\x00"

    tex = PROTECTED_RE.sub(stash, tex)
    tex = WORD_RE.sub(
        lambda m: m.group(1) or r"\mathrm{%s}" % m.group(2), tex)
    tex = re.sub(r"\x00(\d+)\x00", lambda m: saved[int(m.group(1))], tex)
    return tex.strip()

PREAMBLE = f"""#import "{FLETCHER}": diagram, node, edge
#import "{MITEX}": mi, mitex
#set page(width: auto, height: auto, margin: 4pt, fill: white)
#set text(size: 11pt)
"""

MARKS = {"r": "->", "l": "->", "lr": "<->", "u": "->", "d": "->",
         "se": "->", "sw": "->", "ne": "->", "nw": "->", "~": "-",
         "veq": "="}
TILDE_LABEL = {"simeq": r"\simeq", "cong": r"\cong", "equiv": r"\equiv",
               "=": "="}
HOOK = {"hookrightarrow": "hook->", "hookleftarrow": "hook->",
        "twoheadrightarrow": "->>", "twoheadleftarrow": "->>",
        "mapsto": "|->", "longmapsto": "|->",
        "Rightarrow": "=>", "Leftarrow": "=>",
        "seArrow": "=>", "swArrow": "=>", "neArrow": "=>", "nwArrow": "=>"}

STEPS = {"se": (1, 1), "sw": (1, -1), "ne": (-1, 1), "nw": (-1, -1)}

# Travel vector (dx, dy; screen coords, y down) per arrow direction.
TRAVEL = {"r": (1, 0), "l": (-1, 0), "lr": (1, 0), "~": (1, 0),
          "d": (0, 1), "u": (0, -1), "veq": (0, 1),
          "se": (1, 1), "sw": (-1, 1), "ne": (1, -1), "nw": (-1, -1)}
# Where the author's script puts a label, as a viewer-space vector.
WANT = {"above": (0, -1), "below": (0, 1), "east": (1, 0), "west": (-1, 0)}


def label_side(direction: str, placement: str) -> str:
    """fletcher's label-side (left/right relative to travel) that puts the
    label where the itex placement (above/below/east/west) intended."""
    tx, ty = TRAVEL[direction]
    lx, ly = ty, -tx  # left of travel, screen coords
    wx, wy = WANT[placement]
    return "left" if lx * wx + ly * wy > 0 else "right"


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


def arrow_tex(cell) -> str:
    """Rebuild a horizontal arrow cell's TeX from its parsed parts."""
    base = "=" if cell["dir"] == "~" and cell.get("cmd") == "=" else \
        "\\" + cell.get("cmd", "to")
    if cell.get("above"):
        base = "\\overset{%s}{%s}" % (cell["above"], base)
    if cell.get("below"):
        base = "\\underset{%s}{%s}" % (cell["below"], base)
    return base


def signature_rows(grid):
    """Detect the function-signature pattern: two or more rows, each
    exactly `LHS & horizontal-arrow & RHS` (f : A -> B over x |-> f(x)).
    With no vertical structure there is nothing to commute — it is an
    aligned table, not a diagram."""
    rows = []
    for row in grid:
        filled = [cell for cell in row if cell["k"] != "e"]
        if (len(filled) == 3 and filled[0]["k"] == "o"
                and filled[1]["k"] == "h" and filled[2]["k"] == "o"):
            rows.append(filled)
        else:
            return None
    return rows if len(rows) >= 2 else None


# typst arrow symbol per arrow command; stretch() makes every row's arrow
# exactly the same length, so \to and \mapsto line up.
SIG_SYMS = {"mapsto": "arrow.r.bar", "longmapsto": "arrow.r.bar",
            "hookrightarrow": "arrow.r.hook",
            "twoheadrightarrow": "arrow.r.twohead",
            "Rightarrow": "arrow.r.double",
            "leftarrow": "arrow.l", "longleftarrow": "arrow.l",
            "leftrightarrow": "arrow.l.r"}


def sig_arrow(cell) -> str:
    if cell["dir"] == "~":
        return f"mi({ts(arrow_tex(cell))})"
    sym = SIG_SYMS.get(cell.get("cmd", ""), "arrow.r")
    core = f"stretch({sym}, size: #2.4em)"
    attach = []
    if cell.get("above"):
        attach.append(f"t: #text(0.72em, mi({ts(cell['above'])}))")
    if cell.get("below"):
        attach.append(f"b: #text(0.72em, mi({ts(cell['below'])}))")
    if attach:
        return f"$attach({core}, {', '.join(attach)})$"
    return f"${core}$"


def emit_signature(rows) -> str:
    cells = []
    for lhs, arrow, rhs in rows:
        cells.append(f"  mi({ts('\\displaystyle ' + lhs['tex'])}),")
        cells.append(f"  {sig_arrow(arrow)},")
        cells.append(f"  mi({ts('\\displaystyle ' + rhs['tex'])}),")
    return ("#grid(\n  columns: 3, column-gutter: 0.4em,"
            " row-gutter: 1.1em,\n"
            "  align: (right + horizon, center + horizon, left + horizon),\n"
            + "\n".join(cells) + "\n)\n")


def emit_table(grid) -> str:
    """An arrow-free \\array as a centered typst grid (itex's default
    column alignment), e.g. tables of equations or data."""
    cols = max(len(r) for r in grid)
    cells = []
    for row in grid:
        padded = row + [{"k": "e"}] * (cols - len(row))
        for cell in padded:
            if cell["k"] == "o":
                cells.append(
                    f"  mi({ts('\\displaystyle ' + cell['tex'])}),")
            else:
                cells.append("  [],")
    return (f"#grid(\n  columns: {cols}, column-gutter: 1.4em,"
            " row-gutter: 1em,\n  align: center + horizon,\n"
            + "\n".join(cells) + "\n)\n")


def parse_wrapped_grid(body: str):
    """A wrapped array's body as a cell grid, or None if not cleanly
    parseable as a diagram."""
    grid = [[parse_arrays.classify_cell(c)
             for c in parse_arrays.split_depth0(row, ("&",))]
            for row in parse_arrays.split_depth0(body, ("\\\\",))]
    grid = [r for r in grid if any(c["k"] != "e" for c in r)]
    if not grid:
        return None
    parse_arrays.absorb_spills(grid)
    parse_arrays.merge_annotations(grid)
    if any(c["k"] == "?" for r in grid for c in r):
        return None
    return grid


def emit_equation(tex: str) -> str:
    """Fallback for diagrams that are really formulas: \\begin{aligned}
    derivations and equations with embedded arrays. Rendered whole via
    mitex, with itex's \\array translated to a matrix environment."""
    while True:
        found = parse_arrays.find_array(tex)
        if not found:
            break
        start, end, body = found
        tex = (tex[:start] + "\\begin{matrix}" + body + "\\end{matrix}"
               + tex[end:])
    return f"#mitex({ts(tex)})\n"


def emit(grid) -> tuple[str, str | None]:
    sig = signature_rows(grid)
    if sig:
        return "ok", emit_signature(sig)
    objects = {(r, c): cell for r, row in enumerate(grid)
               for c, cell in enumerate(row) if cell["k"] == "o"}
    if not objects:
        return "empty", None

    def quadrant_object(r, c, dr, dc):
        """Nearest object in the quadrant the vector (dr, dc) points into
        (Chebyshev distance), for long-range diagonals whose slope doesn't
        match the grid."""
        best = None
        for (rr, cc) in objects:
            if dr * (rr - r) < 0 or dc * (cc - c) < 0 or (rr, cc) == (r, c):
                continue
            if rr == r and cc == c:
                continue
            d = max(abs(rr - r), abs(cc - c))
            if best is None or d < best[0]:
                best = (d, (rr, cc))
        return best[1] if best else None

    def resolve_diagonal(r, c, direction):
        """Endpoints of a diagonal arrow; authors often place the cell
        under its source (fall back straight up/down), or run it long-range
        across the grid (fall back to nearest object in the quadrant)."""
        dr, dc = STEPS[direction]
        a = (nearest_object(grid, r, c, -dr, -dc)
             or nearest_object(grid, r, c, -1, 0)
             or quadrant_object(r, c, -dr, -dc))
        b = (nearest_object(grid, r, c, dr, dc)
             or nearest_object(grid, r, c, 1, 0)
             or quadrant_object(r, c, dr, dc))
        return a, b

    # Resolve every arrow to its endpoint objects first.
    edges = []
    for r, row in enumerate(grid):
        for c, cell in enumerate(row):
            k = cell["k"]
            if k == "dd":
                for part in cell["parts"]:
                    a, b = resolve_diagonal(r, c, part["dir"])
                    if a is None or b is None:
                        return "dangling", None
                    edges.append((a, b, part))
                continue
            if k not in ("h", "v", "d"):
                continue
            if k == "h":
                a = nearest_object(grid, r, c, 0, -1)
                b = nearest_object(grid, r, c, 0, 1)
                if cell["dir"] == "l":
                    a, b = b, a
                if cell["dir"] == "~" and (a is None or b is None):
                    # An equals used vertically between object rows.
                    a = nearest_object(grid, r, c, -1, 0)
                    b = nearest_object(grid, r, c, 1, 0)
            elif k == "v":
                # Rows of different widths can leave a vertical arrow's
                # endpoint one column off; fall back diagonally.
                a = (nearest_object(grid, r, c, -1, 0)
                     or nearest_object(grid, r, c, -1, -1)
                     or nearest_object(grid, r, c, -1, 1))
                b = (nearest_object(grid, r, c, 1, 0)
                     or nearest_object(grid, r, c, 1, -1)
                     or nearest_object(grid, r, c, 1, 1))
                if cell["dir"] == "u":
                    a, b = b, a
            else:
                a, b = resolve_diagonal(r, c, cell["dir"])
            if a is None or b is None:
                if cell.get("cmd") in ("Downarrow", "Uparrow"):
                    continue  # a 2-cell decoration between arrows; drop it
                return "dangling", None
            edges.append((a, b, cell))

    if not edges:
        # No arrows at all (often swept in by \vec's combining-arrow
        # glyph): an ordinary table of equations, emitted as an aligned
        # grid rather than a diagram.
        return "ok", emit_table(grid)

    # An object no arrow touches, sitting right next to one that is an
    # endpoint, is an annotation ("c \in" before "[X, A_s]"): merge it.
    endpoints = {rc for a, b, _ in edges for rc in (a, b)}
    for rc in sorted(set(objects) - endpoints):
        r, c = rc
        for dc in (1, -1):
            nb = (r, c + dc)
            if nb in objects and nb in endpoints:
                tex, other = objects[rc]["tex"], objects[nb]["tex"]
                objects[nb]["tex"] = (f"{tex} {other}" if dc == 1
                                      else f"{other} {tex}")
                del objects[rc]
                break

    # Compress away rows/columns that hold no objects.
    rmap = {r: i for i, r in enumerate(sorted({r for r, _ in objects}))}
    cmap = {c: i for i, c in enumerate(sorted({c for _, c in objects}))}

    def coord(rc):
        return f"({cmap[rc[1]]}, {rmap[rc[0]]})"

    # Objects are display-style (limits under sums/products); edge labels
    # stay inline and small.
    lines = []
    for rc, cell in sorted(objects.items()):
        label = ts("\\displaystyle " + cell["tex"])
        lines.append(f"  node({coord(rc)}, mi({label})),")

    max_x = max(cmap.values())
    for a, b, cell in edges:
        if cell.get("pair"):
            # Two stacked arrows (\stackrel{arrow}{arrow}), e.g. an
            # adjunction pair: draw both, shifted apart vertically.
            left, right = (b, a) if cell["dir"] == "l" else (a, b)
            for i, sub in enumerate(cell["pair"]):
                aa, bb = ((left, right) if sub["dir"] in ("r", "lr", "~")
                          else (right, left))
                mark = HOOK.get(sub.get("cmd", ""), MARKS[sub["dir"]])
                args = [coord(aa), coord(bb), f'"{mark}"']
                placement = next(
                    (p for p in ("above", "below") if sub.get(p)), None)
                if placement:
                    args.append(
                        f"label: text(0.75em, mi({ts(sub[placement])}))")
                    args.append(
                        f"label-side: {label_side(sub['dir'], placement)}")
                up = 2.5 if sub["dir"] != "l" else -2.5
                args.append(f"shift: {up if i == 0 else -up}pt")
                lines.append(f"  edge({', '.join(args)}),")
            continue
        mark = HOOK.get(cell.get("cmd", ""), MARKS[cell["dir"]])
        args = [coord(a), coord(b), f'"{mark}"']
        placement = next((p for p in ("above", "below", "east", "west")
                          if cell.get(p)), None)
        # Vertical arrows on the diagram's flanks read best with the label
        # pushed outward, whatever side the author's script habit chose.
        if (cell["k"] == "v" and placement in ("east", "west")
                and not (cell.get("east") and cell.get("west"))):
            x = cmap[a[1]]
            outward = ("west" if x * 2 < max_x else
                       "east" if x * 2 > max_x else placement)
            if outward != placement:
                cell[outward] = cell.pop(placement)
                placement = outward
        second = None
        if cell["dir"] == "~":
            if cell.get("cmd") == "=":
                args[2] = '"="'  # double-line equality edge
                if placement:
                    args.append(
                        f"label: text(0.75em, mi({ts(cell[placement])}))")
                    args.append(
                        f"label-side: {label_side('~', placement)}")
            else:
                sym = TILDE_LABEL.get(cell.get("cmd", ""), "=")
                args.append(f"label: text(0.75em, mi({ts(sym)}))")
        elif placement:
            args.append(f"label: text(0.75em, mi({ts(cell[placement])}))")
            args.append(f"label-side: {label_side(cell['dir'], placement)}")
            second = next((p for p in ("above", "below", "east", "west")
                           if p != placement and cell.get(p)), None)
        if cell.get("cmd") in ("rightrightarrows", "leftleftarrows"):
            lines.append(f"  edge({', '.join(args)}, shift: 2pt),")
            args = [a for a in args if not a.startswith("label")]
            lines.append(f"  edge({', '.join(args)}, shift: -2pt),")
            continue
        lines.append(f"  edge({', '.join(args)}),")
        if second:
            # An arrow labelled on both sides ({}^{p_1}\downarrow^{\in F}):
            # fletcher edges carry one label, so a stroke-less ghost edge
            # carries the other.
            lines.append(
                f"  edge({coord(a)}, {coord(b)}, \"-\", stroke: none,"
                f" label: text(0.75em, mi({ts(cell[second])})),"
                f" label-side: {label_side(cell['dir'], second)}),")

    return "ok", ("#diagram(\n  spacing: (2.6em, 2.2em),\n"
                  + "\n".join(lines) + "\n)\n")


def classify(grid) -> str:
    if signature_rows(grid):
        return "signature"
    if not any(c["k"] in ("h", "v", "d", "dd") for row in grid for c in row):
        return "table"
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
    if args.force:
        con.execute("DELETE FROM typst")

    n = 0
    for row in common.pending(
            con, "SELECT hash, grid FROM parsed WHERE status='ok'",
            "typst", args.force):
        grid = json.loads(row["grid"])
        status, body = emit(grid)
        con.execute(
            "INSERT OR REPLACE INTO typst(hash, class, status, code)"
            " VALUES (?,?,?,?)",
            (row["hash"], classify(grid), status,
             PREAMBLE + body if body else None))
        n += 1

    # Formulas that only look like diagrams: aligned derivations
    # (no-array) and arrays embedded in larger formulas (wrapped).
    # Three cases: formulas that are JUST arrays separated by spacing
    # render as side-by-side fletcher diagrams; formulas embedding
    # genuinely 2D (diagonal) diagrams stay unconverted (matrix-izing
    # them looks worse than the original MathML); the rest render whole
    # as mitex equations with arrays as matrices.
    diag_re = re.compile(r"\\[sn][ew][aA]rrow")
    spacing_re = re.compile(r"^(\\qquad|\\quad|\\;|\\,|\\!|[\s.,])*$")
    # Inner gaps may also be a relation joining two diagrams (array = array)
    sep_re = re.compile(r"^(\\qquad|\\quad|\\;|\\,|\\!|[\s.,])*"
                        r"(=|\\simeq|\\cong)?"
                        r"(\\qquad|\\quad|\\;|\\,|\\!|[\s.,])*$")
    for row in common.pending(
            con,
            "SELECT p.hash hash, min(m.mathml) mathml FROM parsed p"
            " JOIN mtables m ON m.hash = p.hash"
            " WHERE p.status IN ('wrapped', 'no-array') GROUP BY p.hash",
            "typst", args.force):
        m = parse_arrays.ANNOTATION_RE.search(row["mathml"])
        if not m:
            continue
        tex = html.unescape(m.group(1)).strip()
        tex = re.sub(r"&#(\d+);", lambda m: chr(int(m.group(1))), tex)
        tex = re.sub(r"&#x([0-9a-fA-F]+);",
                     lambda m: chr(int(m.group(1), 16)), tex)

        spans, pos = [], 0
        while True:
            found = parse_arrays.find_array(tex[pos:])
            if not found:
                break
            start, end, body = found
            spans.append((pos + start, pos + end, body))
            pos += end

        cls, status, code = "equation", "ok", None
        if spans:
            gaps = ([tex[:spans[0][0]]]
                    + [tex[spans[i][1]:spans[i + 1][0]]
                       for i in range(len(spans) - 1)]
                    + [tex[spans[-1][1]:]])
            inner = [sep_re.match(g) for g in gaps[1:-1]]
            if (spacing_re.match(gaps[0]) and spacing_re.match(gaps[-1])
                    and all(inner)):
                grids = [parse_wrapped_grid(b) for _, _, b in spans]
                if all(g is not None for g in grids):
                    results = [emit(g) for g in grids]
                    if all(st == "ok" for st, _ in results):
                        cells = [f"  [{results[0][1].strip()}],"]
                        for m2, (_, b) in zip(inner, results[1:]):
                            sep = m2.group(2)
                            if sep:
                                cells.append(f"  mi({ts(sep)}),")
                            cells.append(f"  [{b.strip()}],")
                        code = (PREAMBLE
                                + f"#grid(columns: {len(cells)},"
                                " column-gutter: 2em, align: horizon,\n"
                                + "\n".join(cells) + "\n)\n")
                        cls = (classify(grids[0]) if len(grids) == 1
                               else "multi-diagram")
        if code is None:
            if any(diag_re.search(b) for _, _, b in spans):
                cls, status = "wrapped-diagram", "wrapped-diagram"
            else:
                code = PREAMBLE + emit_equation(tex)
        con.execute(
            "INSERT OR REPLACE INTO typst(hash, class, status, code)"
            " VALUES (?,?,?,?)", (row["hash"], cls, status, code))
        n += 1
    con.commit()
    print(f"emitted {n} new; totals by status / class:")
    common.report(con, "typst", "status")
    common.report(con, "typst", "class")
    con.close()


if __name__ == "__main__":
    main()
