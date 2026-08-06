# ncatlab-dash

Workflow for building a [Dash](https://kapeli.com/dash) docset for the
[nLab](https://ncatlab.org), the collaborative wiki on category theory, higher
category theory, homotopy theory, and mathematical physics.

This repo contains only the **recipe** — the nLab's content is downloaded at
build time from the official file-based mirrors that the nLab project itself
publishes on GitHub:

- [ncatlab/nlab-content-html](https://github.com/ncatlab/nlab-content-html) —
  every wiki page as rendered XHTML+MathML, laid out as
  `pages/<d>/<d>/<d>/<d>/<id>/{content.html,name,revision_id}` where the shard
  path is the page id's decimal digits, least significant first.
- [ncatlab/nlab-content](https://github.com/ncatlab/nlab-content) — the same
  pages in Markdown+itex source form; used only to harvest
  `[[!redirects ...]]` directives, which become search aliases in the docset
  index.

Math is served as MathML, which Dash's WebKit renders natively — no MathJax
needed. Theorem/section numbering is done by the nLab's own
`thm_numbering.js`, which is vendored into the docset (with `prototype.js`,
which it depends on) along with the site stylesheets.

## Usage

```sh
./build.sh              # downloads mirrors (~1 GB) on first run, then packages
./build.sh --refresh    # re-download mirrors and assets to pick up new content
```

This produces `dist/nLab.docset`. Install it by double-clicking, or via Dash →
Settings → Docsets → `+` → Add Local Docset.

## What the recipe does

1. Downloads tarballs of the two content mirrors into `build/` (cached), and
   fetches the CSS/JS assets from ncatlab.org into `build/assets/`.
2. `make_docset.py` then:
   - builds a page-name → page-id map from the `name` files, plus a
     redirect-alias map from the sources;
   - writes each page to `Documents/pages/<id>.html`, rewriting wiki links
     (`/nlab/show/<name>`) to relative `<id>.html` links via the map;
   - strips the wiki chrome (nav bars, search form, edit/history/cite links)
     and the CDN webfont; unresolvable links (page uploads, missing pages,
     wiki actions) are pointed at `https://ncatlab.org` so they work online;
   - injects Dash `dashAnchor` section anchors at `h2`/`h3` headings so
     Dash's in-page table of contents works;
   - builds the SQLite search index (`docSet.dsidx`) with one `Entry` per
     page name and per redirect alias, and writes `Info.plist` and the icon
     (derived from the nLab logo SVG that ships in every page header).

## Caveats

- Uploaded files (images/PDFs referenced via `/nlab/files/...`) are not in the
  mirrors; those links point at the live site instead.
- The mirrors are updated by the nLab's export job; rerun with `--refresh` to
  pick up new content.
