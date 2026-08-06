"""Shared helpers for the mtable-rewriting pipeline.

Every stage reads and writes tables in the same SQLite database
(rewrites/mtables.db, produced by extract_mtables.py) and keys its work on
`hash`, a deterministic digest of the diagram's MathML markup. Stages skip
hashes they have already processed, so each stage is a cache; pass
--force to redo.
"""

import hashlib
import sqlite3
from pathlib import Path

HERE = Path(__file__).resolve().parent
DB_PATH = HERE / "mtables.db"
OUT = HERE / "out"


def connect(db: Path = DB_PATH) -> sqlite3.Connection:
    con = sqlite3.connect(db)
    con.row_factory = sqlite3.Row
    return con


def mathml_hash(mathml: str) -> str:
    return hashlib.sha1(mathml.encode("utf-8")).hexdigest()[:16]


def ensure_hashes(con: sqlite3.Connection) -> None:
    """Add and populate the mtables.hash column (idempotent)."""
    cols = [r[1] for r in con.execute("PRAGMA table_info(mtables)")]
    if "hash" not in cols:
        con.execute("ALTER TABLE mtables ADD COLUMN hash TEXT")
    for rowid, mathml in con.execute(
            "SELECT id, mathml FROM mtables WHERE hash IS NULL"):
        con.execute("UPDATE mtables SET hash = ? WHERE id = ?",
                    (mathml_hash(mathml), rowid))
    con.commit()


def stage_table(con: sqlite3.Connection, name: str, columns: str) -> None:
    con.execute(f"CREATE TABLE IF NOT EXISTS {name}"
                f"(hash TEXT PRIMARY KEY, {columns})")


def pending(con: sqlite3.Connection, source_sql: str, stage: str,
            force: bool):
    """Rows from source_sql whose hash is not yet in `stage` (all if force)."""
    done = set() if force else {
        r[0] for r in con.execute(f"SELECT hash FROM {stage}")}
    for row in con.execute(source_sql):
        if row["hash"] not in done:
            yield row


def renders_table(con: sqlite3.Connection) -> None:
    con.execute("CREATE TABLE IF NOT EXISTS renders("
                "hash TEXT, kind TEXT, status TEXT, path TEXT, error TEXT,"
                "PRIMARY KEY (hash, kind))")
    cols = [r[1] for r in con.execute("PRAGMA table_info(renders)")]
    if "codehash" not in cols:
        # Invalidates cached renders when the emitted code changes.
        con.execute("ALTER TABLE renders ADD COLUMN codehash TEXT")


def text_hash(text: str) -> str:
    return hashlib.sha1(text.encode("utf-8")).hexdigest()[:16]


def report(con: sqlite3.Connection, table: str, column: str) -> None:
    for value, n in con.execute(
            f"SELECT {column}, count(*) FROM {table} GROUP BY {column}"
            " ORDER BY 2 DESC"):
        print(f"  {str(value):24} {n}")
