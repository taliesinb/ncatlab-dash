//! CLI for the nlab-typ pipeline stages.
mod emit;
mod grid;
mod prose;
mod tikzcd;

use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("grid");
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();

    match mode {
        "dump" => {
            let node = mitex_parser::parse(&input, mitex_spec_gen::DEFAULT_SPEC.clone());
            print_tree(&node, 0);
        }
        "grid" => match grid::parse_formula(&input) {
            Ok(json) => println!("{json}"),
            Err(status) => {
                eprintln!("{status}");
                std::process::exit(3);
            }
        },
        "grids" => {
            use std::io::Write;
            let out = std::io::stdout();
            let mut out = out.lock();
            for rec in input.split('\u{0}') {
                if rec.is_empty() {
                    continue;
                }
                match grid::parse_formula(rec) {
                    Ok(json) => write!(out, "ok\u{1f}{json}\u{0}").unwrap(),
                    Err(status) => write!(out, "{status}\u{1f}\u{0}").unwrap(),
                }
            }
        }
        "page" => {
            std::env::set_var("NLAB_LOCAL_MITEX", "1");
            let title = args.get(2).map(String::as_str);
            print!("{}", prose::page_to_typst(&input, title));
        }
        "tikzcd" => match tikzcd::tikzcd_to_fletcher(&input) {
            Ok((code, warns)) => {
                for w in warns {
                    eprintln!("warn: ignored option {w:?}");
                }
                println!("{code}");
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(3);
            }
        },
        "tikzcds" => {
            use std::io::Write;
            let out = std::io::stdout();
            let mut out = out.lock();
            for rec in input.split('\u{0}') {
                if rec.trim().is_empty() {
                    continue;
                }
                match tikzcd::tikzcd_to_fletcher(rec) {
                    Ok((code, warns)) => {
                        write!(out, "ok\u{1f}{}\u{1f}{}\u{0}", code, warns.join("\u{1e}"))
                            .unwrap()
                    }
                    Err(e) => write!(out, "err:{e}\u{1f}\u{1f}\u{0}").unwrap(),
                }
            }
        }
        "typsts" => {
            use std::io::Write;
            let out = std::io::stdout();
            let mut out = out.lock();
            for rec in input.split('\u{0}') {
                if rec.is_empty() {
                    continue;
                }
                let (class, status, code) = emit::emit_formula(rec);
                write!(
                    out,
                    "{status}\u{1f}{class}\u{1f}{}\u{0}",
                    code.unwrap_or_default()
                )
                .unwrap();
            }
        }
        m => {
            eprintln!("unknown mode: {m}");
            std::process::exit(2);
        }
    }
}

fn print_tree(node: &mitex_parser::syntax::SyntaxNode, depth: usize) {
    let indent = "  ".repeat(depth);
    println!("{indent}{:?}", node.kind());
    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Node(n) => print_tree(&n, depth + 1),
            rowan::NodeOrToken::Token(t) => {
                println!("{indent}  {:?} {:?}", t.kind(), t.text())
            }
        }
    }
}
