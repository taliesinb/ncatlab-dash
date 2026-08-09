#!/usr/bin/env python3
"""Diff the Rust grid parser (crates/nlab-typ) against the Python one.

Feeds every distinct diagram's decoded TeX through `nlab-typ grids` and
compares both the status and the JSON grid with the `parsed` table.
The Python implementation is the reference; any mismatch is a bug in the
port (or an intentional divergence to be reviewed).
"""

import argparse
import html
import json
import re
import subprocess

import common
import parse_arrays

BINARY = common.HERE.parent / "crates/nlab-typ/target/release/nlab-typ"


def decode_tex(mathml: str) -> str | None:
    m = parse_arrays.ANNOTATION_RE.search(mathml)
    if not m:
        return None
    tex = html.unescape(m.group(1))
    tex = re.sub(r"&#(\d+);", lambda m: chr(int(m.group(1))), tex)
    tex = re.sub(r"&#x([0-9a-fA-F]+);",
                 lambda m: chr(int(m.group(1), 16)), tex)
    return tex


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int)
    ap.add_argument("--show", type=int, default=5,
                    help="print this many mismatches")
    ap.add_argument("--emit", action="store_true",
                    help="diff the emitter (typst table) instead of grids")
    args = ap.parse_args()

    if args.emit:
        return diff_emit(args)

    con = common.connect()
    rows = con.execute(
        "SELECT p.hash hash, p.status status, p.grid grid,"
        " min(m.mathml) mathml FROM parsed p JOIN mtables m"
        " ON m.hash = p.hash GROUP BY p.hash").fetchall()
    if args.limit:
        rows = rows[:args.limit]

    cases = []
    for r in rows:
        tex = decode_tex(r["mathml"])
        if tex is None:
            continue
        cases.append((r["hash"], r["status"], r["grid"], tex))

    payload = "\x00".join(tex for _, _, _, tex in cases) + "\x00"
    proc = subprocess.run([str(BINARY), "grids"], input=payload,
                          capture_output=True, text=True, timeout=600)
    outs = [o for o in proc.stdout.split("\x00")]

    n = status_match = grid_match = 0
    shown = 0
    for (h, py_status, py_grid, tex), out in zip(cases, outs):
        rs_status, _, rs_grid = out.partition("\x1f")
        n += 1
        s_ok = rs_status == py_status
        status_match += s_ok
        if py_status == "ok" and s_ok:
            # normalize via json round-trip guards against formatting-only
            # differences; exactness is still the target
            exact = rs_grid.strip() == (py_grid or "").strip()
            semantic = False
            if not exact:
                try:
                    semantic = json.loads(rs_grid) == json.loads(py_grid)
                except Exception:
                    pass
            if exact or semantic:
                grid_match += 1
            elif shown < args.show:
                shown += 1
                print(f"== GRID MISMATCH {h}\n  tex: {tex[:120]!r}")
                print(f"  py: {(py_grid or '')[:220]}")
                print(f"  rs: {rs_grid[:220]}")
        elif not s_ok and shown < args.show:
            shown += 1
            print(f"== STATUS MISMATCH {h}: py={py_status} rs={rs_status}"
                  f"\n  tex: {tex[:120]!r}")

    ok_total = sum(1 for _, s, _, _ in cases if s == "ok")
    print(f"\n{n} diagrams: status match {status_match}/{n}"
          f" ({status_match/n:.1%});"
          f" grid match {grid_match}/{ok_total} of parsed-ok"
          f" ({grid_match/ok_total:.1%})")
    con.close()


def diff_emit(args) -> None:
    con = common.connect()
    rows = con.execute(
        "SELECT p.hash hash, p.status pstatus, min(m.mathml) mathml,"
        " t.status tstatus, t.class tclass, t.code tcode"
        " FROM parsed p JOIN mtables m ON m.hash = p.hash"
        " LEFT JOIN typst t ON t.hash = p.hash GROUP BY p.hash").fetchall()
    if args.limit:
        rows = rows[:args.limit]
    cases = []
    for r in rows:
        tex = decode_tex(r["mathml"])
        if tex is None:
            continue
        cases.append((r, tex))
    payload = "\x00".join(tex for _, tex in cases) + "\x00"
    proc = subprocess.run([str(BINARY), "typsts"], input=payload,
                          capture_output=True, text=True, timeout=1200)
    outs = proc.stdout.split("\x00")
    n = match = 0
    shown = 0
    for (r, tex), out in zip(cases, outs):
        rs_status, rs_class, rs_code = (out.split("\x1f") + ["", ""])[:3]
        n += 1
        if r["tstatus"] is None:
            ok = rs_status.startswith("-")
        else:
            ok = (rs_status == r["tstatus"]
                  and rs_class == (r["tclass"] or "")
                  and (rs_code or "") == (r["tcode"] or ""))
        if ok:
            match += 1
        elif shown < args.show:
            shown += 1
            print(f"== EMIT MISMATCH {r['hash']}"
                  f" py=({r['tstatus']},{r['tclass']})"
                  f" rs=({rs_status},{rs_class})")
            pc, rc = (r["tcode"] or ""), (rs_code or "")
            if pc != rc:
                for i, (a, b) in enumerate(zip(pc.splitlines(), rc.splitlines())):
                    if a != b:
                        print(f"  py| {a[:150]}")
                        print(f"  rs| {b[:150]}")
                        break
                if len(pc.splitlines()) != len(rc.splitlines()):
                    print(f"  (py {len(pc.splitlines())} lines,"
                          f" rs {len(rc.splitlines())} lines)")
    print(f"\n{n} formulas: emit match {match}/{n} ({match/n:.1%})")
    con.close()


if __name__ == "__main__":
    main()
