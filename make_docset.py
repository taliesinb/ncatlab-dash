#!/usr/bin/env python3
"""Package the nLab content mirrors into a Dash docset.

Usage: make_docset.py --html DIR --source DIR --assets DIR --out DIR

Inputs:
  --html    checkout of ncatlab/nlab-content-html: pages/<d>/<d>/<d>/<d>/<id>/
            containing content.html (full XHTML+MathML page), name, revision_id.
            The shard path is the page id's decimal digits, least significant
            first (page 10004 lives at pages/4/0/0/0/10004/).
  --source  checkout of ncatlab/nlab-content (same layout, content.md); only
            used to harvest [[!redirects ...]] directives as search aliases.
  --assets  directory of files fetched from ncatlab.org (CSS + JS), copied
            into the docset verbatim.

Each page becomes Documents/pages/<id>.html. Wiki links (/nlab/show/<name>)
are resolved through the name->id map to relative links; anything unresolved
(new wiki words, uploads, wiki actions) is pointed at ncatlab.org so it works
when online. Editing chrome (nav bars, edit/history/search) is stripped.
Math is MathML, which Dash's WebKit renders natively.
"""

import argparse
import html
import plistlib
import re
import shutil
import sqlite3
import sys
import urllib.parse
from pathlib import Path

# page_helper.js must load between prototype.js (its dependency) and
# thm_numbering.js: its fixRunIn() creates the .theorem_label spans that
# thm_numbering counts. It also shims MathML columnalign for WebKit.
KEEP_SCRIPTS = ("prototype.js", "page_helper.js", "thm_numbering.js")
DROP_SCRIPTS = ("effects.js", "dragdrop.js", "controls.js", "application.js")


def iter_pages(mirror: Path):
    """Yield (id, page_dir) for every page in a content mirror checkout."""
    for name_file in (mirror / "pages").glob("*/*/*/*/*/name"):
        page_dir = name_file.parent
        yield page_dir.name, page_dir


def load_names(html_mirror: Path) -> dict[str, str]:
    """Map page name -> page id."""
    names: dict[str, str] = {}
    dupes = 0
    for page_id, page_dir in iter_pages(html_mirror):
        name = (page_dir / "name").read_text(encoding="utf-8").strip()
        if name in names:
            dupes += 1
            # Keep the higher id (later page) on duplicate names.
            if int(page_id) < int(names[name]):
                continue
        names[name] = page_id
    if dupes:
        print(f"note: {dupes} duplicate page names (kept latest)")
    return names


REDIRECT_RE = re.compile(r"\[\[!redirects\s+([^\]]+?)\s*\]\]")
CATEGORY_RE = re.compile(r"^category\s*:\s*(.+?)\s*$", re.M)


def classify(name: str, categories: set[str]) -> str | None:
    """Dash entry type for a page, or None to leave it out of the index.

    Grounded in the nLab's own conventions: `category:` tags in the page
    sources (people, reference, ...) and page-name conventions (floating
    tables of contents, expository series, archived history subpages).
    "Person" is not an official Dash type but Dash handles unknown types
    gracefully (generic icon, groups under the literal type name).
    """
    if name.endswith(" > history"):
        return None
    if name.endswith("contents"):
        return "Category"
    if name.startswith(("geometry of physics", "Introduction to ")):
        return "Guide"
    if "people" in categories:
        return "Person"
    if "reference" in categories:
        return "Resource"
    return "Entry"


def scan_sources(
    source_mirror: Path, names: dict[str, str]
) -> tuple[dict[str, str], dict[str, set[str]]]:
    """Scan the Markdown sources for [[!redirects ...]] directives
    (alias -> page id) and `category:` tags (page id -> tags)."""
    redirects: dict[str, str] = {}
    categories: dict[str, set[str]] = {}
    if not (source_mirror / "pages").is_dir():
        print("note: no source mirror; skipping redirects and categories")
        return redirects, categories
    for page_id, page_dir in iter_pages(source_mirror):
        md = page_dir / "content.md"
        if not md.is_file():
            continue
        try:
            text = md.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for alias in REDIRECT_RE.findall(text):
            alias = alias.strip()
            if alias and alias not in names:
                redirects.setdefault(alias, page_id)
        for line in CATEGORY_RE.findall(text):
            categories.setdefault(page_id, set()).update(
                tag.strip() for tag in line.split(","))
    return redirects, categories


TITLE_RE = re.compile(r"<title>\s*(.*?)\s+in nLab\s*</title>", re.S)
CSS_LINK_RE = re.compile(r'<link href="/stylesheets/[^"]*"[^>]*>')
FONT_LINK_RE = re.compile(r'<link[^>]*href="https://cdn\.jsdelivr\.net[^"]*"[^>]*/?>')
SCRIPT_SRC_RE = re.compile(r'<script src="/javascripts/([^"?]+)[^"]*"[^>]*></script>\n?')
NAV_RE = re.compile(r'<div class="navigation( navfoot)?">.*?</div>', re.S)
SHOW_LINK_RE = re.compile(r'href="/nlab/show/([^"#]*)(#[^"]*)?"')
ABS_LINK_RE = re.compile(r'(href|src)="(/[^/"][^"]*)"')
HEADING_RE = re.compile(r"<h([23])( [^>]*)?>(.*?)</h\1>", re.S)
TAG_RE = re.compile(r"<[^>]+>")


def transform(text: str, resolve, css_links: str, js_links: str) -> str:
    text = TITLE_RE.sub(lambda m: f"<title>{m.group(1)}</title>", text, count=1)

    # Vendored stylesheets/scripts; drop wiki-app JS and the CDN webfont.
    text, n = CSS_LINK_RE.subn(css_links, text, count=1)
    if n:
        text = CSS_LINK_RE.sub("", text)
    text = FONT_LINK_RE.sub("", text)

    def script(m):
        if m.group(1) == KEEP_SCRIPTS[0]:
            return js_links
        return ""

    text = SCRIPT_SRC_RE.sub(script, text)

    # Strip the header nav (search form, edit links) and footer nav.
    text = NAV_RE.sub("", text)

    # Resolve wiki links against the name->id map.
    def show(m):
        target = resolve(urllib.parse.unquote_plus(m.group(1)))
        frag = m.group(2) or ""
        if target:
            return f'href="{target}.html{frag}"'
        return f'href="https://ncatlab.org/nlab/show/{m.group(1)}{frag}"'

    text = SHOW_LINK_RE.sub(show, text)

    # Everything else absolute (uploads, other wiki actions) -> online site.
    text = ABS_LINK_RE.sub(r'\1="https://ncatlab.org\2"', text)

    # Dash TOC anchors on section headings.
    def anchor(m):
        title = html.unescape(TAG_RE.sub("", m.group(3))).strip()
        if not title:
            return m.group(0)
        ref = urllib.parse.quote(title, safe="")
        return (f'<a name="//apple_ref/cpp/Section/{ref}" class="dashAnchor">'
                f"</a>{m.group(0)}")

    return HEADING_RE.sub(anchor, text)


def build_index(db_path: Path, entries) -> int:
    con = sqlite3.connect(db_path)
    cur = con.cursor()
    cur.execute("CREATE TABLE searchIndex(id INTEGER PRIMARY KEY, name TEXT, "
                "type TEXT, path TEXT)")
    cur.execute("CREATE UNIQUE INDEX anchor ON searchIndex (name, type, path)")
    cur.executemany(
        "INSERT OR IGNORE INTO searchIndex(name, type, path) VALUES (?,?,?)",
        entries)
    n = cur.execute("SELECT count(*) FROM searchIndex").fetchone()[0]
    con.commit()
    con.close()
    return n


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--html", type=Path, required=True)
    ap.add_argument("--source", type=Path, required=True)
    ap.add_argument("--assets", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--only", help="comma-separated page names; build a small "
                    "docset containing just these pages (links to other pages "
                    "fall back to ncatlab.org)")
    args = ap.parse_args()

    if not (args.html / "pages").is_dir():
        sys.exit(f"error: {args.html} is not a nlab-content-html checkout")

    print("scanning page names ...")
    names = load_names(args.html)
    print(f"{len(names)} pages")
    print("harvesting redirects and categories ...")
    redirects, categories = scan_sources(args.source, names)
    print(f"{len(redirects)} redirect aliases, "
          f"{len(categories)} pages with category tags")

    included: set[str] | None = None
    if args.only:
        wanted = [n.strip() for n in args.only.split(",") if n.strip()]
        missing = [n for n in wanted if not (names.get(n) or redirects.get(n))]
        if missing:
            sys.exit(f"error: unknown page names: {missing}")
        included = {pid for n in wanted
                    if (pid := names.get(n) or redirects.get(n))}

    def resolve(name: str) -> str | None:
        pid = names.get(name) or redirects.get(name)
        if pid and included is not None and pid not in included:
            return None
        return pid

    docset = args.out / "nLab.docset"
    documents = docset / "Contents" / "Resources" / "Documents"
    if docset.exists():
        shutil.rmtree(docset)
    (documents / "pages").mkdir(parents=True)
    shutil.copytree(args.assets, documents / "assets")

    # The live site serves XHTML, where DOM tagName is lowercase; our pages
    # are parsed as HTML, where it is uppercase. Patch the section-vs-theorem
    # test in thm_numbering.js accordingly.
    thm = documents / "assets" / "thm_numbering.js"
    thm.write_text(thm.read_text(encoding="utf-8").replace(
        'tag.tagName == "h2"', 'tag.tagName.toLowerCase() == "h2"'),
        encoding="utf-8")

    # Docset-specific tweaks on top of the vendored nLab styles.
    (documents / "assets" / "nlab-dash.css").write_text(
        "/* Commutative diagrams (tikz SVG exports) are sized for a dense\n"
        "   web page; scale them up for comfortable reading in Dash. */\n"
        'div[style*="text-align: center"] > svg { zoom: 1.5; }\n',
        encoding="utf-8")

    css_links = "\n".join(
        f'<link href="../assets/{f}" media="all" rel="stylesheet" '
        'type="text/css" />'
        for f in ("instiki.css", "mathematics.css", "syntax.css", "nlab.css",
                  "nlab-dash.css"))
    js_links = "\n".join(
        f'<script src="../assets/{f}" type="text/javascript"></script>'
        for f in KEEP_SCRIPTS)

    print("transforming pages ...")
    written = 0
    for page_id, page_dir in iter_pages(args.html):
        if included is not None and page_id not in included:
            continue
        src = page_dir / "content.html"
        if not src.is_file():
            continue
        text = src.read_text(encoding="utf-8", errors="surrogateescape")
        text = transform(text, resolve, css_links, js_links)
        (documents / "pages" / f"{page_id}.html").write_text(
            text, encoding="utf-8", errors="surrogateescape")
        written += 1
        if written % 2000 == 0:
            print(f"  {written} pages")
    print(f"{written} pages written")

    # Canonical pages under their own name; redirect aliases searchable under
    # the alias but annotated Wikipedia-style as "-> canonical" via Dash's
    # dash_entry_menuDescription path metadata.
    types: dict[str, str | None] = {}
    entries = []
    for name, pid in names.items():
        types[pid] = typ = classify(name, categories.get(pid, set()))
        if typ and (included is None or pid in included):
            entries.append((name, typ, f"pages/{pid}.html"))
    canonical = {pid: name for name, pid in names.items()}
    for alias, pid in redirects.items():
        typ = types.get(pid)
        if not typ or (included is not None and pid not in included):
            continue
        target = canonical.get(pid, "").replace("<", "").replace(">", "")
        entries.append((
            alias, typ,
            f"<dash_entry_menuDescription=→ {target}>pages/{pid}.html"))
    n = build_index(docset / "Contents" / "Resources" / "docSet.dsidx", entries)
    print(f"indexed {n} entries")

    home = names.get("HomePage")
    if included is not None and home not in included:
        home = min(included)
    plist = {
        "CFBundleIdentifier": "nlab",
        "CFBundleName": "nLab",
        "DocSetPlatformFamily": "nlab",
        "isDashDocset": True,
        "isJavaScriptEnabled": True,
        "DashDocSetFallbackURL": "https://ncatlab.org/nlab/show/",
    }
    if home:
        plist["dashIndexFilePath"] = f"pages/{home}.html"
    with open(docset / "Contents" / "Info.plist", "wb") as f:
        plistlib.dump(plist, f)

    here = Path(__file__).resolve().parent
    for icon in ("icon.png", "icon@2x.png"):
        src = here / "icons" / icon
        if src.exists():
            shutil.copy(src, docset / icon)

    print(f"docset: {docset}")


if __name__ == "__main__":
    main()
