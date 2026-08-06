#!/usr/bin/env python3
"""Stage 4: compile emitted typst code to PNGs (out/typst/<hash>.png).

Results are recorded in `renders` (kind='typst'), so already-rendered
hashes are skipped unless --force. Compilation runs in parallel.
"""

import argparse
import subprocess
from concurrent.futures import ThreadPoolExecutor

import common


def compile_one(hash_: str, code: str) -> tuple[str, str, str]:
    typ = common.OUT / "typst" / f"{hash_}.typ"
    png = common.OUT / "typst" / f"{hash_}.png"
    typ.write_text(code, encoding="utf-8")
    proc = subprocess.run(
        ["typst", "compile", "--format", "png", "--ppi", "144",
         str(typ), str(png)],
        capture_output=True, text=True, timeout=60)
    if proc.returncode == 0 and png.is_file():
        return hash_, "ok", ""
    return hash_, "error", proc.stderr.strip()[:500]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true")
    ap.add_argument("--limit", type=int)
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--hashes", help="file of hashes to (re)render")
    args = ap.parse_args()

    (common.OUT / "typst").mkdir(parents=True, exist_ok=True)
    con = common.connect()
    common.renders_table(con)

    done = {} if args.force else {
        r[0]: r[1] for r in con.execute(
            "SELECT hash, codehash FROM renders WHERE kind='typst'"
            " AND status='ok'")}
    todo = [(r["hash"], r["code"]) for r in con.execute(
        "SELECT hash, code FROM typst WHERE status='ok'")
        if done.get(r["hash"]) != common.text_hash(r["code"])]
    if args.hashes:
        wanted = set(open(args.hashes).read().split())
        todo = [(h, c) for r in con.execute(
            "SELECT hash, code FROM typst WHERE status='ok'")
            for h, c in [(r["hash"], r["code"])] if h in wanted]
    if args.limit:
        todo = todo[:args.limit]

    codes = dict(todo)
    ok = err = 0
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for hash_, status, error in pool.map(
                lambda t: compile_one(*t), todo):
            con.execute(
                "INSERT OR REPLACE INTO renders VALUES (?,?,?,?,?,?)",
                (hash_, "typst", status,
                 f"out/typst/{hash_}.png" if status == "ok" else None,
                 error, common.text_hash(codes[hash_])))
            ok += status == "ok"
            err += status != "ok"
            if (ok + err) % 200 == 0:
                con.commit()
                print(f"  {ok} ok, {err} errors")
    con.commit()
    print(f"typst renders: {ok} ok, {err} errors "
          f"({len(todo)} attempted this run)")
    con.close()


if __name__ == "__main__":
    main()
