//! Behaviour of the public API: validation, both output formats, and
//! S-expression rendering.
//!
//! Each validation case mirrors a behaviour of the tree-sitter CLI, probed
//! directly; the comments say what tree-sitter does with the same grammar.

use treesitter_lang::syntax_lang::{Builder, Element, Node, Span, Token};
use treesitter_lang::{Error, Grammar, Rule};

fn sym(name: &'static str) -> Rule {
    Rule::symbol(name)
}

fn s(text: &'static str) -> Rule {
    Rule::string(text)
}

fn word() -> Rule {
    Rule::pattern("[a-z]+")
}

/// A valid grammar to start each case from.
fn base() -> Grammar {
    Grammar::new("demo")
        .rule("source_file", Rule::repeat(sym("item")))
        .rule("item", word())
}

/// A grammar whose start rule reaches `rules` (each a name) through a
/// `choice`, so they are all checked as reachable rules.
fn reaching(rules: &[&'static str]) -> Grammar {
    Grammar::new("demo").rule(
        "source_file",
        Rule::repeat(Rule::seq([
            s("<"),
            Rule::choice(rules.iter().map(|r| sym(r))),
            s(">"),
        ])),
    )
}

fn undefined(symbol: &str, rule: &str) -> Error {
    Error::UndefinedSymbol {
        symbol: symbol.into(),
        rule: rule.into(),
    }
}

fn matches_empty(rule: &str) -> Error {
    Error::MatchesEmpty { rule: rule.into() }
}

// --- Names, rules, and references --------------------------------------------

#[test]
fn test_validate_base_grammar_passes() {
    assert!(base().to_js().is_ok());
    assert!(base().to_json().is_ok());
}

#[test]
fn test_validate_no_rules_is_refused() {
    assert_eq!(Grammar::new("demo").to_js(), Err(Error::NoRules));
}

#[test]
fn test_validate_invalid_names_are_refused_everywhere() {
    let invalid = |name: &str| Err(Error::InvalidName { name: name.into() });
    assert_eq!(
        Grammar::new("my-lang").rule("a", word()).to_js(),
        invalid("my-lang")
    );
    assert_eq!(Grammar::new("").rule("a", word()).to_js(), invalid(""));
    assert_eq!(base().rule("9lives", word()).to_js(), invalid("9lives"));
    assert_eq!(base().external("bad name").to_js(), invalid("bad name"));
    let field = base().rule("pair", Rule::field("the key", word()));
    assert_eq!(field.to_js(), invalid("the key"));
    let alias = base().rule("pair", Rule::alias(word(), "a.b"));
    assert_eq!(alias.to_js(), invalid("a.b"));
}

#[test]
fn test_validate_proto_is_refused_for_rules_and_externals_only() {
    // In grammar.js, `__proto__` in the rules object sets the prototype; as
    // a field or alias name it is an ordinary string.
    let invalid = Err(Error::InvalidName {
        name: "__proto__".into(),
    });
    assert_eq!(base().rule("__proto__", word()).to_js(), invalid);
    assert_eq!(base().external("__proto__").to_js(), invalid);
    let g = base().rule(
        "x",
        Rule::seq([
            Rule::field("__proto__", s("a")),
            Rule::alias(s("b"), "__proto__"),
        ]),
    );
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_hidden_start_rule_is_refused() {
    let g = Grammar::new("demo").rule("_start", word());
    assert_eq!(
        g.to_js(),
        Err(Error::HiddenStart {
            name: "_start".into()
        })
    );
}

#[test]
fn test_validate_duplicate_rule_reports_first_redefinition() {
    let g = base()
        .rule("b", word())
        .rule("a", word())
        .rule("b", word())
        .rule("a", word());
    assert_eq!(g.to_js(), Err(Error::DuplicateRule { name: "b".into() }));
}

#[test]
fn test_validate_undefined_symbol_in_each_place() {
    // Unreachable rules are checked too: grammar.js evaluates every rule.
    assert_eq!(
        base().rule("x", sym("missing")).to_js(),
        Err(undefined("missing", "x"))
    );
    assert_eq!(
        base().extra(sym("missing")).to_js(),
        Err(undefined("missing", "extras"))
    );
    assert_eq!(
        base().word("missing").to_js(),
        Err(undefined("missing", "word"))
    );
    assert_eq!(
        base().conflict(["item", "missing"]).to_js(),
        Err(undefined("missing", "conflicts"))
    );
    assert_eq!(
        base().inline("missing").to_js(),
        Err(undefined("missing", "inline"))
    );
    assert_eq!(
        base().supertype("missing").to_js(),
        Err(undefined("missing", "supertypes"))
    );
}

#[test]
fn test_validate_externals_resolve_symbols() {
    let g = base()
        .external("indent")
        .rule("block", Rule::seq([sym("indent"), sym("item")]));
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_reports_problems_in_definition_order() {
    let g = Grammar::new("demo")
        .rule("source_file", Rule::seq([sym("first_missing"), s("")]))
        .rule("other", sym("second_missing"));
    assert_eq!(g.to_js(), Err(undefined("first_missing", "source_file")));
}

#[test]
fn test_validate_failure_leaves_buffers_untouched() {
    let broken = base().rule("x", sym("missing"));
    let mut buffer = String::from("prefix");
    assert!(broken.write_js(&mut buffer).is_err());
    assert!(broken.write_json(&mut buffer).is_err());
    assert_eq!(buffer, "prefix");
}

// --- Strings, choices, and tokens -------------------------------------------

#[test]
fn test_validate_empty_pattern_is_refused_anywhere() {
    // An empty pattern cannot be written as `/.../`, reachable or not.
    let empty = Err(Error::EmptyString { rule: "x".into() });
    assert_eq!(base().rule("x", Rule::pattern("")).to_js(), empty);
    assert_eq!(
        base().extra(Rule::pattern("")).to_js(),
        Err(Error::EmptyString {
            rule: "extras".into()
        })
    );
}

/// The `rule` of an invalid-pattern error, or a description of what came
/// back instead.
fn pattern_error(result: Result<String, Error>) -> String {
    match result {
        Err(Error::EmptyString { rule }) => rule,
        other => format!("not an invalid pattern: {other:?}"),
    }
}

#[test]
fn test_regression_h06_unclosed_class_cannot_carry_text_into_code() {
    // ISSUES H06. JavaScript reads `[` as opening a character class, inside
    // which `/` does not end a regular expression literal. In 1.0.0 the
    // pattern `[` was written `/[/`, so the literal ran on through the
    // string after it, whose text after `]` became code that `tree-sitter
    // generate` would run when it evaluated `grammar.js`.
    let payload = r#"]/,require("fs").rmSync("x")),//"#;
    let exploit = |g: Grammar| g.rule("x", Rule::seq([Rule::pattern("["), s(payload)]));
    let expected = "x: unterminated character class at byte 0 of `[`";
    // Refused in both formats, in reachable and unreachable rules alike:
    // `grammar.js` evaluates every rule.
    assert_eq!(pattern_error(exploit(reaching(&["x"])).to_js()), expected);
    assert_eq!(pattern_error(exploit(reaching(&["x"])).to_json()), expected);
    assert_eq!(pattern_error(exploit(base()).to_js()), expected);
    let mut buffer = String::new();
    assert!(exploit(base()).write_js(&mut buffer).is_err());
    assert!(buffer.is_empty());
    // The error says what is wrong and where.
    let error = exploit(base()).to_js().unwrap_err();
    assert_eq!(
        error.to_string(),
        "`x` contains an invalid pattern: unterminated character class at byte 0 of `[`"
    );
    // The same payload behind a valid pattern is just a string.
    let safe = base()
        .rule("x", Rule::seq([Rule::pattern("[[]"), s(payload)]))
        .to_js()
        .unwrap();
    assert!(safe.contains(r#"x: $ => seq(/[[]/, ']/,require("fs").rmSync("x")),//'),"#));
}

#[test]
fn test_validate_patterns_javascript_refuses_are_refused_everywhere() {
    // Each would make `grammar.js` fail to load: node and tree-sitter's
    // QuickJS both throw a SyntaxError for every one of them.
    let cases = [
        ("(a", "unterminated group at byte 0 of `(a`"),
        ("a)", "unmatched `)` at byte 1 of `a)`"),
        (
            "a**",
            "a quantifier has nothing to repeat at byte 2 of `a**`",
        ),
        (
            "a{2,1}",
            "the numbers in a `{}` quantifier are out of order at byte 1 of `a{2,1}`",
        ),
        (
            "[z-a]",
            "a character class range is out of order at byte 1 of `[z-a]`",
        ),
        (
            "(?<=a)*",
            "a quantifier has nothing to repeat at byte 6 of `(?<=a)*`",
        ),
        ("a/[", "unterminated character class at byte 2 of `a/[`"),
    ];
    for (pattern, problem) in cases {
        let unreachable = base().rule("x", Rule::pattern(pattern));
        assert_eq!(pattern_error(unreachable.to_js()), format!("x: {problem}"));
        let extra = base().extra(Rule::pattern(pattern));
        assert_eq!(pattern_error(extra.to_json()), format!("extras: {problem}"));
    }
}

#[test]
fn test_validate_patterns_tree_sitter_refuses_are_refused_where_it_reads_them() {
    // Valid JavaScript, but tree-sitter's regex parser refuses them:
    // "Regex error: Assertions are not supported", "regex parse error".
    // Tree-sitter only parses the patterns of rules it keeps.
    let cases = [
        (
            "^a",
            "assertions (`^`, `$`, `\\b`, `\\B`, ...) are not supported by tree-sitter at byte 0 of `^a`",
        ),
        (
            "(?=a)",
            "look-ahead and look-behind are not supported by tree-sitter at byte 0 of `(?=a)`",
        ),
        (
            r"\1",
            "backreferences and octal escapes (`\\0` to `\\9`) are not supported by tree-sitter at byte 0 of `\\1`",
        ),
        ("[[a]", "unterminated character class at byte 0 of `[[a]`"),
        (
            "a{,2}",
            "a counted repetition needs a number at byte 2 of `a{,2}`",
        ),
        (r"\e", "unrecognized escape sequence at byte 0 of `\\e`"),
    ];
    for (pattern, problem) in cases {
        let unreachable = base().rule("x", Rule::pattern(pattern));
        assert!(unreachable.to_js().is_ok(), "{pattern}");
        let reachable = reaching(&["x"]).rule("x", Rule::pattern(pattern));
        assert_eq!(pattern_error(reachable.to_js()), format!("x: {problem}"));
        let extra = base().extra(Rule::token(Rule::seq([s("#"), Rule::pattern(pattern)])));
        assert_eq!(pattern_error(extra.to_js()), format!("extras: {problem}"));
    }
    // An assertion inside a repetition of exactly zero compiles to nothing,
    // and tree-sitter accepts it.
    assert!(
        reaching(&["x"])
            .rule("x", Rule::pattern("a(?:^){0}"))
            .to_js()
            .is_ok()
    );
}

#[test]
fn test_validate_pattern_error_preview_is_short_and_printable() {
    let long = format!("{}(", "a".repeat(100));
    let problem = pattern_error(base().rule("x", Rule::pattern(long)).to_js());
    assert_eq!(
        problem,
        format!("x: unterminated group at byte 100 of `{}…`", "a".repeat(40))
    );
    let control = pattern_error(base().rule("x", Rule::pattern("\t\u{1b}(")).to_js());
    assert_eq!(control, r"x: unterminated group at byte 2 of `\t\u{1b}(`");
}

#[test]
fn test_valid_patterns_are_written_as_before() {
    // Validation adds no rewriting: every pattern that validates is written
    // exactly as 1.0.0 wrote it.
    let cases = [
        ("[/]", r"/[\/]/"),
        (r"[\]/]", r"/[\]\/]/"),
        (r"[\[]", r"/[\[]/"),
        ("]", "/]/"),
        (r"[^/\\s]?", r"/[^\/\\s]?/"),
        ("a{ 2 }", "/a{ 2 }/"),
        (r"(?<name>[a-z]+)", "/(?<name>[a-z]+)/"),
        ("a\tb", r"/a\tb/"),
        (r"end\", r"/end\\/"),
    ];
    for (pattern, written) in cases {
        let js = reaching(&["x"])
            .rule("x", Rule::pattern(pattern))
            .to_js()
            .unwrap();
        assert!(
            js.contains(&format!("x: $ => {written},")),
            "{pattern}: {js}"
        );
    }
}

#[test]
fn test_validate_empty_string_is_refused_in_reachable_rules_only() {
    // Tree-sitter: "The rule `x` contains an empty string" — but it drops
    // unreachable rules first, and inside a token an empty string is part of
    // a regular expression.
    let g = reaching(&["x"]).rule("x", Rule::seq([s("a"), s("")]));
    assert_eq!(g.to_js(), Err(Error::EmptyString { rule: "x".into() }));
    // Inside a token too: tree-sitter refuses most of these, and the rest
    // can only match nothing.
    let g = reaching(&["x"]).rule("x", Rule::token(Rule::seq([s("a"), s("")])));
    assert_eq!(g.to_js(), Err(Error::EmptyString { rule: "x".into() }));
    let g = reaching(&["x"]).rule("x", Rule::token(s("")));
    assert_eq!(g.to_js(), Err(Error::EmptyString { rule: "x".into() }));
    assert!(
        base()
            .rule("unused", Rule::seq([s("a"), s("")]))
            .to_js()
            .is_ok()
    );
}

#[test]
fn test_validate_empty_choice_is_refused() {
    let g = base().rule("x", Rule::seq([s("a"), Rule::choice([])]));
    assert_eq!(g.to_js(), Err(Error::EmptyChoice { rule: "x".into() }));
}

#[test]
fn test_validate_symbol_inside_token_is_refused() {
    // Tree-sitter: "Unexpected rule `item` in `token()` call", even in an
    // unreachable rule.
    let g = base().rule("x", Rule::token(Rule::seq([s("#"), sym("item")])));
    assert_eq!(
        g.to_js(),
        Err(Error::SymbolInToken {
            symbol: "item".into(),
            rule: "x".into()
        })
    );
    let g = base().rule("x", Rule::immediate(Rule::prec(1, sym("item"))));
    assert!(matches!(g.to_js(), Err(Error::SymbolInToken { .. })));
}

#[test]
fn test_validate_symbol_inside_a_composite_extra_is_refused() {
    // Tree-sitter: "unexpected symbol". An extra that is not a lone symbol
    // is lexed as a token.
    let g = base().extra(Rule::choice([sym("item"), s("!")]));
    assert_eq!(
        g.to_js(),
        Err(Error::SymbolInToken {
            symbol: "item".into(),
            rule: "extras".into()
        })
    );
}

#[test]
fn test_validate_fields_and_aliases_inside_tokens_are_allowed() {
    let g = base().rule("x", Rule::token(Rule::field("f", Rule::alias(s("a"), "b"))));
    assert!(g.to_js().is_ok());
}

#[test]
fn test_empty_seq_is_blank_and_emitted_as_such() {
    // Tree-sitter refuses `token(seq())` but accepts `token(blank())`.
    let g = Grammar::new("demo").rule(
        "source_file",
        Rule::seq([s("x"), Rule::token(Rule::seq([]))]),
    );
    let js = g.to_js().unwrap();
    assert!(js.contains("seq('x', token(blank()))"));
}

// --- The empty string --------------------------------------------------------

#[test]
fn test_validate_referenced_rule_matching_empty_is_refused() {
    let g = reaching(&["block"])
        .rule("block", Rule::seq([s("{"), sym("list"), s("}")]))
        .rule("list", Rule::repeat(sym("item")))
        .rule("item", word());
    assert_eq!(g.to_js(), Err(matches_empty("list")));
}

#[test]
fn test_validate_unreachable_rules_may_match_empty() {
    // Tree-sitter drops unreachable rules before this check.
    let g = base()
        .rule("unused", Rule::seq([s("x"), sym("nullable")]))
        .rule("nullable", Rule::optional(s("b")));
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_unreferenced_start_rule_may_match_empty() {
    let g = Grammar::new("demo")
        .rule("source_file", Rule::repeat(sym("item")))
        .rule("item", word());
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_start_rule_referenced_and_empty_is_refused() {
    let g = Grammar::new("demo")
        .rule("source_file", Rule::repeat(sym("group")))
        .rule("group", Rule::seq([s("("), sym("source_file"), s(")")]));
    assert_eq!(g.to_js(), Err(matches_empty("source_file")));
}

#[test]
fn test_validate_empty_through_a_symbol_reports_the_rule_itself() {
    // `wrapper` refers to `inner`, which can be empty: tree-sitter blames
    // `inner`, the rule with the empty production.
    let g = reaching(&["top"])
        .rule("top", Rule::seq([s("<"), sym("wrapper"), s(">")]))
        .rule("wrapper", sym("inner"))
        .rule("inner", Rule::optional(word()));
    assert_eq!(g.to_js(), Err(matches_empty("inner")));
}

#[test]
fn test_validate_repeating_something_empty_is_refused() {
    // Tree-sitter: "The rule `source_file_repeat1` matches the empty
    // string", even in the unreferenced start rule.
    let start = |body| Grammar::new("demo").rule("source_file", body);
    assert_eq!(
        start(Rule::repeat(Rule::optional(s("a")))).to_js(),
        Err(matches_empty("source_file"))
    );
    assert_eq!(
        start(Rule::repeat1(Rule::blank())).to_js(),
        Err(matches_empty("source_file"))
    );
    assert_eq!(
        start(Rule::repeat1(Rule::repeat(s("a")))).to_js(),
        Err(matches_empty("source_file"))
    );
    let nested = reaching(&["b"]).rule(
        "b",
        Rule::seq([s("x"), Rule::repeat(Rule::optional(s("a")))]),
    );
    assert_eq!(nested.to_js(), Err(matches_empty("b")));
    // Inside a token, repetition is lexical; in an unreachable rule, ignored.
    assert!(
        start(Rule::seq([
            s("x"),
            Rule::token(Rule::repeat(Rule::optional(s("a"))))
        ]))
        .to_js()
        .is_ok()
    );
    assert!(
        base()
            .rule("unused", Rule::repeat(Rule::optional(s("b"))))
            .to_js()
            .is_ok()
    );
}

#[test]
fn test_validate_nullable_extra_rule_is_refused() {
    // Tree-sitter panics on this grammar.
    let g = base()
        .extra(Rule::pattern(r"\s"))
        .extra(sym("note"))
        .rule("note", Rule::optional(s("%")));
    assert_eq!(g.to_js(), Err(matches_empty("note")));
    // The extras reach further rules, which are checked like any other.
    let g = base()
        .extra(sym("note"))
        .rule("note", Rule::seq([s("#"), sym("text")]))
        .rule("text", Rule::optional(word()));
    assert_eq!(g.to_js(), Err(matches_empty("text")));
}

#[test]
fn test_validate_nullable_composite_extra_is_accepted() {
    let g = base()
        .extra(Rule::pattern(r"\s"))
        .extra(Rule::optional(s("#")));
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_repetition_in_an_extra_is_left_to_tree_sitter() {
    // Whether tree-sitter accepts a repetition in a non-terminal extra
    // depends on where it falls (it must be able to tell where the extra
    // ends), which only its parse tables decide.
    let middle = base()
        .extra(sym("note"))
        .rule("note", Rule::seq([s("#"), Rule::repeat(s("!")), s("$")]));
    assert!(middle.to_js().is_ok());
}

#[test]
fn test_validate_indirect_recursion_is_refused() {
    // Tree-sitter: "Grammar contains an indirectly recursive rule: x -> y
    // -> x".
    let cycle = |x: Rule| {
        reaching(&["x"])
            .rule("x", x)
            .rule("y", Rule::choice([sym("x"), s("b")]))
    };
    let refused = Err(Error::IndirectRecursion { rule: "x".into() });
    assert_eq!(cycle(Rule::choice([sym("y"), s("a")])).to_js(), refused);
    assert_eq!(cycle(sym("y")).to_js(), refused);
    assert_eq!(
        cycle(Rule::choice([Rule::optional(sym("y")), s("a")])).to_js(),
        refused
    );
    assert_eq!(
        cycle(Rule::choice([
            Rule::prec(1, Rule::field("f", Rule::alias(sym("y"), "q"))),
            s("a")
        ]))
        .to_js(),
        refused
    );
    assert_eq!(
        cycle(Rule::choice([Rule::choice([sym("y"), s("c")]), s("a")])).to_js(),
        refused
    );
    // A sequence or a repetition is more than the rule alone.
    assert!(
        cycle(Rule::choice([Rule::seq([sym("y"), s("!")]), s("a")]))
            .to_js()
            .is_ok()
    );
    assert!(
        cycle(Rule::choice([Rule::seq([sym("y")]), s("a")]))
            .to_js()
            .is_ok()
    );
    // Unreachable cycles are dropped by tree-sitter.
    let unreachable = base()
        .rule("x", Rule::choice([sym("y"), s("a")]))
        .rule("y", Rule::choice([sym("x"), s("b")]));
    assert!(unreachable.to_js().is_ok());
    // Reachable through an extra counts.
    let via_extra = base()
        .extra(sym("note"))
        .rule("note", Rule::seq([s("#"), sym("x")]))
        .rule("x", Rule::choice([sym("y"), s("a")]))
        .rule("y", Rule::choice([sym("x"), s("b")]));
    assert_eq!(via_extra.to_js(), refused);
}

#[test]
fn test_validate_empty_conflict_set_is_accepted() {
    assert!(base().conflict(Vec::<&'static str>::new()).to_js().is_ok());
}

// --- Tokens: word and externals ---------------------------------------------

/// A grammar whose word rule `id` has `body`, with `other` somewhere in the
/// start rule.
fn with_word(body: Rule, other: Rule) -> Grammar {
    Grammar::new("demo")
        .word("id")
        .rule(
            "source_file",
            Rule::repeat(Rule::choice([sym("id"), Rule::seq([other, s("!")])])),
        )
        .rule("id", body)
}

#[test]
fn test_validate_word_may_be_any_bare_token() {
    assert!(with_word(word(), s("x")).to_js().is_ok());
    assert!(with_word(s("abc"), s("x")).to_js().is_ok());
    assert!(
        with_word(
            Rule::token(Rule::seq([word(), Rule::pattern(r"\d")])),
            s("x")
        )
        .to_js()
        .is_ok()
    );
    assert!(with_word(Rule::immediate(word()), s("x")).to_js().is_ok());
    // An external may be the word token.
    assert!(base().external("ident").word("ident").to_js().is_ok());
}

#[test]
fn test_validate_wrapped_word_is_refused() {
    // Tree-sitter: "Non-terminal symbol 'id' cannot be used as the word
    // token".
    let refused = Err(Error::WordNotToken { name: "id".into() });
    assert_eq!(with_word(Rule::prec(1, word()), s("x")).to_js(), refused);
    assert_eq!(with_word(Rule::field("f", word()), s("x")).to_js(), refused);
    assert_eq!(with_word(Rule::seq([word()]), s("x")).to_js(), refused);
    let start = Grammar::new("demo")
        .word("source_file")
        .rule("source_file", word());
    assert_eq!(
        start.to_js(),
        Err(Error::WordNotToken {
            name: "source_file".into()
        })
    );
}

#[test]
fn test_validate_word_whose_token_is_used_elsewhere_is_refused() {
    // Tree-sitter merges identical tokens, leaving `id` a non-terminal.
    let refused = Err(Error::WordNotToken { name: "id".into() });
    assert_eq!(with_word(word(), word()).to_js(), refused);
    assert_eq!(with_word(word(), Rule::field("f", word())).to_js(), refused);
    assert_eq!(with_word(word(), Rule::alias(word(), "q")).to_js(), refused);
    assert_eq!(with_word(s("foo"), s("foo")).to_js(), refused);
    let token = || Rule::token(word());
    assert_eq!(with_word(token(), token()).to_js(), refused);
    // Not the same token: different precedence, or part of a larger token.
    assert!(
        with_word(word(), Rule::token(Rule::prec(1, word())))
            .to_js()
            .is_ok()
    );
    assert!(
        with_word(word(), Rule::token(Rule::seq([word(), s("?")])))
            .to_js()
            .is_ok()
    );
    // Uses in unreachable rules and in extras do not count.
    assert!(
        with_word(word(), s("x"))
            .rule("unused", Rule::seq([word(), s("!")]))
            .to_js()
            .is_ok()
    );
    assert!(
        with_word(word(), s("x"))
            .extra(Rule::token(word()))
            .to_js()
            .is_ok()
    );
}

#[test]
fn test_validate_hidden_literal_is_not_a_token() {
    // Tree-sitter treats a hidden rule whose token is only a string literal
    // as that anonymous string, so it is not a token rule; a hidden rule
    // whose token holds a pattern or a sequence is.
    let hidden = |body: Rule| {
        Grammar::new("demo")
            .word("_w")
            .rule(
                "source_file",
                Rule::repeat1(Rule::choice([sym("_w"), s("x")])),
            )
            .rule("_w", body)
    };
    let refused = Err(Error::WordNotToken { name: "_w".into() });
    assert_eq!(hidden(s("ab")).to_js(), refused);
    assert_eq!(hidden(Rule::token(s("ab"))).to_js(), refused);
    assert_eq!(hidden(Rule::immediate(s("ab"))).to_js(), refused);
    assert_eq!(hidden(Rule::token(Rule::prec(1, s("ab")))).to_js(), refused);
    assert!(hidden(word()).to_js().is_ok());
    assert!(hidden(Rule::token(word())).to_js().is_ok());
    assert!(hidden(Rule::immediate(word())).to_js().is_ok());
    assert!(
        hidden(Rule::immediate(Rule::seq([s("a"), s("b")])))
            .to_js()
            .is_ok()
    );
    // The same rule decides externals and inlining.
    let external = Grammar::new("demo")
        .external("_t")
        .rule(
            "source_file",
            Rule::repeat1(Rule::choice([sym("_t"), s("x")])),
        )
        .rule("_t", Rule::immediate(s("ab")));
    assert_eq!(
        external.to_js(),
        Err(Error::InvalidExternal { name: "_t".into() })
    );
    let inlined = |name: &'static str, body: Rule| {
        Grammar::new("demo")
            .inline(name)
            .rule(
                "source_file",
                Rule::repeat1(Rule::choice([sym(name), s("x")])),
            )
            .rule(name, body)
    };
    assert!(inlined("_i", Rule::immediate(s("ab"))).to_js().is_ok());
    assert!(inlined("_i", Rule::token(s("ab"))).to_js().is_ok());
    let refused = |name: &str| Err(Error::InvalidInline { name: name.into() });
    assert_eq!(
        inlined("_i", Rule::immediate(word())).to_js(),
        refused("_i")
    );
    assert_eq!(inlined("i", Rule::immediate(s("ab"))).to_js(), refused("i"));
}

#[test]
fn test_validate_word_duplicated_by_another_rule_is_refused() {
    // Tree-sitter: "its rule is duplicated in 'id2'".
    let g = Grammar::new("demo")
        .word("id")
        .rule(
            "source_file",
            Rule::repeat(Rule::choice([sym("id"), sym("id2")])),
        )
        .rule("id", word())
        .rule("id2", word());
    assert_eq!(g.to_js(), Err(Error::WordNotToken { name: "id".into() }));
}

#[test]
fn test_validate_external_may_share_a_token_rules_name() {
    // The rule is the fallback when the external scanner declines.
    assert!(base().external("item").to_js().is_ok());
    let g = reaching(&["_t"])
        .external("_t")
        .rule("_t", Rule::pattern("x+"));
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_external_sharing_a_non_token_rules_name_is_refused() {
    // Tree-sitter: "Rule 't' cannot be used as both an external token and a
    // non-terminal rule".
    let refused = |name: &str| Err(Error::InvalidExternal { name: name.into() });
    let g = reaching(&["t"])
        .external("t")
        .rule("t", Rule::prec(1, s("x")));
    assert_eq!(g.to_js(), refused("t"));
    let g = reaching(&["t"])
        .external("t")
        .rule("t", Rule::seq([s("a"), s("b")]));
    assert_eq!(g.to_js(), refused("t"));
    let g = Grammar::new("demo")
        .external("t")
        .rule(
            "source_file",
            Rule::repeat(Rule::choice([sym("t"), Rule::seq([s("x"), s("!")])])),
        )
        .rule("t", s("x"));
    assert_eq!(g.to_js(), refused("t"));
    let g = Grammar::new("demo")
        .external("source_file")
        .rule("source_file", s("x"));
    assert_eq!(g.to_js(), refused("source_file"));
}

#[test]
fn test_validate_token_identity_follows_tree_sitter() {
    // `token('ab')` is the same token as `'ab'`, and `token(/x+/)` as `/x+/`;
    // `token.immediate`, an inner `prec`, or a second `token` make another.
    let refused = Err(Error::WordNotToken { name: "id".into() });
    assert_eq!(with_word(Rule::token(s("ab")), s("ab")).to_js(), refused);
    assert_eq!(with_word(s("ab"), Rule::token(s("ab"))).to_js(), refused);
    assert_eq!(with_word(Rule::token(word()), word()).to_js(), refused);
    assert!(with_word(Rule::immediate(s("ab")), s("ab")).to_js().is_ok());
    assert!(
        with_word(Rule::token(Rule::prec(1, s("ab"))), s("ab"))
            .to_js()
            .is_ok()
    );
    assert!(
        with_word(Rule::token(Rule::token(s("ab"))), s("ab"))
            .to_js()
            .is_ok()
    );
}

#[test]
fn test_validate_literal_means_one_token_layer() {
    let hidden = |body: Rule| {
        Grammar::new("demo")
            .word("_w")
            .rule(
                "source_file",
                Rule::repeat1(Rule::choice([sym("_w"), s("x")])),
            )
            .rule("_w", body)
    };
    let refused = Err(Error::WordNotToken { name: "_w".into() });
    assert_eq!(
        hidden(Rule::immediate(Rule::prec(2, s("ab")))).to_js(),
        refused
    );
    assert!(
        hidden(Rule::immediate(Rule::immediate(s("ab"))))
            .to_js()
            .is_ok()
    );
    assert!(
        hidden(Rule::token(Rule::immediate(s("ab"))))
            .to_js()
            .is_ok()
    );
    assert!(hidden(Rule::token(Rule::token(s("ab")))).to_js().is_ok());
    // An alias inside the token changes nothing about what is lexed.
    let aliased = Rule::token(Rule::alias(s("ab"), "_x"));
    assert_eq!(hidden(aliased).to_js(), refused);
}

#[test]
fn test_validate_supertypes_are_hidden_for_the_token_test() {
    // A literal supertype is not a token, so it cannot be the word.
    let g = Grammar::new("demo")
        .supertype("a")
        .word("a")
        .rule(
            "source_file",
            Rule::repeat1(Rule::choice([sym("a"), s("x")])),
        )
        .rule("a", s("y"));
    assert_eq!(g.to_js(), Err(Error::WordNotToken { name: "a".into() }));
    // A supertype whose pattern is used elsewhere is merged, not a token.
    let g = Grammar::new("demo")
        .supertype("a")
        .rule(
            "source_file",
            Rule::repeat1(Rule::choice([sym("a"), Rule::prec(2, Rule::pattern("q"))])),
        )
        .rule("a", Rule::pattern("q"));
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_hidden_rule_that_is_itself_under_precedence_is_refused() {
    // Tree-sitter never finishes generating these grammars.
    let refused = Err(Error::IndirectRecursion { rule: "_d".into() });
    let hidden = |body: Rule| reaching(&["_d"]).rule("_d", body);
    assert_eq!(hidden(Rule::prec_left(1, sym("_d"))).to_js(), refused);
    assert_eq!(hidden(Rule::prec(1, sym("_d"))).to_js(), refused);
    assert_eq!(
        hidden(Rule::choice([Rule::prec_right(2, sym("_d")), s("y")])).to_js(),
        refused
    );
    // These it finishes: a visible rule, precedence zero, dynamic
    // precedence, or a field in between (leaving any conflict to it).
    let visible = reaching(&["d"]).rule("d", Rule::prec(1, sym("d")));
    assert!(visible.to_js().is_ok());
    assert!(hidden(Rule::prec_left(0, sym("_d"))).to_js().is_ok());
    assert!(hidden(Rule::prec_dynamic(1, sym("_d"))).to_js().is_ok());
    assert!(
        hidden(Rule::field("f", Rule::prec_left(1, sym("_d"))))
            .to_js()
            .is_ok()
    );
}

// --- Inline -----------------------------------------------------------------

#[test]
fn test_validate_inline_of_ordinary_rules_is_accepted() {
    let g = reaching(&["_name", "_hidden_string", "_glued"])
        .inline("_name")
        .inline("_hidden_string")
        .inline("_glued")
        .rule("_name", Rule::choice([sym("item"), s("self")]))
        .rule("_hidden_string", s("x"))
        .rule("_glued", Rule::immediate(s("y")))
        .rule("item", word());
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_inline_of_tokens_externals_and_the_start_rule_is_refused() {
    let refused = |name: &str| Err(Error::InvalidInline { name: name.into() });
    // "Rule `source_file` cannot be inlined because it is the first rule".
    assert_eq!(base().inline("source_file").to_js(), refused("source_file"));
    // "External token `e` cannot be inlined".
    assert_eq!(base().external("e").inline("e").to_js(), refused("e"));
    // "Token `item` cannot be inlined", for patterns, `token`, and visible
    // strings.
    assert_eq!(base().inline("item").to_js(), refused("item"));
    let g = reaching(&["_t"])
        .inline("_t")
        .rule("_t", Rule::token(Rule::seq([s("a"), s("b")])));
    assert_eq!(g.to_js(), refused("_t"));
    let g = reaching(&["kw"]).inline("kw").rule("kw", s("if"));
    assert_eq!(g.to_js(), refused("kw"));
}

#[test]
fn test_validate_inline_judges_a_supertype_by_its_own_name() {
    // A visible literal is a token to the inline check even when it is also
    // a supertype (which counts as hidden everywhere else).
    let g = Grammar::new("demo")
        .supertype("a")
        .inline("a")
        .rule(
            "source_file",
            Rule::repeat1(Rule::choice([sym("a"), s("x")])),
        )
        .rule("a", Rule::token(s("a")));
    assert_eq!(g.to_js(), Err(Error::InvalidInline { name: "a".into() }));
}

#[test]
fn test_validate_inline_recursion_is_refused() {
    let refused = |name: &str| Err(Error::InvalidInline { name: name.into() });
    // "Rule `_e` cannot be inlined because it contains a reference to
    // itself".
    let g = reaching(&["_e"]).inline("_e").rule(
        "_e",
        Rule::choice([s("a"), Rule::seq([s("("), sym("_e"), s(")")])]),
    );
    assert_eq!(g.to_js(), refused("_e"));
    // Through two inlined rules, tree-sitter loops forever.
    let cycle = |inline_b: bool| {
        let g = reaching(&["_a"])
            .inline("_a")
            .rule("_a", Rule::choice([s("x"), Rule::seq([s("("), sym("_b")])]))
            .rule("_b", Rule::seq([sym("_a"), s(")")]));
        if inline_b { g.inline("_b") } else { g }
    };
    assert_eq!(cycle(true).to_js(), refused("_a"));
    // With only one of the two inlined, the cycle passes through a real rule.
    assert!(cycle(false).to_js().is_ok());
    // A reference inside a repetition moves into tree-sitter's helper rule.
    let g = reaching(&["_a"]).inline("_a").rule(
        "_a",
        Rule::seq([s("p"), Rule::repeat1(Rule::seq([s("q"), sym("_a")]))]),
    );
    assert!(g.to_js().is_ok());
}

// --- Supertypes -------------------------------------------------------------

/// A grammar with supertype `_v` of the given body, reachable, and the leaf
/// rules `a`, `b`, `c`.
fn with_supertype(body: Rule) -> Grammar {
    reaching(&["_v"])
        .supertype("_v")
        .rule("_v", body)
        .rule("a", s("a"))
        .rule("b", s("b"))
        .rule("c", s("c"))
}

#[test]
fn test_validate_supertypes_of_single_nodes_are_accepted() {
    assert!(
        with_supertype(Rule::choice([sym("a"), sym("b")]))
            .to_js()
            .is_ok()
    );
    assert!(with_supertype(sym("a")).to_js().is_ok());
    assert!(
        with_supertype(Rule::choice([sym("a"), s("x")]))
            .to_js()
            .is_ok()
    );
    assert!(
        with_supertype(Rule::choice([sym("a"), Rule::alias(sym("b"), "c2")]))
            .to_js()
            .is_ok()
    );
    assert!(
        with_supertype(Rule::prec(
            1,
            Rule::choice([sym("a"), Rule::field("f", sym("b"))])
        ))
        .to_js()
        .is_ok()
    );
    assert!(
        with_supertype(Rule::seq([sym("a"), Rule::blank()]))
            .to_js()
            .is_ok()
    );
    let nested = with_supertype(Rule::choice([sym("a"), sym("_w")]))
        .rule("_w", Rule::choice([sym("b"), sym("_x")]))
        .rule("_x", sym("c"));
    assert!(nested.to_js().is_ok());
    // A visible supertype, and the start rule as a supertype, are fine.
    let visible = reaching(&["v"])
        .supertype("v")
        .rule("v", Rule::choice([sym("a"), sym("b")]))
        .rule("a", s("a"))
        .rule("b", s("b"));
    assert!(visible.to_js().is_ok());
}

#[test]
fn test_validate_supertypes_with_several_nodes_are_refused() {
    // Tree-sitter: "Supertypes must have a single visible child, but `_v`
    // can have multiple."
    let refused = Err(Error::InvalidSupertype { name: "_v".into() });
    assert_eq!(
        with_supertype(Rule::seq([s("("), sym("a"), s(")")])).to_js(),
        refused
    );
    assert_eq!(
        with_supertype(Rule::choice([sym("a"), Rule::seq([s("!"), sym("a")])])).to_js(),
        refused
    );
    assert_eq!(
        with_supertype(Rule::choice([sym("a"), Rule::repeat1(sym("a"))])).to_js(),
        refused
    );
    let hidden = with_supertype(Rule::choice([sym("a"), sym("_w")]))
        .rule("_w", Rule::seq([s("("), sym("a"), s(")")]));
    assert_eq!(hidden.to_js(), refused);
    // A hidden token is still a node here.
    let token = with_supertype(Rule::choice([sym("a"), Rule::seq([sym("a"), sym("_h")])]))
        .rule("_h", s("h"));
    assert_eq!(token.to_js(), refused);
}

#[test]
fn test_validate_supertype_tokens_and_externals_are_refused() {
    // "Terminal rule '_t' cannot be used as a supertype".
    let g = reaching(&["_t"]).supertype("_t").rule("_t", word());
    assert_eq!(
        g.to_js(),
        Err(Error::InvalidSupertype { name: "_t".into() })
    );
    let g = base().external("e").supertype("e");
    assert_eq!(g.to_js(), Err(Error::InvalidSupertype { name: "e".into() }));
}

#[test]
fn test_validate_supertype_literals_are_accepted() {
    // Only a token with a pattern in it is a "terminal rule" to the
    // supertype check; literals pass, visible or hidden.
    let g = |name: &'static str, body: Rule| reaching(&[name]).supertype(name).rule(name, body);
    assert!(g("t", s("ab")).to_js().is_ok());
    assert!(g("_t", Rule::token(s("ab"))).to_js().is_ok());
    assert!(g("_t", Rule::immediate(s("ab"))).to_js().is_ok());
    assert!(g("_t", Rule::prec(1, word())).to_js().is_ok());
    let refused = |name: &str| Err(Error::InvalidSupertype { name: name.into() });
    assert_eq!(g("t", Rule::token(word())).to_js(), refused("t"));
    assert_eq!(g("_t", Rule::immediate(word())).to_js(), refused("_t"));
    // Unreachable, it is not checked.
    assert!(base().supertype("_t").rule("_t", word()).to_js().is_ok());
}

#[test]
fn test_validate_supertype_that_can_be_itself_is_refused() {
    // Tree-sitter: "Dependency cycle detected in node types".
    let refused = Err(Error::InvalidSupertype { name: "_v".into() });
    assert_eq!(
        with_supertype(Rule::choice([sym("a"), sym("_v")])).to_js(),
        refused
    );
    // A rule that is only ever itself is refused before supertypes are
    // looked at.
    assert_eq!(
        with_supertype(Rule::prec(1, sym("_v"))).to_js(),
        Err(Error::IndirectRecursion { rule: "_v".into() })
    );
    let blank = Rule::seq([sym("_v"), Rule::blank()]);
    assert_eq!(
        with_supertype(Rule::choice([sym("a"), blank])).to_js(),
        refused
    );
    // Through an alias it is another node.
    let aliased = Rule::alias(sym("_v"), "q");
    assert!(
        with_supertype(Rule::choice([sym("a"), aliased]))
            .to_js()
            .is_ok()
    );
}

#[test]
fn test_validate_supertype_alias_renames_each_element() {
    // An alias around a sequence renames each of its elements; it does not
    // make them one node.
    let refused = Err(Error::InvalidSupertype { name: "_v".into() });
    let pair = Rule::alias(Rule::seq([sym("a"), sym("b")]), "pair");
    assert_eq!(with_supertype(pair).to_js(), refused);
    // Around a single symbol it is one renamed node, even of a hidden rule.
    let one = with_supertype(Rule::choice([sym("a"), Rule::alias(sym("_w"), "w")]))
        .rule("_w", Rule::seq([sym("b"), sym("c")]));
    assert!(one.to_js().is_ok());
}

#[test]
fn test_validate_supertype_reaching_itself_through_a_hidden_rule_is_refused() {
    // Tree-sitter: "Dependency cycle detected in node types".
    let g = with_supertype(Rule::choice([sym("a"), Rule::seq([sym("_w")])])).rule("_w", sym("_v"));
    assert_eq!(
        g.to_js(),
        Err(Error::InvalidSupertype { name: "_v".into() })
    );
}

#[test]
fn test_validate_supertype_repetition_is_several_nodes() {
    let refused = Err(Error::InvalidSupertype { name: "_v".into() });
    assert_eq!(
        with_supertype(Rule::choice([sym("a"), Rule::repeat1(s("b"))])).to_js(),
        refused
    );
}

#[test]
fn test_validate_inlined_supertype_is_not_checked_as_one() {
    // Inlined away before supertypes are looked at.
    let g = with_supertype(Rule::seq([s("("), sym("a"), s(")")])).inline("_v");
    assert!(g.to_js().is_ok());
}

#[test]
fn test_validate_unreachable_supertype_shape_is_not_checked() {
    let g = base()
        .supertype("_v")
        .rule("_v", Rule::seq([sym("item"), sym("item")]));
    assert!(g.to_js().is_ok());
}

// --- grammar.js -------------------------------------------------------------

/// `base` with every grammar-level setting used.
fn configured() -> Grammar {
    base()
        .word("item")
        .conflict(["source_file", "_helper"])
        .inline("_helper")
        .inline("_helper")
        .supertype("_helper")
        .external("ext")
        .extra(Rule::pattern(r"\s"))
        .rule("_helper", Rule::choice([sym("item"), sym("ext")]))
}

#[test]
fn test_js_sections_appear_in_fixed_order() {
    let js = configured().to_js().unwrap();
    let at = |needle: &str| {
        js.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing"))
    };
    let order = [
        at("  name:"),
        at("  extras:"),
        at("  externals:"),
        at("  supertypes:"),
        at("  inline:"),
        at("  conflicts:"),
        at("  word:"),
        at("  rules:"),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{js}");
}

#[test]
fn test_js_repeated_inline_entries_are_written_once() {
    // Tree-sitter warns about repeats and keeps the first.
    let js = configured().to_js().unwrap();
    assert!(js.contains("  inline: $ => [\n    $._helper,\n  ],"));
}

#[test]
fn test_validate_pattern_starting_with_a_quantifier_is_refused() {
    // Until 1.0.1 this was written `new RegExp('*a')` (`/*` would open a
    // comment) and left for tree-sitter to refuse; no JavaScript regular
    // expression starts with a quantifier, so validation now refuses it.
    assert_eq!(
        base().rule("x", Rule::pattern("*a")).to_js(),
        Err(Error::EmptyString {
            rule: "x: a quantifier has nothing to repeat at byte 0 of `*a`".into()
        })
    );
}

#[test]
fn test_js_control_characters_in_patterns_are_escaped() {
    let js = base().rule("x", Rule::pattern("a\0b\t")).to_js().unwrap();
    assert!(js.contains(r"x: $ => /a\x00b\t/,"));
}

#[test]
fn test_js_appends_to_existing_buffer() {
    let mut buffer = String::from("// header\n");
    base().write_js(&mut buffer).unwrap();
    assert!(buffer.starts_with("// header\n/// <reference"));
    assert_eq!(&buffer["// header\n".len()..], base().to_js().unwrap());
}

#[test]
fn test_js_output_is_deterministic() {
    assert_eq!(base().to_js(), base().to_js());
    assert_eq!(base().to_json(), base().to_json());
}

#[test]
fn test_js_owned_names_and_text_are_emitted() {
    let name = String::from("dynamic");
    let g = Grammar::new(name.clone())
        .rule(
            format!("{name}_file"),
            Rule::repeat(Rule::symbol(format!("{name}_item"))),
        )
        .rule(format!("{name}_item"), Rule::string(format!("{name}!")));
    let js = g.to_js().unwrap();
    assert!(js.contains("dynamic_file: $ => repeat($.dynamic_item),"));
    assert!(js.contains("dynamic_item: $ => 'dynamic!',"));
}

// --- grammar.json -----------------------------------------------------------

#[test]
fn test_json_grammar_settings() {
    let json = configured()
        .conflict(Vec::<&'static str>::new())
        .to_json()
        .unwrap();
    assert!(json.contains("  \"name\": \"demo\",\n  \"word\": \"item\",\n  \"rules\": {"));
    assert!(json.contains(
        "  \"conflicts\": [\n    [\n      \"source_file\",\n      \"_helper\"\n    ],\n    []\n  ],"
    ));
    assert!(json.contains("  \"precedences\": [],"));
    assert!(json.contains(
        "  \"externals\": [\n    {\n      \"type\": \"SYMBOL\",\n      \"name\": \"ext\"\n    }\n  ],"
    ));
    // Repeated inline entries are written once, as tree-sitter writes them.
    assert!(json.contains("  \"inline\": [\n    \"_helper\"\n  ],"));
    assert!(json.contains("  \"supertypes\": [\n    \"_helper\"\n  ],"));
}

#[test]
fn test_json_explicit_extras_replace_the_default() {
    let json = base().extra(sym("item")).to_json().unwrap();
    assert!(json.contains(
        "  \"extras\": [\n    {\n      \"type\": \"SYMBOL\",\n      \"name\": \"item\"\n    }\n  ],"
    ));
    assert!(!json.contains("\\\\s"));
}

#[test]
fn test_json_optional_is_a_choice_with_blank() {
    let json = base().rule("x", Rule::optional(s("a"))).to_json().unwrap();
    assert!(json.contains(
        "\"type\": \"CHOICE\",\n      \"members\": [\n        {\n          \"type\": \"STRING\",\n          \
         \"value\": \"a\"\n        },\n        {\n          \"type\": \"BLANK\"\n        }\n      ]"
    ));
}

#[test]
fn test_json_control_characters_in_patterns_match_grammar_js() {
    let json = base().rule("x", Rule::pattern("a\0b")).to_json().unwrap();
    assert!(json.contains(r#""value": "a\\x00b""#));
}

// --- S-expressions ----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    File,
    Call,
    Args,
    Group,
    Name,
    Paren,
    Comma,
    Space,
    Comment,
    Mystery,
}

impl K {
    fn name(&self) -> &'static str {
        match self {
            K::File => "file",
            K::Call => "call",
            K::Args => "arguments",
            K::Group => "_group",
            K::Name => "name",
            K::Paren => "(",
            K::Comma => ",",
            K::Space => "whitespace",
            K::Comment => "comment",
            K::Mystery => "mystery",
        }
    }
}

fn sexp_grammar() -> Grammar {
    Grammar::new("calls")
        .extra(Rule::pattern(r"\s"))
        .extra(sym("comment"))
        .rule("file", Rule::repeat(sym("call")))
        .rule("call", Rule::seq([sym("name"), sym("arguments")]))
        .rule(
            "arguments",
            Rule::seq([s("("), Rule::optional(sym("_group")), s(")")]),
        )
        .rule(
            "_group",
            Rule::seq([sym("name"), Rule::repeat(Rule::seq([s(","), sym("name")]))]),
        )
        .rule("name", Rule::pattern("[a-z]+"))
        .rule(
            "comment",
            Rule::token(Rule::seq([s("#"), Rule::pattern(".*")])),
        )
}

fn t(kind: K, start: u32, end: u32) -> Token<K> {
    Token::new(kind, Span::new(start, end))
}

/// `f(a, b) # c` built the way a parser would.
fn call_tree() -> Node<K> {
    let mut b = Builder::new();
    b.start_node(K::File);
    b.start_node(K::Call);
    b.token(t(K::Name, 0, 1));
    b.start_node(K::Args);
    b.token(t(K::Paren, 1, 2));
    b.start_node(K::Group);
    b.token(t(K::Name, 2, 3));
    b.token(t(K::Comma, 3, 4));
    b.token(t(K::Space, 4, 5));
    b.token(t(K::Name, 5, 6));
    b.finish_node();
    b.token(t(K::Paren, 6, 7));
    b.finish_node();
    b.finish_node();
    b.token(t(K::Space, 7, 8));
    b.token(t(K::Comment, 8, 11));
    b.finish_node();
    b.finish().unwrap()
}

#[test]
fn test_sexp_renders_named_nodes_only() {
    let sexp = sexp_grammar().sexp(&call_tree(), K::name).unwrap();
    assert_eq!(
        sexp,
        "(file\n  (call\n    (name)\n    (arguments\n      (name)\n      (name)))\n  (comment))"
    );
}

#[test]
fn test_sexp_unknown_node_is_reported_and_buffer_untouched() {
    let tree = Node::new(
        K::File,
        vec![Element::Node(Node::new(
            K::Mystery,
            vec![Element::Token(t(K::Name, 0, 1))],
        ))],
    );
    let mut out = String::from("keep");
    let result = sexp_grammar().write_sexp(&tree, K::name, &mut out);
    assert_eq!(
        result,
        Err(Error::UnknownNode {
            name: "mystery".into()
        })
    );
    assert_eq!(out, "keep");
}

#[test]
fn test_sexp_unknown_tokens_are_anonymous() {
    let tree = Node::new(K::File, vec![Element::Token(t(K::Mystery, 0, 1))]);
    assert_eq!(sexp_grammar().sexp(&tree, K::name).unwrap(), "(file)");
}

#[test]
fn test_sexp_appends_entries_to_one_buffer() {
    let grammar = sexp_grammar();
    let tree = call_tree();
    let mut corpus = String::new();
    for _ in 0..3 {
        grammar.write_sexp(&tree, K::name, &mut corpus).unwrap();
        corpus.push('\n');
    }
    assert_eq!(corpus.matches("(file").count(), 3);
}

#[test]
fn test_sexp_naming_function_may_borrow_from_its_environment() {
    // Names held in a table the closure borrows, not `&'static str`.
    let table: Vec<String> = [
        "file",
        "call",
        "arguments",
        "_group",
        "name",
        "(",
        ",",
        " ",
        "comment",
        "?",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let index = |k: &K| *k as usize;
    let sexp = sexp_grammar()
        .sexp(&call_tree(), |k| table[index(k)].as_str())
        .unwrap();
    assert!(sexp.starts_with("(file\n  (call"));
}
