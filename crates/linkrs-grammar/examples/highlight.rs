//! Print the tree-sitter highlight classification for a Linkrs query.
//!
//! Usage:
//!   cargo run -p linkrs-grammar --example highlight -- "MATCH (a:Tag) RETURN a.name"

use linkrs_grammar::tokens;

fn main() {
    let source = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "MATCH (a:Tag) RETURN a.name".to_string());

    println!("source:");
    println!("{source}");
    println!();
    println!("tokens (start..end: class):");
    for token in tokens(&source) {
        println!("{}..{}: {}", token.start, token.end, token.class);
    }
}
