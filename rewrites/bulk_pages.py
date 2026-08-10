#!/usr/bin/env python3
"""Bulk-convert the whole nLab content mirror to typst pages.

Every pages/**/content.md becomes build/typ-pages/<id>.typ (+ .pdf).
Content output is never committed, like the mirrors. Resumable: pages
whose .pdf is newer than both the source and the converter binary are
skipped. A failure summary lands in out/bulk-report.txt.

Usage: bulk_pages.py [--limit N] [--no-pdf] [--jobs J]
"""

import argparse
import concurrent.futures
import os
import pathlib
import subprocess
import time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
BUILD = ROOT / "build/nlab-content/pages"
OUT = ROOT / "build/typ-pages"
BIN = ROOT / "crates/nlab-typ/target/release/nlab-typ"
REPORT = HERE / "out/bulk-report.txt"
FILES_ROOT = ROOT / "build/nlab-files"


def one(md: pathlib.Path, pdf: bool):
    pid = md.parent.name
    name = (md.parent / "name").read_text().strip() if (md.parent / "name").exists() else pid
    typ = OUT / f"{pid}.typ"
    target = typ.with_suffix(".pdf") if pdf else typ
    try:
        stamp = max(md.stat().st_mtime, BIN.stat().st_mtime)
        if target.exists() and target.stat().st_mtime > stamp:
            return (pid, name, "ok", True)
    except OSError:
        pass
    env = dict(os.environ, NLAB_FILES_ROOT=str(FILES_ROOT))
    try:
        r = subprocess.run(
            [str(BIN), "page", name],
            input=md.read_text(errors="replace"),
            capture_output=True,
            text=True,
            timeout=120,
            env=env,
        )
    except subprocess.TimeoutExpired:
        return (pid, name, "convert-timeout", False)
    if r.returncode != 0:
        return (pid, name, "convert-fail: " + r.stderr[:120], False)
    typ.write_text(r.stdout)
    if not pdf:
        return (pid, name, "ok", False)
    try:
        c = subprocess.run(
            ["typst", "compile", "--format", "pdf", str(typ), str(typ.with_suffix(".pdf"))],
            capture_output=True,
            text=True,
            timeout=300,
            cwd=str(OUT),
        )
    except subprocess.TimeoutExpired:
        return (pid, name, "compile-timeout", False)
    if c.returncode != 0:
        return (pid, name, "compile-fail: " + c.stderr.splitlines()[0][:140], False)
    return (pid, name, "ok", False)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int)
    ap.add_argument("--no-pdf", action="store_true")
    ap.add_argument("--jobs", type=int, default=8)
    args = ap.parse_args()

    OUT.mkdir(parents=True, exist_ok=True)
    files_link = OUT / "files"
    if not files_link.exists():
        files_link.symlink_to(FILES_ROOT)
    FILES_ROOT.mkdir(parents=True, exist_ok=True)

    pages = sorted(BUILD.rglob("content.md"))
    if args.limit:
        pages = pages[: args.limit]

    t0 = time.time()
    ok = cached = 0
    fails = []
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as ex:
        futs = [ex.submit(one, p, not args.no_pdf) for p in pages]
        for i, f in enumerate(concurrent.futures.as_completed(futs), 1):
            pid, name, status, was_cached = f.result()
            if status == "ok":
                ok += 1
                cached += was_cached
            else:
                fails.append((pid, name, status))
            if i % 500 == 0:
                el = time.time() - t0
                print(f"{i}/{len(pages)} ok={ok} fail={len(fails)}"
                      f" ({el:.0f}s, {i/el:.1f}/s)", flush=True)

    REPORT.parent.mkdir(exist_ok=True)
    with open(REPORT, "w") as f:
        f.write(f"{ok}/{len(pages)} ok ({cached} cached), {len(fails)} failed\n\n")
        for pid, name, status in sorted(fails):
            f.write(f"{pid}\t{name}\t{status}\n")
    print(f"done: {ok}/{len(pages)} ok, {len(fails)} failed -> {REPORT}")


if __name__ == "__main__":
    main()
