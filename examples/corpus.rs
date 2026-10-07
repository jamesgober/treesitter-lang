//! Turns sample programs into a tree-sitter corpus test, using the trees the
//! hand-written `mini` parser builds as the expected output.
//!
//! Each sample is parsed into a `syntax_lang` tree, rendered with
//! `Grammar::sexp`, and written as a corpus entry. `tree-sitter test` then
//! parses the same samples with the generated parser and compares: if the
//! grammar and the hand-written parser ever disagree, the test fails.
//!
//! ```bash
//! cargo run --example generate
//! cargo run --example corpus                   # into target/tree-sitter-mini
//! cd target/tree-sitter-mini && tree-sitter generate && tree-sitter test
//! ```

mod common;

use std::{env, error::Error, fs, path::PathBuf};

use common::mini::{self, Kind};

/// Title and source of each corpus entry.
const SAMPLES: [(&str, &str); 6] = [
    ("Numbers and names", "42;\nanswer;\n"),
    ("Let statements", "let width = 10;\nlet height = width;\n"),
    ("Operator precedence", "1 + 2 * 3 - 4 / 5;\n"),
    ("Calls", "max(a, b + 1, now());\n"),
    ("Parentheses", "(1 + 2) * (3 - 4);\n"),
    (
        "Comments",
        "# setup\nlet a = 1; # one\nlet b = a # inline\n  + 2;\n",
    ),
];

/// The divider lines of a corpus entry.
const HEADER: &str =
    "================================================================================";
const DIVIDER: &str =
    "--------------------------------------------------------------------------------";

fn main() -> Result<(), Box<dyn Error>> {
    let dir = env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("target/tree-sitter-mini"), PathBuf::from);
    let corpus_dir = dir.join("test").join("corpus");
    fs::create_dir_all(&corpus_dir)?;

    let grammar = mini::grammar();
    let mut corpus = String::new();
    for (i, (title, source)) in SAMPLES.iter().enumerate() {
        let tree = mini::parse(source).map_err(|e| format!("sample {title:?}: {e}"))?;

        if i > 0 {
            corpus.push('\n');
        }
        corpus.push_str(HEADER);
        corpus.push('\n');
        corpus.push_str(title);
        corpus.push('\n');
        corpus.push_str(HEADER);
        corpus.push_str("\n\n");
        corpus.push_str(source.trim_end());
        corpus.push_str("\n\n");
        corpus.push_str(DIVIDER);
        corpus.push_str("\n\n");
        grammar.write_sexp(&tree, Kind::name, &mut corpus)?;
        corpus.push('\n');
    }

    let path = corpus_dir.join("mini.txt");
    fs::write(&path, &corpus)?;
    println!("{corpus}");
    println!("wrote {} ({} entries)", path.display(), SAMPLES.len());
    Ok(())
}
