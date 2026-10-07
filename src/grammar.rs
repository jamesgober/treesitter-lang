//! The grammar: named rules plus tree-sitter's grammar-level settings.

use alloc::{borrow::Cow, string::String, vec::Vec};

use syntax_lang::Node;

use crate::{Error, Rule, check, js, json, rule::Text, sexp};

/// A tree-sitter grammar, built in Rust and emitted as `grammar.js` or
/// `grammar.json`.
///
/// A grammar is a name, an ordered list of rules, and the grammar-level
/// settings tree-sitter supports: extras, external tokens, supertypes,
/// inlined rules, conflicts, and the word token. The first rule is the start
/// rule — the root of every tree.
///
/// Build one by chaining: every method takes the grammar by value and returns
/// it, and nothing is checked until it is emitted. [`to_js`](Grammar::to_js)
/// and [`to_json`](Grammar::to_json) validate the whole grammar first and
/// report the first problem as an [`Error`]; a grammar that passes produces
/// output `tree-sitter generate` can load. [`sexp`](Grammar::sexp) renders a
/// [`syntax_lang::Node`] the way tree-sitter prints its own trees, for corpus
/// tests that hold the generated parser to the hand-written one.
///
/// # Examples
///
/// ```
/// use treesitter_lang::{Grammar, Rule};
///
/// let e = || Rule::symbol("_expression");
/// let grammar = Grammar::new("calc")
///     .rule("source_file", Rule::repeat(e()))
///     .rule("_expression", Rule::choice([
///         Rule::symbol("number"),
///         Rule::symbol("sum"),
///         Rule::symbol("product"),
///     ]))
///     .rule("sum", Rule::prec_left(1, Rule::seq([
///         Rule::field("left", e()),
///         Rule::string("+"),
///         Rule::field("right", e()),
///     ])))
///     .rule("product", Rule::prec_left(2, Rule::seq([
///         Rule::field("left", e()),
///         Rule::string("*"),
///         Rule::field("right", e()),
///     ])))
///     .rule("number", Rule::pattern(r"\d+"));
///
/// let js = grammar.to_js()?;
/// assert!(js.starts_with("/// <reference types=\"tree-sitter-cli/dsl\" />"));
/// assert!(js.contains("  name: 'calc',"));
/// assert!(js.contains("    number: $ => /\\d+/,"));
///
/// let json = grammar.to_json()?;
/// assert!(json.contains("\"type\": \"PREC_LEFT\""));
/// # Ok::<(), treesitter_lang::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct Grammar {
    pub(crate) name: Text,
    pub(crate) rules: Vec<(Text, Rule)>,
    /// `None` until [`Grammar::extra`] is first called: tree-sitter's default
    /// extras (whitespace) apply.
    pub(crate) extras: Option<Vec<Rule>>,
    pub(crate) externals: Vec<Text>,
    pub(crate) supertypes: Vec<Text>,
    pub(crate) inline: Vec<Text>,
    pub(crate) conflicts: Vec<Vec<Text>>,
    pub(crate) word: Option<Text>,
}

impl Grammar {
    /// Starts an empty grammar called `name`.
    ///
    /// The name becomes the language's name in tree-sitter: the generated
    /// parser exports `tree_sitter_<name>()`, and the package is conventionally
    /// called `tree-sitter-<name>`. It must be an identifier
    /// ([`Error::InvalidName`]), checked when the grammar is emitted.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `name` | The language name, conventionally lowercase: a `&'static str` (not copied) or a `String`. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("toml").rule("document", Rule::blank()).to_js()?;
    /// assert!(js.contains("name: 'toml'"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    ///
    /// A name built at run time:
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let dialect = "v2";
    /// let grammar = Grammar::new(format!("config_{dialect}")).rule("document", Rule::blank());
    /// assert!(grammar.to_js()?.contains("name: 'config_v2'"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn new(name: impl Into<Cow<'static, str>>) -> Grammar {
        Grammar {
            name: name.into(),
            rules: Vec::new(),
            extras: None,
            externals: Vec::new(),
            supertypes: Vec::new(),
            inline: Vec::new(),
            conflicts: Vec::new(),
            word: None,
        }
    }

    /// Adds a rule called `name` matching `rule`.
    ///
    /// Rules are emitted in the order they are added, and the **first rule is
    /// the start rule**: every tree has it at the root, and it must be visible
    /// ([`Error::HiddenStart`]). A name starting with `_` makes the rule
    /// hidden — it matches like any other, but its node is left out of the
    /// tree and its children take its place. Names must be identifiers
    /// ([`Error::InvalidName`]) and unique ([`Error::DuplicateRule`]).
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `name` | The rule's name and the name of its nodes, conventionally `snake_case`. |
    /// | `rule` | What the rule matches. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("_definition")))
    ///     .rule("_definition", Rule::symbol("constant"))
    ///     .rule("constant", Rule::seq([Rule::string("const"), Rule::pattern("[A-Z]+")]))
    ///     .to_js()?;
    ///
    /// // Emitted in order, the start rule first.
    /// let start = js.find("source_file:").unwrap();
    /// let constant = js.find("constant:").unwrap();
    /// assert!(start < constant);
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    ///
    /// Rules generated from data:
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let keywords = ["if", "else", "while"];
    /// let mut grammar = Grammar::new("demo").rule(
    ///     "source_file",
    ///     Rule::repeat(Rule::choice(keywords.map(|k| Rule::symbol(format!("kw_{k}"))))),
    /// );
    /// for k in keywords {
    ///     grammar = grammar.rule(format!("kw_{k}"), Rule::string(k));
    /// }
    /// assert!(grammar.to_js()?.contains("kw_while: $ => 'while',"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn rule(mut self, name: impl Into<Cow<'static, str>>, rule: Rule) -> Grammar {
        self.rules.push((name.into(), rule));
        self
    }

    /// Adds an extra: something that may appear anywhere between tokens, such
    /// as whitespace or a comment.
    ///
    /// Until this is first called, tree-sitter's default applies: whitespace
    /// (`/\s/`) is the only extra. **The first call replaces that default**,
    /// so a grammar that adds a comment extra must add whitespace too. Extras
    /// are usually a pattern, or a [`Rule::symbol`] naming a rule — a named
    /// extra such as `comment` appears in the tree wherever it occurs.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rule` | The extra. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .extra(Rule::pattern(r"\s"))
    ///     .extra(Rule::symbol("comment"))
    ///     .rule("source_file", Rule::repeat(Rule::pattern("[a-z]+")))
    ///     .rule("comment", Rule::token(Rule::seq([Rule::string("#"), Rule::pattern(".*")])))
    ///     .to_js()?;
    /// assert!(js.contains("  extras: $ => [\n    /\\s/,\n    $.comment,\n  ],"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    ///
    /// Without any `extra`, the default whitespace extra is implied —
    /// `grammar.js` leaves the field out, and `grammar.json` spells it out:
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let grammar = Grammar::new("demo").rule("source_file", Rule::string("x"));
    /// assert!(!grammar.to_js()?.contains("extras"));
    /// assert!(grammar.to_json()?.contains("\"value\": \"\\\\s\""));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn extra(mut self, rule: Rule) -> Grammar {
        self.extras.get_or_insert_with(Vec::new).push(rule);
        self
    }

    /// Declares an external token: one lexed by a hand-written scanner in
    /// `src/scanner.c` rather than by the generated lexer.
    ///
    /// External tokens handle what regular expressions cannot — indentation,
    /// heredocs, nested comments. Rules refer to them with [`Rule::symbol`].
    /// Tree-sitter calls the scanner with the externals in declaration order,
    /// so the order must match the scanner's token enum. The name must be an
    /// identifier ([`Error::InvalidName`]). It may also be the name of a
    /// rule that is a token, which then serves as the fallback when the
    /// scanner declines ([`Error::InvalidExternal`] if the rule is not a
    /// token).
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `name` | The token's name. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("indented")
    ///     .external("indent")
    ///     .external("dedent")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("block")))
    ///     .rule("block", Rule::seq([
    ///         Rule::pattern("[a-z]+"),
    ///         Rule::string(":"),
    ///         Rule::symbol("indent"),
    ///         Rule::repeat1(Rule::pattern("[a-z]+")),
    ///         Rule::symbol("dedent"),
    ///     ]))
    ///     .to_js()?;
    /// assert!(js.contains("  externals: $ => [\n    $.indent,\n    $.dedent,\n  ],"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn external(mut self, name: impl Into<Cow<'static, str>>) -> Grammar {
        self.externals.push(name.into());
        self
    }

    /// Marks a hidden rule as a supertype.
    ///
    /// A supertype is a hidden rule that is a choice between other rules —
    /// `_expression`, `_statement`. It still does not appear in trees, but
    /// tree-sitter records it in `node-types.json` and queries can match on it
    /// (`(_expression)` matches any expression). The name must be a rule of
    /// the grammar ([`Error::UndefinedSymbol`]) that is not a token, is never
    /// itself, and produces a single node in each alternative
    /// ([`Error::InvalidSupertype`]). Tree-sitter treats a supertype as
    /// hidden even if its name does not start with `_`.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `name` | A hidden rule's name. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .supertype("_literal")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("_literal")))
    ///     .rule("_literal", Rule::choice([Rule::symbol("number"), Rule::symbol("string")]))
    ///     .rule("number", Rule::pattern(r"\d+"))
    ///     .rule("string", Rule::pattern("\"[^\"]*\""))
    ///     .to_js()?;
    /// assert!(js.contains("  supertypes: $ => [\n    $._literal,\n  ],"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn supertype(mut self, name: impl Into<Cow<'static, str>>) -> Grammar {
        self.supertypes.push(name.into());
        self
    }

    /// Asks tree-sitter to inline a rule: to substitute its body wherever it
    /// is used instead of giving it parse states of its own.
    ///
    /// Inlining suits small hidden helper rules used in many places, and can
    /// resolve conflicts that arise only because the helper is a separate
    /// rule. An inlined rule never appears in the tree. The name must be a
    /// rule of the grammar ([`Error::UndefinedSymbol`]) — tree-sitter only
    /// warns, but an inline entry that names nothing is always a typo — and
    /// not the start rule, a token, or a rule that refers to itself through
    /// inlined rules ([`Error::InvalidInline`]). Repeated entries are written
    /// once, as tree-sitter keeps them.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `name` | A rule's name, usually a hidden one. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .inline("_name")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("_name")))
    ///     .rule("_name", Rule::choice([Rule::symbol("identifier"), Rule::symbol("keyword")]))
    ///     .rule("identifier", Rule::pattern("[a-z]+"))
    ///     .rule("keyword", Rule::string("self"))
    ///     .to_js()?;
    /// assert!(js.contains("  inline: $ => [\n    $._name,\n  ],"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn inline(mut self, name: impl Into<Cow<'static, str>>) -> Grammar {
        self.inline.push(name.into());
        self
    }

    /// Declares an expected conflict between rules.
    ///
    /// When tree-sitter's parser generator finds an ambiguity that precedence
    /// does not settle, it stops and names the rules involved. Declaring the
    /// conflict tells it the ambiguity is intended: the generated parser then
    /// explores the alternatives at run time (GLR parsing) and keeps the one
    /// that succeeds. Every name must be a rule or external of the grammar
    /// ([`Error::UndefinedSymbol`]).
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `names` | The conflicting rules, as tree-sitter listed them: an array or any iterator of names. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .conflict(["type", "expression"])
    ///     .rule("source_file", Rule::repeat(Rule::choice([
    ///         Rule::symbol("type"),
    ///         Rule::symbol("expression"),
    ///     ])))
    ///     .rule("type", Rule::symbol("identifier"))
    ///     .rule("expression", Rule::symbol("identifier"))
    ///     .rule("identifier", Rule::pattern("[a-z]+"))
    ///     .to_js()?;
    /// assert!(js.contains("  conflicts: $ => [\n    [$.type, $.expression],\n  ],"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    ///
    /// Names known only at run time:
    ///
    /// ```
    /// use treesitter_lang::Grammar;
    ///
    /// let pair: Vec<String> = vec!["pattern".into(), "expression".into()];
    /// let grammar = Grammar::new("demo").conflict(pair);
    /// # let _ = grammar;
    /// ```
    #[must_use]
    pub fn conflict<N>(mut self, names: impl IntoIterator<Item = N>) -> Grammar
    where
        N: Into<Cow<'static, str>>,
    {
        self.conflicts
            .push(names.into_iter().map(Into::into).collect());
        self
    }

    /// Names the word token: the identifier-like token keywords are carved
    /// out of.
    ///
    /// With a word token, tree-sitter lexes keywords by first matching the
    /// word token and then checking the text against the keyword strings.
    /// That makes the lexer smaller and fixes a class of bugs where a keyword
    /// is recognized inside a longer identifier (`if` in `iffy`). The rule
    /// must be a token: its body a string, a pattern, or a
    /// [`Rule::token`] or [`Rule::immediate`], with nothing wrapped around
    /// it ([`Error::WordNotToken`]) — or an external. Calling `word` again
    /// replaces the earlier choice.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `name` | The identifier rule's name. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .word("identifier")
    ///     .rule("source_file", Rule::repeat(Rule::choice([
    ///         Rule::string("if"),
    ///         Rule::symbol("identifier"),
    ///     ])))
    ///     .rule("identifier", Rule::pattern("[a-z_]+"))
    ///     .to_js()?;
    /// assert!(js.contains("  word: $ => $.identifier,"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    ///
    /// A word rule that is not a token is refused:
    ///
    /// ```
    /// use treesitter_lang::{Error, Grammar, Rule};
    ///
    /// let grammar = Grammar::new("demo")
    ///     .word("identifier")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("identifier")))
    ///     .rule("identifier", Rule::prec(1, Rule::pattern("[a-z]+")));
    /// assert_eq!(
    ///     grammar.to_js(),
    ///     Err(Error::WordNotToken { name: "identifier".into() }),
    /// );
    /// ```
    #[must_use]
    pub fn word(mut self, name: impl Into<Cow<'static, str>>) -> Grammar {
        self.word = Some(name.into());
        self
    }

    /// Validates the grammar and emits it as `grammar.js`, the source file of
    /// a tree-sitter grammar repository.
    ///
    /// The output is formatted the way tree-sitter grammars are written by
    /// hand — two-space indentation, short sequences on one line, trailing
    /// commas — and begins with the `tree-sitter-cli/dsl` type reference, so it
    /// reads naturally and diffs cleanly when committed. Save it as
    /// `grammar.js` and run `tree-sitter generate`.
    ///
    /// # Errors
    ///
    /// The first problem validation finds; see [`Error`].
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("pairs")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("pair")))
    ///     .rule("pair", Rule::seq([
    ///         Rule::field("key", Rule::symbol("identifier")),
    ///         Rule::string("="),
    ///         Rule::field("value", Rule::choice([
    ///             Rule::symbol("identifier"),
    ///             Rule::symbol("number"),
    ///         ])),
    ///     ]))
    ///     .rule("identifier", Rule::pattern("[a-z]+"))
    ///     .rule("number", Rule::pattern(r"\d+"))
    ///     .to_js()?;
    ///
    /// assert_eq!(js, r#"/// <reference types="tree-sitter-cli/dsl" />
    /// // @ts-check
    ///
    /// module.exports = grammar({
    ///   name: 'pairs',
    ///
    ///   rules: {
    ///     source_file: $ => repeat($.pair),
    ///
    ///     pair: $ => seq(
    ///       field('key', $.identifier),
    ///       '=',
    ///       field('value', choice($.identifier, $.number)),
    ///     ),
    ///
    ///     identifier: $ => /[a-z]+/,
    ///
    ///     number: $ => /\d+/,
    ///   },
    /// });
    /// "#);
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    pub fn to_js(&self) -> Result<String, Error> {
        let mut out = String::new();
        self.write_js(&mut out)?;
        Ok(out)
    }

    /// Validates the grammar and appends it to `out` as `grammar.js`.
    ///
    /// The same output as [`to_js`](Grammar::to_js), written into a buffer the
    /// caller owns — reuse one buffer to emit many grammars without
    /// reallocating. On error nothing is appended.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `out` | The buffer to append to. Existing contents are kept. |
    ///
    /// # Errors
    ///
    /// The first problem validation finds; see [`Error`]. `out` is unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let dialects = ["alpha", "beta"];
    /// let mut buffer = String::new();
    /// for name in dialects {
    ///     buffer.clear();
    ///     Grammar::new(name)
    ///         .rule("source_file", Rule::repeat(Rule::pattern("[a-z]+")))
    ///         .write_js(&mut buffer)?;
    ///     assert!(buffer.contains(&format!("name: '{name}'")));
    ///     // std::fs::write(format!("tree-sitter-{name}/grammar.js"), &buffer)?;
    /// }
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    pub fn write_js(&self, out: &mut String) -> Result<(), Error> {
        check::validate(self)?;
        js::write(self, out);
        Ok(())
    }

    /// Validates the grammar and emits it as `grammar.json`, tree-sitter's
    /// machine-readable grammar format.
    ///
    /// This is the file `tree-sitter generate` itself writes to
    /// `src/grammar.json`, and it accepts it as input in place of
    /// `grammar.js` — useful in a build that has no JavaScript runtime. Both
    /// inputs produce the same parser, and with tree-sitter 0.25 or later the
    /// output is byte-identical to the file tree-sitter writes when it
    /// generates from this grammar's [`to_js`](Grammar::to_js) output (0.24
    /// writes the same file without the final, empty `reserved` field; it and
    /// older versions accept the field). Patterns are written as they
    /// appear between the slashes in `grammar.js`, which is what tree-sitter
    /// records. Without any [`extra`](Grammar::extra), the default whitespace
    /// extra is written out, since `grammar.json` has no implicit default.
    ///
    /// # Errors
    ///
    /// The first problem validation finds; see [`Error`].
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let json = Grammar::new("tiny")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("word")))
    ///     .rule("word", Rule::pattern("[a-z]+"))
    ///     .to_json()?;
    ///
    /// assert_eq!(json, r#"{
    ///   "$schema": "https://tree-sitter.github.io/tree-sitter/assets/schemas/grammar.schema.json",
    ///   "name": "tiny",
    ///   "rules": {
    ///     "source_file": {
    ///       "type": "REPEAT",
    ///       "content": {
    ///         "type": "SYMBOL",
    ///         "name": "word"
    ///       }
    ///     },
    ///     "word": {
    ///       "type": "PATTERN",
    ///       "value": "[a-z]+"
    ///     }
    ///   },
    ///   "extras": [
    ///     {
    ///       "type": "PATTERN",
    ///       "value": "\\s"
    ///     }
    ///   ],
    ///   "conflicts": [],
    ///   "precedences": [],
    ///   "externals": [],
    ///   "inline": [],
    ///   "supertypes": [],
    ///   "reserved": {}
    /// }"#);
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    pub fn to_json(&self) -> Result<String, Error> {
        let mut out = String::new();
        self.write_json(&mut out)?;
        Ok(out)
    }

    /// Validates the grammar and appends it to `out` as `grammar.json`.
    ///
    /// The same output as [`to_json`](Grammar::to_json), written into a buffer
    /// the caller owns. On error nothing is appended.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `out` | The buffer to append to. Existing contents are kept. |
    ///
    /// # Errors
    ///
    /// The first problem validation finds; see [`Error`]. `out` is unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Error, Grammar, Rule};
    ///
    /// let mut out = String::from("existing");
    /// let broken = Grammar::new("demo").rule("source_file", Rule::symbol("missing"));
    /// assert!(matches!(broken.write_json(&mut out), Err(Error::UndefinedSymbol { .. })));
    /// assert_eq!(out, "existing");
    /// ```
    pub fn write_json(&self, out: &mut String) -> Result<(), Error> {
        check::validate(self)?;
        json::write(self, out);
        Ok(())
    }

    /// Renders a concrete syntax tree as the S-expression tree-sitter prints
    /// for the same input.
    ///
    /// This is the bridge between a language's own parser and its generated
    /// tree-sitter grammar. Parse a sample with the parser that builds
    /// [`syntax_lang::Node`] trees, render the tree with `sexp`, and write the
    /// sample and the S-expression into a tree-sitter corpus file
    /// (`test/corpus/*.txt`). `tree-sitter test` then checks that the
    /// generated parser builds the same tree — the two parsers cannot drift
    /// apart unnoticed.
    ///
    /// Every node and token is named by `name`, then shown the way tree-sitter
    /// shows it:
    ///
    /// | The name is… | Shown as |
    /// |---|---|
    /// | A visible rule that lexes a single token (a string, pattern, or `token`, possibly under `prec` or `field`) | A leaf, `(name)`; anything beneath it is part of the token. |
    /// | A visible rule that names no other node (no symbol or alias in it) | `(name ...)` around only the extras, such as comments, beneath it. |
    /// | Any other visible rule, or a name given by [`alias`](Rule::alias) | `(name ...)` around its children; a token gets `(name)`. |
    /// | A hidden rule (`_name`), an [inlined](Grammar::inline) rule, or a [supertype](Grammar::supertype) | Not shown; its children take its place. |
    /// | An [external](Grammar::external) token | `(name)`, unless hidden. |
    /// | Not in the grammar, on a token | Not shown: an anonymous token, such as punctuation, keywords written as strings, or whitespace. |
    /// | Not in the grammar, on a node | An [`Error::UnknownNode`]. |
    ///
    /// Children are indented two spaces per level, as `tree-sitter test`
    /// formats them. Fields are not reconstructed — the tree does not record
    /// them, and `tree-sitter test` ignores fields when the expected tree has
    /// none — and an alias shows up only where the tree itself uses the
    /// aliased name. `sexp` does not validate the grammar; emit it
    /// with [`to_js`](Grammar::to_js) for that.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `tree` | The tree to render; its root is rendered like any other node. |
    /// | `name` | Names a node or token kind, matching the grammar's rule names. Usually a method on the kind type. |
    ///
    /// # Errors
    ///
    /// [`Error::UnknownNode`] if a node's name is not a rule, external, or
    /// alias name of the grammar.
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    /// use treesitter_lang::syntax_lang::{Builder, Span, Token};
    ///
    /// #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    /// enum Kind { File, Sum, Number, Plus, Space }
    ///
    /// impl Kind {
    ///     fn name(&self) -> &'static str {
    ///         match self {
    ///             Kind::File => "source_file",
    ///             Kind::Sum => "sum",
    ///             Kind::Number => "number",
    ///             Kind::Plus => "+",
    ///             Kind::Space => " ",
    ///         }
    ///     }
    /// }
    ///
    /// let grammar = Grammar::new("calc")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("_expression")))
    ///     .rule("_expression", Rule::choice([Rule::symbol("number"), Rule::symbol("sum")]))
    ///     .rule("sum", Rule::prec_left(1, Rule::seq([
    ///         Rule::symbol("_expression"),
    ///         Rule::string("+"),
    ///         Rule::symbol("_expression"),
    ///     ])))
    ///     .rule("number", Rule::pattern(r"\d+"));
    ///
    /// // The tree a hand-written parser builds for `1 + 2`.
    /// let mut b = Builder::new();
    /// b.start_node(Kind::File);
    /// b.start_node(Kind::Sum);
    /// b.token(Token::new(Kind::Number, Span::new(0, 1)));
    /// b.token(Token::new(Kind::Space, Span::new(1, 2)));
    /// b.token(Token::new(Kind::Plus, Span::new(2, 3)));
    /// b.token(Token::new(Kind::Space, Span::new(3, 4)));
    /// b.token(Token::new(Kind::Number, Span::new(4, 5)));
    /// b.finish_node();
    /// b.finish_node();
    /// let tree = b.finish()?;
    ///
    /// assert_eq!(
    ///     grammar.sexp(&tree, Kind::name)?,
    ///     "(source_file\n  (sum\n    (number)\n    (number)))",
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn sexp<'n, K>(
        &self,
        tree: &Node<K>,
        name: impl Fn(&K) -> &'n str,
    ) -> Result<String, Error> {
        let mut out = String::new();
        self.write_sexp(tree, name, &mut out)?;
        Ok(out)
    }

    /// Appends the S-expression for `tree` to `out`.
    ///
    /// The same output as [`sexp`](Grammar::sexp), written into a buffer the
    /// caller owns — reuse one buffer across a whole corpus. On error nothing
    /// is appended.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `tree` | The tree to render. |
    /// | `name` | Names a node or token kind, matching the grammar's rule names. |
    /// | `out` | The buffer to append to. Existing contents are kept. |
    ///
    /// # Errors
    ///
    /// [`Error::UnknownNode`] if a node's name is not a rule, external, or
    /// alias name of the grammar. `out` is unchanged.
    ///
    /// # Examples
    ///
    /// Writing a corpus entry — a header, the source, a divider, and the
    /// expected tree — in the format `tree-sitter test` reads:
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    /// use treesitter_lang::syntax_lang::{Element, Node, Span, Token};
    ///
    /// let grammar = Grammar::new("words")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("word")))
    ///     .rule("word", Rule::pattern("[a-z]+"));
    ///
    /// let source = "hi there";
    /// let tree = Node::new("source_file", vec![
    ///     Element::Token(Token::new("word", Span::new(0, 2))),
    ///     Element::Token(Token::new("space", Span::new(2, 3))),
    ///     Element::Token(Token::new("word", Span::new(3, 8))),
    /// ]);
    ///
    /// let mut corpus = String::new();
    /// corpus.push_str("==================\nTwo words\n==================\n\n");
    /// corpus.push_str(source);
    /// corpus.push_str("\n\n---\n\n");
    /// grammar.write_sexp(&tree, |kind| *kind, &mut corpus)?;
    /// corpus.push('\n');
    ///
    /// assert!(corpus.ends_with("---\n\n(source_file\n  (word)\n  (word))\n"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    pub fn write_sexp<'n, K>(
        &self,
        tree: &Node<K>,
        name: impl Fn(&K) -> &'n str,
        out: &mut String,
    ) -> Result<(), Error> {
        let start = out.len();
        let result = sexp::write(self, tree, &name, out);
        if result.is_err() {
            out.truncate(start);
        }
        result
    }
}

impl Grammar {
    /// The inline entries with repeats removed, first occurrence kept — what
    /// tree-sitter does with them. Inline lists are short, so the quadratic
    /// scan beats building a set.
    pub(crate) fn inline_names(&self) -> impl Iterator<Item = &Text> {
        self.inline
            .iter()
            .enumerate()
            .filter(|&(i, name)| !self.inline[..i].contains(name))
            .map(|(_, name)| name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_records_settings_in_order() {
        let g = Grammar::new("demo")
            .rule("a", Rule::string("a"))
            .rule("b", Rule::string("b"))
            .extra(Rule::pattern(r"\s"))
            .external("ext")
            .supertype("_s")
            .inline("_i")
            .conflict(["a", "b"])
            .word("b")
            .word("a");
        let names: Vec<&str> = g.rules.iter().map(|(n, _)| &**n).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(g.extras.as_ref().map(Vec::len), Some(1));
        assert_eq!(g.externals, ["ext"]);
        assert_eq!(g.supertypes, ["_s"]);
        assert_eq!(g.inline, ["_i"]);
        assert_eq!(g.conflicts, [["a", "b"]]);
        assert_eq!(g.word.as_deref(), Some("a"));
    }

    #[test]
    fn test_public_types_are_send_and_sync() {
        fn shareable<T: Send + Sync>() {}
        shareable::<Grammar>();
        shareable::<Rule>();
        shareable::<Error>();
    }

    #[test]
    fn test_extras_default_to_none_until_added() {
        assert!(Grammar::new("x").extras.is_none());
    }

    #[test]
    fn test_write_js_leaves_buffer_untouched_on_error() {
        let mut out = String::from("keep");
        let result = Grammar::new("x").write_js(&mut out);
        assert_eq!(result, Err(Error::NoRules));
        assert_eq!(out, "keep");
    }
}
