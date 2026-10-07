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

[Unreleased]: https://github.com/jamesgober/treesitter-lang/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/jamesgober/treesitter-lang/compare/v0.2.0...v1.0.0
[0.2.0]: https://github.com/jamesgober/treesitter-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/treesitter-lang/releases/tag/v0.1.0
