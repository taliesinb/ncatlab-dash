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
    rotate: Option<f64>,
    marking: bool,
    xshift: f64, // pt, for name= anchor placement
    yshift: f64,
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
    color: Option<String>,
    shorten_start: f64, // pt
    shorten_end: f64,
    loop_bend: Option<f64>, // self-loop, bend degrees
    out_angle: Option<f64>, // tikz out= departure angle
    to_path: Option<(Vec<PathPt>, Option<(String, f64)>)>,
    label_pos_hint: Option<f64>, // bare "near start" seen before a label
    warnings: Vec<String>,
}

/// One vertex of a `to path={ ... -- ... }` detour, as an offset from
/// the start or target node (pt, y down).
#[derive(Clone)]
struct PathPt {
    on_target: bool,
    anchor: char, // s/n/e/w/c
    dx: f64,
    dy: f64,
}

/// Parse the common tikz detour idiom:
/// `to path={ ([yshift=-12pt]\tikztostart.south) -- node[below]{L} ... }`
fn parse_to_path(p: &str) -> Option<(Vec<PathPt>, Option<(String, f64)>)> {
    let body = p.split_once('{')?.1.rsplit_once('}')?.0;
    let mut pts = Vec::new();
    let mut label: Option<(String, f64)> = None;
    for seg in split_double_dash(body) {
        let b: Vec<char> = seg.trim().chars().collect();
        let mut k = 0usize;
        if b.get(..4).map(|w| w.iter().collect::<String>()) == Some("node".into()) {
            // node[mods]{tex} riding on this segment
            k = 4;
            let mut dy = 0.0;
            if let Some((mods, end)) = read_group(&b, k, '[', ']') {
                for m in mods.split(',') {
                    let m = m.trim().replace(' ', "");
                    if m == "below" {
                        dy += 7.0;
                    } else if m == "above" {
                        dy -= 7.0;
                    } else if let Some(v) = m.strip_prefix("yshift=") {
                        dy -= len_pt(v);
                    }
                }
                k = end;
            }
            while k < b.len() && b[k].is_whitespace() {
                k += 1;
            }
            if let Some((tex, end)) = read_group(&b, k, '{', '}') {
                label =
                    Some((strip_braces(&tex).to_string(), if dy == 0.0 { 7.0 } else { dy }));
                k = end;
            }
            while k < b.len() && b[k].is_whitespace() {
                k += 1;
            }
        }
        if k >= b.len() {
            continue;
        }
        let (inner, _) = read_group(&b[k..], 0, '(', ')')?;
        let inner = inner.trim();
        let (opts, rest) = if inner.starts_with('[') {
            let bi: Vec<char> = inner.chars().collect();
            let (o, e) = read_group(&bi, 0, '[', ']')?;
            (o, inner[e..].trim().to_string())
        } else {
            (String::new(), inner.to_string())
        };
        let (mut dx, mut dy) = (0.0, 0.0);
        for m in opts.split(',') {
            let m = m.trim().replace(' ', "");
            if let Some(v) = m.strip_prefix("yshift=") {
                dy -= len_pt(v); // tikz y up, ours down
            } else if let Some(v) = m.strip_prefix("xshift=") {
                dx += len_pt(v);
            }
        }
        let on_target = rest.contains("tikztotarget");
        if !on_target && !rest.contains("tikztostart") {
            return None; // absolute coordinates: out of scope
        }
        let anchor = rest
            .rsplit('.')
            .next()
            .and_then(|a| a.trim().chars().next())
            .filter(|c| "snewc".contains(*c) && rest.contains('.'))
            .unwrap_or('c');
        pts.push(PathPt { on_target, anchor, dx, dy });
    }
    if pts.len() < 2 {
        return None;
    }
    // tikz begins the path at \tikztostart implicitly; give the rail
    // its entry stub when the author's first point is already offset
    let f = &pts[0];
    if f.on_target || f.dx != 0.0 || f.dy != 0.0 {
        let anchor = if f.on_target { 'c' } else { f.anchor };
        pts.insert(0, PathPt { on_target: false, anchor, dx: 0.0, dy: 0.0 });
    }
    Some((pts, label))
}

/// Split on top-level `--` (outside any parens/braces/brackets).
fn split_double_dash(s: &str) -> Vec<String> {
    let b: Vec<char> = s.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            '-' if depth == 0 && b.get(i + 1) == Some(&'-') => {
                parts.push(cur.clone());
                cur.clear();
                i += 2;
                continue;
            }
            _ => {}
        }
        cur.push(b[i]);
        i += 1;
    }
    parts.push(cur);
    parts
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
            loop_bend: None,
            out_angle: None,
            to_path: None,
            label_pos_hint: None,
            warnings: Vec::new(),
        }
    }

    fn final_mark(&self) -> String {
        if self.no_head {
            // `Rightarrow, no head` is the tikzcd idiom for a double line
            return if self.mark == "=>" {
                "=".into()
            } else if self.dotted {
                "..".into()
            } else if self.dashed {
                "--".into()
            } else {
                "-".into()
            };
        }
        if self.dashed && self.mark == "->" {
            "-->".into()
        } else if self.dotted && self.mark == "->" {
            "..>".into()
        } else if self.dashed && self.mark == "-" {
            "--".into()
        } else if self.dotted && self.mark == "-" {
            "..".into()
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
fn split_top(s: &str, rows: bool) -> (Vec<String>, Vec<f64>) {
    let b: Vec<char> = s.chars().collect();
    let mut parts = Vec::new();
    let mut gaps: Vec<f64> = vec![0.0]; // gaps[r] = \\[dim] before row r
    let mut cur = String::new();
    let (mut brace, mut env) = (0i32, 0i32);
    let mut i = 0;
    let starts = |i: usize, word: &str| {
        b[i..].iter().take(word.len()).collect::<String>() == word
    };
    while i < b.len() {
        let c = b[i];
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
        // \ar[...] / \arrow[...] argument brackets are consumed whole
        // (quote-aware) so their commas, &s and \\s never split cells;
        // any OTHER bracket is math (\big[ ...) and ignored entirely
        let is_ar = c == '\\'
            && (starts(i, "\\arrow")
                || (starts(i, "\\ar")
                    && !b.get(i + 3).map(|c| c.is_ascii_alphabetic()).unwrap_or(false)));
        if is_ar {
            let n = if starts(i, "\\arrow") { 6 } else { 3 };
            for k in 0..n {
                cur.push(b[i + k]);
            }
            i += n;
            while i < b.len() && b[i].is_whitespace() {
                cur.push(b[i]);
                i += 1;
            }
            if i < b.len() && b[i] == '[' {
                let mut depth = 0i32;
                let mut quote = false;
                while i < b.len() {
                    let ch = b[i];
                    cur.push(ch);
                    if ch == '"' {
                        quote = !quote;
                    } else if !quote {
                        if ch == '\\' && i + 1 < b.len() {
                            cur.push(b[i + 1]);
                            i += 2;
                            continue;
                        }
                        if ch == '[' {
                            depth += 1;
                        } else if ch == ']' {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                    }
                    i += 1;
                }
            }
            continue;
        }
        if c == '\\' && i + 1 < b.len() {
            if rows && b[i + 1] == '\\' && brace == 0 {
                parts.push(cur.clone());
                cur.clear();
                i += 2;
                // optional row-spacing option \\[6pt]
                while i < b.len() && b[i].is_whitespace() {
                    i += 1;
                }
                let mut gap = 0.0;
                if i < b.len() && b[i] == '[' {
                    let mut dim = String::new();
                    i += 1;
                    while i < b.len() && b[i] != ']' {
                        dim.push(b[i]);
                        i += 1;
                    }
                    i += 1;
                    gap = len_pt(dim.trim());
                }
                gaps.push(gap);
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
            _ => {}
        }
        if !rows && c == AMP && brace == 0 {
            parts.push(cur.clone());
            cur.clear();
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    parts.push(cur);
    while gaps.len() < parts.len() {
        gaps.push(0.0);
    }
    (parts, gaps)
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
            "description" => label.side = Side::Center,
            "anchor=center" | "marking" => {
                label.side = Side::Center;
                label.marking = true;
            }
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
            _ if m.starts_with("rotate=") => {
                label.rotate = m[7..].trim().parse().ok();
            }
            _ if m.starts_with("xshift=") => {
                label.xshift = len_pt(m[7..].trim());
            }
            _ if m.starts_with("yshift=") => {
                label.yshift = len_pt(m[7..].trim());
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
        rotate: None,
        marking: false,
        xshift: 0.0,
        yshift: 0.0,
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
    // rotated turnstiles render badly through rotate(); use the glyph
    if let Some(deg) = label.rotate {
        let t = label.tex.trim();
        let glyph = match (t, deg.round() as i64) {
            ("\\dashv", -90) | ("\\vdash", 90) => Some("\\bot"),
            ("\\dashv", 90) | ("\\vdash", -90) => Some("\\top"),
            _ => None,
        };
        if let Some(g) = glyph {
            label.tex = g.to_string();
            label.rotate = None;
        }
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
        } else if matches!(
            p,
            "near start" | "very near start" | "at start" | "near end" | "very near end"
                | "at end"
        ) {
            let pos = match p {
                "near start" => 0.25,
                "very near start" => 0.1,
                "at start" => 0.0,
                "near end" => 0.75,
                "very near end" => 0.9,
                _ => 1.0,
            };
            if let Some(l) = arrow.labels.last_mut() {
                if l.pos.is_none() {
                    l.pos = Some(pos);
                }
            } else {
                arrow.label_pos_hint = Some(pos);
            }
        } else if p.starts_with('"') {
            if let Some(mut l) = parse_label(p) {
                if l.pos.is_none() {
                    l.pos = arrow.label_pos_hint.take();
                }
                arrow.labels.push(l);
            }
        } else {
            parse_style(arrow, p);
        }
    }
    positioned
}

fn parse_style(arrow: &mut Arrow, p: &str) {
    if p.trim_start().starts_with("to path") {
        arrow.to_path = parse_to_path(p);
        if arrow.to_path.is_none() {
            arrow.warnings.push("to path (unparsed)".into());
        }
        return;
    }
    match p {
        "Rightarrow" => arrow.mark = "=>".into(),
        "Leftarrow" => arrow.mark = "<=".into(),
        "Leftrightarrow" => arrow.mark = "<=>".into(),
        "-" | "dash" => arrow.mark = "-".into(),
        "-Latex" | "-latex" | "-Stealth" | "-stealth" => arrow.mark = "->".into(),
        "Latex-" | "latex-" | "Stealth-" | "stealth-" => arrow.mark = "<-".into(),
        "no head" => arrow.no_head = true,
        "equals" | "equal" | "Equal" => arrow.mark = "=".into(),
        "->>" | "two heads" | "twoheadrightarrow" => arrow.mark = "->>".into(),
        ">->" | "tail" | "rightarrowtail" => arrow.mark = ">->".into(),
        "hook" | "hookrightarrow" => arrow.mark = "hook->".into(),
        "hook'" => arrow.mark = "hook'->".into(),
        "mapsto" | "maps to" | "|->" => arrow.mark = "|->".into(),
        "<-|" => arrow.mark = "<-|".into(),
        "<-" => arrow.mark = "<-".into(),
        "<->" | "leftrightarrow" => arrow.mark = "<->".into(),
        "-->" | "dashrightarrow" => arrow.dashed = true,
        "dashed" => arrow.dashed = true,
        "dotted" => arrow.dotted = true,
        "phantom" => {
            arrow.stroke_none = true;
            arrow.mark = "-".into();
        }
        "loop" | "loop above" | "loop left" => arrow.loop_bend = Some(130.0),
        "loop below" | "loop right" => arrow.loop_bend = Some(-130.0),
        "draw=none" => arrow.stroke_none = true,
        "crossing over" => arrow.crossing = true,
        _ => parse_style_kv(arrow, p),
    }
}

/// quiver-style `{rgb,255:red,225;green,5;blue,23}` color specs
fn rgb_color(v: &str) -> Option<String> {
    let v = v.trim_start_matches('{').trim_end_matches('}');
    if !v.starts_with("rgb") {
        return None;
    }
    let comps = v.split_once(':')?.1;
    let mut rgb = [0u32; 3];
    for part in comps.split(';') {
        let (name, val) = part.split_once(',')?;
        let val: u32 = val.trim().parse().ok()?;
        match name.trim() {
            "red" => rgb[0] = val,
            "green" => rgb[1] = val,
            "blue" => rgb[2] = val,
            _ => {}
        }
    }
    Some(format!("rgb({}, {}, {})", rgb[0], rgb[1], rgb[2]))
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
        arrow.color = Some(c.to_string());
    } else if let Some(v) = norm.strip_prefix("color=") {
        if let Some(c) = named_color(v) {
            arrow.color = Some(c.to_string());
        } else if let Some(c) = rgb_color(v) {
            arrow.color = Some(c);
        }
    } else if let Some(v) = norm.strip_prefix("draw=") {
        if let Some(c) = named_color(v) {
            arrow.color = Some(c.to_string());
        }
    } else if let Some(v) = norm.strip_prefix("out=") {
        // keep the departure angle: it orients a bare out=/in= self-loop
        arrow.out_angle = eval_angle(v);
    } else if norm.starts_with("linewidth")
        || norm.starts_with("distance=")
        || norm.starts_with("looseness=")
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

/// Evaluate the simple angle arithmetic seen in the corpus (`180-50`).
fn eval_angle(v: &str) -> Option<f64> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    if let Some((a, b)) = v[1..].split_once('-') {
        let head = &v[..1];
        return Some(format!("{}{}", head, a).parse::<f64>().ok()? - b.trim().parse::<f64>().ok()?);
    }
    if let Some((a, b)) = v.split_once('+') {
        return Some(a.trim().parse::<f64>().ok()? + b.trim().parse::<f64>().ok()?);
    }
    v.parse().ok()
}

fn len_pt(v: &str) -> f64 {
    let v = v.strip_prefix('+').unwrap_or(v);
    let num: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let n: f64 = num.parse().unwrap_or(0.0);
    if v.ends_with("ex") {
        n * 4.3
    } else if v.ends_with("em") {
        n * 11.0
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
    let has_unit = v.ends_with("pt") || v.ends_with("em") || v.ends_with("ex")
        || v.ends_with("cm") || v.ends_with("mm");
    let n = len_pt(v);
    let n = if has_unit { n } else { n * 3.5 };
    // clamp the magnitude only: shift right=-20pt is a LEFT shift
    n.signum() * n.abs().clamp(1.0, 45.0)
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
                // any number of [mods]{label} pairs may follow the
                // direction: \ar{dl}{Sp}[swap]{\perp}
                let mut side = Side::Left;
                loop {
                    while j < b.len() && b[j].is_whitespace() {
                        j += 1;
                    }
                    if let Some((mods, end)) = read_group(&b, j, '[', ']') {
                        side = if mods.contains("swap") {
                            Side::Right
                        } else {
                            Side::Left
                        };
                        j = end;
                        continue;
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
                                rotate: None,
                                marking: false,
                                xshift: 0.0,
                                yshift: 0.0,
                            });
                        }
                        j = end;
                        side = Side::Right;
                        continue;
                    }
                    break;
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
                        let n = len_pt(inner.trim_end_matches('}'));
                        (n - 16.0).max(6.0)
                    } else if v.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') {
                        len_pt(v).max(2.0)
                    } else {
                        cur
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

/// Estimated physical layout of the fletcher grid: column/row centers
/// in pt, derived from spacing plus estimated node sizes, with linear
/// interpolation for fractional (anchor) coordinates.
struct PhysGrid {
    xs: Vec<f64>,
    ys: Vec<f64>,
    colsp: f64,
    rowsp: f64,
}

fn interp(cs: &[f64], step: f64, v: f64) -> f64 {
    if cs.len() < 2 {
        return v * step + cs.first().copied().unwrap_or(0.0);
    }
    let last = cs.len() - 1;
    if v <= 0.0 {
        return cs[0] + v * (cs[1] - cs[0]);
    }
    if v >= last as f64 {
        return cs[last] + (v - last as f64) * (cs[last] - cs[last - 1]);
    }
    let i = v.floor() as usize;
    cs[i] + (v - i as f64) * (cs[i + 1] - cs[i])
}

fn uninterp(cs: &[f64], step: f64, x: f64) -> f64 {
    if cs.len() < 2 {
        let base = cs.first().copied().unwrap_or(0.0);
        return if step > 0.0 { (x - base) / step } else { 0.0 };
    }
    let last = cs.len() - 1;
    if x <= cs[0] {
        return (x - cs[0]) / (cs[1] - cs[0]);
    }
    if x >= cs[last] {
        return last as f64 + (x - cs[last]) / (cs[last] - cs[last - 1]);
    }
    for i in 0..last {
        if x <= cs[i + 1] {
            return i as f64 + (x - cs[i]) / (cs[i + 1] - cs[i]);
        }
    }
    last as f64
}

impl PhysGrid {
    fn x(&self, c: f64) -> f64 {
        interp(&self.xs, self.colsp, c)
    }
    fn y(&self, r: f64) -> f64 {
        interp(&self.ys, self.rowsp, r)
    }
    fn rx(&self, x: f64) -> f64 {
        uninterp(&self.xs, self.colsp, x)
    }
    fn ry(&self, y: f64) -> f64 {
        uninterp(&self.ys, self.rowsp, y)
    }
}

/// Crude rendered-width estimate (pt at 11pt math) for rail anchoring:
/// tikz clips shifted arrows at node borders, fletcher can't, so we
/// guess the border. Multi-line cells measure their widest row.

/// Measure the real rendered size of each cell by compiling a probe
/// document with typst and querying `measure()` results. Falls back to
/// None (callers keep the glyph-count estimate) on any failure or when
/// NLAB_MEASURE=0.
fn measure_nodes(labels_ts: &[String]) -> Option<Vec<(f64, f64)>> {
    if labels_ts.is_empty()
        || std::env::var("NLAB_MEASURE").map(|v| v == "0").unwrap_or(false)
    {
        return None;
    }
    let mut doc = String::from(
        "#import \"@local/mitex:0.2.7\": mi-itex\n#set text(size: 11pt)\n#context {\n  let ms = (\n",
    );
    for l in labels_ts {
        doc.push_str(&format!("    measure(mi-itex({})),\n", l));
    }
    doc.push_str(
        "  )\n  [#metadata(ms.map(s => (s.width.pt(), s.height.pt())))<m>]\n}\n",
    );
    let path = std::env::temp_dir().join(format!(
        "nlab-measure-{}-{}.typ",
        std::process::id(),
        labels_ts.len()
    ));
    std::fs::write(&path, doc).ok()?;
    let out = std::process::Command::new("typst")
        .args(["query", path.to_str()?, "<m>", "--field", "value", "--one"])
        .output()
        .ok()?;
    let _ = std::fs::remove_file(&path);
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let re = regex::Regex::new(r"[0-9]+(?:\.[0-9]+)?").unwrap();
    let nums: Vec<f64> = re
        .find_iter(&text)
        .filter_map(|m| m.as_str().parse().ok())
        .collect();
    if nums.len() != labels_ts.len() * 2 {
        return None;
    }
    Some(nums.chunks(2).map(|c| (c[0], c[1])).collect())
}

fn est_halfwidth_pt(tex: &str) -> f64 {
    fn row_width(row: &str) -> f64 {
        let b: Vec<char> = row.chars().collect();
        let mut w = 0.0;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == '\\' {
                let mut j = i + 1;
                let mut name = String::new();
                while j < b.len() && b[j].is_ascii_alphabetic() {
                    name.push(b[j]);
                    j += 1;
                }
                if name.is_empty() {
                    w += 4.0;
                    i += 2;
                    continue;
                }
                w += match name.as_str() {
                    "text" | "mathrm" | "mathbf" | "mathcal" | "mathbb" | "mathsf"
                    | "mathtt" | "mathit" | "mathfrak" | "mathscr" | "big" | "Big"
                    | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr" | "left"
                    | "right" | "displaystyle" | "textstyle" | "scriptstyle" | "begin"
                    | "end" | "array" | "aligned" | "overset" | "underset" => 0.0,
                    "quad" => 11.0,
                    "qquad" => 22.0,
                    _ => 8.0,
                };
                i = j;
                continue;
            }
            w += match c {
                '{' | '}' | '^' | '_' => 0.0,
                c if c.is_whitespace() => 0.0,
                _ => 6.6,
            };
            i += 1;
        }
        w
    }
    tex.split("\\\\").map(row_width).fold(0.0, f64::max) / 2.0
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
    let (body_rows, row_gaps) = split_top(&body, true);
    for (r, rowsrc) in body_rows.iter().enumerate() {
        if rowsrc.trim().is_empty() {
            continue;
        }
        for (c, cell) in split_top(rowsrc, false).0.iter().enumerate() {
            // `&[-30pt]`: column-spacing adjustment riding the separator
            let dim_re =
                regex::Regex::new(r"^\s*\[\s*[+-]?[\d.]+\s*(pt|em|ex|cm|mm)?\s*\]").unwrap();
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

    let (mut colsp, mut rowsp) = spacing_from_opts(&opts);
    // tikzcd spacing is border-to-border: a diagram of bare arrows
    // (all cells empty) still spreads; with no node sizes to add, our
    // spacing IS the arrow length, so widen it
    if nodes.iter().all(|(_, _, t)| {
        t.trim_matches(|c: char| c.is_whitespace() || c == '{' || c == '}').is_empty()
    }) {
        colsp *= 2.2;
        rowsp *= 2.0;
    }

    // drop cells that render to nothing (pure \phantom spacers) BEFORE
    // deciding what the rail anchors below may attach to
    let nodes: Vec<(i32, i32, String)> = nodes
        .into_iter()
        .filter(|(_, _, tex)| {
            !clean_tex(tex)
                .trim_matches(|c: char| c.is_whitespace() || c == '{' || c == '}')
                .is_empty()
        })
        .collect();

    let labels_ts: Vec<String> =
        nodes.iter().map(|(_, _, tex)| emit::ts(&clean_tex(tex))).collect();
    let measured = measure_nodes(&labels_ts);

    let mut node_halfw: std::collections::HashMap<(i32, i32), f64> =
        std::collections::HashMap::new();
    let mut node_halfh: std::collections::HashMap<(i32, i32), f64> =
        std::collections::HashMap::new();
    for (i, (r, c, tex)) in nodes.iter().enumerate() {
        let (w, h) = measured
            .as_ref()
            .map(|m| m[i])
            .unwrap_or_else(|| (2.0 * est_halfwidth_pt(&clean_tex(tex)), 14.0));
        node_halfw.insert((*r, *c), w / 2.0);
        node_halfh.insert((*r, *c), h / 2.0);
    }

    let ncols = 1 + nodes.iter().map(|(_, c, _)| *c).max().unwrap_or(0).max(2) as usize;
    let nrows = 1 + nodes.iter().map(|(r, _, _)| *r).max().unwrap_or(0).max(2) as usize;
    let mut colw = vec![0.0f64; ncols];
    let mut rowh = vec![0.0f64; nrows];
    for ((r, c), hw) in &node_halfw {
        if let Some(w) = colw.get_mut(*c as usize) {
            *w = w.max(2.0 * hw);
        }
        let hh = node_halfh.get(&(*r, *c)).copied().unwrap_or(7.0);
        if let Some(h) = rowh.get_mut(*r as usize) {
            *h = h.max((2.0 * hh).max(10.0));
        }
    }

    // tikzcd widens column separation so horizontal edge labels fit;
    // fletcher doesn't, so pad middle columns with invisible spacers
    let mut spacers: Vec<(i32, i32, f64)> = Vec::new();
    {
        let xs_now = {
            let mut cs = vec![0.0];
            for i in 1..colw.len() {
                cs.push(cs[i - 1] + colw[i - 1] / 2.0 + colsp + colw[i] / 2.0);
            }
            cs
        };
        let mut extra: std::collections::HashMap<i32, f64> = std::collections::HashMap::new();
        let mut extra_row: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
        for a in &arrows {
            let (Coord::Cell(r1, c1), Coord::Cell(r2, c2)) = (&a.from, &a.to) else {
                continue;
            };
            if r1 != r2 || (c2 - c1).abs() < 2 {
                continue;
            }
            let Some(l) = a.labels.iter().find(|l| {
                !label_is_blank(&l.tex)
                    && l.pos.map(|p| (0.15..=0.85).contains(&p)).unwrap_or(true)
            }) else {
                continue;
            };
            let lw = 2.0 * est_halfwidth_pt(&clean_tex(&l.tex)) * 0.75;
            let (lo, hi) = (*c1.min(c2), *c1.max(c2));
            let span = xs_now[hi as usize] - xs_now[lo as usize]
                - colw[lo as usize] / 2.0
                - colw[hi as usize] / 2.0;
            let need = lw + 50.0;
            if span < need {
                let mids: Vec<i32> = ((lo + 1)..hi)
                    .filter(|c| colw.get(*c as usize).copied().unwrap_or(0.0) < 1.0)
                    .collect();
                if !mids.is_empty() {
                    let per = (need - span) / mids.len() as f64;
                    for m in mids {
                        let e = extra.entry(m).or_insert(0.0);
                        if per > *e {
                            *e = per;
                            extra_row.insert(m, *r1);
                        }
                    }
                }
            }
        }
        for (c, w) in extra {
            if let Some(cw) = colw.get_mut(c as usize) {
                *cw = cw.max(w);
            }
            spacers.push((extra_row[&c], c, w));
        }
    }

    // aim single-step diagonal arrows at a legible slope: squashed
    // pentagons and stretched cubes both come from a fixed row spacing
    // that ignores how wide the columns actually are
    let norm_opts = opts.replace(' ', "");
    if !norm_opts.contains("rowsep=") && !norm_opts.contains("sep=") {
        let xs_tmp = {
            let mut cs = vec![0.0];
            for i in 1..colw.len() {
                cs.push(cs[i - 1] + colw[i - 1] / 2.0 + colsp + colw[i] / 2.0);
            }
            cs
        };
        let mut targets: Vec<f64> = Vec::new();
        for a in &arrows {
            if let (Coord::Cell(r1, c1), Coord::Cell(r2, c2)) = (&a.from, &a.to) {
                let (dr, dc) = ((r2 - r1).abs(), (c2 - c1).abs());
                if dr > 0 && dc > 0 {
                    let x1 = xs_tmp.get(*c1 as usize).copied().unwrap_or(0.0);
                    let x2 = xs_tmp.get(*c2 as usize).copied().unwrap_or(0.0);
                    let dx = (x2 - x1).abs();
                    targets.push((0.55 * dx / dr as f64 - 14.0).max(8.0));
                }
            }
        }
        if targets.len() >= 2 {
            targets.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = targets[targets.len() / 2];
            rowsp = med.clamp(rowsp * 0.4, rowsp * 3.5);
        }
    }

    // build the physical grid first: everything below measures in pt
    let centers = |sizes: &[f64], sp: f64| -> Vec<f64> {
        let mut cs = vec![0.0];
        for i in 1..sizes.len() {
            cs.push(cs[i - 1] + sizes[i - 1] / 2.0 + sp + sizes[i] / 2.0);
        }
        cs
    };
    let grid = PhysGrid {
        xs: centers(&colw, colsp),
        ys: centers(&rowh, rowsp),
        colsp,
        rowsp,
    };

    // \\[dim] row-spacing tweaks become fractional row offsets, scaled
    // by the actual physical step between the rows they separate
    let mut row_off: Vec<f64> = Vec::with_capacity(row_gaps.len());
    let mut acc = 0.0;
    for (r, g) in row_gaps.iter().enumerate() {
        let step = if r > 0 && r < grid.ys.len() {
            (grid.ys[r] - grid.ys[r - 1]).max(10.0)
        } else {
            rowsp + 14.0
        };
        // large negative pulls land rows next to earlier content; our
        // row heights differ from tikz's, so cushion the overlap a bit
        let cushion = if *g <= -50.0 { 16.0 } else { 0.0 };
        acc += (g + cushion) / step;
        row_off.push(acc);
    }
    #[allow(clippy::type_complexity)]
    let eff = |p: (f64, f64)| -> (f64, f64) {
        let r = p.0.round();
        let off = if r >= 0.0 {
            row_off.get(r as usize).copied().unwrap_or(acc)
        } else {
            0.0
        };
        (p.0 + off, p.1)
    };

    let mut debug_dots: Vec<(f64, f64)> = Vec::new();
    // resolve name= anchors to points on their carrying arrow, in
    // physical coordinates (mixing pt with grid units broke whenever
    // rows were non-uniform)
    let mut anchors = std::collections::HashMap::new();
    for a in &arrows {
        for l in &a.labels {
            if let Some(name) = &l.name {
                let eff_c = |c: &Coord, p: (f64, f64)| {
                    if matches!(c, Coord::Cell(..)) { eff(p) } else { p }
                };
                if let (Some(f), Some(t)) = (
                    resolve(&a.from, &anchors).map(|p| eff_c(&a.from, p)),
                    resolve(&a.to, &anchors).map(|p| eff_c(&a.to, p)),
                ) {
                    // fletcher builds bent edges on the full center-to-
                    // center chord and only then crops at the nodes, so
                    // arc geometry uses untrimmed centers
                    let (x1, y1) = (grid.x(f.1), grid.y(f.0));
                    let (x2, y2) = (grid.x(t.1), grid.y(t.0));
                    let t01 = l.pos.unwrap_or(0.5);
                    let mut px = x1 + t01 * (x2 - x1);
                    let mut py = y1 + t01 * (y2 - y1);
                    let (dx, dy) = (x2 - x1, y2 - y1);
                    let len = dx.hypot(dy).max(1.0);
                    let (nx, ny) = (dy / len, -dx / len); // left of travel
                    if let Some(bend) = a.bend {
                        let th = bend.to_radians();
                        let sag = len * (1.0 - th.cos()).abs()
                            / (2.0 * th.sin().abs().max(0.05));
                        px += nx * sag * bend.signum();
                        py += ny * sag * bend.signum();
                    }
                    if let Some(sh) = a.shift {
                        px += nx * sh;
                        py += ny * sh;
                    }
                    px += l.xshift;
                    py -= l.yshift; // tikz yshift is upward
                    if std::env::var("NLAB_DEBUG_ANCHORS").is_ok() {
                        debug_dots.push((grid.ry(py), grid.rx(px)));
                    }
                    anchors.insert(name.clone(), (grid.ry(py), grid.rx(px)));
                }
            }
        }
    }

    let mut lines: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut loops_at: std::collections::HashMap<(i32, i32), usize> =
        std::collections::HashMap::new();
    for (r, c, tex) in &nodes {
        let cleaned = clean_tex(tex);
        let label = emit::ts(&cleaned);
        let er = *r as f64 + row_off.get(*r as usize).copied().unwrap_or(0.0);
        lines.push(format!(
            "  node(({}, {}), mi({}), name: <n{}-{}>),",
            c,
            fmt_f(er),
            label,
            r,
            c
        ));
    }
    for a in &arrows {
        let eff_c = |c: &Coord, p: (f64, f64)| {
            if matches!(c, Coord::Cell(..)) { eff(p) } else { p }
        };
        let from = resolve(&a.from, &anchors)
            .map(|p| eff_c(&a.from, p))
            .ok_or("unresolved-anchor")?;
        let to = resolve(&a.to, &anchors)
            .map(|p| eff_c(&a.to, p))
            .ok_or("unresolved-anchor")?;
        warnings.extend(a.warnings.iter().cloned());

        // squared detour rails: `to path={ (..) -- node{L} (..) -- .. }`
        if let Some((pts, plabel)) = &a.to_path {
            let mut vs: Vec<(f64, f64)> = Vec::new(); // (x, y) pt
            for pt in pts {
                let (r, c) = if pt.on_target { to } else { from };
                let hw = node_halfw
                    .get(&(r.round() as i32, c.round() as i32))
                    .copied()
                    .unwrap_or(6.0);
                let mut x = grid.x(c) + pt.dx;
                let mut y = grid.y(r) + pt.dy;
                match pt.anchor {
                    's' => y += 9.0,
                    'n' => y -= 9.0,
                    'e' => x += hw + 2.0,
                    'w' => x -= hw - 2.0,
                    _ => {}
                }
                if vs
                    .last()
                    .map(|(px, py): &(f64, f64)| (px - x).abs() + (py - y).abs() > 1.0)
                    .unwrap_or(true)
                {
                    vs.push((x, y));
                }
            }
            if vs.len() < 2 {
                continue;
            }
            let mut args: Vec<String> = vs
                .iter()
                .map(|(x, y)| coord_str((grid.ry(*y), grid.rx(*x))))
                .collect();
            args.push(format!("\"{}\"", a.final_mark()));
            args.push("corner-radius: 3pt".into());
            if let Some((tex, ldy)) = plabel {
                // put the label on the outward side of the longest leg
                let (mut bx, mut blen) = (1.0, -1.0);
                for w in vs.windows(2) {
                    let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
                    if dx.hypot(dy) > blen {
                        blen = dx.hypot(dy);
                        bx = dx;
                    }
                }
                let left_y = -bx; // left normal of (bx, by) is (by, -bx)
                let side = if left_y * ldy > 0.0 { "left" } else { "right" };
                args.push(format!(
                    "label: text(0.75em, mi({}))",
                    emit::ts(&clean_tex(tex))
                ));
                args.push(format!("label-side: {}", side));
                args.push("label-sep: 0.15em".into());
            }
            if let Some(c) = &a.color {
                args.push(format!("stroke: {}", c));
            }
            lines.push(format!("  edge({}),", args.join(", ")));
            continue;
        }

        // geometry adjustments, in estimated physical space (x right,
        // y down): fletcher auto-sizes columns, so uniform grid*spacing
        // badly underestimates lengths next to wide nodes
        let is_anchor =
            matches!(a.from, Coord::Name(_)) || matches!(a.to, Coord::Name(_));
        let (x1, y1) = (grid.x(from.1), grid.y(from.0));
        let (x2, y2) = (grid.x(to.1), grid.y(to.0));
        let (dx, dy) = (x2 - x1, y2 - y1);
        let len = dx.hypot(dy);
        let mut loop_bend = a.loop_bend;
        if len < 0.01 && loop_bend.is_none() {
            if let Some(out) = a.out_angle {
                // bare out=/in= on a stationary arrow is a self-loop
                loop_bend = Some(if out.rem_euclid(360.0) < 180.0 { 130.0 } else { -130.0 });
            }
        }
        if let Some(base) = loop_bend {
            // several loops on one node fan out as distinct lobes
            let key = (from.0.round() as i32, from.1.round() as i32);
            let k = *loops_at.entry(key).and_modify(|k| *k += 1).or_insert(0usize);
            loop_bend = Some(match k % 4 {
                0 => base,
                1 => -base,
                2 => base.signum() * 165.0,
                _ => -base.signum() * 165.0,
            });
        }
        if len < 0.01 && a.bend.is_none() && loop_bend.is_none() {
            continue; // degenerate; drop quietly
        }
        let (mut xa, mut ya, mut xb, mut yb) = (x1, y1, x2, y2);
        let mut shift_arg = a.shift;
        let mut moved = false;
        // per-endpoint clearance: anchors (2-cells onto arrows) stand
        // off 6pt; empty cells 3.5pt so consecutive bare arrows don't
        // fuse; node cells 0 — fletcher clips at the node border itself
        let end_inset = |c: &Coord| -> f64 {
            match c {
                Coord::Name(_) => 0.5,
                Coord::Cell(r, cc) => {
                    if node_halfw.contains_key(&(*r, *cc)) {
                        0.0
                    } else {
                        3.5
                    }
                }
            }
        };
        if len > 0.01 {
            let mut s1 = end_inset(&a.from)
                + if matches!(a.from, Coord::Name(_)) { a.shorten_start } else { 0.0 };
            let mut s2 = end_inset(&a.to)
                + if matches!(a.to, Coord::Name(_)) { a.shorten_end } else { 0.0 };
            if is_anchor && len - s1 - s2 < 12.0 {
                let s = (len - 12.0) / 2.0;
                s1 = s;
                s2 = s;
            }
            if s1 != 0.0 || s2 != 0.0 {
                xa += dx / len * s1;
                ya += dy / len * s1;
                xb -= dx / len * s2;
                yb -= dy / len * s2;
                moved = true;
            }
        }
        // arrows shifted beyond ~8pt detach from the nodes (adjoint-
        // triple rails); fletcher would draw a shifted center-to-center
        // line, so pin them to the facing node-border anchors instead
        let mut rail: Option<(String, String)> = None;
        if let Some(sh) = shift_arg {
            if sh.abs() > 8.0 {
                if let (Coord::Cell(r1, c1), Coord::Cell(r2, c2)) = (&a.from, &a.to) {
                    let horiz = r1 == r2;
                    if (horiz || c1 == c2)
                        && node_halfw.contains_key(&(*r1, *c1))
                        && node_halfw.contains_key(&(*r2, *c2))
                    {
                        let (a1, a2) = if horiz {
                            if c2 > c1 {
                                ("east", "west")
                            } else {
                                ("west", "east")
                            }
                        } else if r2 > r1 {
                            ("south", "north")
                        } else {
                            ("north", "south")
                        };
                        rail = Some((
                            format!("(name: \"n{}-{}\", anchor: \"{}\")", r1, c1, a1),
                            format!("(name: \"n{}-{}\", anchor: \"{}\")", r2, c2, a2),
                        ));
                    } else if (horiz || c1 == c2) && len > 0.01 {
                        // endpoint without a node: geometric fallback
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
            }
        }
        let (from, to) = if moved {
            ((grid.ry(ya), grid.rx(xa)), (grid.ry(yb), grid.rx(xb)))
        } else {
            (from, to)
        };
        let (v1, v2) = match &rail {
            Some((v1, v2)) => (v1.clone(), v2.clone()),
            None => (coord_str(from), coord_str(to)),
        };

        let mut args = vec![v1.clone(), v2.clone(), format!("\"{}\"", a.final_mark())];
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
            if label_is_blank(&l.tex) {
                continue; // pure name= anchor, possibly spelled "{\ }"
            }
            if first {
                push_label_args(&mut args, l, a.stroke_none, a.color.as_deref());
                first = false;
            } else {
                extra_labels.push(l);
            }
        }
        if let Some(bend) = a.bend.or(loop_bend) {
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
        } else if let Some(c) = &a.color {
            args.push(format!("stroke: {}", c));
        }
        lines.push(format!("  edge({}),", args.join(", ")));
        for l in extra_labels {
            let mut args = vec![v1.clone(), v2.clone(), "\"-\"".to_string()];
            push_label_args(&mut args, l, true, a.color.as_deref());
            if let Some(bend) = a.bend {
                args.push(format!("bend: {}deg", fmt_f(bend)));
            }
            args.push("stroke: none".into());
            lines.push(format!("  edge({}),", args.join(", ")));
        }
    }

    for (r, c, w) in &spacers {
        let er = *r as f64 + row_off.get(*r as usize).copied().unwrap_or(0.0);
        lines.push(format!(
            "  node(({}, {}), box(width: {}pt)),",
            c,
            fmt_f(er),
            fmt_f(*w)
        ));
    }
    for (r, c) in &debug_dots {
        lines.push(format!(
            "  node(({}, {}), circle(radius: 1.4pt, fill: red)),",
            fmt_f(*c),
            fmt_f(*r)
        ));
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

/// Labels that are only spacing commands (`{\ }`, `\,`, `~`) exist to
/// carry a name= anchor; rendering them would punch a blank hole in the
/// carrier arrow's stroke.
fn label_is_blank(tex: &str) -> bool {
    let re = regex::Regex::new(r"^(\\[ ,;!:]|\\quad|\\qquad|~|\s|\{|\}|\\)*$").unwrap();
    re.is_match(tex)
}

fn push_label_args(args: &mut Vec<String>, l: &Label, centered: bool, color: Option<&str>) {
    let fill = color.map(|c| format!("fill: {}, ", c)).unwrap_or_default();
    let mut content = format!("mi({})", emit::ts(&clean_tex(&l.tex)));
    if let Some(deg) = l.rotate {
        // tikz rotates counterclockwise for positive angles, typst clockwise
        content = format!("rotate({}deg, reflow: true, {})", fmt_f(-deg), content);
    }
    args.push(format!("label: text(0.75em, {}{})", fill, content));
    if l.marking {
        // ticks/bullets drawn on the stroke: don't crop the line
        args.push("label-fill: none".into());
    }
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


/// `{A} \atop {B}` -> a stacked 2-row matrix (mitex lays atop flat).
fn stack_atop(s: &str) -> String {
    let mut s = s.to_string();
    while let Some(pos) = s.find("\\atop") {
        let before: Vec<char> = s[..pos].chars().collect();
        let after_str = s[pos + 5..].to_string();
        let after: Vec<char> = after_str.chars().collect();
        // preceding balanced {...} group
        let mut i = before.len();
        while i > 0 && before[i - 1].is_whitespace() {
            i -= 1;
        }
        if i == 0 || before[i - 1] != '}' {
            break;
        }
        let mut depth = 0i32;
        let mut j = i;
        while j > 0 {
            j -= 1;
            match before[j] {
                '}' => depth += 1,
                '{' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        let a: String = before[j + 1..i - 1].iter().collect();
        // following balanced {...} group
        let mut k = 0;
        while k < after.len() && after[k].is_whitespace() {
            k += 1;
        }
        let Some((b, end)) = read_group(&after, k, '{', '}') else { break };
        let head: String = before[..j].iter().collect();
        let tail: String = after[end..].iter().collect();
        s = format!("{}\\begin{{matrix}}{}\\\\{}\\end{{matrix}}{}", head, a, b, tail);
    }
    s
}

/// Drop wrappers mitex has no handler for; keep their visible argument.
fn clean_tex(s: &str) -> String {
    let mut s = stack_atop(&split_mbox(&emit::fix_itex_builtins(s)));
    s = regex::Regex::new(r"\\color\{[^}]*\}")
        .unwrap()
        .replace_all(&s, "")
        .to_string();
    s = regex::Regex::new(r"\\begin\{tabular\}\{[^}]*\}")
        .unwrap()
        .replace_all(&s, "\\begin{matrix}")
        .to_string();
    s = s.replace("\\end{tabular}", "\\end{matrix}");
    s = s.replace("\\textbf", "\\mathbf");
    s = s.replace("\\textit", "\\mathit");
    s = s.replace("\\textsf", "\\mathsf");
    s = s.replace("\\texttt", "\\mathtt");
    let phantom_re = regex::Regex::new(r"\\([hv]?)phantom\s*\{([^{}]*)\}").unwrap();
    s = phantom_re
        .replace_all(&s, |c: &regex::Captures| {
            if &c[1] == "v" {
                return String::new();
            }
            // tikz authors use phantoms as width padding; translate to
            // horizontal space of roughly the same width
            let w = 2.0 * est_halfwidth_pt(&c[2]);
            format!("\\hspace{{{}pt}}", fmt_f(w.max(2.0)))
        })
        .to_string();
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
