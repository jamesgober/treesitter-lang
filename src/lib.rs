//! # treesitter_lang
//!
//! Generate [tree-sitter](https://tree-sitter.github.io/) grammars from Rust:
//! describe a language's syntax with [`Grammar`] and [`Rule`], and emit the
//! `grammar.js` (or `grammar.json`) that `tree-sitter generate` turns into a
//! parser for syntax highlighting, code navigation, and editor tooling.
//!
//! A language built with the `-lang` family already has a parser: a
//! hand-written one that builds a lossless [`syntax_lang::Node`] tree. Editors
//! want a tree-sitter grammar for the same language. This crate keeps the two
//! in step. The grammar is written in Rust, next to the parser, where it can be
//! generated from data, shared between dialects, and checked at build time.
//! And [`Grammar::sexp`] renders the trees the hand-written parser builds the
//! way tree-sitter prints its own, so every sample the parser handles becomes a
//! tree-sitter corpus test.
//!
//! ## The pieces
//!
//! - [`Rule`] — tree-sitter's rule language: [`symbol`](Rule::symbol),
//!   [`string`](Rule::string), [`pattern`](Rule::pattern),
//!   [`seq`](Rule::seq), [`choice`](Rule::choice),
//!   [`optional`](Rule::optional), [`repeat`](Rule::repeat),
//!   [`field`](Rule::field), [`alias`](Rule::alias), [`token`](Rule::token),
//!   [`prec`](Rule::prec), and the rest, one constructor per DSL function.
//! - [`Grammar`] — the named rules, the first being the start rule, plus
//!   extras, externals, supertypes, inlined rules, conflicts, and the word
//!   token. [`Grammar::to_js`] and [`Grammar::to_json`] validate it and emit
//!   it; [`Grammar::sexp`] renders a tree for a corpus test.
//! - [`Error`] — what validation found: an undefined symbol, a rule other
//!   rules refer to that can match nothing, a symbol inside a token, and the
//!   other mistakes `tree-sitter generate` would otherwise stop on.
//!
//! ## Example
//!
//! ```
//! use treesitter_lang::{Grammar, Rule};
//!
//! let e = || Rule::symbol("_expression");
//! let calc = Grammar::new("calc")
//!     .extra(Rule::pattern(r"\s"))
//!     .extra(Rule::symbol("comment"))
//!     .rule("source_file", Rule::repeat(e()))
//!     .rule("_expression", Rule::choice([
//!         Rule::symbol("number"),
//!         Rule::symbol("binary"),
//!         Rule::seq([Rule::string("("), e(), Rule::string(")")]),
//!     ]))
//!     .rule("binary", Rule::choice([
//!         Rule::prec_left(1, Rule::seq([
//!             Rule::field("left", e()),
//!             Rule::field("operator", Rule::choice([Rule::string("+"), Rule::string("-")])),
//!             Rule::field("right", e()),
//!         ])),
//!         Rule::prec_left(2, Rule::seq([
//!             Rule::field("left", e()),
//!             Rule::field("operator", Rule::choice([Rule::string("*"), Rule::string("/")])),
//!             Rule::field("right", e()),
//!         ])),
//!     ]))
//!     .rule("number", Rule::pattern(r"\d+(\.\d+)?"))
//!     .rule("comment", Rule::token(Rule::seq([Rule::string("#"), Rule::pattern(".*")])));
//!
//! let js = calc.to_js()?;
//! assert!(js.contains("module.exports = grammar({"));
//! assert!(js.contains("      field('operator', choice('+', '-')),"));
//! // std::fs::write("tree-sitter-calc/grammar.js", js)?;
//! # Ok::<(), treesitter_lang::Error>(())
//! ```
//!
//! ## What is checked
//!
//! Emitting validates the grammar and refuses, with an [`Error`], what
//! tree-sitter would refuse — or crash or hang on: names that are not
//! identifiers, a grammar with no rules or a hidden start rule, duplicate
//! rules, symbols that name nothing, empty strings and patterns, symbols
//! inside a token, rules (and repetitions) that can match nothing where
//! tree-sitter needs a token, cycles of rules that can each be just the next,
//! and word, external, inline, and supertype declarations tree-sitter cannot
//! honour. Each check was probed against the tree-sitter CLI, and the whole
//! was cross-checked by running thousands of random grammars through both.
//! What validation does not do is build parse tables: conflicts are found by
//! `tree-sitter generate`, and declared with [`Grammar::conflict`].
//!
//! ## Features
//!
//! - `std` (default) — the standard library. Without it the crate is `no_std`
//!   and needs only `alloc`. Forwards to `syntax-lang/std`.
//!
//! ## Re-exports
//!
//! [`syntax_lang`] is re-exported whole: [`Grammar::sexp`] takes its
//! [`Node`](syntax_lang::Node), and the re-export lets a caller build trees
//! with the same version this crate was compiled against.
//!
//! ## Stability
//!
//! The public surface is frozen and stable as of `1.0.0`: it follows Semantic
//! Versioning, with no breaking changes before `2.0`. The full surface, the
//! validation and output contracts that are part of it, and the SemVer
//! promise are catalogued in
//! [`docs/API.md`](https://github.com/jamesgober/treesitter-lang/blob/main/docs/API.md#stability).

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![deny(
    warnings,
    missing_docs,
    unsafe_op_in_unsafe_fn,
    unused_must_use,
    unused_results,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::dbg_macro,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::undocumented_unsafe_blocks
)]

extern crate alloc;

mod check;
mod error;
mod grammar;
mod js;
mod json;
mod rule;
mod sexp;
mod text;

pub use error::Error;
pub use grammar::Grammar;
pub use rule::Rule;

// The tree type `Grammar::sexp` reads, and the builder that makes it, at the
// exact version this crate uses.
pub use syntax_lang;

/// Compiles and runs the `rust` code blocks in `README.md` and `docs/API.md` as
/// part of `cargo test`, so the published examples cannot drift from the API.
///
/// Present only while collecting doctests (`#[cfg(doctest)]`); it is not part of
/// the public surface and does not appear in the built library or its docs.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../docs/API.md")]
pub struct MarkdownDocTests;
