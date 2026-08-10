//! Instiki (nLab-flavored Markdown) -> typst markup.
//!
//! Order matters: itex math is extracted before Markdown parsing (its
//! underscores would read as emphasis), along with wiki links, includes,
//! and the `+--{: .class}` / `=--` environment fences, all stashed
//! behind private-use-area placeholders; pulldown-cmark handles the
//! remaining standard Markdown; a final pass resolves placeholders into
//! typst (math through mi-itex / the diagram emitters, links to
//! ncatlab.org, fences into nlab-env blocks).

use crate::emit;
use crate::tikzcd;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

const P0: char = '\u{E000}'; // placeholder delimiters
const P1: char = '\u{E001}';

pub(crate) struct Stash {
    items: Vec<String>, // resolved typst fragments
}

impl Stash {
    fn put(&mut self, typst: String) -> String {
        self.items.push(typst);
        format!("{}{}{}", P0, self.items.len() - 1, P1)
    }
}

// ------------------------------------------------------------ pre-pass

/// Pull `\label{Id}` out of a math body; returns (tex, typst anchors).
fn split_math_labels(tex: &str) -> (String, String) {
    let re = regex::Regex::new(r"\\label\{([A-Za-z0-9:_.-]+)\}").unwrap();
    let mut anchors = String::new();
    let tex = re
        .replace_all(tex, |c: &regex::Captures| {
            anchors.push_str(&format!("#metadata(none)#label(\"{}\")", &c[1]));
            String::new()
        })
        .to_string();
    (tex, anchors)
}

fn extract_math(src: &str, stash: &mut Stash) -> String {
    // \[ ... \] display math first (chars, so the $-scanner never sees it)
    let bracket_re = regex::Regex::new(r"(?s)\\\[(.*?)\\\]").unwrap();
    let src = bracket_re
        .replace_all(src, |c: &regex::Captures| {
            let (tex, anchors) = split_math_labels(&c[1]);
            stash.put(format!("{}{}", math_to_typst(&tex, true), anchors))
        })
        .to_string();
    let mut out = String::new();
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == '$' {
            let display = i + 1 < b.len() && b[i + 1] == '$';
            let open = if display { 2 } else { 1 };
            // find closing delimiter
            let mut j = i + open;
            let mut found = None;
            while j < b.len() {
                if b[j] == '$' && b[j - 1] != '\\' {
                    if display {
                        if j + 1 < b.len() && b[j + 1] == '$' {
                            found = Some(j);
                            break;
                        }
                    } else {
                        found = Some(j);
                        break;
                    }
                }
                j += 1;
            }
            if let Some(j) = found {
                let tex: String = b[i + open..j].iter().collect();
                let (tex, anchors) = split_math_labels(&tex);
                out.push_str(
                    &stash.put(format!("{}{}", math_to_typst(&tex, display), anchors)),
                );
                i = j + open;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn math_to_typst(tex: &str, display: bool) -> String {
    let tex = &emit::fix_itex_builtins(tex);
    if display {
        if let Some(body) = emit::emit_formula_body(tex) {
            return format!(
                "#align(center, block(breakable: false)[\n{}])",
                body.trim_end()
            );
        }
        // plain display math (arrays inside become matrices)
        // emit_equation already includes the leading `#`
        emit::emit_equation_pub(tex).trim_end().to_string()
    } else {
        format!("#mi({})", emit::ts(tex))
    }
}

/// Inline `<img>`/`<figure>` HTML referencing the nLab file store.
/// Images already present in the local cache (NLAB_FILES_ROOT, default
/// build/nlab-files next to the content mirror) embed as #figure/#image
/// with a `files/<name>` path; otherwise they degrade to a link.
fn extract_images(src: &str, stash: &mut Stash) -> String {
    // presentation wrappers around figures would make pulldown-cmark
    // swallow the whole block (with our placeholder) as raw HTML
    let src = regex::Regex::new(r"(?m)^\s*</?(center|div)[^>]*>\s*$")
        .unwrap()
        .replace_all(src, "")
        .to_string();
    let src = src.as_str();
    let fig_re = regex::Regex::new(
        r#"(?s)<figure[^>]*>\s*.*?<img[^>]*src="([^"]+)"[^>]*>.*?(?:<figcaption[^>]*>(.*?)</figcaption>)?\s*</figure>"#,
    )
    .unwrap();
    let src = fig_re
        .replace_all(src, |c: &regex::Captures| {
            let cap = c.get(2).map(|m| m.as_str()).unwrap_or("");
            stash.put(image_typst(&c[1], cap))
        })
        .to_string();
    let img_re = regex::Regex::new(r#"<img[^>]*src="([^"]+)"[^>]*/?>"#).unwrap();
    img_re
        .replace_all(&src, |c: &regex::Captures| stash.put(image_typst(&c[1], "")))
        .to_string()
}

fn files_root() -> std::path::PathBuf {
    std::env::var("NLAB_FILES_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("build/nlab-files"))
}

fn image_typst(url: &str, caption_html: &str) -> String {
    let caption = escape_typst(
        &regex::Regex::new(r"<[^>]+>")
            .unwrap()
            .replace_all(caption_html, "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    );
    // typst's #figure supplies its own "Figure N:" prefix
    let caption = regex::Regex::new(r"^Figure \d+\s*[:.]\s*")
        .unwrap()
        .replace(&caption, "")
        .to_string();
    let name = url.rsplit('/').next().unwrap_or(url);
    let cached = url.contains("/nlab/files/") && files_root().join(name).exists();
    if cached {
        let img = format!("image(\"files/{}\", width: 74%)", name);
        if caption.is_empty() {
            format!("#align(center, {})", img)
        } else {
            format!("#figure({}, caption: [{}])", img, caption)
        }
    } else {
        let text = if caption.is_empty() {
            "(figure)".to_string()
        } else {
            caption
        };
        format!("#link(\"{}\")[{}]", url, text)
    }
}

fn wiki_target_url(name: &str) -> String {
    let mut enc = String::new();
    for c in name.trim().chars() {
        match c {
            ' ' => enc.push('+'),
            c if c.is_ascii_alphanumeric() || "+-_.".contains(c) => enc.push(c),
            c => {
                let mut buf = [0u8; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    enc.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    format!("https://ncatlab.org/nlab/show/{}", enc)
}

fn extract_wiki(src: &str, stash: &mut Stash) -> String {
    let re = regex::Regex::new(r"\[\[([^\[\]]+)\]\]").unwrap();
    re.replace_all(src, |c: &regex::Captures| {
        let inner = &c[1];
        if let Some(rest) = inner.strip_prefix('!') {
            // directives: skip includes/redirects in the page body
            let _ = rest;
            return String::new();
        }
        let (target, text) = match inner.split_once('|') {
            Some((t, x)) => (t, x),
            None => (inner, inner),
        };
        // [[Name.pdf:file]] links to an uploaded file, not a wiki page
        if let Some(file) = target.strip_suffix(":file") {
            let text = text.strip_suffix(":file").unwrap_or(text);
            return stash.put(format!(
                "#link(\"https://ncatlab.org/nlab/files/{}\")[{}]",
                file.trim().replace(' ', "+"),
                escape_typst(text)
            ));
        }
        stash.put(format!(
            "#link(\"{}\")[{}]",
            wiki_target_url(target),
            escape_typst(text)
        ))
    })
    .to_string()
}

/// `+-- {: .num_defn #id}` ... `=--` fences (nestable) into env markers.
fn extract_fences(src: &str, stash: &mut Stash) -> String {
    let open_re =
        regex::Regex::new(r"^\+--\s*\{:\s*([^}]*)\}\s*$").unwrap();
    let mut out: Vec<String> = Vec::new();
    let mut skip_depth = 0usize; // inside a dropped block (.rightHandSide)
    let mut open_stack: Vec<bool> = Vec::new(); // true = emitted env
    for line in src.lines() {
        let t = line.trim();
        if let Some(c) = open_re.captures(t) {
            let attrs = c[1].to_string();
            if skip_depth > 0 {
                skip_depth += 1;
                continue;
            }
            if attrs.contains(".rightHandSide") || attrs.contains(".toc") {
                skip_depth = 1;
                continue;
            }
            let id = attrs
                .split_whitespace()
                .find(|w| w.starts_with('#'))
                .map(|w| w[1..].to_string());
            let class = attrs
                .split_whitespace()
                .find(|w| w.starts_with('.'))
                .map(|w| w[1..].to_string())
                .unwrap_or_default();
            let (kind, style) = env_kind(&class);
            let id_arg = id
                .map(|i| format!(", id: \"{}\"", i))
                .unwrap_or_default();
            out.push(stash.put(format!("#nlab-env(\"{}\", \"{}\"{})[", kind, style, id_arg)));
            open_stack.push(true);
            continue;
        }
        if t == "=--" {
            if skip_depth > 0 {
                skip_depth -= 1;
                continue;
            }
            if open_stack.pop().unwrap_or(false) {
                out.push(stash.put("]".to_string()));
            }
            continue;
        }
        if skip_depth > 0 {
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

fn env_kind(class: &str) -> (&'static str, &'static str) {
    match class {
        "num_defn" | "un_defn" => ("Definition", "num"),
        "num_theorem" | "un_theorem" => ("Theorem", "num"),
        "num_lemma" | "un_lemma" => ("Lemma", "num"),
        "num_prop" | "un_prop" => ("Proposition", "num"),
        "num_cor" | "un_cor" => ("Corollary", "num"),
        "num_remark" | "un_remark" => ("Remark", "num"),
        "num_example" | "un_example" => ("Example", "num"),
        "num_note" | "un_note" => ("Note", "num"),
        "proof" => ("Proof", "proof"),
        _ => ("", "plain"),
    }
}

// -------------------------------------------------------- markdown pass

fn escape_typst(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '#' | '*' | '_' | '`' | '$' | '[' | ']' | '<' | '>' | '@' | '\\' | '\'' | '"' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

fn heading_eq(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 1,
        HeadingLevel::H3 => 2,
        HeadingLevel::H4 => 3,
        HeadingLevel::H5 => 4,
        HeadingLevel::H6 => 5,
    }
}

fn markdown_to_typst(src: &str) -> String {
    let parser = Parser::new_ext(src, Options::ENABLE_TABLES);
    let mut out = String::new();
    let mut skip_heading = false; // h6 titles inside envs are dropped
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut item_depth = 0usize;
    for ev in parser {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                if level == HeadingLevel::H6 {
                    skip_heading = true;
                } else {
                    out.push_str(&format!("\n{} ", "=".repeat(heading_eq(level))));
                }
            }
            Event::End(TagEnd::Heading(level)) => {
                if level == HeadingLevel::H6 {
                    skip_heading = false;
                } else {
                    out.push('\n');
                }
            }
            Event::Start(Tag::Paragraph) => {
                // paragraphs inside a list item stay in the item
                if item_depth == 0 {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if item_depth == 0 {
                    out.push('\n');
                }
            }
            Event::Start(Tag::Emphasis) => out.push('_'),
            Event::End(TagEnd::Emphasis) => out.push('_'),
            Event::Start(Tag::Strong) => out.push('*'),
            Event::End(TagEnd::Strong) => out.push('*'),
            Event::Start(Tag::List(start)) => list_stack.push(start),
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                out.push('\n');
            }
            Event::Start(Tag::Item) => {
                item_depth += 1;
                let marker = if matches!(list_stack.last(), Some(Some(_))) {
                    "+"
                } else {
                    "-"
                };
                out.push_str(&format!("\n{} ", marker));
            }
            Event::End(TagEnd::Item) => {
                item_depth = item_depth.saturating_sub(1);
            }
            Event::Start(Tag::BlockQuote(_)) => out.push_str("\n#quote(block: true)["),
            Event::End(TagEnd::BlockQuote(_)) => out.push_str("]\n"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                if dest_url.is_empty() {
                    out.push_str("#[");
                } else if let Some(anchor) = dest_url.strip_prefix('#') {
                    out.push_str(&format!("#nlab-anchor(\"{}\")[", anchor));
                } else {
                    let dest = dest_url.replace('\\', "%5C").replace('"', "%22");
                    out.push_str(&format!("#link(\"{}\")[", dest));
                }
            }
            Event::End(TagEnd::Link) => out.push(']'),
            Event::Start(Tag::CodeBlock(_)) => out.push_str("\n```\n"),
            Event::End(TagEnd::CodeBlock) => out.push_str("\n```\n"),
            Event::Code(c) => {
                out.push_str(&format!("`{}`", c));
            }
            Event::Text(t) => {
                if !skip_heading {
                    out.push_str(&escape_typst(&t));
                }
            }
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push_str(" \\\n"),
            Event::Rule => out.push_str("\n#line(length: 100%)\n"),
            _ => {}
        }
    }
    out
}

// ------------------------------------------------------------ assembly

const PAGE_PREAMBLE: &str = r##"#import "@local/mitex:0.2.7": mi-itex, mitex-itex, mitex-scope
#import "@preview/fletcher:0.5.8": diagram, node, edge
// natively-emitted math calls the same helpers the mitex plugin's eval
// scope provides; bind the ones the converter can reference
#let mitexdisplay = mitex-scope.at("mitexdisplay")
#let mitexinline = mitex-scope.at("mitexinline")
#let mitexmathbf = mitex-scope.at("mitexmathbf")
#let mitexupright = mitex-scope.at("mitexupright")
#let mitexitalic = mitex-scope.at("mitexitalic")
#let mitexsans = mitex-scope.at("mitexsans")
#let mitexfrak = mitex-scope.at("mitexfrak")
#let mitexmono = mitex-scope.at("mitexmono")
#let mitexsqrt = mitex-scope.at("mitexsqrt")
#let mitexset = mitex-scope.at("mitexset")
#let mitexnot = mitex-scope.at("mitexnot")
#let mitexscript = mitex-scope.at("mitexscript")
#let mitexsscript = mitex-scope.at("mitexsscript")
#let mitexarray = mitex-scope.at("mitexarray")
#let mitexlabel = mitex-scope.at("mitexlabel")
#let mitexoverbrace = mitex-scope.at("mitexoverbrace")
#let mitexunderbrace = mitex-scope.at("mitexunderbrace")
#let mitexoverbracket = mitex-scope.at("mitexoverbracket")
#let mitexunderbracket = mitex-scope.at("mitexunderbracket")
#let mitexcomment = mitex-scope.at("mitexcomment")
#let scope-or(name, fallback) = if name in mitex-scope { mitex-scope.at(name) } else { fallback }
#let xarrow = scope-or("xarrow", none)
#let operatorname = scope-or("operatorname", none)
#let operatornamewithlimits = scope-or("operatornamewithlimits", none)
#let underset = scope-or("underset", none)
#let overset = scope-or("overset", none)
#let underoverset = scope-or("underoverset", none)
#let stackrel = scope-or("stackrel", none)
#let textmath = scope-or("textmath", none)
#let big = scope-or("big", none)
#let Big = scope-or("Big", none)
#let bigg = scope-or("bigg", none)
#let Bigg = scope-or("Bigg", none)
#let aligned = scope-or("aligned", none)
#let matrix = scope-or("matrix", none)
#let pmatrix = scope-or("pmatrix", none)
#let bmatrix = scope-or("bmatrix", none)
#let Bmatrix = scope-or("Bmatrix", none)
#let vmatrix = scope-or("vmatrix", none)
#let Vmatrix = scope-or("Vmatrix", none)
#let smallmatrix = scope-or("smallmatrix", none)
#let substack = scope-or("substack", none)
#let cases = scope-or("cases", std.math.cases)
#let atop = scope-or("atop", none)
#let negthinspace = scope-or("negthinspace", none)
#let mitexcite = scope-or("mitexcite", none)
#let mitexcolor = scope-or("mitexcolor", none)
#let phantom = scope-or("phantom", none)
#let mathop = scope-or("mathop", none)
#let Set = scope-or("Set", none)
#let mathclap = scope-or("mathclap", none)
#let rlap = scope-or("rlap", none)
#let llap = scope-or("llap", none)
#let tfrac = scope-or("tfrac", none)
#let dfrac = scope-or("dfrac", none)
#let mathsf = scope-or("mathsf", none)
#let mathscr = scope-or("mathscr", none)
#let mathrm = scope-or("mathrm", none)
#let mathit = scope-or("mathit", none)
#set page(width: 17cm, height: 25cm, margin: 1.6cm, fill: white, numbering: "1")
#set text(10.5pt)
#set heading(numbering: "1.")
#show link: set text(fill: rgb("#1a6318"))
#show figure.caption: set text(0.85em)
#let nlab-count = counter("nlab-env")
#show heading.where(level: 1): it => { nlab-count.update(0); it }
#let nlab-env(kind, style, id: none, body) = {
  if style == "num" {
    nlab-count.step()
    block(inset: (left: 0.6em), above: 1em, below: 1em)[#context {
      let num = str(counter(heading).get().at(0, default: 0)) + "." + str(nlab-count.get().first())
      if id != none { [#metadata(num)#label(id)] }
      strong[#kind #num.]
    } #body]
  } else if style == "proof" {
    block(inset: (left: 0.6em), above: 1em, below: 1em)[_Proof._ #body #h(1fr) $qed$]
  } else {
    block(above: 1em, below: 1em, body)
  }
}
#let nlab-ref(id) = context {
  let ms = query(label(id))
  if ms.len() > 0 and ms.first().func() == metadata and ms.first().value != none {
    link(label(id), ms.first().value)
  } else if ms.len() > 0 {
    link(label(id))[(above)]
  } else {
    [(ref)]
  }
}
#let nlab-anchor(id, body) = context {
  if query(label(id)).len() > 0 { link(label(id), body) } else { body }
}
"##;

pub(crate) fn page_to_typst(src: &str, title: Option<&str>) -> String {
    let mut stash = Stash { items: Vec::new() };
    // strip trailing metadata lines
    let src: String = src
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.starts_with("category:") && !t.starts_with("[[!redirects")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let src = src.replace("\\tableofcontents", &stash.put("#outline(depth: 2)".into()));
    // raw tikzcd blocks (the live site renders these server-side)
    let tikz_re =
        regex::Regex::new(r"(?s)\\begin\{tikzcd\}.*?\\end\{tikzcd\}").unwrap();
    let src = tikz_re
        .replace_all(&src, |c: &regex::Captures| {
            match tikzcd::tikzcd_to_fletcher(&c[0]) {
                Ok((code, _)) => stash.put(format!(
                    "#align(center, block(breakable: false)[\n{}])",
                    code.trim_end()
                )),
                Err(_) => c[0].to_string(),
            }
        })
        .to_string();
    // raw LaTeX-style theorem environments in prose (as opposed to the
    // +-- fences), optionally carrying a \label{Id}
    const ENVS: &str =
        "proposition|theorem|lemma|corollary|definition|example|remark|note|proof";
    // one sequential pass so a stray \end (author error) is the one
    // dropped, not the document's final legitimate close
    let env_re = regex::Regex::new(&format!(
        r"\\(begin|end)\{{({})\}}(\s*\\label\{{([A-Za-z0-9:_.-]+)\}})?",
        ENVS
    ))
    .unwrap();
    let mut env_depth = 0i32;
    let src = env_re
        .replace_all(&src, |c: &regex::Captures| {
            if &c[1] == "end" {
                return if env_depth > 0 {
                    env_depth -= 1;
                    stash.put("]".into())
                } else {
                    String::new()
                };
            }
            env_depth += 1;
            let (kind, style) = match &c[2] {
                "proof" => ("Proof", "proof"),
                "proposition" => ("Proposition", "num"),
                "theorem" => ("Theorem", "num"),
                "lemma" => ("Lemma", "num"),
                "corollary" => ("Corollary", "num"),
                "definition" => ("Definition", "num"),
                "example" => ("Example", "num"),
                "remark" => ("Remark", "num"),
                _ => ("Note", "num"),
            };
            let id_arg = c
                .get(4)
                .map(|m| format!(", id: \"{}\"", m.as_str()))
                .unwrap_or_default();
            stash.put(format!("#nlab-env(\"{}\", \"{}\"{})[", kind, style, id_arg))
        })
        .to_string();
    // any still-unclosed environments get their closes at document end
    let mut src = src;
    for _ in 0..env_depth.max(0) {
        src = format!("{}\n{}", src, stash.put("]".into()));
    }
    let src = src.replace("\\linebreak", " ");
    // maruku table-of-contents list item ("* table of contents\n{:toc}",
    // "* automatic toc {: toc}", ...)
    let src = regex::Regex::new(r"(?m)^\*[^\n{]*\n?\{:\s*toc[^}]*\}\s*$")
        .unwrap()
        .replace_all(&src, |_: &regex::Captures| {
            stash.put("#outline(depth: 2)".into())
        })
        .to_string();
    // closed ATX headings without spaces (`#Contents#`) aren't CommonMark
    let src = regex::Regex::new(r"(?m)^(#+)\s*(.*?)\s*#+\s*$")
        .unwrap()
        .replace_all(&src, "$1 $2")
        .to_string();
    let src = extract_images(&src, &mut stash);
    let src = extract_math(&src, &mut stash);
    let src = extract_fences(&src, &mut stash);
    let src = extract_wiki(&src, &mut stash);
    // \ref{Id}: Maruku turns these into numbered links to environment
    // anchors (the live site fills the number with JavaScript; we query
    // the env's metadata statically).
    let ref_re = regex::Regex::new(r"\\ref\{([A-Za-z0-9:_-]+)\}").unwrap();
    let src = ref_re
        .replace_all(&src, |c: &regex::Captures| {
            stash.put(format!("#nlab-ref(\"{}\")", &c[1]))
        })
        .to_string();
    // an anchor at the end of a heading would ride into the heading
    // body and be duplicated by #outline(); move it below the heading
    let heading_anchor_re =
        regex::Regex::new(r"(?m)^(#+[^\n]*?)\s*\{#([A-Za-z0-9:_-]+)\}\s*$").unwrap();
    let src = heading_anchor_re
        .replace_all(&src, "$1\n\n{#$2}")
        .to_string();
    // Maruku anchor IALs: {#Id} standalone or inline; a repeated id
    // would be a duplicate typst label, so only the first one anchors
    let anchor_re = regex::Regex::new(r"\{#([A-Za-z0-9:_-]+)\}").unwrap();
    let mut seen_anchors = std::collections::HashSet::new();
    let src = anchor_re
        .replace_all(&src, |c: &regex::Captures| {
            if seen_anchors.insert(c[1].to_string()) {
                stash.put(format!("#metadata(none)#label(\"{}\")", &c[1]))
            } else {
                String::new()
            }
        })
        .to_string();
    let body = markdown_to_typst(&src);
    // resolve placeholders (escape pass never touches PUA chars); a
    // literal ( or [ right after a #call() would chain as another
    // argument list, so terminate the expression with `;` first
    let re = regex::Regex::new(&format!("{}(\\d+){}([\\(\\[])?", P0, P1)).unwrap();
    let body = re
        .replace_all(&body, |c: &regex::Captures| {
            let item = &stash.items[c[1].parse::<usize>().unwrap()];
            match c.get(2) {
                Some(next) if item.ends_with(')') || item.ends_with(']') => {
                    format!("{};{}", item, next.as_str())
                }
                Some(next) => format!("{}{}", item, next.as_str()),
                None => item.clone(),
            }
        })
        .to_string();
    let title_block = title
        .map(|t| {
            format!(
                "#align(center)[#text(1.7em, weight: 700)[{}]]\n#v(0.5em)\n",
                escape_typst(t)
            )
        })
        .unwrap_or_default();
    let body = emit::nativize_calls(&format!("{}{}\n", title_block, body));
    // typst labels must be unique: only the first attachment of an id
    // survives (anchors, env ids, and math \label{}s share a namespace)
    let mut seen = std::collections::HashSet::new();
    let dedupe_re = regex::Regex::new(
        r#"#metadata\(none\)#label\("([A-Za-z0-9:_.-]+)"\)|, id: "([A-Za-z0-9:_.-]+)""#,
    )
    .unwrap();
    let body = dedupe_re
        .replace_all(&body, |c: &regex::Captures| {
            let id = c.get(1).or(c.get(2)).unwrap().as_str().to_string();
            if seen.insert(id) {
                c[0].to_string()
            } else {
                String::new()
            }
        })
        .to_string();
    emit::localize_calls(format!("{}\n{}", PAGE_PREAMBLE, body))
}
