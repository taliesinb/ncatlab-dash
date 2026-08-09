//! CLI for the nlab-typ pipeline stages.
mod emit;
mod grid;

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
