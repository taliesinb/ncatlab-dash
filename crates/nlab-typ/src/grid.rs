//! Phase 1 of the Rust migration: parse itex `\array{}` diagrams and
//! produce the same JSON cell grid as rewrites/parse_arrays.py, so the
//! two implementations can be diffed across the whole diagram corpus.
//!
//! The mitex CST replaces the bug-prone parts of the Python (top-level
//! command detection, i.e. Python's depth0_text); label/object text is
//! taken as source slices with faithful ports of the Python string
//! helpers, so a matching diagram produces byte-identical JSON.
//!
//! Modes:
//!   dump   pretty-print the mitex CST of stdin (debugging)
//!   grid   stdin: one TeX formula -> stdout: JSON grid (or status line)
//!   grids  stdin: \x00-separated formulas -> \x00-separated outputs,
//!          each "ok\x1f<json>" or "<status>\x1f"

use mitex_parser::syntax::SyntaxKind;
use mitex_spec_gen::DEFAULT_SPEC;

// ---------------------------------------------------------------- tables

const H_CMDS: &[(&str, &str)] = &[
    ("to", "r"),
    ("rightarrow", "r"),
    ("longrightarrow", "r"),
    ("Rightarrow", "r"),
    ("longmapsto", "r"),
    ("mapsto", "r"),
    ("hookrightarrow", "r"),
    ("twoheadrightarrow", "r"),
    ("rightharpoonup", "r"),
    ("leftarrow", "l"),
    ("longleftarrow", "l"),
    ("Leftarrow", "l"),
    ("hookleftarrow", "l"),
    ("twoheadleftarrow", "l"),
    ("rightrightarrows", "r"),
    ("leftleftarrows", "l"),
    ("leftrightarrow", "lr"),
    ("simeq", "~"),
    ("cong", "~"),
    ("equiv", "~"),
];
const V_CMDS: &[(&str, &str)] = &[
    ("downarrow", "d"),
    ("Downarrow", "d"),
    ("uparrow", "u"),
    ("Uparrow", "u"),
];
const D_CMDS: &[(&str, &str)] = &[
    ("searrow", "se"),
    ("swarrow", "sw"),
    ("nearrow", "ne"),
    ("nwarrow", "nw"),
    ("seArrow", "se"),
    ("swArrow", "sw"),
    ("neArrow", "ne"),
    ("nwArrow", "nw"),
];

fn h_dir(c: &str) -> Option<&'static str> {
    H_CMDS.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}
fn v_dir(c: &str) -> Option<&'static str> {
    V_CMDS.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}
fn d_dir(c: &str) -> Option<&'static str> {
    D_CMDS.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}
fn is_arrow(c: &str) -> bool {
    h_dir(c).is_some() || v_dir(c).is_some() || d_dir(c).is_some()
}

// ------------------------------------------------------------------ cell

/// A cell as an ordered list of JSON key/values, mirroring the insertion
/// order of the Python dicts so serialization matches byte-for-byte.
#[derive(Clone, Debug)]
pub(crate) enum Val {
    S(String),
    Parts(Vec<Cell>),
}
pub(crate) type Cell = Vec<(&'static str, Val)>;

pub(crate) fn cell(kind: &str) -> Cell {
    vec![("k", Val::S(kind.to_string()))]
}
pub(crate) fn get<'a>(c: &'a Cell, key: &str) -> Option<&'a str> {
    c.iter().find(|(k, _)| *k == key).and_then(|(_, v)| match v {
        Val::S(s) => Some(s.as_str()),
        _ => None,
    })
}
pub(crate) fn set(c: &mut Cell, key: &'static str, v: String) {
    c.push((key, Val::S(v)));
}
pub(crate) fn kind(c: &Cell) -> &str {
    get(c, "k").unwrap()
}

pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) > 0x7f => {
                // json.dumps default: \uXXXX escapes (ensure_ascii=True)
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn cell_json(c: &Cell) -> String {
    let fields: Vec<String> = c
        .iter()
        .map(|(k, v)| match v {
            Val::S(s) => format!("\"{}\": \"{}\"", k, json_escape(s)),
            Val::Parts(cells) => format!(
                "\"{}\": [{}]",
                k,
                cells.iter().map(cell_json).collect::<Vec<_>>().join(", ")
            ),
        })
        .collect();
    format!("{{{}}}", fields.join(", "))
}

pub(crate) fn grid_json(grid: &[Vec<Cell>]) -> String {
    let rows: Vec<String> = grid
        .iter()
        .map(|row| {
            format!(
                "[{}]",
                row.iter().map(cell_json).collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    format!("[{}]", rows.join(", "))
}

// ------------------------------------------- string helpers (Python ports)

/// Port of parse_arrays.find_array.
pub(crate) fn find_array(tex: &str) -> Option<(usize, usize, String)> {
    let b = tex.as_bytes();
    let i = tex.find("\\array")?;
    let j = i + tex[i..].find('{')?;
    let mut depth = 0i32;
    for k in j..b.len() {
        if b[k] == b'{' && (k == 0 || b[k - 1] != b'\\') {
            depth += 1;
        } else if b[k] == b'}' && b[k - 1] != b'\\' {
            depth -= 1;
            if depth == 0 {
                return Some((i, k + 1, tex[j + 1..k].to_string()));
            }
        }
    }
    None
}

/// Port of split_depth0 (separators at brace depth 0).
pub(crate) fn split_depth0(text: &str, seps: &[&str]) -> Vec<String> {
    let b = text.as_bytes();
    let mut parts = Vec::new();
    let mut buf = String::new();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c == b'{' && (i == 0 || b[i - 1] != b'\\') {
            depth += 1;
        } else if c == b'}' && i > 0 && b[i - 1] != b'\\' {
            depth -= 1;
        }
        if depth == 0 {
            if let Some(sep) = seps.iter().find(|s| text[i..].starts_with(**s)) {
                parts.push(std::mem::take(&mut buf));
                i += sep.len();
                continue;
            }
        }
        let ch_len = text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        buf.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    parts.push(buf);
    parts
}

/// Port of read_group.
pub(crate) fn read_group(text: &str) -> (String, String) {
    let text = text.trim_start();
    if text.is_empty() {
        return (String::new(), String::new());
    }
    let b = text.as_bytes();
    if b[0] == b'{' {
        let mut depth = 0i32;
        for (k, c) in text.char_indices() {
            if c == '{' && (k == 0 || b[k - 1] != b'\\') {
                depth += 1;
            } else if c == '}' && b[k - 1] != b'\\' {
                depth -= 1;
                if depth == 0 {
                    return (text[1..k].to_string(), text[k + 1..].to_string());
                }
            }
        }
    }
    if b[0] == b'\\' {
        let end = text[1..]
            .find(|c: char| !c.is_ascii_alphabetic())
            .map(|e| e + 1)
            .unwrap_or(text.len());
        if end > 1 {
            return (text[..end].to_string(), text[end..].to_string());
        }
    }
    let ch_len = text.chars().next().unwrap().len_utf8();
    (text[..ch_len].to_string(), text[ch_len..].to_string())
}

/// Port of spanning_parens.
fn spanning_parens(s: &str) -> bool {
    if s.starts_with("\\left(") && s.ends_with("\\right)") {
        return true;
    }
    if !(s.starts_with('(') && s.ends_with(')')) {
        return false;
    }
    let mut depth = 0i32;
    let n = s.chars().count();
    for (i, c) in s.chars().enumerate() {
        depth += (c == '(') as i32 - (c == ')') as i32;
        if depth == 0 && i < n - 1 {
            return false;
        }
    }
    depth == 0
}

/// Port of trim_label.
fn trim_label(frag: &str) -> String {
    let mut frag = remove_laps(frag).trim().to_string();
    loop {
        if frag.is_empty() {
            break;
        }
        if let Some(r) = frag.strip_prefix("{}") {
            frag = r.trim().to_string();
        } else if frag.starts_with("\\,")
            || frag.starts_with("\\;")
            || frag.starts_with("\\:")
            || frag.starts_with("\\!")
        {
            frag = frag[2..].trim().to_string();
        } else if frag.starts_with('^') || frag.starts_with('_') {
            frag = frag[1..].trim().to_string();
        } else if frag.starts_with('{') && frag.ends_with('}') {
            let b = frag.as_bytes();
            let mut depth = 0i32;
            let mut spanning = true;
            for (i, &c) in b.iter().enumerate() {
                depth += (c == b'{') as i32 - (c == b'}') as i32;
                if depth == 0 && i < b.len() - 1 {
                    spanning = false;
                    break;
                }
            }
            if !spanning {
                break;
            }
            frag = frag[1..frag.len() - 1].trim().to_string();
        } else {
            break;
        }
    }
    frag
}

/// Python: re.sub(r"\\math[lr]lap\b", "", frag)
fn remove_laps(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("\\math") {
        let after = &rest[i + 5..];
        let is_lap = (after.starts_with("llap") || after.starts_with("rlap"))
            && !after[4..].starts_with(|c: char| c.is_ascii_alphabetic());
        if is_lap {
            out.push_str(&rest[..i]);
            rest = &after[4..];
        } else {
            out.push_str(&rest[..i + 5]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Python: re.sub(r"\\[bB]igg?\b", "", s).strip()
fn remove_bigs(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('\\') {
        let after = &rest[i + 1..];
        let m = ["bigg", "Bigg", "big", "Big"]
            .iter()
            .find(|w| {
                after.starts_with(**w)
                    && !after[w.len()..].starts_with(|c: char| c.is_ascii_alphabetic())
            })
            .copied();
        match m {
            Some(w) => {
                out.push_str(&rest[..i]);
                rest = &after[w.len()..];
            }
            None => {
                out.push_str(&rest[..i + 1]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Commands appearing at brace depth 0, via the mitex CST: walk the tree
/// but never descend into curly groups (Python's depth0_text semantics).
fn top_cmds(cell_src: &str) -> Vec<String> {
    let node = mitex_parser::parse(cell_src, DEFAULT_SPEC.clone());
    let mut cmds = Vec::new();
    let mut escaped_depth = 0i32;
    collect_top_cmds(&node, &mut cmds, &mut escaped_depth);
    cmds
}

fn collect_top_cmds(
    node: &mitex_parser::syntax::SyntaxNode,
    out: &mut Vec<String>,
    escaped_depth: &mut i32,
) {
    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Node(n) => {
                if n.kind() == SyntaxKind::ItemCurly {
                    continue;
                }
                // \left\{ ... \right\} with brace delimiters also hides
                // its contents under Python's depth0_text semantics.
                if n.kind() == SyntaxKind::ItemLR && n.text().to_string().contains("\\{") {
                    continue;
                }
                if n.kind() == SyntaxKind::ItemCmd {
                    if let Some(tok) = n
                        .children_with_tokens()
                        .filter_map(|c| c.into_token())
                        .find(|t| t.kind() == SyntaxKind::ClauseCommandName)
                    {
                        let name = tok.text().trim_start_matches('\\');
                        // Escaped/set-builder braces \{...\} hide their
                        // contents, matching Python's depth0_text (whose
                        // regex strips any brace-delimited span).
                        match name {
                            "{" | "lbrace" => *escaped_depth += 1,
                            "}" | "rbrace" => *escaped_depth -= 1,
                            _ if *escaped_depth == 0 => out.push(name.to_string()),
                            _ => {}
                        }
                    }
                }
                collect_top_cmds(&n, out, escaped_depth);
            }
            rowan::NodeOrToken::Token(_) => {}
        }
    }
}

// ------------------------------------------------------ classify (port)

const X_NAMES: &[&str] = &[
    "rightarrow",
    "leftarrow",
    "hookrightarrow",
    "hookleftarrow",
    "twoheadrightarrow",
    "mapsto",
    "to",
];

pub(crate) fn classify_cell(cell_src: &str) -> Cell {
    let s0 = cell_src.trim();
    if s0.is_empty() {
        return cell("e");
    }
    let s_owned = remove_bigs(s0);
    let s = s_owned.as_str();

    // \xrightarrow[below]{above}
    if let Some(rest0) = s.strip_prefix("\\x") {
        let name = X_NAMES
            .iter()
            .find(|n| {
                rest0.starts_with(**n)
                    && !rest0[n.len()..].starts_with(|c: char| c.is_ascii_alphabetic())
            })
            .copied();
        if let Some(name) = name {
            let cmd = if name == "to" { "rightarrow" } else { name };
            let mut rest = rest0[name.len()..].trim_start().to_string();
            let mut below: Option<String> = None;
            if rest.starts_with('[') {
                if let Some(close) = rest.find(']') {
                    if close > 0 {
                        below = Some(rest[1..close].to_string());
                        rest = rest[close + 1..].to_string();
                    }
                }
            }
            let (above, rest2) = read_group(&rest);
            if rest2.trim().is_empty() {
                let mut res = cell("h");
                set(&mut res, "dir", h_dir(cmd).unwrap().to_string());
                set(&mut res, "cmd", cmd.to_string());
                if !above.trim().is_empty() {
                    set(&mut res, "above", above.trim().to_string());
                }
                if let Some(b) = below {
                    if !b.trim().is_empty() {
                        set(&mut res, "below", b.trim().to_string());
                    }
                }
                return res;
            }
        }
    }

    // \stackrel{..}{..} and friends
    for which in ["stackrel", "overset", "underset", "underoverset"] {
        let pat = format!("\\{}", which);
        if let Some(rest0) = s.strip_prefix(pat.as_str()) {
            if rest0.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            let (label0, rest1) = read_group(rest0);
            let mut label = label0;
            let mut label2: Option<String> = None;
            let mut rest = rest1;
            if which == "underoverset" {
                label2 = Some(label.clone());
                let (l, r) = read_group(&rest);
                label = l;
                rest = r;
            }
            let (arrow, rest2) = read_group(&rest);
            if rest2.trim().is_empty() {
                if which == "stackrel" {
                    let top = classify_cell(&label);
                    let bottom = classify_cell(&arrow);
                    if kind(&top) == "h"
                        && kind(&bottom) == "h"
                        && get(&top, "dir") != Some("~")
                        && get(&bottom, "dir") != Some("~")
                    {
                        let mut res = cell("h");
                        let dir = get(&top, "dir").unwrap().to_string();
                        set(&mut res, "dir", dir);
                        res.push(("pair", Val::Parts(vec![top, bottom])));
                        return res;
                    }
                }
                let at = arrow.trim();
                if at == "=" || at == "\\simeq" || at == "\\cong" {
                    let mut res = cell("h");
                    set(&mut res, "dir", "~".to_string());
                    let cmd = at.trim_start_matches('\\');
                    set(
                        &mut res,
                        "cmd",
                        if cmd.is_empty() { "=" } else { cmd }.to_string(),
                    );
                    let key: &'static str = if which == "underset" { "below" } else { "above" };
                    set(&mut res, key, label.trim().to_string());
                    return res;
                }
                let cmd = at.trim_start_matches('\\');
                if at == format!("\\{}", cmd) && h_dir(cmd).is_some() {
                    let mut res = cell("h");
                    set(&mut res, "dir", h_dir(cmd).unwrap().to_string());
                    set(&mut res, "cmd", cmd.to_string());
                    if which == "underset" {
                        set(&mut res, "below", label.trim().to_string());
                    } else {
                        set(&mut res, "above", label.trim().to_string());
                    }
                    if let Some(l2) = label2 {
                        if !l2.is_empty() {
                            set(&mut res, "below", l2.trim().to_string());
                        }
                    }
                    return res;
                }
            }
        }
    }

    let bare: String = s.chars().filter(|c| *c != ' ').collect();
    let tops = top_cmds(s);

    // single bare command
    if tops.len() == 1 && bare == format!("\\{}", tops[0]) {
        let c = tops[0].clone();
        let c = c.as_str();
        if let Some(d) = h_dir(c) {
            let mut res = cell("h");
            set(&mut res, "dir", d.to_string());
            set(&mut res, "cmd", c.to_string());
            return res;
        }
        if let Some(d) = v_dir(c) {
            let mut res = cell("v");
            set(&mut res, "dir", d.to_string());
            set(&mut res, "cmd", c.to_string());
            return res;
        }
        if let Some(d) = d_dir(c) {
            let mut res = cell("d");
            set(&mut res, "dir", d.to_string());
            set(&mut res, "cmd", c.to_string());
            return res;
        }
    }

    if bare == "=" || bare == "\\simeq" || bare == "\\cong" {
        let mut res = cell("h");
        set(&mut res, "dir", "~".to_string());
        let cmd = bare.trim_start_matches('\\');
        set(
            &mut res,
            "cmd",
            if cmd.is_empty() { "=" } else { cmd }.to_string(),
        );
        return res;
    }
    if bare == "\\|" || bare == "\\Vert" || bare == "\\parallel" {
        let mut res = cell("v");
        set(&mut res, "dir", "veq".to_string());
        set(&mut res, "cmd", "=".to_string());
        return res;
    }

    // spilled object text with leading/trailing arrow
    if let Some(res) = spill_cell(s) {
        return res;
    }

    // parenthesized formula is an object
    if spanning_parens(s) {
        let mut res = cell("o");
        set(&mut res, "tex", s.to_string());
        return res;
    }

    // two diagonals in one cell
    if let Some(res) = double_diagonal(s) {
        return res;
    }

    // generic vertical/diagonal with labels
    if let Some(res) = parse_vd_cell(s, &tops) {
        return res;
    }

    if tops.iter().any(|c| is_arrow(c)) {
        let mut res = cell("?");
        set(&mut res, "tex", s.to_string());
        return res;
    }
    let mut res = cell("o");
    set(&mut res, "tex", s.to_string());
    res
}

/// Python parse_vd_cell.
fn parse_vd_cell(s: &str, tops: &[String]) -> Option<Cell> {
    let vd: Vec<&String> = tops
        .iter()
        .filter(|c| v_dir(c).is_some() || d_dir(c).is_some())
        .collect();
    let h_blockers = tops
        .iter()
        .any(|c| h_dir(c).map(|d| d != "~").unwrap_or(false));
    if vd.len() != 1 || h_blockers {
        return None;
    }
    let c = vd[0].as_str();
    let i = s.find(&format!("\\{}", c))?;
    let (k, dir) = match v_dir(c) {
        Some(d) => ("v", d),
        None => ("d", d_dir(c).unwrap()),
    };
    let mut res = cell(k);
    set(&mut res, "dir", dir.to_string());
    set(&mut res, "cmd", c.to_string());
    let west = trim_label(&s[..i]);
    let east = trim_label(&s[i + c.len() + 1..]);
    if !west.is_empty() {
        set(&mut res, "west", west);
    }
    if !east.is_empty() {
        set(&mut res, "east", east);
    }
    Some(res)
}

/// Python double-diagonal branch ("f \searrow \swarrow g").
fn double_diagonal(s: &str) -> Option<Cell> {
    let names = ["searrow", "swarrow", "nearrow", "nwarrow"];
    let mut found: Vec<(usize, &str)> = Vec::new();
    let mut idx = 0usize;
    while let Some(i) = s[idx..].find('\\') {
        let at = idx + i;
        let after = &s[at + 1..];
        if let Some(n) = names.iter().find(|n| {
            after.starts_with(**n)
                && !after[n.len()..].starts_with(|c: char| c.is_ascii_alphabetic())
        }) {
            found.push((at, n));
            idx = at + 1 + n.len();
        } else {
            idx = at + 1;
        }
    }
    if found.len() != 2 {
        return None;
    }
    let (i1, c1) = found[0];
    let (i2, c2) = found[1];
    let between = &s[i1 + 1 + c1.len()..i2];
    if !between.trim().is_empty() {
        return None;
    }
    let pre = trim_label(&s[..i1]);
    let post = trim_label(&s[i2 + 1 + c2.len()..]);
    for lab in [&pre, &post] {
        if !lab.is_empty() && !top_cmds(lab).is_empty() {
            return None;
        }
    }
    let mut parts = Vec::new();
    for (c, key, lab) in [(c1, "west", pre), (c2, "east", post)] {
        let mut part = cell("d");
        set(&mut part, "dir", d_dir(c).unwrap().to_string());
        set(&mut part, "cmd", c.to_string());
        if !lab.is_empty() {
            let key: &'static str = if key == "west" { "west" } else { "east" };
            set(&mut part, key, lab);
        }
        parts.push(part);
    }
    let mut res = cell("dd");
    res.push(("parts", Val::Parts(parts)));
    Some(res)
}

/// Python spill branch: object text sharing a cell with a lone arrow.
fn spill_cell(s: &str) -> Option<Cell> {
    let mut alts: Vec<&str> = H_CMDS.iter().map(|(k, _)| *k).collect();
    alts.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    // trailing arrow: ^(.*\S)\s*\\(alts)$
    for name in &alts {
        let suffix = format!("\\{}", name);
        if let Some(head) = s.strip_suffix(suffix.as_str()) {
            let head_trim = head.trim_end();
            if head_trim.is_empty() {
                continue;
            }
            if arrows_in(head_trim) {
                continue;
            }
            let mut res = cell("h");
            set(&mut res, "dir", h_dir(name).unwrap().to_string());
            set(&mut res, "cmd", name.to_string());
            set(&mut res, "spill_west", head_trim.to_string());
            return Some(res);
        }
    }
    // leading arrow: ^\\(alts)\s+(\S.*)$
    for name in &alts {
        let prefix = format!("\\{}", name);
        if let Some(tail) = s.strip_prefix(prefix.as_str()) {
            if !tail.starts_with(|c: char| c.is_whitespace()) {
                continue;
            }
            let tail_trim = tail.trim_start();
            if tail_trim.is_empty() {
                continue;
            }
            if arrows_in(tail_trim) {
                continue;
            }
            let mut res = cell("h");
            set(&mut res, "dir", h_dir(name).unwrap().to_string());
            set(&mut res, "cmd", name.to_string());
            set(&mut res, "spill_east", tail_trim.to_string());
            return Some(res);
        }
    }
    None
}

/// Python's spill guard scans ALL commands in the text (not just depth 0).
fn arrows_in(tex: &str) -> bool {
    let mut rest = tex;
    while let Some(i) = rest.find('\\') {
        let after = &rest[i + 1..];
        let end = after
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(after.len());
        if end > 0 && is_arrow(&after[..end]) {
            return true;
        }
        rest = &after[end.max(1).min(after.len())..];
    }
    false
}

// -------------------------------------------------- grid passes (ports)

pub(crate) fn absorb_spills(grid: &mut [Vec<Cell>]) {
    for r in 0..grid.len() {
        for c in 0..grid[r].len() {
            for (key, dc) in [("spill_west", -1i32), ("spill_east", 1i32)] {
                let tex = match get(&grid[r][c], key) {
                    Some(t) => t.to_string(),
                    None => continue,
                };
                grid[r][c].retain(|(k, _)| *k != key);
                let row_len = grid[r].len() as i32;
                let mut cc = c as i32 + dc;
                while cc >= 0 && cc < row_len && kind(&grid[r][cc as usize]) == "e" {
                    cc += dc;
                }
                if cc >= 0 && cc < row_len && kind(&grid[r][cc as usize]) == "o" {
                    let target = &mut grid[r][cc as usize];
                    let old = get(target, "tex").unwrap().to_string();
                    let new = if dc < 0 {
                        format!("{} {}", old, tex)
                    } else {
                        format!("{} {}", tex, old)
                    };
                    for (k, v) in target.iter_mut() {
                        if *k == "tex" {
                            *v = Val::S(new.clone());
                        }
                    }
                } else {
                    let mut q = cell("?");
                    set(&mut q, "tex", tex);
                    grid[r][c] = q;
                }
            }
        }
    }
}

pub(crate) fn merge_annotations(grid: &mut Vec<Vec<Cell>>) {
    let cols = grid.iter().map(|r| r.len()).max().unwrap_or(0);
    for row in grid.iter_mut() {
        while row.len() < cols {
            row.push(cell("e"));
        }
    }
    for c in 0..cols {
        let filled: Vec<usize> = (0..grid.len())
            .filter(|r| kind(&grid[*r][c]) != "e")
            .collect();
        if filled.len() != 1 || kind(&grid[filled[0]][c]) != "o" {
            continue;
        }
        let r = filled[0];
        let tex = get(&grid[r][c], "tex").unwrap().to_string();
        for dc in [1i32, -1i32] {
            let cc = c as i32 + dc;
            if cc < 0 || cc >= cols as i32 {
                continue;
            }
            let cc = cc as usize;
            let ok = kind(&grid[r][cc]).to_string();
            if ok == "v" || ok == "d" {
                if grid[r].iter().any(|cl| kind(cl) == "h") {
                    continue;
                }
                let key: &'static str = if dc == 1 { "west" } else { "east" };
                if get(&grid[r][cc], key).is_none() {
                    set(&mut grid[r][cc], key, tex.clone());
                }
            } else if ok == "o" {
                let old = get(&grid[r][cc], "tex").unwrap().to_string();
                let new = if dc == 1 {
                    format!("{} {}", tex, old)
                } else {
                    format!("{} {}", old, tex)
                };
                for (k, v) in grid[r][cc].iter_mut() {
                    if *k == "tex" {
                        *v = Val::S(new.clone());
                    }
                }
            } else {
                continue;
            }
            grid[r][c] = cell("e");
            break;
        }
    }
}

// ------------------------------------------------------------ pipeline

pub(crate) fn parse_formula(tex: &str) -> Result<String, String> {
    parse_formula_grid(tex).map(|g| grid_json(&g))
}

pub(crate) fn parse_formula_grid(tex: &str) -> Result<Vec<Vec<Cell>>, String> {
    let (start, end, body) = match find_array(tex) {
        Some(t) => t,
        None => return Err("no-array".into()),
    };
    if !tex[..start].trim().is_empty() {
        return Err("wrapped".into());
    }
    if !trailing_is_punct(&tex[end..]) {
        return Err("wrapped".into());
    }
    let mut grid: Vec<Vec<Cell>> = split_depth0(&body, &["\\\\"])
        .iter()
        .map(|row| {
            split_depth0(row, &["&"])
                .iter()
                .map(|c| classify_cell(c))
                .collect()
        })
        .collect();
    grid.retain(|row: &Vec<Cell>| row.iter().any(|c| kind(c) != "e"));
    if grid.is_empty() {
        return Err("empty".into());
    }
    absorb_spills(&mut grid);
    merge_annotations(&mut grid);
    let unknown = grid.iter().flatten().filter(|c| kind(c) == "?").count();
    if unknown > 0 {
        return Err(format!("cells:{}?", unknown));
    }
    Ok(grid)
}

/// Python: re.sub(r"\\[,;:!]|\\q?quad|[\s.,]", "", trailing) must be empty.
fn trailing_is_punct(s: &str) -> bool {
    let mut rest = s;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '.' || c == ',');
        if rest.is_empty() {
            return true;
        }
        if let Some(r) = rest.strip_prefix('\\') {
            if let Some(r2) = r.strip_prefix("qquad").or_else(|| r.strip_prefix("quad")) {
                rest = r2;
                continue;
            }
            let mut chars = r.chars();
            if matches!(chars.next(), Some(',') | Some(';') | Some(':') | Some('!')) {
                rest = chars.as_str();
                continue;
            }
        }
        return false;
    }
}


// ----------------------------------------------- extras for the emitter

/// A wrapped array's body as a grid (Python parse_wrapped_grid), or None
/// if any cell is unparseable.
pub(crate) fn parse_body_grid(body: &str) -> Option<Vec<Vec<Cell>>> {
    let mut grid: Vec<Vec<Cell>> = split_depth0(body, &["\\\\"])
        .iter()
        .map(|row| {
            split_depth0(row, &["&"])
                .iter()
                .map(|c| classify_cell(c))
                .collect()
        })
        .collect();
    grid.retain(|row: &Vec<Cell>| row.iter().any(|c| kind(c) != "e"));
    if grid.is_empty() {
        return None;
    }
    absorb_spills(&mut grid);
    merge_annotations(&mut grid);
    if grid.iter().flatten().any(|c| kind(c) == "?") {
        return None;
    }
    Some(grid)
}

pub(crate) fn parts<'a>(c: &'a Cell) -> Option<&'a [Cell]> {
    for (k, v) in c {
        if (*k == "pair" || *k == "parts") && matches!(v, Val::Parts(_)) {
            if let Val::Parts(p) = v {
                return Some(p.as_slice());
            }
        }
    }
    None
}

pub(crate) fn unset(c: &mut Cell, key: &str) {
    c.retain(|(k, _)| *k != key);
}
