#!/usr/bin/env python3
"""Stage 2: parse the itex \\array{...} source of each diagram.

The mirror's MathML carries the original TeX in an <annotation> element, so
no markdown alignment is needed. For each distinct diagram hash this stage
extracts the TeX, locates the \\array{...} block, splits it into rows/cells
(respecting brace depth), and classifies every cell:

  o  object (a TeX label)
  h  horizontal arrow  (\\to and friends, optional \\stackrel/\\overset label)
  v  vertical arrow    (\\downarrow / \\uparrow, optional ^{\\mathrlap{...}})
  d  diagonal arrow    (\\searrow etc.)
  e  empty
  ?  unrecognized (blocks conversion of the whole diagram)

Result rows land in `parsed` with a JSON cell grid and a status:
ok | no-array | wrapped (array embedded in a larger formula) | cells:<n>?
"""

import argparse
import html
import json
import re

import common

ANNOTATION_RE = re.compile(
    r'<annotation encoding="application/x-tex">(.*?)</annotation>', re.S)

# arrow-command -> (kind, direction)
H_CMDS = {
    "to": "r", "rightarrow": "r", "longrightarrow": "r", "Rightarrow": "r",
    "longmapsto": "r", "mapsto": "r", "hookrightarrow": "r",
    "twoheadrightarrow": "r", "rightharpoonup": "r",
    "leftarrow": "l", "longleftarrow": "l", "Leftarrow": "l",
    "hookleftarrow": "l", "twoheadleftarrow": "l",
    "rightrightarrows": "r", "leftleftarrows": "l",
    "leftrightarrow": "lr", "simeq": "~", "cong": "~", "equiv": "~",
}
V_CMDS = {"downarrow": "d", "Downarrow": "d", "uparrow": "u", "Uparrow": "u"}
D_CMDS = {"searrow": "se", "swarrow": "sw", "nearrow": "ne", "nwarrow": "nw",
          # itex's double diagonal arrows
          "seArrow": "se", "swArrow": "sw", "neArrow": "ne", "nwArrow": "nw"}

CMD_RE = re.compile(r"\\([a-zA-Z]+)")
# \stackrel{lbl}{arrow} / \overset{lbl}{arrow} / \underset{lbl}{arrow}
OVER_RE = re.compile(r"\\(stackrel|overset|underset|underoverset)\s*")


def find_array(tex: str):
    """Return (start, end, body) of the first \\array{...} block."""
    i = tex.find(r"\array")
    if i < 0:
        return None
    j = tex.find("{", i)
    if j < 0:
        return None
    depth = 0
    for k in range(j, len(tex)):
        if tex[k] == "{" and (k == 0 or tex[k - 1] != "\\"):
            depth += 1
        elif tex[k] == "}" and tex[k - 1] != "\\":
            depth -= 1
            if depth == 0:
                return i, k + 1, tex[j + 1:k]
    return None


def split_depth0(text: str, seps: tuple[str, ...]) -> list[str]:
    """Split on separator strings occurring at brace depth 0."""
    parts, buf, depth, i = [], [], 0, 0
    while i < len(text):
        c = text[i]
        if c == "{" and (i == 0 or text[i - 1] != "\\"):
            depth += 1
        elif c == "}" and text[i - 1] != "\\":
            depth -= 1
        if depth == 0:
            for sep in seps:
                if text.startswith(sep, i):
                    parts.append("".join(buf))
                    buf = []
                    i += len(sep)
                    break
            else:
                buf.append(c)
                i += 1
        else:
            buf.append(c)
            i += 1
    parts.append("".join(buf))
    return parts


def read_group(text: str) -> tuple[str, str]:
    """Consume a {...} group (or single token) from the front of text."""
    text = text.lstrip()
    if not text:
        return "", ""
    if text[0] == "{":
        depth = 0
        for k, c in enumerate(text):
            if c == "{" and (k == 0 or text[k - 1] != "\\"):
                depth += 1
            elif c == "}" and text[k - 1] != "\\":
                depth -= 1
                if depth == 0:
                    return text[1:k], text[k + 1:]
    m = CMD_RE.match(text)
    if m:
        return m.group(0), text[m.end():]
    return text[0], text[1:]


def extract_laps(s: str) -> tuple[str, str | None, str | None]:
    """Pull \\mathllap{...} (west) and \\mathrlap{...} (east) out of a cell,
    returning the remainder."""
    west = east = None
    for cmd, side in (("\\mathllap", "west"), ("\\mathrlap", "east")):
        i = s.find(cmd)
        if i < 0:
            continue
        body, rest = read_group(s[i + len(cmd):])
        s = s[:i] + " " + rest
        if side == "west":
            west = body
        else:
            east = body
    return s.strip(), west, east


def spanning_parens(s: str) -> bool:
    """True when the whole cell is one parenthesized group."""
    if s.startswith("\\left(") and s.endswith("\\right)"):
        return True
    if not (s.startswith("(") and s.endswith(")")):
        return False
    depth = 0
    for i, c in enumerate(s):
        depth += (c == "(") - (c == ")")
        if depth == 0 and i < len(s) - 1:
            return False
    return depth == 0


def classify_cell(cell: str) -> dict:
    s = cell.strip()
    if not s:
        return {"k": "e"}
    s = re.sub(r"\\[bB]igg?\b", "", s).strip()  # \big etc. are cosmetic

    # \stackrel{f}{\to} and friends
    m = OVER_RE.match(s)
    if m:
        which = m.group(1)
        label, rest = read_group(s[m.end():])
        label2 = None
        if which == "underoverset":  # \underoverset{below}{above}{arrow}
            label2 = label
            label, rest = read_group(rest)
        arrow, rest2 = read_group(rest)
        if not rest2.strip():
            if arrow.strip() in ("=", "\\simeq", "\\cong"):
                res = {"k": "h", "dir": "~",
                       "cmd": arrow.strip().lstrip("\\") or "="}
                res["below" if which == "underset" else "above"] = \
                    label.strip()
                return res
            cmds = CMD_RE.findall(arrow)
            if len(cmds) == 1 and cmds[0] in H_CMDS and \
                    arrow.strip() == "\\" + cmds[0]:
                res = {"k": "h", "dir": H_CMDS[cmds[0]], "cmd": cmds[0]}
                if which == "underset":
                    res["below"] = label.strip()
                else:
                    res["above"] = label.strip()
                if label2:
                    res["below"] = label2.strip()
                return res

    cmds = CMD_RE.findall(s)
    bare = s.replace(" ", "")

    if len(cmds) == 1 and bare == "\\" + cmds[0]:
        c = cmds[0]
        if c in H_CMDS:
            return {"k": "h", "dir": H_CMDS[c], "cmd": c}
        if c in V_CMDS:
            return {"k": "v", "dir": V_CMDS[c], "cmd": c}
        if c in D_CMDS:
            return {"k": "d", "dir": D_CMDS[c], "cmd": c}

    # \downarrow^{\mathrlap{f}} / \uparrow_{...} / {}^{f}\downarrow
    # \mathllap{f}\downarrow / \downarrow\mathrlap{f}
    # A pre-script/llap sits to the arrow's west, post-script/rlap east.
    s_nolap, lap_west, lap_east = extract_laps(s)
    if lap_west or lap_east:
        cmds_nolap = CMD_RE.findall(s_nolap)
        if len(cmds_nolap) == 1 and s_nolap == "\\" + cmds_nolap[0] and \
                cmds_nolap[0] in {**V_CMDS, **D_CMDS}:
            c = cmds_nolap[0]
            res = {"k": "v" if c in V_CMDS else "d",
                   "dir": V_CMDS.get(c) or D_CMDS[c], "cmd": c}
            if lap_west:
                res["west"] = lap_west
            if lap_east:
                res["east"] = lap_east
            return res

    for c in (*V_CMDS, *D_CMDS):
        pat = re.compile(
            r"^(?:\{\}[\^_]\{(?P<pre>.*)\})?\\" + c +
            r"(?:[\^_]\{?(?P<post>.*?)\}?)?$")
        m = pat.match(s.replace(" ", ""))
        if m and (m.group("pre") or m.group("post") or bare == "\\" + c):
            res = {"k": "v" if c in V_CMDS else "d",
                    "dir": V_CMDS.get(c) or D_CMDS[c], "cmd": c}
            for group, key in (("pre", "west"), ("post", "east")):
                label = m.group(group)
                if label:
                    label = re.sub(r"\\math[lr]lap", "", label).strip("{}")
                    if label.strip("\\; "):
                        res[key] = label
            return res

    if bare in ("=", "\\simeq", "\\cong"):
        return {"k": "h", "dir": "~", "cmd": bare.lstrip("\\") or "="}
    if bare in ("\\|", "\\Vert", "\\parallel"):  # vertical identity edge
        return {"k": "v", "dir": "veq", "cmd": "="}

    # Object text sharing a cell with a trailing/leading arrow ("\times_d c
    # \rightrightarrows"): split, spilling the text into the neighbour.
    alts = "|".join(sorted(H_CMDS, key=len, reverse=True))
    for pat, spill in ((rf"^(.*\S)\s*\\({alts})$", "spill_west"),
                       (rf"^\\({alts})\s+(\S.*)$", "spill_east")):
        m = re.match(pat, s, re.S)
        if m:
            tex, cmd = ((m.group(1), m.group(2)) if spill == "spill_west"
                        else (m.group(2), m.group(1)))
            if not any(x in H_CMDS or x in V_CMDS or x in D_CMDS
                       for x in CMD_RE.findall(tex)):
                return {"k": "h", "dir": H_CMDS[cmd], "cmd": cmd, spill: tex}

    # A parenthesized formula is an object even if it mentions arrows
    # inside: "(L(c) \\overset{f}{\\to} d)" is a morphism-as-object.
    if spanning_parens(s):
        return {"k": "o", "tex": s}

    # An arrow command mixed into anything else -> too clever for now.
    if any(c in H_CMDS or c in V_CMDS or c in D_CMDS for c in cmds):
        return {"k": "?", "tex": s}
    return {"k": "o", "tex": s}


def absorb_spills(grid) -> None:
    """Attach spilled object text from split cells to the nearest object
    in the spill direction; if there is none, the cell is unconvertible."""
    for r, row in enumerate(grid):
        for c, cell in enumerate(row):
            for key, dc in (("spill_west", -1), ("spill_east", 1)):
                tex = cell.pop(key, None)
                if tex is None:
                    continue
                cc = c + dc
                while 0 <= cc < len(row) and row[cc]["k"] == "e":
                    cc += dc
                if 0 <= cc < len(row) and row[cc]["k"] == "o":
                    row[cc]["tex"] = (f"{row[cc]['tex']} {tex}" if dc < 0
                                      else f"{tex} {row[cc]['tex']}")
                else:
                    grid[r][c] = {"k": "?", "tex": tex}


def merge_annotations(grid) -> None:
    """Authors sometimes put an arrow label or an object annotation in its
    own cell (`g & \\downarrow` or `c \\in & [X,A]`). Left as nodes these
    open huge gaps, so: an object that is alone in its column merges into
    the horizontally adjacent arrow (as a west/east label) or object (as
    concatenated TeX)."""
    cols = max(len(r) for r in grid)
    for row in grid:
        row.extend({"k": "e"} for _ in range(cols - len(row)))
    for c in range(cols):
        filled = [r for r in range(len(grid)) if grid[r][c]["k"] != "e"]
        if len(filled) != 1 or grid[filled[0]][c]["k"] != "o":
            continue
        r = filled[0]
        tex = grid[r][c]["tex"]
        for dc in (1, -1):
            cc = c + dc
            if not 0 <= cc < cols:
                continue
            other = grid[r][cc]
            if other["k"] in ("v", "d"):
                other.setdefault("west" if dc == 1 else "east", tex)
            elif other["k"] == "o":
                other["tex"] = (f"{tex} {other['tex']}" if dc == 1
                                else f"{other['tex']} {tex}")
            else:
                continue
            grid[r][c] = {"k": "e"}
            break


def parse(mathml: str) -> tuple[str, list | None]:
    m = ANNOTATION_RE.search(mathml)
    if not m:
        return "no-annotation", None
    tex = html.unescape(m.group(1))
    # Annotations are sometimes double-escaped, leaving numeric character
    # references (&#643;) in the TeX; decode them to the actual character.
    tex = re.sub(r"&#(\d+);", lambda m: chr(int(m.group(1))), tex)
    tex = re.sub(r"&#x([0-9a-fA-F]+);", lambda m: chr(int(m.group(1), 16)),
                 tex)
    found = find_array(tex)
    if not found:
        return "no-array", None
    start, end, body = found
    trailing = re.sub(r"\\[,;:!]|\\q?quad|[\s.,]", "", tex[end:])
    if tex[:start].strip() or trailing:
        return "wrapped", None
    grid = [[classify_cell(c) for c in split_depth0(row, ("&",))]
            for row in split_depth0(body, ("\\\\",))]
    grid = [r for r in grid if any(c["k"] != "e" for c in r)]
    if not grid:
        return "empty", None
    absorb_spills(grid)
    merge_annotations(grid)
    unknown = sum(c["k"] == "?" for r in grid for c in r)
    return ("ok" if not unknown else f"cells:{unknown}?"), grid


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()

    con = common.connect()
    common.ensure_hashes(con)
    common.stage_table(con, "parsed", "status TEXT, grid TEXT")

    n = 0
    for row in common.pending(
            con, "SELECT hash, mathml FROM mtables GROUP BY hash",
            "parsed", args.force):
        status, grid = parse(row["mathml"])
        con.execute(
            "INSERT OR REPLACE INTO parsed(hash, status, grid) VALUES (?,?,?)",
            (row["hash"], status, json.dumps(grid) if grid else None))
        n += 1
    con.commit()
    print(f"parsed {n} new diagrams; totals:")
    common.report(con, "parsed", "status")
    con.close()


if __name__ == "__main__":
    main()
