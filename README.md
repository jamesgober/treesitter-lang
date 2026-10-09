<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>treesitter-lang</b>
    <br>
    <sub><sup>TREE-SITTER GENERATION</sup></sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/treesitter-lang"><img alt="Crates.io" src="https://img.shields.io/crates/v/treesitter-lang"></a>
    <a href="https://crates.io/crates/treesitter-lang"><img alt="Downloads" src="https://img.shields.io/crates/d/treesitter-lang?color=%230099ff"></a>
    <a href="https://docs.rs/treesitter-lang"><img alt="docs.rs" src="https://img.shields.io/docsrs/treesitter-lang"></a>
    <a href="https://github.com/jamesgober/treesitter-lang/actions"><img alt="CI" src="https://github.com/jamesgober/treesitter-lang/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>treesitter-lang</strong> builds <a href="https://tree-sitter.github.io/">tree-sitter</a> grammars in Rust. Describe a language's syntax with rules, and it emits the <code>grammar.js</code> &mdash; or <code>grammar.json</code> &mdash; that <code>tree-sitter generate</code> turns into a parser for syntax highlighting, code folding, structural navigation, and the rest of what editors build on tree-sitter.
    </p>
    <p>
        A language made with the <code>-lang</code> family already has a parser: a hand-written one that builds a lossless <a href="https://crates.io/crates/syntax-lang"><code>syntax-lang</code></a> tree. Editors want a tree-sitter grammar for the same language, and two parsers for one language drift apart unless something holds them together. treesitter-lang is that something. The grammar lives in Rust, next to the parser, where it can be built from the same operator and keyword tables. It is validated before tree-sitter ever sees it, against the rules tree-sitter itself enforces. And the trees the hand-written parser builds render as the S-expressions tree-sitter prints, so every sample the parser handles becomes a tree-sitter corpus test: if the two parsers ever disagree, <code>tree-sitter test</code> fails.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition). <code>no_std</code>-compatible (needs only <code>alloc</code>), <code>#![forbid(unsafe_code)]</code>, one dependency: <a href="https://crates.io/crates/syntax-lang"><code>syntax-lang</code></a>.
    </p>
    <blockquote>
        <strong>1.0.0 is the API freeze.</strong> The public surface is stable and follows Semantic Versioning &mdash; no breaking changes before <code>2.0</code>. See <a href="./docs/API.md#stability"><code>docs/API.md</code></a> for the frozen surface, the validation and output contracts, and the SemVer promise, and <a href="./CHANGELOG.md"><code>CHANGELOG.md</code></a>.
    </blockquote>
</div>

<hr>
<br>

## The model

Three types, one per job:

- A **[`Rule`](./docs/API.md#rule)** is one rule expression. Its constructors are tree-sitter's DSL, one for one: `symbol`, `string`, `pattern`, `seq`, `choice`, `optional`, `repeat`, `repeat1`, `field`, `alias`, `token`, `immediate`, `prec`, `prec_left`, `prec_right`, `prec_dynamic`, `blank`.
- A **[`Grammar`](./docs/API.md#grammar)** is the named rules — the first one is the start rule — and the grammar-level settings: extras, externals, supertypes, inlined rules, conflicts, the word token. It validates itself and emits **[`grammar.js`](./docs/API.md#grammarto_js)** or **[`grammar.json`](./docs/API.md#grammarto_json)**, and renders a `syntax-lang` tree as a **[corpus S-expression](./docs/API.md#grammarsexp)**.
- An **[`Error`](./docs/API.md#error)** says what validation found, in tree-sitter's terms.

<br>

What the crate guarantees about its output, checked in CI against the real tree-sitter CLI:

| Guarantee | How it is held |
|---|---|
| A grammar that validates is one `tree-sitter generate` loads, up to the conflicts only its parse tables reveal. | Validation mirrors tree-sitter's own checks, each probed against the CLI, and is cross-checked by running thousands of random grammars through both. |
| `grammar.js` and `grammar.json` describe the same grammar. | Both generate the same `parser.c`, and the emitted `grammar.json` is byte-identical to the one tree-sitter writes from the emitted `grammar.js` (tree-sitter 0.25 and later). |
| Any text survives escaping. | Property tests read both formats back with independent parsers, over strings full of quotes, backslashes, control characters, and line separators. |
| A corpus built from the hand-written parser passes against the generated parser. | The `mini` example's corpus runs through `tree-sitter test` on every change. |

<hr>
<br>

## Installation

```toml
[dependencies]
treesitter-lang = "1"
```

Or from the terminal:

```bash
cargo add treesitter-lang
```

Grammars are usually generated by a small program or a build step, so a dev-dependency works just as well. `syntax-lang` is re-exported as `treesitter_lang::syntax_lang`. MSRV: Rust 1.85 (Rust 2024 edition).

<hr>
<br>

## Quick start

A calculator, from rules to `grammar.js`:

```rust
use treesitter_lang::{Grammar, Rule};

let e = || Rule::symbol("_expression");
let grammar = Grammar::new("calc")
    .extra(Rule::pattern(r"\s"))
    .extra(Rule::symbol("comment"))
    .rule("source_file", Rule::repeat(e()))
    .rule("_expression", Rule::choice([
        Rule::symbol("number"),
        Rule::symbol("binary_expression"),
        Rule::seq([Rule::string("("), e(), Rule::string(")")]),
    ]))
    .rule("binary_expression", Rule::choice([
        Rule::prec_left(1, Rule::seq([
            Rule::field("left", e()),
            Rule::field("operator", Rule::choice([Rule::string("+"), Rule::string("-")])),
            Rule::field("right", e()),
        ])),
        Rule::prec_left(2, Rule::seq([
            Rule::field("left", e()),
            Rule::field("operator", Rule::choice([Rule::string("*"), Rule::string("/")])),
            Rule::field("right", e()),
        ])),
    ]))
    .rule("number", Rule::pattern(r"\d+(\.\d+)?"))
    .rule("comment", Rule::token(Rule::seq([Rule::string("#"), Rule::pattern(".*")])));

let js = grammar.to_js()?;
assert!(js.contains("    number: $ => /\\d+(\\.\\d+)?/,"));
// std::fs::write("tree-sitter-calc/grammar.js", &js)?;
// Then, in tree-sitter-calc/:  tree-sitter generate && tree-sitter parse example.calc
# Ok::<(), treesitter_lang::Error>(())
```

The output reads like a grammar written by hand:

```js
/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: 'calc',

  extras: $ => [
    /\s/,
    $.comment,
  ],

  rules: {
    source_file: $ => repeat($._expression),

    _expression: $ => choice(
      $.number,
      $.binary_expression,
      seq('(', $._expression, ')'),
    ),

    binary_expression: $ => choice(
      prec.left(1, seq(
        field('left', $._expression),
        field('operator', choice('+', '-')),
        field('right', $._expression),
      )),
      prec.left(2, seq(
        field('left', $._expression),
        field('operator', choice('*', '/')),
        field('right', $._expression),
      )),
    ),

    number: $ => /\d+(\.\d+)?/,

    comment: $ => token(seq('#', /.*/)),
  },
});
```

### Mistakes caught before tree-sitter runs

Validation reports what `tree-sitter generate` would stop on, naming the rule involved:

```rust
use treesitter_lang::{Error, Grammar, Rule};

// `body` can be empty, and `block` refers to it: tree-sitter refuses that.
let grammar = Grammar::new("demo")
    .rule("source_file", Rule::repeat(Rule::symbol("block")))
    .rule("block", Rule::seq([Rule::string("{"), Rule::symbol("body"), Rule::string("}")]))
    .rule("body", Rule::repeat(Rule::symbol("statement")))
    .rule("statement", Rule::seq([Rule::pattern("[a-z]+"), Rule::string(";")]));

assert_eq!(grammar.to_js(), Err(Error::MatchesEmpty { rule: "body".into() }));
```

Undefined symbols, symbols inside tokens, empty strings, repetitions of nothing, cycles of rules that are each just the next, a hidden start rule, a word rule that is not a token, word, external, inline, and supertype declarations tree-sitter cannot honour, invalid names, and duplicate rules are caught the same way — including the few that make tree-sitter crash or hang instead of reporting an error. The full list is in [What validation checks](./docs/API.md#what-validation-checks).

### Corpus tests from your own parser

Name the parser's node kinds after the grammar's rules, and `sexp` renders its trees exactly as tree-sitter prints its own — hidden rules spliced, anonymous tokens left out, token rules as leaves:

```rust
use treesitter_lang::{Grammar, Rule};
use treesitter_lang::syntax_lang::{Element, Node, Span, Token};

let grammar = Grammar::new("calc")
    .rule("source_file", Rule::repeat(Rule::symbol("_expression")))
    .rule("_expression", Rule::choice([Rule::symbol("number"), Rule::symbol("sum")]))
    .rule("sum", Rule::prec_left(1, Rule::seq([
        Rule::symbol("_expression"), Rule::string("+"), Rule::symbol("_expression"),
    ])))
    .rule("number", Rule::pattern(r"\d+"));

// The tree a hand-written parser built for `1 + 2`.
let t = |kind, start, end| Element::Token(Token::new(kind, Span::new(start, end)));
let sum = Node::new("sum", vec![t("number", 0, 1), t(" ", 1, 2), t("+", 2, 3), t(" ", 3, 4), t("number", 4, 5)]);
let tree = Node::new("source_file", vec![Element::Node(sum)]);

let mut corpus = String::from("==========\nSum\n==========\n\n1 + 2\n\n---\n\n");
grammar.write_sexp(&tree, |kind| *kind, &mut corpus)?;
assert!(corpus.ends_with("(source_file\n  (sum\n    (number)\n    (number)))"));
// Write it to test/corpus/sum.txt and run `tree-sitter test`.
# Ok::<(), treesitter_lang::Error>(())
```

<hr>
<br>

## Examples

Three runnable examples ship in [`examples/`](./examples). Two share `mini`, a small expression language in [`examples/common/mini.rs`](./examples/common/mini.rs), set up the way a real `-lang` language would be: its tree-sitter grammar built with this crate from the same operator table its parser uses, and a hand-written lexer and Pratt parser that build `syntax-lang` trees, placing comments where tree-sitter places extras.

- **Calc** — the smallest complete use: a calculator grammar with precedence, associativity, and a token rule, printed as `grammar.js`; then a deliberately broken variant, caught by validation.
  ```bash
  cargo run --example calc
  ```
- **Generate** — writes `mini`'s `grammar.js` and `grammar.json` into a directory, through one reused buffer.
  ```bash
  cargo run --example generate                  # into target/tree-sitter-mini
  ```
- **Corpus** — parses six samples with the hand-written parser and writes them as a tree-sitter corpus, the expected trees rendered by `Grammar::sexp`.
  ```bash
  cargo run --example corpus
  cd target/tree-sitter-mini && tree-sitter generate && tree-sitter test
  ```

The last command is the point of the crate: the generated parser and the hand-written one parse every sample into the same tree. CI runs it on every change.

<hr>
<br>

## Performance

Emitting is a single pass over the rules into one output buffer: strings are escaped by copying the unescaped stretches between special characters in bulk, the work stack and scratch buffers are reused across rules, and the `write_` methods append to a caller-owned buffer so a build that emits many grammars or corpus entries does not reallocate. Validation sorts the rule names once, resolves every symbol by binary search, and gathers the references between rules in the same walk that checks them; the reachability, recursion, and token analyses then work on that compact graph. Nothing recurses, so the depth of a rule or tree is bounded only by memory.

Measured with the benchmarks in [`benches/`](./benches), x86_64, Rust stable, release profile, into a reused buffer:

| Benchmark | What it measures | Windows | Linux (WSL2) |
|---|---|---:|---:|
| `emit/js/small` | Validate and emit a 9-rule expression grammar as `grammar.js`. | ~2.6 µs | ~1.5 µs |
| `emit/json/small` | The same grammar as `grammar.json`. | ~3.8 µs | ~2.6 µs |
| `emit/js/large` | A 1,000-rule grammar (about 250 KB of `grammar.js`). | ~0.74 ms | ~0.61 ms |
| `emit/json/large` | The same as `grammar.json` (about 1.9 MB). | ~1.1 ms | ~0.93 ms |
| `emit/escapes` | 400 strings and patterns that all need escaping, both formats. | ~44 µs | ~33 µs |
| `sexp/10000` | Render a 10,000-element tree. | ~170 µs | ~157 µs |
| `sexp/100000` | Render a 100,000-element tree. | ~1.7 ms | ~1.6 ms |
| `build/large` | Construct the 1,000-rule grammar and drop it. | ~0.72 ms | ~0.51 ms |
| `patterns/many/100000` | Validate and emit 100,000 realistic patterns as `grammar.json`. | ~60 ms ¹ | — |
| `patterns/long/1MB` | One pattern of about a megabyte. | ~24 ms ¹ | — |

The rows above the line are 1.0.0's figures. 1.0.1 added pattern validation, which every `emit` figure now includes; the `patterns` rows measure it. ¹ Measured for 1.0.1 on a machine busy with other builds, so treat them as upper bounds: about 0.6 µs per pattern for validation and emission together, and about 24 ns per byte of a long pattern, each pattern parsed twice — as JavaScript and as tree-sitter would. Linux figures for 1.0.1 are not measured yet.

The `emit` figures include validation. Validating and emitting even the 1,000-rule grammar takes about as long as building it in the first place — under a millisecond — and is far below what `tree-sitter generate` spends on the output. Run them yourself:

```bash
cargo bench --bench bench
```

Criterion writes per-benchmark reports to `target/criterion/`. Numbers vary by CPU; use the trend across runs, not a single absolute.

<hr>
<br>

## Design notes

- **Tree-sitter's model, not a new one.** Every `Rule` constructor is one DSL function, and every `Grammar` setting is one `grammar.js` field. Nothing has to be translated in either direction, and tree-sitter's documentation applies as written.
- **Validation mirrors tree-sitter.** Each check corresponds to a refusal of tree-sitter's own, probed against the CLI, down to the subtle ones: tree-sitter drops unreachable rules before most checks; a rule may match the empty string only if no rule refers to it; it merges identical tokens, so a word or external rule whose token appears elsewhere is no longer a token; it treats a hidden rule whose token is a bare string as that anonymous string; and supertypes must be one node per alternative. The model was then cross-checked by generating thousands of random grammars and running each through both validation and `tree-sitter generate`; every disagreement it turned up was traced to its cause and fixed. A few checks are deliberately stricter, rejecting an empty `choice`, an `inline` entry naming no rule, and empty patterns and empty strings inside tokens, which tree-sitter accepts in places or merely warns about. What only parse tables reveal — conflicts, and extras whose end tree-sitter cannot tell — is left to `tree-sitter generate`; `Grammar::conflict` declares the conflicts.
- **Byte-for-byte with tree-sitter.** `grammar.json` follows tree-sitter's own field order and layout, and records patterns as they appear between the slashes in `grammar.js` — so the two generate the same parser, and with tree-sitter 0.25 or later the file is identical to the one tree-sitter derives from the emitted `grammar.js`. Older releases back to 0.20 load it too; see the [compatibility table](./docs/API.md#from-rust-to-tree-sitter).
- **Corpus tests as the bridge.** The crate does not try to infer a grammar from a parser, which cannot be done reliably. It makes the comparison cheap instead: `sexp` renders the parser's trees in tree-sitter's notation, with tree-sitter's rules for hidden, inlined, token, and external rules, and `tree-sitter test` does the comparing.
- **Patterns stay patterns.** Every pattern is checked against both parsers that read it — the JavaScript runtime that loads `grammar.js` (ECMAScript Annex B, no flags) and tree-sitter's own regular expression parser (regex-syntax 0.8, after tree-sitter's rewrite of `\d` and friends) — and refused with the byte offset and reason if either would refuse it. However a pattern is written, its literal ends at its own closing slash: an unclosed `[` is written `\[`, an empty pattern as `new RegExp('')`. A property test holds hostile patterns next to strings that would run as code if a literal swallowed them, and a CI job evaluates fuzzed `grammar.js` files in a sandboxed Node.js and compares them with `grammar.json`. The accepted forms are listed in [`docs/API.md`](./docs/API.md#accepted-patterns); before 1.0.1 an unclosed class could carry the text after it into code (ISSUES H06).
- **Errors, not panics.** Invalid grammars and unknown tree nodes are `Error`s; failed emission appends nothing to the caller's buffer. Dropping, validating, emitting, and rendering are iterative, so deeply nested rules and trees cannot overflow the stack inside the crate.

<hr>
<br>

## Testing

The suite runs on Windows, Linux (WSL2 Ubuntu), and macOS through the CI matrix, on stable and the 1.85 MSRV:

```bash
cargo test                       # unit + integration + property + doctests
cargo clippy --all-targets --all-features -- -D warnings
cargo bench --bench bench
```

The property tests in [`tests/proptests.rs`](./tests/proptests.rs) generate random rule trees — strings full of quotes, backslashes, control characters, U+2028, and arbitrary Unicode — emit them both ways, and read them back with independent `grammar.js` and `grammar.json` parsers written in the test; the trees must come back unchanged. Further properties check the empty-string rule against a direct recursive definition, and `sexp` against a reference renderer and its indentation rule. A conformance job in CI installs the tree-sitter CLI and holds a grammar using every rule type and setting, the `mini` grammar, and a minimal grammar to it: generation must succeed, `grammar.json` must match tree-sitter's byte for byte, both formats must generate the same parser, and the `mini` corpus must pass `tree-sitter test`. Every `rust` example in this README and in [`docs/API.md`](./docs/API.md) is compiled and run as a doctest. A pattern property holds hostile patterns next to strings that would run as code if a literal swallowed them: each must be refused, or read back — by a scanner that follows JavaScript's rules for character classes — as exactly one literal equal to its `grammar.json` value. And [`tests/node.rs`](./tests/node.rs), ignored by default and run by a CI job (`cargo test --test node -- --ignored`), uses Node.js as an oracle: fuzzed `grammar.js` files must pass `node --check` and evaluate, in a sandbox with no `require` or `process`, to exactly their `grammar.json`.

<hr>
<br>

## Cross-platform support

- Linux (x86_64, aarch64)
- macOS (x86_64, Apple Silicon)
- Windows (x86_64)

The crate uses no operating-system facilities and no platform-specific code; its output is identical on every platform, byte for byte.

<hr>
<br>

## Contributing

See [`REPS.md`](./REPS.md) for the engineering standards every change is held to, and [`dev/ROADMAP.md`](./dev/ROADMAP.md) for the plan to 1.0. Before a PR: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` must be clean. Changes to emission should also pass `dev/conformance.sh` against the tree-sitter CLI, as CI does.

<br>

<div id="license">
    <h2>License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> &mdash; <a href="./LICENSE-APACHE">LICENSE-APACHE</a></li>
        <li><b>MIT License</b> &mdash; <a href="./LICENSE-MIT">LICENSE-MIT</a></li>
    </ul>
    <p>at your option.</p>
</div>

<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>James Gober <me@jamesgober.com>.</strong></sup>
</div>
