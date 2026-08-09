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

fn extract_math(src: &str, stash: &mut Stash) -> String {
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
                out.push_str(&stash.put(math_to_typst(&tex, display)));
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
    if display {
        if let Some(body) = emit::emit_formula_body(tex) {
            return format!(
                "#align(center, block(breakable: false)[\n{}])",
                body.trim_end()
            );
        }
        // plain display math (arrays inside become matrices)
        format!("#{}", emit::emit_equation_pub(tex).trim_end())
    } else {
        format!("#mi({})", emit::ts(tex))
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
            let label = id
                .map(|i| format!("#metadata(none)#label(\"{}\") ", i))
                .unwrap_or_default();
            out.push(stash.put(format!("#nlab-env(\"{}\", \"{}\")[{}", kind, style, label)));
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
                if let Some(anchor) = dest_url.strip_prefix('#') {
                    out.push_str(&format!("#link(label(\"{}\"))[", anchor));
                } else {
                    out.push_str(&format!("#link(\"{}\")[", dest_url));
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

const PAGE_PREAMBLE: &str = r##"#import "@local/mitex:0.2.7": mi-itex, mitex-itex
#import "@preview/fletcher:0.5.8": diagram, node, edge
#set page(width: 17cm, height: auto, margin: 1.6cm, fill: white)
#set text(10.5pt)
#set heading(numbering: "1.")
#show link: set text(fill: rgb("#1a6318"))
#let nlab-count = counter("nlab-env")
#show heading.where(level: 1): it => { nlab-count.update(0); it }
#let nlab-env(kind, style, body) = {
  if style == "num" {
    nlab-count.step()
    block(inset: (left: 0.6em), above: 1em, below: 1em)[
      #strong[#kind #context { counter(heading).get().at(0, default: 0) }.#context { nlab-count.get().first() }.] #body]
  } else if style == "proof" {
    block(inset: (left: 0.6em), above: 1em, below: 1em)[_Proof._ #body #h(1fr) $qed$]
  } else {
    block(above: 1em, below: 1em, body)
  }
}
"##;

pub(crate) fn page_to_typst(src: &str) -> String {
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
    let src = extract_math(&src, &mut stash);
    let src = extract_fences(&src, &mut stash);
    let src = extract_wiki(&src, &mut stash);
    // Maruku anchor IALs: {#Id} standalone or inline
    let anchor_re = regex::Regex::new(r"\{#([A-Za-z0-9:_-]+)\}").unwrap();
    let src = anchor_re
        .replace_all(&src, |c: &regex::Captures| {
            stash.put(format!("#metadata(none)#label(\"{}\")", &c[1]))
        })
        .to_string();
    let body = markdown_to_typst(&src);
    // resolve placeholders (escape pass never touches PUA chars)
    let re = regex::Regex::new(&format!("{}(\\d+){}", P0, P1)).unwrap();
    let body = re
        .replace_all(&body, |c: &regex::Captures| {
            stash.items[c[1].parse::<usize>().unwrap()].clone()
        })
        .to_string();
    emit::localize_calls(format!("{}\n{}\n", PAGE_PREAMBLE, body))
}
