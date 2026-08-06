#!/usr/bin/env python3
"""Stage 5: render the original MathML diagrams to PNGs via WebKit.

Wraps each diagram's <math> markup in a minimal page styled with the same
nLab stylesheets the docset uses, renders it offscreen in a WKWebView (the
engine Dash and Safari share) via the webkit_snap helper, and stores
out/mathml/<hash>.png in `renders` (kind='mathml').

By default renders only hashes that already have a typst render, since the
point is side-by-side comparison; use --all for everything.
"""

import argparse
import random
import subprocess

import common

CSS_FILES = ("instiki.css", "mathematics.css", "nlab.css")

PAGE = """<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8" />
{links}
<style>
  body {{ margin: 0; background: white; }}
  #snap {{ display: inline-block; padding: 6px; }}
</style>
</head>
<body><div id="snap">{math}</div></body>
</html>
"""


def snap_binary() -> str:
    src = common.HERE / "webkit_snap.swift"
    binary = common.HERE / "bin" / "webkit_snap"
    if not binary.is_file() or binary.stat().st_mtime < src.stat().st_mtime:
        binary.parent.mkdir(exist_ok=True)
        subprocess.run(
            ["swiftc", "-O", str(src), "-o", str(binary)], check=True)
    return str(binary)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true")
    ap.add_argument("--all", action="store_true",
                    help="render every diagram, not just typst-rendered ones")
    ap.add_argument("--limit", type=int)
    ap.add_argument("--sample", type=int,
                    help="random sample of N (seeded, deterministic)")
    ap.add_argument("--hashes", help="file of hashes to render")
    args = ap.parse_args()

    assets = (common.HERE.parent / "build" / "assets").resolve()
    links = "\n".join(
        f'<link rel="stylesheet" href="file://{assets}/{f}" />'
        for f in CSS_FILES)

    (common.OUT / "mathml").mkdir(parents=True, exist_ok=True)
    (common.OUT / "html").mkdir(parents=True, exist_ok=True)
    con = common.connect()
    common.renders_table(con)
    binary = snap_binary()

    if args.hashes:
        wanted = set(open(args.hashes).read().split())
        rows = [r for r in con.execute(
            "SELECT hash, mathml FROM mtables GROUP BY hash")
            if r["hash"] in wanted]
    elif args.all:
        rows = con.execute(
            "SELECT hash, mathml FROM mtables GROUP BY hash").fetchall()
    else:
        rows = con.execute(
            "SELECT m.hash hash, m.mathml mathml FROM mtables m"
            " JOIN renders r ON r.hash = m.hash AND r.kind='typst'"
            " AND r.status='ok' GROUP BY m.hash").fetchall()
    done = set() if args.force else {
        r[0] for r in con.execute(
            "SELECT hash FROM renders WHERE kind='mathml'")}
    todo = [r for r in rows if r["hash"] not in done]
    if args.sample:
        todo = random.Random(0).sample(todo, min(args.sample, len(todo)))
    if args.limit:
        todo = todo[:args.limit]

    ok = err = 0
    for row in todo:
        h = row["hash"]
        html_file = common.OUT / "html" / f"{h}.html"
        png = common.OUT / "mathml" / f"{h}.png"
        html_file.write_text(PAGE.format(links=links, math=row["mathml"]),
                             encoding="utf-8")
        proc = subprocess.run([binary, str(html_file), str(png)],
                              capture_output=True, text=True)
        status = "ok" if proc.returncode == 0 and png.is_file() else "error"
        con.execute(
            "INSERT OR REPLACE INTO renders VALUES (?,?,?,?,?)",
            (h, "mathml", status,
             f"out/mathml/{h}.png" if status == "ok" else None,
             proc.stderr.strip()[:500] if status == "error" else ""))
        ok += status == "ok"
        err += status != "ok"
        if (ok + err) % 50 == 0:
            con.commit()
            print(f"  {ok} ok, {err} errors")
    con.commit()
    print(f"mathml renders: {ok} ok, {err} errors "
          f"({len(todo)} attempted this run)")
    con.close()


if __name__ == "__main__":
    main()
