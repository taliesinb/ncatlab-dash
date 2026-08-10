//! tikzcd -> typst/fletcher converter.
//!
//! Parses the `\begin{tikzcd}[opts] ... \end{tikzcd}` blocks that appear
//! raw in nLab prose (the live site renders them server-side) and emits
//! the same `#diagram(...)` shape as the array emitter in `emit.rs`, so
//! cell and label math goes through the same mitex path.
//!
//! Supported: `[lrud]+` directions, `from=R-C`/`to=R-C`, old-style
//! `\ar{dir}[swap]{label}` and `\rar`-family shortcuts, quoted labels
//! with side/pos modifiers, `name=` label anchors targeted by 2-cell
//! `Rightarrow`s, phantom/dashed/dotted/hook/mapsto/equals/two-heads
//! marks, bend, shift, crossing over, and the common sep options.
//! Unknown arrow options are ignored (collected as warnings), so a
//! diagram converts unless its structure fails to parse.

use crate::emit;

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
    Center,
}

#[derive(Clone)]
struct Label {
    tex: String,
    side: Side,
    pos: Option<f64>,
    sloped: bool,
    name: Option<String>,
}

#[derive(Clone)]
enum Coord {
    Cell(i32, i32), // row, col (0-based)
    Name(String),
}

#[derive(Clone)]
struct Arrow {
    from: Coord,
    to: Coord,
    labels: Vec<Label>,
    mark: String,
    stroke_none: bool,
    dashed: bool,
    dotted: bool,
    bend: Option<f64>,
    shift: Option<f64>, // pt, positive = left of travel
    crossing: bool,
    swap: bool,
    no_head: bool,
    color: Option<&'static str>,
    shorten_start: f64, // pt
    shorten_end: f64,
    warnings: Vec<String>,
}

impl Arrow {
    fn at(r: i32, c: i32) -> Arrow {
        Arrow {
            from: Coord::Cell(r, c),
            to: Coord::Cell(r, c),
            labels: Vec::new(),
            mark: "->".into(),
            stroke_none: false,
            dashed: false,
            dotted: false,
            bend: None,
            shift: None,
            crossing: false,
            swap: false,
            no_head: false,
            color: None,
            shorten_start: 0.0,
            shorten_end: 0.0,
            warnings: Vec::new(),
        }
    }

    fn final_mark(&self) -> String {
        if self.no_head {
            // `Rightarrow, no head` is the tikzcd idiom for a double line
            return if self.mark == "=>" { "=".into() } else { "-".into() };
        }
        if self.dashed && self.mark == "->" {
            "-->".into()
        } else if self.dotted && self.mark == "->" {
            "..>".into()
        } else if self.dashed && self.mark == "-" {
            "--".into()
        } else {
            self.mark.clone()
        }
    }
}

// ------------------------------------------------------------- scanning

/// Strip unescaped `%` comments; like TeX, the newline is consumed.
fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        let b: Vec<char> = line.chars().collect();
        let mut i = 0;
        let mut cut = None;
        while i < b.len() {
            if b[i] == '\\' {
                i += 2;
                continue;
            }
            if b[i] == '%' {
                cut = Some(i);
                break;
            }
            i += 1;
        }
        match cut {
            Some(c) => out.push_str(&b[..c].iter().collect::<String>()),
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

const AMP: char = '\u{E010}'; // stand-in for the active cell separator

/// Replace the active cell separator with AMP, escape-aware: `\\` stays
/// a row break (its backslashes are never consumed by a following `&`),
/// and an escaped `\&` in normal mode becomes a literal (E011).
fn mask_amps(body: &str, amp_replacement: bool) -> String {
    let b: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut env = 0i32;
    let mut i = 0;
    let starts = |i: usize, word: &str| {
        b[i..].iter().take(word.len()).collect::<String>() == word
    };
    while i < b.len() {
        if b[i] == '\\' && starts(i, "\\begin{") {
            env += 1;
            out.push_str("\\begin{");
            i += 7;
            continue;
        }
        if b[i] == '\\' && starts(i, "\\end{") {
            env -= 1;
            out.push_str("\\end{");
            i += 5;
            continue;
        }
        if env > 0 {
            out.push(b[i]);
            i += 1;
            continue;
        }
        if b[i] == '\\' && i + 1 < b.len() {
            if b[i + 1] == '\\' {
                out.push_str("\\\\");
                i += 2;
                continue;
            }
            if b[i + 1] == '&' {
                out.push(if amp_replacement { AMP } else { '\u{E011}' });
                i += 2;
                continue;
            }
            out.push('\\');
            out.push(b[i + 1]);
            i += 2;
            continue;
        }
        if b[i] == '&' && !amp_replacement {
            out.push(AMP);
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    out
}

/// Split at top-level separators. `sep` is either AMP (cells) or '\\'
/// meaning the row break pair `\\`.
fn split_top(s: &str, rows: bool) -> Vec<String> {
    let b: Vec<char> = s.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let (mut brace, mut bracket, mut env) = (0i32, 0i32, 0i32);
    let mut quote = false;
    let mut i = 0;
    let starts = |i: usize, word: &str| {
        b[i..].iter().take(word.len()).collect::<String>() == word
    };
    while i < b.len() {
        let c = b[i];
        if c == '"' && bracket > 0 {
            quote = !quote;
            cur.push(c);
            i += 1;
            continue;
        }
        if quote {
            cur.push(c);
            i += 1;
            continue;
        }
        if c == '\\' && starts(i, "\\begin{") {
            env += 1;
            i += 7;
            cur.push_str("\\begin{");
            while i < b.len() && b[i] != '}' {
                cur.push(b[i]);
                i += 1;
            }
            if i < b.len() {
                cur.push('}');
                i += 1;
            }
            continue;
        }
        if c == '\\' && starts(i, "\\end{") {
            env -= 1;
            i += 5;
            cur.push_str("\\end{");
            while i < b.len() && b[i] != '}' {
                cur.push(b[i]);
                i += 1;
            }
            if i < b.len() {
                cur.push('}');
                i += 1;
            }
            continue;
        }
        if env > 0 {
            cur.push(c);
            i += if c == '\\' && i + 1 < b.len() {
                cur.push(b[i + 1]);
                2
            } else {
                1
            };
            continue;
        }
        if c == '\\' && i + 1 < b.len() {
            if rows && b[i + 1] == '\\' && brace == 0 && bracket == 0 {
                parts.push(cur.clone());
                cur.clear();
                i += 2;
                // skip optional row-spacing option \\[6pt]
                while i < b.len() && b[i].is_whitespace() {
                    i += 1;
                }
                if i < b.len() && b[i] == '[' {
                    while i < b.len() && b[i] != ']' {
                        i += 1;
                    }
                    i += 1;
                }
                continue;
            }
            cur.push(c);
            cur.push(b[i + 1]);
            i += 2;
            continue;
        }
        match c {
            '{' => brace += 1,
            '}' => brace -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            _ => {}
        }
        if !rows && c == AMP && brace == 0 && bracket == 0 {
            parts.push(cur.clone());
            cur.clear();
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    parts.push(cur);
    parts
}

/// Split an `\ar[...]` argument list at top-level commas.
fn split_args(s: &str) -> Vec<String> {
    let b: Vec<char> = s.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let (mut brace, mut bracket) = (0i32, 0i32);
    let mut quote = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '"' {
            quote = !quote;
        } else if !quote {
            match c {
                '\\' if i + 1 < b.len() => {
                    cur.push(c);
                    cur.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                '{' => brace += 1,
                '}' => brace -= 1,
                '[' => bracket += 1,
                ']' => bracket -= 1,
                ',' if brace == 0 && bracket == 0 => {
                    parts.push(cur.trim().to_string());
                    cur.clear();
                    i += 1;
                    continue;
                }
                _ => {}
            }
        }
        cur.push(c);
        i += 1;
    }
    let last = cur.trim().to_string();
    if !last.is_empty() || !parts.is_empty() {
        parts.push(last);
    }
    parts.retain(|p| !p.is_empty());
    parts
}

/// Read a balanced group starting at `b[i]` (an opener); returns
/// (contents, index-after-closer). Quote-aware for bracket groups.
fn read_group(b: &[char], i: usize, open: char, close: char) -> Option<(String, usize)> {
    if b.get(i) != Some(&open) {
        return None;
    }
    let mut depth = 0i32;
    let mut quote = false;
    let mut j = i;
    while j < b.len() {
        let c = b[j];
        if c == '"' {
            quote = !quote;
        } else if !quote {
            if c == '\\' {
                j += 2;
                continue;
            }
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some((b[i + 1..j].iter().collect(), j + 1));
                }
            }
        }
        j += 1;
    }
    None
}

// ------------------------------------------------------- arrow parsing

fn parse_coord(v: &str) -> Coord {
    let v = v.trim();
    if let Some((r, c)) = v.split_once('-') {
        if let (Ok(r), Ok(c)) = (r.trim().parse::<i32>(), c.trim().parse::<i32>()) {
            return Coord::Cell(r - 1, c - 1);
        }
    }
    Coord::Name(v.to_string())
}

fn rel_coord(dir: &str, r: i32, c: i32) -> Coord {
    let (mut dr, mut dc) = (0, 0);
    for ch in dir.chars() {
        match ch {
            'r' => dc += 1,
            'l' => dc -= 1,
            'd' => dr += 1,
            'u' => dr -= 1,
            _ => {}
        }
    }
    Coord::Cell(r + dr, c + dc)
}

fn apply_dir(arrow: &mut Arrow, dir: &str, r: i32, c: i32) {
    let (mut dr, mut dc) = (0, 0);
    for ch in dir.chars() {
        match ch {
            'r' => dc += 1,
            'l' => dc -= 1,
            'd' => dr += 1,
            'u' => dr -= 1,
            _ => {}
        }
    }
    arrow.to = Coord::Cell(r + dr, c + dc);
}

fn strip_braces(s: &str) -> &str {
    let t = s.trim();
    if t.starts_with('{') && t.ends_with('}') {
        let inner: Vec<char> = t.chars().collect();
        if read_group(&inner, 0, '{', '}').map(|(_, e)| e) == Some(inner.len()) {
            return t[1..t.len() - 1].trim();
        }
    }
    t
}

fn parse_label_mods(label: &mut Label, mods: &str) {
    for m in split_args(mods) {
        let m = m.trim();
        match m {
            "description" | "anchor=center" | "marking" => label.side = Side::Center,
            "swap" | "below" | "right" => label.side = Side::Right,
            "above" | "left" => {}
            "sloped" => label.sloped = true,
            "near start" => label.pos = Some(0.25),
            "very near start" => label.pos = Some(0.1),
            "at start" => label.pos = Some(0.0),
            "near end" => label.pos = Some(0.75),
            "very near end" => label.pos = Some(0.9),
            "at end" => label.pos = Some(1.0),
            _ if m.starts_with("pos=") => {
                label.pos = m[4..].trim().parse().ok();
            }
            _ if m.starts_with("name=") => {
                label.name = Some(m[5..].trim().to_string());
            }
            _ => {} // rotate, shifts, fonts: cosmetic, ignore
        }
    }
}

/// Parse a quoted-label argument: `"tex"` + optional `'` + modifiers.
fn parse_label(part: &str) -> Option<Label> {
    let b: Vec<char> = part.chars().collect();
    if b.first() != Some(&'"') {
        return None;
    }
    let close = b.iter().skip(1).position(|c| *c == '"')? + 1;
    let tex: String = b[1..close].iter().collect();
    let mut label = Label {
        tex: strip_braces(&tex).to_string(),
        side: Side::Left,
        pos: None,
        sloped: false,
        name: None,
    };
    let rest: String = b[close + 1..].iter().collect();
    let mut rest = rest.trim().to_string();
    if rest.starts_with('\'') {
        label.side = Side::Right;
        rest = rest[1..].trim().to_string();
    }
    if rest.starts_with('{') && rest.ends_with('}') {
        parse_label_mods(&mut label, &rest[1..rest.len() - 1]);
    } else if !rest.is_empty() {
        parse_label_mods(&mut label, &rest);
    }
    Some(label)
}

/// Returns whether any positioning token (direction, from=, to=) was seen.
fn parse_arrow_args(arrow: &mut Arrow, args: &str, r: i32, c: i32) -> bool {
    let dir_re = |s: &str| !s.is_empty() && s.chars().all(|c| "lrud".contains(c));
    let mut positioned = false;
    for part in split_args(args) {
        let p = part.trim();
        if dir_re(p) {
            positioned = true;
            apply_dir(arrow, p, r, c);
        } else if let Some(v) = p.strip_prefix("from=") {
            positioned = true;
            arrow.from = if dir_re(v.trim()) {
                rel_coord(v.trim(), r, c)
            } else {
                parse_coord(v)
            };
        } else if let Some(v) = p.strip_prefix("to=") {
            positioned = true;
            arrow.to = if dir_re(v.trim()) {
                rel_coord(v.trim(), r, c)
            } else {
                parse_coord(v)
            };
        } else if p == "swap" {
            arrow.swap = true;
        } else if p.starts_with('"') {
            if let Some(l) = parse_label(p) {
                arrow.labels.push(l);
            }
        } else {
            parse_style(arrow, p);
        }
    }
    positioned
}

fn parse_style(arrow: &mut Arrow, p: &str) {
    match p {
        "Rightarrow" => arrow.mark = "=>".into(),
        "Leftarrow" => arrow.mark = "<=".into(),
        "Leftrightarrow" => arrow.mark = "<=>".into(),
        "-" | "dash" => arrow.mark = "-".into(),
        "no head" => arrow.no_head = true,
        "equals" | "equal" | "Equal" => arrow.mark = "=".into(),
        "->>" | "two heads" | "twoheadrightarrow" => arrow.mark = "->>".into(),
        ">->" | "tail" | "rightarrowtail" => arrow.mark = ">->".into(),
        "hook" | "hookrightarrow" => arrow.mark = "hook->".into(),
        "hook'" => arrow.mark = "hook'->".into(),
        "mapsto" | "maps to" | "|->" => arrow.mark = "|->".into(),
        "<-|" => arrow.mark = "<-|".into(),
        "<-" => arrow.mark = "<-".into(),
        "<->" => arrow.mark = "<->".into(),
        "-->" | "dashrightarrow" => arrow.dashed = true,
        "dashed" => arrow.dashed = true,
        "dotted" => arrow.dotted = true,
        "phantom" => {
            arrow.stroke_none = true;
            arrow.mark = "-".into();
        }
        "draw=none" => arrow.stroke_none = true,
        "crossing over" => arrow.crossing = true,
        _ => parse_style_kv(arrow, p),
    }
}

fn named_color(name: &str) -> Option<&'static str> {
    Some(match name {
        "red" => "red",
        "blue" => "blue",
        "green" => "green",
        "olive" => "olive",
        "orange" => "orange",
        "purple" | "violet" => "purple",
        "teal" | "cyan" => "teal",
        "gray" | "grey" | "lightgray" | "lightgrey" | "darkgray" => "gray",
        "brown" => "maroon",
        _ => return None,
    })
}

fn parse_style_kv(arrow: &mut Arrow, p: &str) {
    let norm = p.replace(' ', "");
    if let Some(v) = norm.strip_prefix("bendleft") {
        arrow.bend = Some(v.strip_prefix('=').and_then(|x| x.parse().ok()).unwrap_or(30.0));
    } else if let Some(v) = norm.strip_prefix("bendright") {
        arrow.bend =
            Some(-v.strip_prefix('=').and_then(|x| x.parse::<f64>().ok()).unwrap_or(30.0));
    } else if let Some(v) = norm.strip_prefix("shiftleft") {
        arrow.shift = Some(shift_len(v));
    } else if let Some(v) = norm.strip_prefix("shiftright") {
        arrow.shift = Some(-shift_len(v));
    } else if let Some(v) = norm.strip_prefix("shorten<=") {
        arrow.shorten_start = len_pt(v);
    } else if let Some(v) = norm.strip_prefix("shorten>=") {
        arrow.shorten_end = len_pt(v);
    } else if let Some(v) = norm.strip_prefix("shorten=") {
        arrow.shorten_start = len_pt(v);
        arrow.shorten_end = len_pt(v);
    } else if let Some(c) = named_color(norm.as_str()) {
        arrow.color = Some(c);
    } else if let Some(v) = norm.strip_prefix("color=") {
        if let Some(c) = named_color(v) {
            arrow.color = Some(c);
        }
    } else if norm.starts_with("linewidth")
        || norm.starts_with("out=")
        || norm.starts_with("in=")
        || norm.starts_with("startanchor")
        || norm.starts_with("endanchor")
        || norm.starts_with("opacity=")
        || norm.starts_with("draw=")
        || norm.starts_with("nodes=")
        || matches!(norm.as_str(), "thick" | "thin" | "roundedcorners" | "controls" | "white")
    {
        // cosmetic / path-shaping options with no fletcher analogue
    } else if !p.is_empty() {
        arrow.warnings.push(p.to_string());
    }
}

fn len_pt(v: &str) -> f64 {
    let num: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let n: f64 = num.parse().unwrap_or(0.0);
    if v.ends_with("ex") {
        n * 4.3
    } else if v.ends_with("em") {
        n * 10.0
    } else if v.ends_with("cm") {
        n * 28.35
    } else if v.ends_with("mm") {
        n * 2.835
    } else {
        n
    }
}

fn shift_len(v: &str) -> f64 {
    let v = v.strip_prefix('=').unwrap_or("").trim();
    if v.is_empty() {
        return 3.0;
    }
    len_pt(v).clamp(1.0, 30.0)
}

// --------------------------------------------------------- cell parsing

/// Extract every `\ar`/`\arrow` command from a cell; returns the
/// remaining node tex.
fn extract_arrows(cell: &str, r: i32, c: i32, arrows: &mut Vec<Arrow>) -> Result<String, String> {
    let b: Vec<char> = cell.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let is_cmd = b[i] == '\\'
            && (b[i..].iter().collect::<String>().starts_with("\\arrow")
                || (b[i..].iter().collect::<String>().starts_with("\\ar")
                    && !b
                        .get(i + 3)
                        .map(|c| c.is_ascii_alphabetic())
                        .unwrap_or(false)));
        if !is_cmd {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let mut j = i + if b[i..].iter().collect::<String>().starts_with("\\arrow") { 6 } else { 3 };
        while j < b.len() && b[j].is_whitespace() {
            j += 1;
        }
        let mut arrow = Arrow::at(r, c);
        let mut positioned = false;
        let mut had_bracket = false;
        if let Some((args, end)) = read_group(&b, j, '[', ']') {
            positioned = parse_arrow_args(&mut arrow, &args, r, c);
            had_bracket = true;
            j = end;
            while j < b.len() && b[j].is_whitespace() {
                j += 1;
            }
        }
        if !positioned {
            // old style: \ar{dir}[mods]{label}, or hybrid \ar[mapsto]{r}
            if let Some((dir, end)) = read_group(&b, j, '{', '}') {
                apply_dir(&mut arrow, dir.trim(), r, c);
                j = end;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                let mut side = Side::Left;
                if let Some((mods, end)) = read_group(&b, j, '[', ']') {
                    if mods.contains("swap") {
                        side = Side::Right;
                    }
                    j = end;
                    while j < b.len() && b[j].is_whitespace() {
                        j += 1;
                    }
                }
                if let Some((tex, end)) = read_group(&b, j, '{', '}') {
                    let tex = strip_braces(&tex).to_string();
                    if !tex.trim().is_empty() {
                        arrow.labels.push(Label {
                            tex,
                            side,
                            pos: None,
                            sloped: false,
                            name: None,
                        });
                    }
                    j = end;
                }
            } else if !had_bracket {
                return Err("ar-without-args".into());
            }
        }
        arrows.push(arrow);
        i = j;
    }
    Ok(out.trim().to_string())
}

// ------------------------------------------------------------- options

fn spacing_from_opts(opts: &str) -> (f64, f64) {
    // defaults match the array emitter's diagram spacing
    let (mut col, mut row) = (2.6f64 * 11.0, 2.2 * 11.0); // in pt (11pt em)
    for tok in split_args(opts) {
        let t = tok.trim().replace(' ', "");
        let apply = |v: &str, cur: f64| -> f64 {
            match v {
                "tiny" => cur * 0.35,
                "scriptsize" => cur * 0.5,
                "small" => cur * 0.7,
                "normal" => cur,
                "large" => cur * 1.3,
                "huge" => cur * 1.7,
                _ => {
                    if let Some(inner) = v.strip_prefix("{betweenorigins,") {
                        let n: f64 = inner
                            .trim_end_matches('}')
                            .trim_end_matches("pt")
                            .parse()
                            .unwrap_or(40.0);
                        (n - 16.0).max(6.0)
                    } else {
                        let n: f64 = v.trim_end_matches("pt").parse().unwrap_or(cur);
                        n.max(4.0)
                    }
                }
            }
        };
        if let Some(v) = t.strip_prefix("columnsep=") {
            col = apply(v, col);
        } else if let Some(v) = t.strip_prefix("rowsep=") {
            row = apply(v, row);
        } else if let Some(v) = t.strip_prefix("sep=") {
            col = apply(v, col);
            row = apply(v, row);
        }
    }
    (col, row)
}

// -------------------------------------------------------------- output

fn fmt_f(x: f64) -> String {
    if (x - x.round()).abs() < 1e-9 {
        format!("{}", x.round() as i64)
    } else {
        format!("{:.2}", x)
    }
}

fn coord_str(rc: (f64, f64)) -> String {
    format!("({}, {})", fmt_f(rc.1), fmt_f(rc.0))
}

fn resolve(c: &Coord, anchors: &std::collections::HashMap<String, (f64, f64)>) -> Option<(f64, f64)> {
    match c {
        Coord::Cell(r, c) => Some((*r as f64, *c as f64)),
        Coord::Name(n) => anchors.get(n).copied(),
    }
}

/// Convert a full `\begin{tikzcd}...\end{tikzcd}` block. Returns the
/// `#diagram(...)` code and any warnings for ignored options.
pub(crate) fn tikzcd_to_fletcher(src: &str) -> Result<(String, Vec<String>), String> {
    let src = normalize_shortcuts(&strip_comments(src));
    let begin = src.find("\\begin{tikzcd}").ok_or("no-begin")?;
    let after = begin + "\\begin{tikzcd}".len();
    let b: Vec<char> = src[after..].chars().collect();
    let mut k = 0;
    while k < b.len() && b[k].is_whitespace() {
        k += 1;
    }
    let (opts, body_start) = match read_group(&b, k, '[', ']') {
        Some((o, e)) => (o, e),
        None => (String::new(), 0),
    };
    let body: String = b[body_start..].iter().collect();
    let body = body
        .split("\\end{tikzcd}")
        .next()
        .ok_or("no-end")?
        .to_string();
    if body.contains("\\begin{tikz") {
        return Err("nested-tikz".into());
    }

    // active cell separator: & normally, \& under ampersand replacement
    let amp_repl = opts.replace(' ', "").contains("ampersandreplacement=\\&");
    let body = mask_amps(&body, amp_repl);
    let restore = |s: &str| s.replace('\u{E011}', "\\&");

    let mut arrows: Vec<Arrow> = Vec::new();
    let mut nodes: Vec<(i32, i32, String)> = Vec::new();
    for (r, rowsrc) in split_top(&body, true).iter().enumerate() {
        if rowsrc.trim().is_empty() {
            continue;
        }
        for (c, cell) in split_top(rowsrc, false).iter().enumerate() {
            // `&[-30pt]`: column-spacing adjustment riding the separator
            let dim_re =
                regex::Regex::new(r"^\s*\[\s*-?[\d.]+\s*(pt|em|ex|cm|mm)?\s*\]").unwrap();
            let cell = dim_re.replace(cell, "").to_string();
            let tex = extract_arrows(&cell, r as i32, c as i32, &mut arrows)?;
            let tex = restore(&tex);
            if !tex.trim().is_empty() {
                nodes.push((r as i32, c as i32, tex.trim().to_string()));
            }
        }
    }
    if nodes.is_empty() && arrows.is_empty() {
        return Err("empty".into());
    }

    let (colsp, rowsp) = spacing_from_opts(&opts);

    // resolve name= anchors to points on their carrying arrow; a bent
    // carrier displaces the point sideways by the arc's sagitta
    let mut anchors = std::collections::HashMap::new();
    for a in &arrows {
        for l in &a.labels {
            if let Some(name) = &l.name {
                if let (Some(f), Some(t)) = (resolve(&a.from, &anchors), resolve(&a.to, &anchors)) {
                    let t01 = l.pos.unwrap_or(0.5);
                    let mut p = (f.0 + t01 * (t.0 - f.0), f.1 + t01 * (t.1 - f.1));
                    if let Some(bend) = a.bend {
                        // pt-space chord and its left normal
                        let (dx, dy) = ((t.1 - f.1) * colsp, (t.0 - f.0) * rowsp);
                        let len = dx.hypot(dy).max(1.0);
                        let th = bend.to_radians();
                        let sag = len * (1.0 - th.cos()).abs() / (2.0 * th.sin().abs().max(0.05));
                        let sag = sag * bend.signum();
                        let (nx, ny) = (dy / len, -dx / len);
                        p = (p.0 + ny * sag / rowsp, p.1 + nx * sag / colsp);
                    }
                    anchors.insert(name.clone(), p);
                }
            }
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for (r, c, tex) in &nodes {
        let cleaned = clean_tex(tex);
        let gauge = regex::Regex::new(r"\\[hv]?phantom\s*\{[^{}]*\}")
            .unwrap()
            .replace_all(&cleaned, "")
            .to_string();
        if gauge.trim_matches(|c: char| c.is_whitespace() || c == '{' || c == '}').is_empty() {
            continue;
        }
        let label = emit::ts(&format!("\\displaystyle {}", cleaned));
        lines.push(format!(
            "  node(({}, {}), mi({})),",
            c, r, label
        ));
    }
    for a in &arrows {
        let from = resolve(&a.from, &anchors).ok_or("unresolved-anchor")?;
        let to = resolve(&a.to, &anchors).ok_or("unresolved-anchor")?;
        warnings.extend(a.warnings.iter().cloned());

        // geometry adjustments, in pt space (x right, y down)
        let is_anchor =
            matches!(a.from, Coord::Name(_)) || matches!(a.to, Coord::Name(_));
        let (x1, y1) = (from.1 * colsp, from.0 * rowsp);
        let (x2, y2) = (to.1 * colsp, to.0 * rowsp);
        let (dx, dy) = (x2 - x1, y2 - y1);
        let len = dx.hypot(dy);
        if len < 0.01 && a.bend.is_none() {
            continue; // degenerate (loops we can't draw); drop quietly
        }
        let (mut xa, mut ya, mut xb, mut yb) = (x1, y1, x2, y2);
        let mut shift_arg = a.shift;
        let mut moved = false;
        if is_anchor && len > 0.01 {
            // 2-cells between arrows: honor shorten, keep clear of the
            // carriers, and never collapse below a legible length
            let mut s1 = a.shorten_start + 4.0;
            let mut s2 = a.shorten_end + 4.0;
            if len - s1 - s2 < 12.0 {
                let s = (len - 12.0) / 2.0;
                s1 = s;
                s2 = s;
            }
            xa += dx / len * s1;
            ya += dy / len * s1;
            xb -= dx / len * s2;
            yb -= dy / len * s2;
            moved = true;
        }
        if let Some(sh) = shift_arg {
            let axis = from.0 == to.0 || from.1 == to.1;
            if axis && sh.abs() > 8.0 && len > 0.01 {
                // a rail detached from the nodes (adjoint-triple style);
                // fletcher would draw a shifted center-to-center line
                let (nx, ny) = (dy / len, -dx / len);
                let inset = (len * 0.2).min(14.0);
                xa += nx * sh + dx / len * inset;
                ya += ny * sh + dy / len * inset;
                xb += nx * sh - dx / len * inset;
                yb += ny * sh - dy / len * inset;
                shift_arg = None;
                moved = true;
            }
        }
        let (from, to) = if moved {
            ((ya / rowsp, xa / colsp), (yb / rowsp, xb / colsp))
        } else {
            (from, to)
        };

        let mut args = vec![
            coord_str(from),
            coord_str(to),
            format!("\"{}\"", a.final_mark()),
        ];
        let mut extra_labels: Vec<&Label> = Vec::new();
        let mut first = true;
        let swapped: Vec<Label> = a
            .labels
            .iter()
            .map(|l| {
                let mut l = l.clone();
                if a.swap {
                    l.side = match l.side {
                        Side::Left => Side::Right,
                        Side::Right => Side::Left,
                        Side::Center => Side::Center,
                    };
                }
                l
            })
            .collect();
        for l in &swapped {
            if l.tex.trim().is_empty() {
                continue; // pure name= anchor
            }
            if first {
                push_label_args(&mut args, l, a.stroke_none, a.color);
                first = false;
            } else {
                extra_labels.push(l);
            }
        }
        if let Some(bend) = a.bend {
            args.push(format!("bend: {}deg", fmt_f(bend)));
        }
        if let Some(shift) = shift_arg {
            args.push(format!("shift: {}pt", fmt_f(shift)));
        }
        if a.crossing {
            args.push("crossing: true".into());
        }
        if a.stroke_none {
            args.push("stroke: none".into());
        } else if let Some(c) = a.color {
            args.push(format!("stroke: {}", c));
        }
        lines.push(format!("  edge({}),", args.join(", ")));
        for l in extra_labels {
            let mut args = vec![coord_str(from), coord_str(to), "\"-\"".to_string()];
            push_label_args(&mut args, l, true, a.color);
            if let Some(bend) = a.bend {
                args.push(format!("bend: {}deg", fmt_f(bend)));
            }
            args.push("stroke: none".into());
            lines.push(format!("  edge({}),", args.join(", ")));
        }
    }

    Ok((
        format!(
            "#diagram(\n  spacing: ({}pt, {}pt),\n{}\n)\n",
            fmt_f(colsp),
            fmt_f(rowsp),
            lines.join("\n")
        ),
        warnings,
    ))
}

fn push_label_args(args: &mut Vec<String>, l: &Label, centered: bool, color: Option<&str>) {
    let fill = color.map(|c| format!("fill: {}, ", c)).unwrap_or_default();
    args.push(format!(
        "label: text(0.75em, {}mi({}))",
        fill,
        emit::ts(&clean_tex(&l.tex))
    ));
    let side = if centered && l.side == Side::Left {
        Side::Center
    } else {
        l.side
    };
    args.push(format!(
        "label-side: {}",
        match side {
            Side::Left => "left",
            Side::Right => "right",
            Side::Center => "center",
        }
    ));
    args.push("label-sep: 0.1em".into());
    if let Some(pos) = l.pos {
        args.push(format!("label-pos: {}", fmt_f(pos)));
    }
    if l.sloped {
        args.push("label-angle: auto".into());
    }
}

/// Normalize `\rar`, `\dar`, `\dlar`, ... shortcuts to `\ar{d}` form.
pub(crate) fn normalize_shortcuts(src: &str) -> String {
    let re = regex::Regex::new(r"\\([rdlu]{1,2})ar\b").unwrap();
    re.replace_all(src, "\\ar{$1}").to_string()
}

/// `\mbox{$B_2$ Kan complex}` -> `B_2 \text{ Kan complex}`: text-mode
/// boxes with embedded inline math become alternating text/math runs.
fn split_mbox(s: &str) -> String {
    let mut s = s.to_string();
    for cmd in ["\\mbox", "\\text"] {
        let mut from = 0;
        while let Some(rel) = s[from..].find(cmd) {
            let pos = from + rel;
            let b: Vec<char> = s[pos + cmd.len()..].chars().collect();
            let mut k = 0;
            while k < b.len() && b[k].is_whitespace() {
                k += 1;
            }
            let Some((inner, end)) = read_group(&b, k, '{', '}') else {
                from = pos + cmd.len();
                continue;
            };
            if !inner.contains('$') {
                from = pos + cmd.len();
                continue;
            }
            let mut out = String::new();
            for (i, seg) in inner.split('$').enumerate() {
                if seg.is_empty() {
                    continue;
                }
                if i % 2 == 1 {
                    out.push_str(seg); // math run
                } else {
                    out.push_str(&format!("\\text{{{}}}", seg));
                }
                out.push(' ');
            }
            let tail: String = b[end..].iter().collect();
            s = format!("{}{}{}", &s[..pos], out.trim_end(), tail);
            from = pos;
        }
    }
    s
}

/// Drop wrappers mitex has no handler for; keep their visible argument.
fn clean_tex(s: &str) -> String {
    let mut s = split_mbox(s);
    s = s.replace("\\mbox", "\\text");
    s = s.replace("\\shortmid", "\\vert");
    s = s.replace("\\\"", "\"");
    s = s.replace("\\rmfamily", "");
    for (cmd, keep_last_of) in [
        ("\\scalebox", 2),
        ("\\rotatebox", 2),
        ("\\mathcolor", 2),
        ("\\raisebox", 2),
        ("\\mathclap", 1),
        ("\\clap", 1),
    ] {
        while let Some(pos) = s.find(cmd) {
            let b: Vec<char> = s[pos + cmd.len()..].chars().collect();
            let mut idx = 0usize;
            let mut kept = None;
            for k in 0..keep_last_of {
                while idx < b.len() && b[idx].is_whitespace() {
                    idx += 1;
                }
                match read_group(&b, idx, '{', '}') {
                    Some((inner, end)) => {
                        idx = end;
                        if k == keep_last_of - 1 {
                            kept = Some(inner);
                        }
                    }
                    None => break,
                }
            }
            match kept {
                Some(inner) => {
                    let tail: String = b[idx..].iter().collect();
                    s = format!("{}{}{}", &s[..pos], inner, tail);
                }
                None => {
                    s = s.replacen(cmd, "", 1);
                }
            }
        }
    }
    // inline math dollars surface when a text-mode wrapper is unwrapped
    let mut out = String::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' && i + 1 < b.len() {
            out.push(b[i]);
            out.push(b[i + 1]);
            i += 2;
            continue;
        }
        if b[i] != '$' {
            out.push(b[i]);
        }
        i += 1;
    }
    out
}
