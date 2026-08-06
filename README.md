# ncatlab-dash

A recipe for turning the [nLab](https://ncatlab.org) — the collaborative wiki
on category theory, higher category theory, homotopy theory, type theory, and
mathematical physics — into an offline, instantly-searchable **Dash docset**.

[Dash](https://kapeli.com/dash) is a macOS documentation browser: it stores
"docsets" (self-contained bundles of HTML plus a SQLite search index) fully
offline and provides as-you-type fuzzy search across everything you have
installed. Docsets are an open format, so they work beyond Dash itself — in
particular [Bolt](https://github.com/BoltDocs), a free open-source docset
browser for iOS/iPadOS, can load the same bundle, giving you the entire nLab
on an iPhone or iPad with no network connection.

This repository contains **only the recipe**. The nLab's content is fetched at
build time from the official file-based mirrors that the nLab project itself
publishes:

- [ncatlab/nlab-content-html](https://github.com/ncatlab/nlab-content-html) —
  every wiki page as rendered XHTML+MathML, laid out as
  `pages/<d>/<d>/<d>/<d>/<id>/{content.html,name,revision_id}`, where the
  shard path is the page id's decimal digits, least significant first
  (page 10004 lives at `pages/4/0/0/0/10004/`).
- [ncatlab/nlab-content](https://github.com/ncatlab/nlab-content) — the same
  pages as Markdown+itex source, used to harvest `[[!redirects ...]]`
  directives (search aliases) and `category:` tags (page classification).
- CSS/JS assets fetched from ncatlab.org (page styles, `thm_numbering.js` and
  its `prototype.js` dependency, `page_helper.js`).

## What the recipe adds

Beyond mechanically repackaging ~20,700 pages, `make_docset.py` does a fair
amount of curation:

- **Native math, no MathJax.** nLab pages ship MathML, which the WebKit view
  inside Dash renders natively. The nLab's own theorem/section numbering
  scripts are vendored so propositions read "Proposition 3.4." with proof
  tombstones, exactly like the live site.
- **Working offline hyperlinks.** Wiki links (`/nlab/show/<name>`) are
  resolved through a name→page-id map to relative links inside the docset;
  anything unresolvable (uploads, missing pages, wiki actions) points at the
  live site instead. Editing chrome (nav bars, search form, edit/history
  links) is stripped.
- **A typed index from the nLab's own ontology.** `category:` tags and
  page-name conventions classify pages as `Person` (~6,000 biography pages),
  `Resource` (books/papers), `Category` (floating tables of contents),
  `Guide` (expository series like *geometry of physics*), with concepts as
  `Entry`. Archived "… > history" subpages, blanked "empty NNN" placeholders,
  and Sandbox pages stay out of the index.
- **Wikipedia-style redirects.** Every nLab redirect stays searchable under
  its own name but displays as "alias → canonical name" (via Dash's
  `dash_entry_menuDescription` path metadata) and opens the canonical page —
  aliases are index rows, never duplicated pages.
- **Aggressive alias de-noising.** Of ~55,000 raw redirect names, more than
  half are spelling variants, and the build collapses them: case, dash-style
  (`--` vs en/em-dash), hyphen-vs-space, `∞` vs "infinity", plural
  inflections (toposes *and* topoi, spectra, simplices), escaped-apostrophe
  residue, people initials ("A. A. Markov Jr"), non-Latin-script names, and
  one-letter transliteration wobbles (Andrei/Andrej/Andrey). Genuinely
  different names survive ("Bill Lawvere", "Souslin", unaccented
  "Poincare"). Pages that declare combinatorial alias walls (>20 aliases)
  additionally drop names that only add one qualifier word to an
  already-indexed name.
- **Slim pages.** The per-page boilerplate the wiki ships 20,000× over —
  identical inline styles/scripts, the inlined logo SVG, hidden TeX
  `<annotation>` sources — is deduplicated into shared assets or dropped,
  cutting the docset from 730 MB to ~530 MB with pixel-identical rendering.
- **Dash niceties.** In-page table-of-contents anchors at every section
  heading, commutative diagrams scaled 1.5× for comfortable reading, docset
  icon derived from the nLab logo, and a fallback URL so entries can be
  opened on ncatlab.org.

The result: ~46,000 searchable names over 20,146 indexed pages.

## Building it yourself

Requirements: macOS or Linux with `curl`, `tar`, and Python 3 (stdlib only).

```sh
./build.sh              # first run downloads the mirrors (~150 MB archives,
                        # ~1.5 GB unpacked into build/), then packages
./build.sh --refresh    # re-download mirrors and assets to pick up new content
```

This produces `dist/nLab.docset` (~530 MB). Install it by double-clicking, or
via Dash → Settings → Docsets → `+` → Add Local Docset. Dash spends a few
minutes full-text-indexing on first install. Search with the `nlab:` keyword
(e.g. `nlab:yoneda`).

For iterating on the pipeline there is a fast path that builds a small docset
from named pages only:

```sh
python3 make_docset.py \
    --html build/nlab-content-html --source build/nlab-content \
    --assets build/assets --out dist-mini \
    --only "pullback,Yoneda lemma,adjoint functor"
```

## Caveats

- Uploaded files (images/PDFs under `/nlab/files/...`) are not part of the
  mirrors; those links point at the live site.
- The mirrors track the wiki with some lag; rerun with `--refresh` to update.
- A couple of oddities are upstream wiki content, not build artifacts (e.g.
  empty theorem cross-references on a few pages, reproducible on
  ncatlab.org).
