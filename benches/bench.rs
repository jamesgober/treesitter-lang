//! Benchmarks for emitting grammars and rendering trees.
//!
//! - `emit/js/*`, `emit/json/*`: validate and emit a grammar into a reused
//!   buffer, for a small real grammar and a synthetic one of 1,000 rules.
//! - `emit/escapes`: a grammar whose strings and patterns all need escaping.
//! - `sexp/*`: render a concrete syntax tree of about 10,000 and 100,000
//!   elements into a reused buffer.
//! - `build/large`: construct the 1,000-rule grammar, then drop it.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use treesitter_lang::syntax_lang::{Element, Node, Span, Token};
use treesitter_lang::{Grammar, Rule};

/// A small expression language, about the size of a configuration format.
fn small() -> Grammar {
    let e = || Rule::symbol("_expression");
    Grammar::new("small")
        .extra(Rule::pattern(r"\s"))
        .extra(Rule::symbol("comment"))
        .word("identifier")
        .rule("source_file", Rule::repeat(Rule::symbol("statement")))
        .rule("statement", Rule::seq([e(), Rule::string(";")]))
        .rule(
            "_expression",
            Rule::choice([
                Rule::symbol("identifier"),
                Rule::symbol("number"),
                Rule::symbol("binary"),
                Rule::symbol("call"),
            ]),
        )
        .rule(
            "binary",
            Rule::choice([
                Rule::prec_left(
                    1,
                    Rule::seq([
                        Rule::field("left", e()),
                        Rule::string("+"),
                        Rule::field("right", e()),
                    ]),
                ),
                Rule::prec_left(
                    2,
                    Rule::seq([
                        Rule::field("left", e()),
                        Rule::string("*"),
                        Rule::field("right", e()),
                    ]),
                ),
            ]),
        )
        .rule(
            "call",
            Rule::seq([
                Rule::field("function", Rule::symbol("identifier")),
                Rule::string("("),
                Rule::optional(Rule::seq([
                    e(),
                    Rule::repeat(Rule::seq([Rule::string(","), e()])),
                ])),
                Rule::string(")"),
            ]),
        )
        .rule("identifier", Rule::pattern("[a-z_][a-z0-9_]*"))
        .rule("number", Rule::pattern(r"\d+"))
        .rule(
            "comment",
            Rule::token(Rule::seq([Rule::string("#"), Rule::pattern(".*")])),
        )
}

/// 1,000 rules, each referring to the next, with every rule type in use.
fn large() -> Grammar {
    const RULES: usize = 1_000;
    let mut grammar = Grammar::new("large")
        .word("identifier")
        .rule("source_file", Rule::repeat(Rule::symbol("rule_0")));
    for i in 0..RULES {
        let next = if i + 1 < RULES {
            Rule::symbol(format!("rule_{}", i + 1))
        } else {
            Rule::symbol("identifier")
        };
        let body = Rule::prec_left(
            (i % 7) as i32,
            Rule::seq([
                Rule::string(format!("kw{i}")),
                Rule::field("child", next),
                Rule::optional(Rule::choice([
                    Rule::symbol("identifier"),
                    Rule::alias(Rule::symbol("number"), "literal"),
                    Rule::token(Rule::seq([Rule::string("@"), Rule::pattern(r"\d+")])),
                ])),
                Rule::repeat(Rule::seq([Rule::string(","), Rule::symbol("identifier")])),
            ]),
        );
        grammar = grammar.rule(format!("rule_{i}"), body);
    }
    grammar
        .rule("identifier", Rule::pattern("[a-z_][a-z0-9_]*"))
        .rule("number", Rule::pattern(r"\d+"))
}

/// Strings and patterns full of characters both formats must escape.
fn escapes() -> Grammar {
    let strings = (0..200).map(|i| Rule::string(format!("it's \"{i}\" \\ tab\there\u{2028}")));
    let patterns = (0..200).map(|i| Rule::pattern(format!("a/b{i}\\/c[^/]")));
    Grammar::new("escapes").rule(
        "source_file",
        Rule::repeat(Rule::choice(strings.chain(patterns))),
    )
}

fn bench_emit(c: &mut Criterion) {
    let mut group = c.benchmark_group("emit");
    let mut buffer = String::new();
    for (name, grammar) in [("small", small()), ("large", large())] {
        buffer.clear();
        grammar.write_js(&mut buffer).unwrap();
        group.throughput(Throughput::Bytes(buffer.len() as u64));
        group.bench_with_input(BenchmarkId::new("js", name), &grammar, |b, g| {
            b.iter(|| {
                buffer.clear();
                g.write_js(&mut buffer).unwrap();
                black_box(buffer.len())
            });
        });
        buffer.clear();
        grammar.write_json(&mut buffer).unwrap();
        group.throughput(Throughput::Bytes(buffer.len() as u64));
        group.bench_with_input(BenchmarkId::new("json", name), &grammar, |b, g| {
            b.iter(|| {
                buffer.clear();
                g.write_json(&mut buffer).unwrap();
                black_box(buffer.len())
            });
        });
    }
    let grammar = escapes();
    group.throughput(Throughput::Elements(400));
    group.bench_function("escapes", |b| {
        b.iter(|| {
            buffer.clear();
            grammar.write_js(&mut buffer).unwrap();
            grammar.write_json(&mut buffer).unwrap();
            black_box(buffer.len())
        });
    });
    group.finish();
}

/// Tree names: three visible rules, one hidden rule, anonymous punctuation.
fn name(kind: &u8) -> &'static str {
    ["call", "args", "_group", "identifier", "(", ","][usize::from(*kind)]
}

fn tree_grammar() -> Grammar {
    Grammar::new("trees")
        .rule("call", Rule::repeat1(Rule::symbol("args")))
        .rule(
            "args",
            Rule::seq([Rule::string("("), Rule::symbol("_group")]),
        )
        .rule("_group", Rule::repeat1(Rule::symbol("identifier")))
        .rule("identifier", Rule::pattern("[a-z]+"))
}

/// A tree of nested calls with roughly `elements` elements.
fn tree(elements: usize) -> Node<u8> {
    let token = |kind: u8| Element::Token(Token::new(kind, Span::new(0, 1)));
    let per_call = 8;
    let calls = (0..elements / per_call)
        .map(|_| {
            let group = Node::new(2, vec![token(3), token(5), token(3), token(5), token(3)]);
            Element::Node(Node::new(1, vec![token(4), Element::Node(group)]))
        })
        .collect();
    Node::new(0, calls)
}

fn bench_sexp(c: &mut Criterion) {
    let grammar = tree_grammar();
    let mut group = c.benchmark_group("sexp");
    let mut buffer = String::new();
    for elements in [10_000, 100_000] {
        let root = tree(elements);
        group.throughput(Throughput::Elements(elements as u64));
        group.bench_with_input(BenchmarkId::from_parameter(elements), &root, |b, root| {
            b.iter(|| {
                buffer.clear();
                grammar.write_sexp(root, name, &mut buffer).unwrap();
                black_box(buffer.len())
            });
        });
    }
    group.finish();
}

fn bench_build(c: &mut Criterion) {
    c.bench_function("build/large", |b| b.iter(|| black_box(large())));
}

criterion_group!(benches, bench_emit, bench_sexp, bench_build);
criterion_main!(benches);
