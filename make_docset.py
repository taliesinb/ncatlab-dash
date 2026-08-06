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

DASH_RUN_RE = re.compile(r"[-‐‒–—―]+")
# Suffix -> singular replacement, applied to a normalized name to generate
# candidate base forms (topoi -> topos, categories -> category, spectra ->
# spectrum, simplices -> simplex, bases -> basis, monoids -> monoid, ...).
SINGULAR_RULES = [("sses", "ss"), ("ies", "y"), ("ices", "ex"),
                  ("ices", "ix"), ("oi", "os"), ("a", "um"), ("a", "on"),
                  ("es", "is"), ("es", ""), ("s", "")]


def norm_key(name: str) -> str:
    """Case-, dash-, whitespace-, and infinity-symbol-insensitive form."""
    key = name.replace("∞", "infinity")
    key = key.replace("/'", "'").replace("\\'", "'")  # escaped-quote residue
    key = DASH_RUN_RE.sub("-", key)
    return re.sub(r"\s+", " ", key).strip().lower().lstrip("+*# ")


def name_variants(name: str) -> set[str]:
    """Normalized forms under which a name is considered redundant: case,
    dash style, whitespace, the infinity symbol, and plural inflections."""
    key = norm_key(name)
    variants = {key}
    for suffix, singular in SINGULAR_RULES:
        if key.endswith(suffix):
            variants.add(key[: len(key) - len(suffix)] + singular)
    # Hyphenation is not a meaningful distinction ("J-rule" vs "J rule").
    variants |= {v.replace("-", " ") for v in variants}
    return variants


def person_tokens(name: str) -> list[str]:
    """Words of a person name, dots/commas ignored: 'A. A. Markov Jr.' ->
    ['a', 'a', 'markov', 'jr']."""
    return norm_key(re.sub(r"[.,]", " ", name)).split()


def edit_distance_le_1(a: str, b: str) -> bool:
    if abs(len(a) - len(b)) > 1:
        return False
    i = 0
    while i < min(len(a), len(b)) and a[i] == b[i]:
        i += 1
    if len(a) == len(b):
        return a[i + 1:] == b[i + 1:]
    if len(a) < len(b):
        a, b = b, a
    return a[i + 1:] == b[i:]


def person_token_known(token: str, known: set[str]) -> bool:
    """A name word adds nothing if it is a bare initial, already known, or
    a one-letter transliteration wobble of a known word ('andrej'/'andrey',
    'andreievich'/'andreevich'). The shared-prefix requirement keeps real
    variant surnames distinct ('souslin' vs 'suslin'), and non-ASCII words
    are never folded ('poincaré' vs 'poincare' stays searchable)."""
    if len(token) == 1 or token in known:
        return True
    if not token.isascii() or len(token) < 5:
        return False
    return any(
        k.isascii() and len(k) >= 5 and token[:4] == k[:4]
        and edit_distance_le_1(token, k)
        for k in known)


def trim_aliases(canonical_name: str, aliases: list[str],
                 person: bool = False) -> list[str]:
    """Drop aliases that are mere spelling variants (case, dashes, plurals)
    of the canonical name or of an already-kept alias. All names here refer
    to the same page, so a variant-collision is always a true redundancy.
    ASCII, singular, shorter spellings are preferred as the kept
    representative: an alias whose plural-reduced variants hit another
    candidate's key is itself the inflected form, so it is processed after
    the base form it reduces to."""
    all_keys = {norm_key(canonical_name)} | {norm_key(a) for a in aliases}

    def inflected(alias: str) -> bool:
        return bool((name_variants(alias) - {norm_key(alias)}) & all_keys)

    kept = []
    seen = name_variants(canonical_name)
    known = set(person_tokens(canonical_name))
    pseen = {" ".join(person_tokens(canonical_name))}
    for alias in sorted(
            aliases, key=lambda a: (not a.isascii(), inflected(a), len(a), a)):
        if person:
            if any(c.isalpha() and ord(c) > 0x24F for c in alias):
                continue  # non-Latin script; nobody types this into Dash
            tokens = person_tokens(alias)
            pkey = " ".join(tokens)
            if pkey in pseen:
                continue  # punctuation/initial-style variant of a kept name
            if all(person_token_known(t, known) for t in tokens):
                continue  # only initials and (near-)known words
            pseen.add(pkey)
            known.update(tokens)
        variants = name_variants(alias)
        if variants & seen:
            continue
        seen |= variants
        kept.append(alias)
    return kept


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
    if "empty" in categories and re.fullmatch(r"empty ?\d+", name):
        return None  # blanked/deleted placeholder pages
    if "sandbox" in name.lower():
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


TITLE_RE = re.compile(r"<title>\s*(.*?)(?:\s+in nLab)?\s*</title>", re.S)
HEAD_RE = re.compile(r"<head>.*?</head>", re.S)
STYLE_RE = re.compile(r"<style[^>]*>(.*?)</style>", re.S)
INLINE_SCRIPT_RE = re.compile(
    r'<script type="text/javascript">(.*?)</script>', re.S)
CDATA_RE = re.compile(r"<!--/\*-->|<!\[CDATA\[/?\*?>?<!--\*/"
                      r"|/\*\]\]>\*/-->|<!--//-->|<!\[CDATA\[//><!--"
                      r"|//--><!\]\]>")
NAV_RE = re.compile(r'<div class="navigation( navfoot)?">.*?</div>', re.S)
# The <img> keeps the original span wrapper: the svg's em-based size
# resolves against the span's (smaller) font-size, not the h1's.
LOGO_RE = re.compile(
    r'(<span style="float: left[^"]*">)\s*<svg.*?</svg>\s*(</span>)', re.S)
LOGO_IMG = (r'\1<img src="../assets/logo.svg" alt="" '
            r'style="width: 1.872em; height: 1.8em" />\2')
TEX_ANNOTATION_RE = re.compile(
    r'<annotation encoding="application/x-tex">.*?</annotation>', re.S)
SHOW_LINK_RE = re.compile(r'href="/nlab/show/([^"#]*)(#[^"]*)?"')
ABS_LINK_RE = re.compile(r'(href|src)="(/[^/"][^"]*)"')
HEADING_RE = re.compile(r"<h([23])( [^>]*)?>(.*?)</h\1>", re.S)
TAG_RE = re.compile(r"<[^>]+>")


def extract_head_assets(text: str, assets_dir: Path) -> None:
    """Every page ships the same several KB of inline styles and scripts in
    its <head>; extract them once from a sample page into shared asset files
    so the per-page head can be a few lines. (The text/x-mathjax-config
    block is dropped: it only matters when MathML is unsupported and the
    CDN MathJax fallback loads, which never happens offline in Dash.)"""
    m = HEAD_RE.search(text)
    if not m:
        sys.exit("error: could not find <head> in sample page")
    head = m.group(0)
    css = "\n".join(CDATA_RE.sub("", s).strip() for s in STYLE_RE.findall(head))
    (assets_dir / "nlab-head.css").write_text(css, encoding="utf-8")
    js = "\n".join(CDATA_RE.sub("", s).strip()
                   for s in INLINE_SCRIPT_RE.findall(head))
    (assets_dir / "nlab-head.js").write_text(js, encoding="utf-8")


def transform(text: str, resolve, css_links: str, js_links: str) -> str:
    # Replace the whole boilerplate <head> (identical across pages except
    # the title) with a minimal one referencing the shared assets.
    m = TITLE_RE.search(text)
    title = re.sub(r"\s+", " ", m.group(1)).strip() if m else "nLab"
    new_head = (
        "<head>\n"
        f"<title>{title}</title>\n"
        '<meta http-equiv="Content-Type" content="text/html; charset=UTF-8" />\n'
        '<meta name="viewport" content="width=device-width, initial-scale=1" />\n'
        f"{css_links}\n{js_links}\n</head>")
    text = HEAD_RE.sub(lambda _: new_head, text, count=1)

    # The nLab logo SVG is inlined into every page header; share it instead.
    text = LOGO_RE.sub(LOGO_IMG, text, count=1)

    # Hidden TeX source of every formula (used only by a double-click-to-
    # view-TeX popup that Dash's web view cannot open anyway).
    text = TEX_ANNOTATION_RE.sub("", text)

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

    here = Path(__file__).resolve().parent
    shutil.copy(here / "icons" / "logo.svg", documents / "assets" / "logo.svg")

    css_links = "\n".join(
        f'<link href="../assets/{f}" media="all" rel="stylesheet" '
        'type="text/css" />'
        for f in ("instiki.css", "mathematics.css", "syntax.css", "nlab.css",
                  "nlab-head.css", "nlab-dash.css"))
    js_links = "\n".join(
        f'<script src="../assets/{f}" type="text/javascript"></script>'
        for f in KEEP_SCRIPTS + ("nlab-head.js",))

    print("transforming pages ...")
    written = 0
    for page_id, page_dir in iter_pages(args.html):
        if included is not None and page_id not in included:
            continue
        src = page_dir / "content.html"
        if not src.is_file():
            continue
        text = src.read_text(encoding="utf-8", errors="surrogateescape")
        if written == 0:
            extract_head_assets(text, documents / "assets")
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
    by_page: dict[str, list[str]] = {}
    for alias, pid in redirects.items():
        by_page.setdefault(pid, []).append(alias)
    total_aliases = trimmed = 0
    for pid, aliases in by_page.items():
        typ = types.get(pid)
        if not typ or (included is not None and pid not in included):
            continue
        total_aliases += len(aliases)
        kept = trim_aliases(canonical.get(pid, ""), aliases,
                            person=typ == "Person")
        trimmed += len(aliases) - len(kept)
        target = canonical.get(pid, "").replace("<", "").replace(">", "")
        for alias in kept:
            entries.append((
                alias, typ,
                f"<dash_entry_menuDescription=→ {target}>pages/{pid}.html"))
    print(f"aliases: {total_aliases - trimmed} kept, "
          f"{trimmed} trimmed as spelling/plural variants")
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
