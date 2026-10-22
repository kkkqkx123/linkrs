//! Batch-process every fixture under `fixtures/` and write token classifications
//! to `outputs/`.
//!
//! Usage:
//!   cargo run -p linkrs-grammar --example highlight_fixtures

use std::fs;
use std::path::PathBuf;

use linkrs_grammar::{parse, tokens};

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures_dir = manifest.join("fixtures");
    let outputs_dir = manifest.join("outputs");
    fs::create_dir_all(&outputs_dir).expect("create outputs directory");

    let mut entries: Vec<PathBuf> = fs::read_dir(&fixtures_dir)
        .expect("read fixtures directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    entries.sort();

    for path in entries {
        let source = fs::read_to_string(&path).expect("read fixture");
        let outcome = parse(&source);
        let mut output = String::new();
        output.push_str("source:\n");
        output.push_str(&source);
        output.push_str("\n\nparse_has_error: ");
        output.push_str(&outcome.has_error.to_string());
        output.push_str("\n\ntokens (start..end: class):\n");
        for token in tokens(&source) {
            output.push_str(&format!(
                "{}..{}: {}\n",
                token.start, token.end, token.class
            ));
        }

        let output_path = outputs_dir
            .join(path.file_stem().unwrap_or_default())
            .with_extension("txt");
        fs::write(&output_path, output).expect("write output");
        println!("wrote {}", output_path.display());
    }
}
