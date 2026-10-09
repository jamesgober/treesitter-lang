<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br><b>CHANGELOG</b>
</h1>
<p>
  All notable changes to <code>treesitter-lang</code> will be documented in this file. The format is based on <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a>,
  and this project adheres to <a href="https://semver.org/spec/v2.0.0.html/">Semantic Versioning</a>.
</p>

---

## [Unreleased]

---

## [1.0.1] - 2026-10-08

A security patch. Patterns are now validated, and `grammar.js` writes every
pattern so that its regular expression literal ends where the pattern ends.
No public API is added or removed.

### Security

- **Code injection through `grammar.js` (ISSUES H06).** `grammar.js` writes
  a pattern as a `/.../` literal, escaping each `/`, but a JavaScript lexer
  reads a `[` as opening a character class inside which `/` does not end the
  literal. A pattern with an unclosed `[` therefore kept the literal open past
  its closing slash, and the text after it in `grammar.js` — the next string
  of a one-line `seq` or `choice`, under the grammar author's control — was
  read as code, which ran when `tree-sitter generate` evaluated the file.
  `seq([pattern("["), string(r#"]/,require("fs").rmSync("x")),//"#)])`
  emitted `seq(/[/, ']/,require("fs").rmSync("x")),//')`. Affected: 0.2.0
  through 1.0.0, when pattern or string text comes from somewhere untrusted.
  `to_json` was not affected. Fixed twice over: validation refuses every
  pattern JavaScript would not parse (below), and emission no longer depends
  on it — from the first `[` whose class would never close, every `[` is
  written `\[`, and an empty pattern is written `new RegExp('')` (`//` would
  open a comment), as a pattern starting with `*` already was. Valid
  patterns are written exactly as before.

### Fixed

- Patterns are validated, against both parsers that read them, and refused
  with `Error::EmptyString` — the variant for a pattern that cannot be
  written as a regular expression literal — whose `rule` holds the rule's
  name, `: `, the reason, the byte offset in the pattern, and its start:
  `` "number: unterminated character class at byte 0 of `[0-9`" ``. `Display`
  reads ``"`number` contains an invalid pattern: …"``.
  - In every rule and extra: the pattern must be a regular expression
    literal JavaScript accepts — ECMAScript Annex B syntax with no flags:
    balanced groups and classes, quantifiers with something to repeat,
    `{n,m}` and class ranges in order, valid group syntax, named groups and
    `\k` references, and ES2025 modifier groups.
  - In rules reachable from the start rule, and in extras: the pattern must
    also be one tree-sitter's parser accepts — regex-syntax 0.8 after
    tree-sitter's textual rewrite of `\w`, `\s`, `\d`, `\W`, `\S`, `\D`: no
    assertions (`^`, `$`, `\b`, …) outside a repetition of exactly zero, no
    look-around, backreferences, octal escapes, or escapes regex-syntax does
    not define, classes closed as regex-syntax reads them, counted
    repetitions with numbers, and at most 250 levels of nesting.
  These patterns never produced a working grammar: either `grammar.js`
  failed to load or `tree-sitter generate` failed. A grammar emitted only as
  `grammar.json` whose pattern JavaScript refuses but tree-sitter's parser
  accepts (`a**`) is refused now too, since its `grammar.js` could not load.
  Not checked, as before: `\p{…}` property names, the identifier classes of
  non-ASCII group-name characters, and repeated group names in rules
  tree-sitter drops. The full list is in `docs/API.md#accepted-patterns`.
- A pattern starting with `*` was emitted as `new RegExp('*a')` for
  tree-sitter to refuse; it is now refused by validation.

### Added

Tests and CI only; no public API.

- The H06 regression test, an exhaustive test that every pattern of up to
  five characters from a hostile alphabet stays inside its literal when
  printed without validation, and a property test that a hostile pattern is
  either refused or read back — by a scanner that follows JavaScript's rules
  for classes — as exactly one literal equal to the `grammar.json` value,
  with the hostile string after it intact.
- The test JavaScript reader in `tests/proptests.rs` reads character
  classes as JavaScript does; it used to end a literal at the first `/`.
- `tests/node.rs`, ignored by default: fuzzed grammars whose `grammar.js`
  must pass `node --check` and evaluate, in a sandbox with no `require` or
  `process`, to exactly their `grammar.json`. A CI job runs it on Ubuntu
  with Node.js 24.
- `patterns/*` benchmarks: 100,000 patterns, and one pattern of about a
  megabyte.

---

## [1.0.0] - 2026-10-07

The API freeze. The 0.2.0 surface is now the stable `1.x` contract; the code
is unchanged.

### Added

- `docs/API.md#stability`: the frozen surface, the validation and output
  contracts, the `sexp` rendering rules, `syntax-lang` 1 as a public
  dependency, and MSRV 1.85 are recorded as the SemVer promise, together
  with what is not promised.
- A Stability section in the crate documentation.

### Changed

- Version 1.0.0. `README.md` and `docs/API.md` mark the API stable.

---

## [0.2.0] - 2026-10-07

The core: tree-sitter grammars built in Rust, validated against tree-sitter's
own rules, emitted as `grammar.js` or `grammar.json`, and held to a language's
hand-written parser through corpus tests.

### Added

- `Rule`: tree-sitter's rule language, one constructor per DSL function —
  `symbol`, `string`, `pattern`, `blank`, `seq`, `choice`, `optional`,
  `repeat`, `repeat1`, `field`, `alias`, `token`, `immediate`, `prec`,
  `prec_left`, `prec_right`, `prec_dynamic`. Text arguments take
  `impl Into<Cow<'static, str>>`, so literals are never copied. An empty
  `seq` is `blank`. Dropping a rule is iterative at any depth.
- `Grammar`: a by-value builder — `new`, `rule`, `extra`, `external`,
  `supertype`, `inline`, `conflict`, `word` — that emits `grammar.js`
  (`to_js`, `write_js`) and `grammar.json` (`to_json`, `write_json`). The
  JavaScript is laid out as grammars are written by hand. Both formats
  generate the same parser, and the JSON is byte-identical to the
  `src/grammar.json` tree-sitter 0.25 and later write from the emitted
  `grammar.js`; tree-sitter 0.20 through 0.27 load it.
- Validation before emission, mirroring tree-sitter's refusals — and the
  grammars it crashes or hangs on — as probed against its CLI and
  cross-checked by differential fuzzing: identifier names, at least one
  rule, a visible start rule, no duplicate rules, defined symbols
  everywhere, no empty patterns or choices, no symbols inside tokens; for
  rules reachable from the start rule and the extras, no empty strings, no
  referenced rule or repetition that matches the empty string, and no cycle
  of rules that are each just the next; and word, external, inline, and
  supertype declarations tree-sitter can honour, following its rules for
  which rules are tokens.
- `Grammar::sexp` and `Grammar::write_sexp`: render a `syntax_lang::Node` as
  the S-expression tree-sitter prints — hidden, inlined, and supertype rules
  spliced, single-token rules as leaves, alias names recognized — for corpus
  tests that hold the generated parser to a hand-written one.
- `Error`, `#[non_exhaustive]`, with fifteen documented variants.
- `syntax_lang` re-exported.
- Examples `calc`, `generate`, and `corpus`, sharing a small language with a
  hand-written lexer and Pratt parser.
- Unit, integration, property, and conformance tests; Criterion benchmarks
  for emission, rendering, and construction; `README.md` and `docs/API.md`
  examples run as doctests.
- A `tree-sitter` CI job and `dev/conformance.sh`, which run the tree-sitter
  CLI on emitted grammars and the `mini` corpus.

### Changed

- Wired `syntax-lang` 1 as the only dependency.
- `Cargo.toml` description, keywords, and categories describe the crate.
- Removed the scaffold's `serde` feature and `loom` dev-dependency, which had
  no code behind them.
- CI also runs clippy and tests without default features and builds the
  examples.

### Fixed

- `Cargo.toml` listed `keywords` and `categories` unquoted, so the manifest
  did not parse.
- `clippy.toml` declared MSRV 1.87 against the crate's 1.85.
- `deny.toml` named another project in its header.
- `dev/ROADMAP.md` and `docs/API.md` carried byte-order marks.
- The README linked a `dev/DIRECTIVES.md` that does not exist.

---

## [0.1.0] - 2026-06-18

Initial scaffold and repository bootstrap. No domain logic yet &mdash; this release establishes the structure, tooling, and quality gates the implementation will be built on.

### Added

- `Cargo.toml` with crate metadata, Rust 2024 edition, MSRV 1.85.
- Dual `Apache-2.0 OR MIT` license files.
- `README.md`, `CHANGELOG.md`, and a documentation skeleton.
- `REPS.md` compliance baseline.
- `.github/workflows/ci.yml` CI matrix; `deny.toml`, `clippy.toml`, `rustfmt.toml`.
- `dev/DIRECTIVES.md` and `dev/ROADMAP.md` (committed engineering standards + plan).

[Unreleased]: https://github.com/jamesgober/treesitter-lang/compare/v1.0.1...HEAD
[1.0.1]: https://github.com/jamesgober/treesitter-lang/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/jamesgober/treesitter-lang/compare/v0.2.0...v1.0.0
[0.2.0]: https://github.com/jamesgober/treesitter-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/treesitter-lang/releases/tag/v0.1.0
