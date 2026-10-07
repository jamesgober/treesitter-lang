//! Writes the `mini` language's tree-sitter grammar to disk, as both
//! `grammar.js` and `grammar.json`.
//!
//! ```bash
//! cargo run --example generate                 # into target/tree-sitter-mini
//! cargo run --example generate -- path/to/dir
//! cd target/tree-sitter-mini && tree-sitter generate
//! ```
//!
//! `tree-sitter generate` reads `grammar.js` by default. `grammar.json` is the
//! same grammar in tree-sitter's own format, for builds without a JavaScript
//! runtime: `tree-sitter generate grammar.json`.

mod common;

use std::{env, error::Error, fs, path::PathBuf};

use common::mini;

fn main() -> Result<(), Box<dyn Error>> {
    let dir = env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("target/tree-sitter-mini"), PathBuf::from);
    fs::create_dir_all(&dir)?;

    let grammar = mini::grammar();

    // One buffer serves both files.
    let mut buffer = String::new();
    grammar.write_js(&mut buffer)?;
    fs::write(dir.join("grammar.js"), &buffer)?;
    println!(
        "wrote {} ({} bytes)",
        dir.join("grammar.js").display(),
        buffer.len()
    );

    buffer.clear();
    grammar.write_json(&mut buffer)?;
    fs::write(dir.join("grammar.json"), &buffer)?;
    println!(
        "wrote {} ({} bytes)",
        dir.join("grammar.json").display(),
        buffer.len()
    );

    println!("\nnext: cd {} && tree-sitter generate", dir.display());
    Ok(())
}
