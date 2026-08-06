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
    "leftrightarrow": "lr", "simeq": "~", "cong": "~", "equiv": "~",
}
V_CMDS = {"downarrow": "d", "Downarrow": "d", "uparrow": "u", "Uparrow": "u"}
D_CMDS = {"searrow": "se", "swarrow": "sw", "nearrow": "ne", "nwarrow": "nw"}

CMD_RE = re.compile(r"\\([a-zA-Z]+)")
# \stackrel{lbl}{arrow} / \overset{lbl}{arrow} / \underset{lbl}{arrow}
OVER_RE = re.compile(r"\\(stackrel|overset|underset)\s*")


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


def classify_cell(cell: str) -> dict:
    s = cell.strip()
    if not s:
        return {"k": "e"}

    # \stackrel{f}{\to} and friends
    m = OVER_RE.match(s)
    if m:
        which = m.group(1)
        label, rest = read_group(s[m.end():])
        arrow, rest2 = read_group(rest)
        if not rest2.strip():
            cmds = CMD_RE.findall(arrow)
            if len(cmds) == 1 and cmds[0] in H_CMDS and \
                    arrow.strip() == "\\" + cmds[0]:
                side = "below" if which == "underset" else "above"
                return {"k": "h", "dir": H_CMDS[cmds[0]],
                        "cmd": cmds[0], side: label.strip()}

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
    for c in (*V_CMDS, *D_CMDS):
        pat = re.compile(
            r"^(?:\{\}[\^_]\{(?P<pre>.*)\})?\\" + c +
            r"(?:[\^_]\{?(?P<post>.*?)\}?)?$")
        m = pat.match(s.replace(" ", ""))
        if m and (m.group("pre") or m.group("post") or bare == "\\" + c):
            label = m.group("pre") or m.group("post") or ""
            label = re.sub(r"\\math[lr]lap", "", label).strip("{}")
            kind = "v" if c in V_CMDS else "d"
            return {"k": kind, "dir": V_CMDS.get(c) or D_CMDS[c],
                    "cmd": c, "above": label} if label else \
                   {"k": kind, "dir": V_CMDS.get(c) or D_CMDS[c], "cmd": c}

    if bare in ("=", "\\simeq", "\\cong"):
        return {"k": "h", "dir": "~", "cmd": bare.lstrip("\\") or "="}

    # An arrow command mixed into anything else -> too clever for now.
    if any(c in H_CMDS or c in V_CMDS or c in D_CMDS for c in cmds):
        return {"k": "?", "tex": s}
    return {"k": "o", "tex": s}


def parse(mathml: str) -> tuple[str, list | None]:
    m = ANNOTATION_RE.search(mathml)
    if not m:
        return "no-annotation", None
    tex = html.unescape(m.group(1))
    found = find_array(tex)
    if not found:
        return "no-array", None
    start, end, body = found
    if tex[:start].strip() or tex[end:].strip():
        return "wrapped", None
    grid = [[classify_cell(c) for c in split_depth0(row, ("&",))]
            for row in split_depth0(body, ("\\\\",))]
    grid = [r for r in grid if any(c["k"] != "e" for c in r)]
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
