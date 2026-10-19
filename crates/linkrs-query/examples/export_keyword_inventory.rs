//! Export the lexical inventory consumed by the tree-sitter grammar, the CLI
//! completer and the frontend highlighter.
//!
//! Run from the workspace root:
//!
//! ```text
//! cargo run -p linkrs-query --example export_keyword_inventory
//! ```
//!
//! The output path defaults to `crates/linkrs-grammar/data/keywords.json` and
//! can be overridden with `--out <path>`. CI regenerates the file and asserts
//! it is unchanged.

use std::path::PathBuf;

use serde::Serialize;

use linkrs_query::executor::expression::functions::FunctionRegistry;
use linkrs_query::parser::lexing::{
    MULTIWORD_CONDITIONAL, MULTIWORD_UNCONDITIONAL, OPERATOR_SYMBOLS, PUNCTUATION_SYMBOLS,
    SOFT_KEYWORDS, function_category, keyword_table,
};

const DEFAULT_OUT: &str = "crates/linkrs-grammar/data/keywords.json";

#[derive(Serialize)]
struct Keyword {
    spelling: &'static str,
    kind: String,
}

#[derive(Serialize)]
struct Function {
    name: String,
    arity: usize,
    variadic: bool,
    description: String,
    category: &'static str,
}

#[derive(Serialize)]
struct Symbol {
    symbol: &'static str,
    kind: &'static str,
}

#[derive(Serialize)]
struct Inventory {
    version: u32,
    generated_by: &'static str,
    keywords: Vec<Keyword>,
    soft_keywords: Vec<&'static str>,
    multiword_unconditional: Vec<&'static str>,
    multiword_conditional: Vec<&'static str>,
    operators: Vec<Symbol>,
    punctuation: Vec<Symbol>,
    functions: Vec<Function>,
}

fn operators() -> Vec<Symbol> {
    OPERATOR_SYMBOLS
        .iter()
        .map(|(symbol, kind)| Symbol { symbol, kind })
        .collect()
}

fn punctuation() -> Vec<Symbol> {
    PUNCTUATION_SYMBOLS
        .iter()
        .map(|(symbol, kind)| Symbol { symbol, kind })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut out = PathBuf::from(DEFAULT_OUT);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => match args.next() {
                Some(path) => out = PathBuf::from(path),
                None => {
                    eprintln!("--out requires a path");
                    std::process::exit(2);
                }
            },
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let keywords = keyword_table()
        .iter()
        .map(|entry| Keyword {
            spelling: entry.spelling,
            kind: format!("{:?}", entry.kind),
        })
        .collect();

    let mut functions: Vec<Function> = FunctionRegistry::new()
        .function_specs()
        .into_iter()
        .map(|spec| {
            let category = function_category(&spec.name);
            Function {
                name: spec.name,
                arity: spec.arity,
                variadic: spec.variadic,
                description: spec.description,
                category,
            }
        })
        .collect();
    functions.sort_by(|a, b| a.name.cmp(&b.name).then(a.arity.cmp(&b.arity)));
    functions.dedup_by(|a, b| a.name == b.name && a.arity == b.arity);

    let inventory = Inventory {
        version: 1,
        generated_by: "linkrs-query/examples/export_keyword_inventory.rs",
        keywords,
        soft_keywords: SOFT_KEYWORDS.to_vec(),
        multiword_unconditional: MULTIWORD_UNCONDITIONAL.to_vec(),
        multiword_conditional: MULTIWORD_CONDITIONAL.to_vec(),
        operators: operators(),
        punctuation: punctuation(),
        functions,
    };

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).expect("create inventory output directory");
    }
    let json = serde_json::to_string_pretty(&inventory).expect("serialize inventory");
    std::fs::write(&out, format!("{json}\n")).expect("write inventory");
    println!("wrote {}", out.display());
}
