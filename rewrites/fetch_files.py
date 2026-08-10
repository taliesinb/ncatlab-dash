#!/usr/bin/env python3
"""Download nLab uploaded files referenced by content pages.

Scans content.md files for https://ncatlab.org/nlab/files/<name>
references and downloads missing ones into build/nlab-files/ (never
committed, like the content mirrors). Rate-limited politely.

Usage: fetch_files.py --ids 24688,395 | --all [--limit N]
"""

import argparse
import pathlib
import re
import time
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
BUILD = HERE.parent / "build/nlab-content/pages"
FILES = HERE.parent / "build/nlab-files"

REF_RE = re.compile(r"https?://ncatlab\.org/nlab/files/([A-Za-z0-9_.%+-]+)")


def refs_in(md: pathlib.Path):
    return set(REF_RE.findall(md.read_text(errors="replace")))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ids", help="comma-separated page ids")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--limit", type=int, default=200)
    args = ap.parse_args()

    pages = sorted(BUILD.rglob("content.md"))
    if args.ids:
        want = set(args.ids.split(","))
        pages = [p for p in pages if p.parent.name in want]
    elif not args.all:
        raise SystemExit("pass --ids or --all")

    names = set()
    for p in pages:
        names |= refs_in(p)

    FILES.mkdir(parents=True, exist_ok=True)
    todo = [n for n in sorted(names) if not (FILES / n).exists()]
    print(f"{len(names)} referenced, {len(todo)} to fetch")
    fetched = 0
    for n in todo:
        if fetched >= args.limit:
            print(f"limit {args.limit} reached")
            break
        url = f"https://ncatlab.org/nlab/files/{n}"
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "ncatlab-dash/1.0"})
            with urllib.request.urlopen(req, timeout=30) as r:
                data = r.read()
            (FILES / n).write_bytes(data)
            fetched += 1
            print(f"  {n} ({len(data)//1024}kB)")
        except Exception as e:
            print(f"  FAIL {n}: {e}")
        time.sleep(0.5)
    print(f"fetched {fetched}")


if __name__ == "__main__":
    main()
