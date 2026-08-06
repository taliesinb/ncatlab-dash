#!/usr/bin/env python3
"""Stage 6: build a side-by-side gallery for hand verification.

Picks a (seeded) random sample of diagrams that have both a WebKit MathML
render and a typst/fletcher render, and writes out/compare.html showing
original vs regenerated with provenance. Nothing touches the docset until
this gallery passes eyeball review.
"""

import argparse

import common

ROW = """
<tr id="r{n}">
 <td class="num"><a href="#r{n}">{n}</a></td>
 <td class="meta">{name}<br /><code>{hash}</code><br />{cls}</td>
 <td><img src="mathml/{hash}.png" /></td>
 <td><img src="typst/{hash}.png" /></td>
</tr>"""

PAGE = """<!DOCTYPE html>
<html><head><meta charset="utf-8" />
<title>mtable vs typst</title>
<style>
 body {{ font: 14px sans-serif; }}
 table {{ border-collapse: collapse; }}
 td {{ border: 1px solid #ccc; padding: 8px; vertical-align: middle; }}
 td.meta {{ font-size: 11px; color: #555; max-width: 16em; }}
 td.num {{ font-size: 18px; font-weight: bold; }}
 img {{ max-width: 480px; }}
 th {{ padding: 8px; background: #eee; }}
</style></head><body>
<h1>{n} sampled diagrams: WebKit MathML (left) vs typst+fletcher (right)</h1>
<table><tr><th>#</th><th>page</th><th>original</th><th>regenerated</th></tr>
{rows}
</table></body></html>
"""


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=int, default=40)
    ap.add_argument("--class", dest="cls", help="restrict to one class")
    args = ap.parse_args()

    con = common.connect()
    sql = """
        SELECT t.hash hash, t.class class,
               (SELECT page_name FROM mtables m WHERE m.hash = t.hash) name
        FROM typst t
        JOIN renders rm ON rm.hash = t.hash AND rm.kind='mathml'
             AND rm.status='ok'
        JOIN renders rt ON rt.hash = t.hash AND rt.kind='typst'
             AND rt.status='ok'
        """
    if args.cls:
        sql += f" WHERE t.class = '{args.cls}'"
    sql += " ORDER BY t.hash LIMIT ?"
    rows = con.execute(sql, (args.sample,)).fetchall()
    out = common.OUT / "compare.html"
    out.write_text(PAGE.format(n=len(rows), rows="".join(
        ROW.format(n=i + 1, hash=r["hash"], name=r["name"], cls=r["class"])
        for i, r in enumerate(rows))), encoding="utf-8")
    print(f"{len(rows)} pairs -> {out}")
    con.close()


if __name__ == "__main__":
    main()
