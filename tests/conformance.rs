//! Grammars that exercise every rule type, every grammar-level setting, and
//! the escaping edge cases, checked against the real tree-sitter CLI.
//!
//! `cargo test` validates and emits them. With
//! `TREESITTER_LANG_CONFORMANCE_DIR` set, each is also written to
//! `<dir>/<name>/grammar.js` and `grammar.json`; the CI `tree-sitter` job
//! then runs `tree-sitter generate` on both and requires the `grammar.json`
//! tree-sitter writes to equal ours byte for byte, and both inputs to produce
//! the same `parser.c`.

use std::{env, fs, path::Path};

use treesitter_lang::{Grammar, Rule};

/// Every construct in one grammar for a small configuration language.
fn kitchen_sink() -> Grammar {
    let value = || Rule::symbol("_value");
    let s = Rule::string;
    Grammar::new("kitchen_sink")
        .extra(Rule::pattern(r"\s"))
        .extra(Rule::symbol("comment"))
        .external("heredoc_body")
        .external("_error_sentinel")
        .supertype("_value")
        .inline("_key")
        // `< a b >` is both a group of identifiers and a tuple of values;
        // the group's dynamic precedence picks it at run time.
        .conflict(["_value", "group"])
        .word("identifier")
        .rule(
            "document",
            Rule::repeat(Rule::choice([
                Rule::symbol("pair"),
                Rule::symbol("section"),
                Rule::symbol("heredoc"),
                Rule::symbol("odd_tokens"),
            ])),
        )
        .rule(
            "pair",
            Rule::seq([
                Rule::field("key", Rule::symbol("_key")),
                s("="),
                Rule::field("value", value()),
                Rule::optional(s(";")),
            ]),
        )
        .rule(
            "_key",
            Rule::choice([Rule::symbol("identifier"), Rule::symbol("quoted")]),
        )
        .rule(
            "section",
            Rule::seq([
                s("["),
                Rule::field(
                    "name",
                    Rule::alias(Rule::symbol("identifier"), "section_name"),
                ),
                s("]"),
            ]),
        )
        .rule(
            "heredoc",
            Rule::seq([s("<<"), Rule::symbol("heredoc_body")]),
        )
        .rule(
            "_value",
            Rule::choice([
                Rule::symbol("number"),
                Rule::symbol("quoted"),
                Rule::symbol("boolean"),
                Rule::symbol("list"),
                Rule::symbol("path"),
                Rule::symbol("negation"),
                Rule::symbol("power"),
                Rule::symbol("group"),
                Rule::symbol("tuple"),
                Rule::symbol("identifier"),
            ]),
        )
        .rule(
            "number",
            Rule::token(Rule::seq([
                Rule::optional(s("-")),
                Rule::pattern(r"\d+"),
                Rule::optional(Rule::seq([s("."), Rule::pattern(r"\d+")])),
            ])),
        )
        .rule(
            "quoted",
            Rule::seq([
                s("\""),
                Rule::repeat(Rule::choice([
                    Rule::immediate(Rule::prec(1, Rule::pattern(r#"[^"\\\n]+"#))),
                    Rule::symbol("escape"),
                ])),
                Rule::immediate(s("\"")),
            ]),
        )
        .rule(
            "escape",
            Rule::immediate(Rule::seq([s("\\"), Rule::pattern(".")])),
        )
        .rule("boolean", Rule::choice([s("true"), s("false")]))
        .rule(
            "list",
            Rule::seq([
                s("["),
                Rule::optional(Rule::seq([
                    value(),
                    Rule::repeat(Rule::seq([s(","), value()])),
                ])),
                s("]"),
            ]),
        )
        // A slash inside a pattern, and inside a character class.
        .rule("path", Rule::pattern("[a-z]+(/[a-z]+)+[^/\\s]?"))
        .rule("negation", Rule::prec(2, Rule::seq([s("!"), value()])))
        .rule(
            "power",
            Rule::prec_right(1, Rule::seq([value(), s("^"), value()])),
        )
        .rule(
            "group",
            Rule::prec_dynamic(
                1,
                Rule::seq([s("<"), Rule::repeat1(Rule::symbol("identifier")), s(">")]),
            ),
        )
        // Anonymous tokens with characters that need escaping in both
        // JavaScript and JSON, and one blank alternative.
        .rule(
            "odd_tokens",
            Rule::seq([
                s("@"),
                Rule::choice([
                    s("it's"),
                    s("\"q\""),
                    s("back\\slash"),
                    s("tab\there"),
                    s("é→✓"),
                    s("line\u{2028}sep"),
                    Rule::blank(),
                ]),
                s("@"),
            ]),
        )
        .rule("tuple", Rule::seq([s("<"), Rule::repeat1(value()), s(">")]))
        .rule("identifier", Rule::pattern("[a-z_][a-z0-9_]*"))
        .rule(
            "comment",
            Rule::token(Rule::seq([s("#"), Rule::pattern(".*")])),
        )
}

/// The smallest grammar tree-sitter accepts, with default extras.
fn minimal() -> Grammar {
    Grammar::new("minimal").rule("source_file", Rule::repeat(Rule::pattern(r"\w+")))
}

fn grammars() -> Vec<Grammar> {
    vec![kitchen_sink(), minimal()]
}

fn write(dir: &Path, name: &str, js: &str, json: &str) {
    let dir = dir.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("grammar.js"), js).unwrap();
    fs::write(dir.join("grammar.json"), json).unwrap();
}

#[test]
fn test_conformance_grammars_emit_both_formats() {
    let out = env::var_os("TREESITTER_LANG_CONFORMANCE_DIR");
    for grammar in grammars() {
        let js = grammar.to_js().unwrap();
        let json = grammar.to_json().unwrap();
        assert!(js.starts_with("/// <reference types=\"tree-sitter-cli/dsl\" />\n"));
        assert!(js.ends_with("});\n"));
        assert!(json.starts_with("{\n  \"$schema\": "));
        assert!(json.ends_with("\"reserved\": {}\n}"));
        if let Some(dir) = &out {
            let name = json
                .lines()
                .find_map(|l| l.strip_prefix("  \"name\": \""))
                .and_then(|l| l.strip_suffix("\","))
                .unwrap();
            write(Path::new(dir), name, &js, &json);
        }
    }
}

#[test]
fn test_kitchen_sink_escapes_survive_both_formats() {
    let grammar = kitchen_sink();
    let js = grammar.to_js().unwrap();
    assert!(js.contains(r"'it\'s'"));
    assert!(js.contains(r#"'"q"'"#));
    assert!(js.contains(r"'back\\slash'"));
    assert!(js.contains(r"'tab\there'"));
    assert!(js.contains("'é→✓'"));
    assert!(js.contains("'line\\u2028sep'"));
    assert!(js.contains(r"/[a-z]+(\/[a-z]+)+[^\/\s]?/"));

    let json = grammar.to_json().unwrap();
    assert!(json.contains(r#""value": "it's""#));
    assert!(json.contains(r#""value": "\"q\"""#));
    assert!(json.contains(r#""value": "back\\slash""#));
    assert!(json.contains(r#""value": "tab\there""#));
    assert!(json.contains("\"value\": \"line\u{2028}sep\""));
    assert!(json.contains(r#""value": "[a-z]+(\\/[a-z]+)+[^\\/\\s]?""#));
}
