//! A calculator grammar, printed as `grammar.js`.
//!
//! The smallest complete use of the crate: build a grammar from rules, emit
//! it, and see validation catch a mistake before tree-sitter would.
//!
//! ```bash
//! cargo run --example calc
//! cargo run --example calc > grammar.js && tree-sitter generate
//! ```

use treesitter_lang::{Error, Grammar, Rule};

fn calculator() -> Grammar {
    let e = || Rule::symbol("_expression");
    let binary = |power, op: &'static str| {
        Rule::prec_left(
            power,
            Rule::seq([
                Rule::field("left", e()),
                Rule::field("operator", Rule::string(op)),
                Rule::field("right", e()),
            ]),
        )
    };

    Grammar::new("calc")
        .rule("source_file", Rule::repeat(e()))
        .rule(
            "_expression",
            Rule::choice([
                Rule::symbol("number"),
                Rule::symbol("binary_expression"),
                Rule::symbol("unary_expression"),
                Rule::symbol("parenthesized_expression"),
            ]),
        )
        .rule(
            "binary_expression",
            Rule::choice([
                binary(1, "+"),
                binary(1, "-"),
                binary(2, "*"),
                binary(2, "/"),
                // Exponentiation groups to the right: 2 ^ 3 ^ 2 is 2 ^ (3 ^ 2).
                Rule::prec_right(
                    3,
                    Rule::seq([
                        Rule::field("left", e()),
                        Rule::field("operator", Rule::string("^")),
                        Rule::field("right", e()),
                    ]),
                ),
            ]),
        )
        .rule(
            "unary_expression",
            Rule::prec(
                4,
                Rule::seq([Rule::string("-"), Rule::field("operand", e())]),
            ),
        )
        .rule(
            "parenthesized_expression",
            Rule::seq([Rule::string("("), e(), Rule::string(")")]),
        )
        .rule(
            "number",
            Rule::token(Rule::seq([
                Rule::pattern(r"\d+"),
                Rule::optional(Rule::seq([Rule::string("."), Rule::pattern(r"\d+")])),
            ])),
        )
}

fn main() -> Result<(), Error> {
    let grammar = calculator();
    println!("{}", grammar.to_js()?);

    // Mistakes are caught before tree-sitter sees them. Here a rule that can
    // match nothing is referred to by another rule, which tree-sitter rejects.
    let broken = calculator()
        .rule(
            "statement",
            Rule::seq([Rule::symbol("label"), Rule::symbol("_expression")]),
        )
        .rule("label", Rule::repeat(Rule::pattern("[a-z]+:")));
    match broken.to_js() {
        Err(error) => eprintln!("validation caught: {error}"),
        Ok(_) => eprintln!("validation unexpectedly passed"),
    }
    Ok(())
}
