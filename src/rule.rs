//! Rule expressions: the right-hand sides of a grammar's rules.

use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use core::{mem, slice};

/// Text a rule holds: a name, a string literal, or a pattern. Borrowed when it
/// is a `&'static str`, so grammars written with literals never copy them.
pub(crate) type Text = Cow<'static, str>;

/// A rule expression: what one rule of a grammar matches.
///
/// `Rule` is tree-sitter's rule language in Rust. Each constructor corresponds
/// to one function of tree-sitter's `grammar.js` DSL, and is emitted as that
/// function:
///
/// | Constructor | `grammar.js` | Matches |
/// |---|---|---|
/// | [`Rule::symbol`] | `$.name` | The rule (or external token) called `name`. |
/// | [`Rule::string`] | `'text'` | The exact text. |
/// | [`Rule::pattern`] | `/regex/` | Text matching a regular expression. |
/// | [`Rule::blank`] | `blank()` | Nothing. |
/// | [`Rule::seq`] | `seq(a, b)` | Each rule in order. |
/// | [`Rule::choice`] | `choice(a, b)` | Any one of the rules. |
/// | [`Rule::optional`] | `optional(a)` | The rule, or nothing. |
/// | [`Rule::repeat`] | `repeat(a)` | The rule zero or more times. |
/// | [`Rule::repeat1`] | `repeat1(a)` | The rule one or more times. |
/// | [`Rule::field`] | `field('name', a)` | The rule, labelled as a field of its parent. |
/// | [`Rule::alias`] | `alias(a, $.name)` | The rule, appearing in the tree as `name`. |
/// | [`Rule::token`] | `token(a)` | The rule, lexed as one token. |
/// | [`Rule::immediate`] | `token.immediate(a)` | One token, with no whitespace before it. |
/// | [`Rule::prec`] and friends | `prec(n, a)`, `prec.left`, `prec.right`, `prec.dynamic` | The rule, with a precedence. |
///
/// Rules are plain values: build them, store them in variables, and clone a
/// shared piece to use it in several places. Nothing is checked until the
/// grammar is emitted, so a rule may name a symbol that is defined later.
///
/// `Clone` and `Debug` recurse into nested rules, so a rule nested tens of
/// thousands of levels deep can exhaust the stack in them; dropping a rule is
/// iterative and safe at any depth, and so is emitting one.
///
/// # Examples
///
/// ```
/// use treesitter_lang::{Grammar, Rule};
///
/// // A comma-separated list of numbers in brackets: `[1, 2, 3]`.
/// let number = Rule::symbol("number");
/// let list = Rule::seq([
///     Rule::string("["),
///     Rule::optional(Rule::seq([
///         number.clone(),
///         Rule::repeat(Rule::seq([Rule::string(","), number])),
///     ])),
///     Rule::string("]"),
/// ]);
///
/// let js = Grammar::new("lists")
///     .rule("list", list)
///     .rule("number", Rule::pattern(r"\d+"))
///     .to_js()?;
/// assert!(js.contains("optional(seq("));
/// # Ok::<(), treesitter_lang::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct Rule(pub(crate) Expr);

/// The rule language, one variant per tree-sitter rule type.
#[derive(Clone, Debug)]
pub(crate) enum Expr {
    Blank,
    String(Text),
    Pattern(Text),
    Symbol(Text),
    Seq(Vec<Rule>),
    Choice(Vec<Rule>),
    Repeat(Box<Rule>),
    Repeat1(Box<Rule>),
    Field(Text, Box<Rule>),
    Alias(Text, Box<Rule>),
    Token(Box<Rule>),
    Immediate(Box<Rule>),
    Prec(Prec, i32, Box<Rule>),
}

/// Which of tree-sitter's four precedence functions a [`Expr::Prec`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Prec {
    Plain,
    Left,
    Right,
    Dynamic,
}

impl Rule {
    /// A reference to another rule, or to an external token, by name.
    ///
    /// Emitted as `$.name`. The name is resolved when the grammar is emitted,
    /// so rules may refer to each other in any order. A name starting with `_`
    /// refers to a hidden rule, which matches like any other but does not
    /// appear as a node in the tree.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `name` | The rule's name: a `&'static str` (not copied) or a `String`. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("_item")))
    ///     .rule("_item", Rule::choice([Rule::symbol("word"), Rule::symbol("number")]))
    ///     .rule("word", Rule::pattern("[a-z]+"))
    ///     .rule("number", Rule::pattern(r"\d+"))
    ///     .to_js()?;
    /// assert!(js.contains("source_file: $ => repeat($._item)"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[inline]
    #[must_use]
    pub fn symbol(name: impl Into<Cow<'static, str>>) -> Rule {
        Rule(Expr::Symbol(name.into()))
    }

    /// Exactly this text. Emitted as a quoted string, escaped as needed.
    ///
    /// A string inside a rule becomes an anonymous node in the tree: it is
    /// there, but tree-sitter's S-expressions (and
    /// [`Grammar::sexp`](crate::Grammar::sexp)) leave it out. A rule whose
    /// whole body is one string is a named token, and does appear. The text
    /// must not be empty ([`Error::EmptyString`](crate::Error::EmptyString)).
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `text` | The literal text: a `&'static str` (not copied) or a `String`. Any characters, including quotes and newlines. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::seq([Rule::string("let"), Rule::string("'")]))
    ///     .to_js()?;
    /// assert!(js.contains(r"seq('let', '\'')"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[inline]
    #[must_use]
    pub fn string(text: impl Into<Cow<'static, str>>) -> Rule {
        Rule(Expr::String(text.into()))
    }

    /// Text matching a regular expression.
    ///
    /// The pattern is written as the source of a JavaScript regular expression
    /// — exactly what goes between the slashes of `/.../` — and tree-sitter
    /// compiles it into its lexer. Write it as a raw string (`r"\d+"`) so
    /// backslashes need no doubling. A `/` in the pattern is written as `\/`,
    /// and a line break as `\n` or `\r`, so it fits in a `/.../` literal; the
    /// regular expression is the same. The pattern must not be empty
    /// ([`Error::EmptyString`](crate::Error::EmptyString)).
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `regex` | The pattern source: a `&'static str` (not copied) or a `String`. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("path")))
    ///     .rule("path", Rule::pattern(r"[a-z]+(/[a-z]+)*"))
    ///     .to_js()?;
    /// assert!(js.contains(r"path: $ => /[a-z]+(\/[a-z]+)*/"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[inline]
    #[must_use]
    pub fn pattern(regex: impl Into<Cow<'static, str>>) -> Rule {
        Rule(Expr::Pattern(regex.into()))
    }

    /// Matches nothing. Emitted as `blank()`.
    ///
    /// Rarely needed directly — [`Rule::optional`] is `choice(rule, blank())`
    /// — but useful as an explicit empty alternative in a
    /// [`choice`](Rule::choice).
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::choice([Rule::blank(), Rule::string("x")]))
    ///     .to_js()?;
    /// assert!(js.contains("choice(blank(), 'x')"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[inline]
    #[must_use]
    pub const fn blank() -> Rule {
        Rule(Expr::Blank)
    }

    /// Each rule in order, one after another.
    ///
    /// Whitespace and other [extras](crate::Grammar::extra) may appear between
    /// the parts, because each part is lexed separately — to match text with
    /// no gaps, wrap the sequence in [`Rule::token`]. An empty sequence
    /// matches nothing: it *is* [`Rule::blank`], and is emitted as `blank()`.
    /// (Tree-sitter treats the two alike except inside a token, where it
    /// refuses an empty `seq()` but accepts `blank()`.)
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rules` | The parts, in order: an array, a `Vec`, or any iterator of rules. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let assignment = Rule::seq([
    ///     Rule::symbol("identifier"),
    ///     Rule::string("="),
    ///     Rule::symbol("identifier"),
    /// ]);
    /// let js = Grammar::new("demo")
    ///     .rule("assignment", assignment)
    ///     .rule("identifier", Rule::pattern("[a-z]+"))
    ///     .to_js()?;
    /// assert!(js.contains("seq($.identifier, '=', $.identifier)"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    ///
    /// Parts built in a loop:
    ///
    /// ```
    /// use treesitter_lang::Rule;
    ///
    /// let keywords = ["pub", "fn"].map(Rule::string);
    /// let signature = Rule::seq(keywords.into_iter().chain([Rule::symbol("name")]));
    /// # let _ = signature;
    /// ```
    #[must_use]
    pub fn seq(rules: impl IntoIterator<Item = Rule>) -> Rule {
        let rules: Vec<Rule> = rules.into_iter().collect();
        if rules.is_empty() {
            return Rule::blank();
        }
        Rule(Expr::Seq(rules))
    }

    /// Any one of the rules.
    ///
    /// When more than one alternative could match, tree-sitter resolves the
    /// ambiguity with [precedence](Rule::prec) or, failing that, reports a
    /// conflict for you to declare with
    /// [`Grammar::conflict`](crate::Grammar::conflict). A choice must have at
    /// least one alternative ([`Error::EmptyChoice`](crate::Error::EmptyChoice)).
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rules` | The alternatives: an array, a `Vec`, or any iterator of rules. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let literal = Rule::choice([
    ///     Rule::string("true"),
    ///     Rule::string("false"),
    ///     Rule::symbol("number"),
    /// ]);
    /// let js = Grammar::new("demo")
    ///     .rule("literal", literal)
    ///     .rule("number", Rule::pattern(r"\d+"))
    ///     .to_js()?;
    /// assert!(js.contains("choice('true', 'false', $.number)"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn choice(rules: impl IntoIterator<Item = Rule>) -> Rule {
        Rule(Expr::Choice(rules.into_iter().collect()))
    }

    /// The rule, or nothing.
    ///
    /// The same as `choice([rule, Rule::blank()])`, which is how tree-sitter
    /// represents it in `grammar.json`; `grammar.js` shows it as
    /// `optional(...)`.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rule` | What may be left out. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let ret = Rule::seq([Rule::string("return"), Rule::optional(Rule::symbol("number"))]);
    /// let js = Grammar::new("demo")
    ///     .rule("return_statement", ret)
    ///     .rule("number", Rule::pattern(r"\d+"))
    ///     .to_js()?;
    /// assert!(js.contains("seq('return', optional($.number))"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn optional(rule: Rule) -> Rule {
        Rule(Expr::Choice(alloc::vec![rule, Rule::blank()]))
    }

    /// The rule zero or more times.
    ///
    /// Because it can match nothing, a `repeat` makes the enclosing rule able
    /// to match the empty string — fine in the start rule, but an
    /// [`Error::MatchesEmpty`](crate::Error::MatchesEmpty) in a rule that
    /// others refer to. Use [`Rule::repeat1`] there.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rule` | What repeats. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("statement")))
    ///     .rule("statement", Rule::seq([Rule::pattern("[a-z]+"), Rule::string(";")]))
    ///     .to_js()?;
    /// assert!(js.contains("source_file: $ => repeat($.statement)"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn repeat(rule: Rule) -> Rule {
        Rule(Expr::Repeat(Box::new(rule)))
    }

    /// The rule one or more times.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rule` | What repeats. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("block")))
    ///     .rule("block", Rule::seq([
    ///         Rule::string("{"),
    ///         Rule::repeat1(Rule::symbol("word")),
    ///         Rule::string("}"),
    ///     ]))
    ///     .rule("word", Rule::pattern("[a-z]+"))
    ///     .to_js()?;
    /// assert!(js.contains("repeat1($.word)"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn repeat1(rule: Rule) -> Rule {
        Rule(Expr::Repeat1(Box::new(rule)))
    }

    /// The rule, labelled as a field of the node it appears in.
    ///
    /// Fields give children names, so a tree consumer can ask for "the left
    /// operand" instead of "the first child". They do not change what the
    /// rule matches. The name must be an identifier
    /// ([`Error::InvalidName`](crate::Error::InvalidName)).
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `name` | The field name, conventionally `snake_case`. |
    /// | `rule` | The labelled part. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let call = Rule::seq([
    ///     Rule::field("function", Rule::symbol("identifier")),
    ///     Rule::string("("),
    ///     Rule::string(")"),
    /// ]);
    /// let js = Grammar::new("demo")
    ///     .rule("call", call)
    ///     .rule("identifier", Rule::pattern("[a-z]+"))
    ///     .to_js()?;
    /// assert!(js.contains("field('function', $.identifier)"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn field(name: impl Into<Cow<'static, str>>, rule: Rule) -> Rule {
        Rule(Expr::Field(name.into(), Box::new(rule)))
    }

    /// The rule, appearing in the tree as a named node called `name`.
    ///
    /// Use it to give an anonymous piece of syntax a node of its own, or to
    /// have one rule show up under another rule's name in some context. The
    /// name must be an identifier
    /// ([`Error::InvalidName`](crate::Error::InvalidName)); it need not be a
    /// rule of the grammar.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rule` | What matches. |
    /// | `name` | The node name it appears as. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::repeat(Rule::alias(Rule::string("..."), "ellipsis")))
    ///     .to_js()?;
    /// assert!(js.contains("alias('...', $.ellipsis)"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn alias(rule: Rule, name: impl Into<Cow<'static, str>>) -> Rule {
        Rule(Expr::Alias(name.into(), Box::new(rule)))
    }

    /// The rule, lexed as a single token.
    ///
    /// Inside a token there are no gaps for whitespace, and the whole match
    /// becomes one leaf of the tree. The contents may only be strings,
    /// patterns, and combinators over them; a [`Rule::symbol`] inside is
    /// refused ([`Error::SymbolInToken`](crate::Error::SymbolInToken)).
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rule` | The token's lexical structure. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// // `1.5e3` as one token, not five pieces separated by optional spaces.
    /// let float = Rule::token(Rule::seq([
    ///     Rule::pattern(r"\d+"),
    ///     Rule::string("."),
    ///     Rule::pattern(r"\d+"),
    ///     Rule::optional(Rule::seq([Rule::string("e"), Rule::pattern(r"\d+")])),
    /// ]));
    /// let js = Grammar::new("demo").rule("float", float).to_js()?;
    /// assert!(js.contains("float: $ => token(seq("));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn token(rule: Rule) -> Rule {
        Rule(Expr::Token(Box::new(rule)))
    }

    /// A token that must follow the previous one with nothing in between.
    ///
    /// Emitted as `token.immediate(...)`. Use it where whitespace changes the
    /// meaning: a suffix glued to a number, or the contents right after an
    /// opening quote. The same content restrictions as [`Rule::token`] apply.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `rule` | The token's lexical structure. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// // `10px` but not `10 px`.
    /// let length = Rule::seq([Rule::pattern(r"\d+"), Rule::immediate(Rule::string("px"))]);
    /// let js = Grammar::new("demo").rule("length", length).to_js()?;
    /// assert!(js.contains("token.immediate('px')"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn immediate(rule: Rule) -> Rule {
        Rule(Expr::Immediate(Box::new(rule)))
    }

    /// The rule with a numeric precedence, emitted as `prec(level, ...)`.
    ///
    /// When tree-sitter finds two ways to proceed, the alternative with the
    /// higher precedence wins. Plain `prec` settles which rule to reduce; use
    /// [`prec_left`](Rule::prec_left) and [`prec_right`](Rule::prec_right)
    /// for operator associativity. Inside a [`Rule::token`], precedence picks
    /// between tokens that match the same text.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `level` | The precedence. Higher binds tighter; the default is `0`, and negative values are allowed. |
    /// | `rule` | The rule it applies to. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::repeat(Rule::choice([
    ///         Rule::symbol("keyword"),
    ///         Rule::symbol("identifier"),
    ///     ])))
    ///     .rule("keyword", Rule::token(Rule::prec(1, Rule::string("if"))))
    ///     .rule("identifier", Rule::pattern("[a-z]+"))
    ///     .to_js()?;
    /// assert!(js.contains("token(prec(1, 'if'))"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn prec(level: i32, rule: Rule) -> Rule {
        Rule(Expr::Prec(Prec::Plain, level, Box::new(rule)))
    }

    /// The rule with a precedence and left associativity, emitted as
    /// `prec.left(level, ...)`: `a - b - c` groups as `(a - b) - c`.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `level` | The precedence. Higher binds tighter. |
    /// | `rule` | The rule it applies to, usually a binary-operator sequence. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let e = || Rule::symbol("_expression");
    /// let js = Grammar::new("calc")
    ///     .rule("source_file", Rule::repeat(e()))
    ///     .rule("_expression", Rule::choice([Rule::symbol("number"), Rule::symbol("sum")]))
    ///     .rule("sum", Rule::prec_left(1, Rule::seq([e(), Rule::string("+"), e()])))
    ///     .rule("number", Rule::pattern(r"\d+"))
    ///     .to_js()?;
    /// assert!(js.contains("prec.left(1, seq($._expression, '+', $._expression))"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn prec_left(level: i32, rule: Rule) -> Rule {
        Rule(Expr::Prec(Prec::Left, level, Box::new(rule)))
    }

    /// The rule with a precedence and right associativity, emitted as
    /// `prec.right(level, ...)`: `a = b = c` groups as `a = (b = c)`.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `level` | The precedence. Higher binds tighter. |
    /// | `rule` | The rule it applies to. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let e = || Rule::symbol("_expression");
    /// let js = Grammar::new("calc")
    ///     .rule("source_file", Rule::repeat(e()))
    ///     .rule("_expression", Rule::choice([Rule::symbol("number"), Rule::symbol("power")]))
    ///     .rule("power", Rule::prec_right(2, Rule::seq([e(), Rule::string("^"), e()])))
    ///     .rule("number", Rule::pattern(r"\d+"))
    ///     .to_js()?;
    /// assert!(js.contains("prec.right(2, seq($._expression, '^', $._expression))"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn prec_right(level: i32, rule: Rule) -> Rule {
        Rule(Expr::Prec(Prec::Right, level, Box::new(rule)))
    }

    /// The rule with a dynamic precedence, emitted as
    /// `prec.dynamic(level, ...)`.
    ///
    /// Dynamic precedence is applied at parse time, between the parses of a
    /// conflict declared with [`Grammar::conflict`](crate::Grammar::conflict)
    /// that are still alive at the end: the parse with the higher total wins.
    ///
    /// | Parameter | Meaning |
    /// |---|---|
    /// | `level` | The dynamic precedence. |
    /// | `rule` | The rule it applies to. |
    ///
    /// # Examples
    ///
    /// ```
    /// use treesitter_lang::{Grammar, Rule};
    ///
    /// let js = Grammar::new("demo")
    ///     .rule("source_file", Rule::repeat(Rule::symbol("cast")))
    ///     .rule("cast", Rule::prec_dynamic(1, Rule::seq([
    ///         Rule::string("("),
    ///         Rule::pattern("[a-z]+"),
    ///         Rule::string(")"),
    ///     ])))
    ///     .to_js()?;
    /// assert!(js.contains("prec.dynamic(1, seq('(', /[a-z]+/, ')'))"));
    /// # Ok::<(), treesitter_lang::Error>(())
    /// ```
    #[must_use]
    pub fn prec_dynamic(level: i32, rule: Rule) -> Rule {
        Rule(Expr::Prec(Prec::Dynamic, level, Box::new(rule)))
    }

    /// The rule's direct sub-rules: the members of a sequence or choice, the
    /// one rule a wrapper applies to, or nothing for a leaf.
    #[inline]
    pub(crate) fn children(&self) -> &[Rule] {
        match &self.0 {
            Expr::Seq(rules) | Expr::Choice(rules) => rules,
            Expr::Repeat(rule)
            | Expr::Repeat1(rule)
            | Expr::Field(_, rule)
            | Expr::Alias(_, rule)
            | Expr::Token(rule)
            | Expr::Immediate(rule)
            | Expr::Prec(_, _, rule) => slice::from_ref(&**rule),
            Expr::Blank | Expr::String(_) | Expr::Pattern(_) | Expr::Symbol(_) => &[],
        }
    }

    /// The `optional(...)` shape: a two-way choice whose second alternative is
    /// blank. Returns the first alternative.
    #[inline]
    pub(crate) fn as_optional(&self) -> Option<&Rule> {
        match &self.0 {
            Expr::Choice(rules) => match rules.as_slice() {
                [rule, Rule(Expr::Blank)] => Some(rule),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether tree-sitter treats a rule with this body as a single terminal:
    /// a string, a pattern, or a token, with nothing wrapped around it.
    #[inline]
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self.0,
            Expr::String(_) | Expr::Pattern(_) | Expr::Token(_) | Expr::Immediate(_)
        )
    }

    /// Whether a rule with this body produces a single token: a terminal,
    /// possibly under `prec` and `field`, which change nothing about what is
    /// lexed. Such a rule has no children in tree-sitter's trees.
    pub(crate) fn is_single_token(&self) -> bool {
        let mut rule = self;
        loop {
            match &rule.0 {
                Expr::Prec(_, _, inner) | Expr::Field(_, inner) => rule = inner,
                _ => return rule.is_terminal(),
            }
        }
    }

    /// Whether the rule refers to another rule or names a node: whether
    /// anything in it can appear in the tree as a named child.
    pub(crate) fn names_nodes(&self) -> bool {
        let mut stack = alloc::vec![self];
        while let Some(rule) = stack.pop() {
            if matches!(rule.0, Expr::Symbol(_) | Expr::Alias(..)) {
                return true;
            }
            stack.extend(rule.children());
        }
        false
    }

    /// Structural equality, compared with an explicit stack. Tree-sitter
    /// merges identical tokens, so it needs to know when two are the same.
    pub(crate) fn same(&self, other: &Rule) -> bool {
        let mut stack = alloc::vec![(self, other)];
        while let Some((a, b)) = stack.pop() {
            let leaves_equal = match (&a.0, &b.0) {
                (Expr::Blank, Expr::Blank) => true,
                (Expr::String(x), Expr::String(y))
                | (Expr::Pattern(x), Expr::Pattern(y))
                | (Expr::Symbol(x), Expr::Symbol(y)) => x == y,
                (Expr::Seq(_), Expr::Seq(_))
                | (Expr::Choice(_), Expr::Choice(_))
                | (Expr::Repeat(_), Expr::Repeat(_))
                | (Expr::Repeat1(_), Expr::Repeat1(_))
                | (Expr::Token(_), Expr::Token(_))
                | (Expr::Immediate(_), Expr::Immediate(_)) => true,
                (Expr::Field(x, _), Expr::Field(y, _)) | (Expr::Alias(x, _), Expr::Alias(y, _)) => {
                    x == y
                }
                (Expr::Prec(k, n, _), Expr::Prec(l, m, _)) => k == l && n == m,
                _ => false,
            };
            let (left, right) = (a.children(), b.children());
            if !leaves_equal || left.len() != right.len() {
                return false;
            }
            stack.extend(left.iter().zip(right));
        }
        true
    }

    /// Moves every sub-rule that has sub-rules of its own onto `stack`,
    /// leaving a blank in its place. Leaf sub-rules stay where they are: they
    /// drop without recursing.
    fn detach_into(&mut self, stack: &mut Vec<Rule>) {
        let mut detach = |rule: &mut Rule| {
            if !rule.children().is_empty() {
                stack.push(mem::replace(rule, Rule::blank()));
            }
        };
        match &mut self.0 {
            Expr::Seq(rules) | Expr::Choice(rules) => rules.iter_mut().for_each(detach),
            Expr::Repeat(rule)
            | Expr::Repeat1(rule)
            | Expr::Field(_, rule)
            | Expr::Alias(_, rule)
            | Expr::Token(rule)
            | Expr::Immediate(rule)
            | Expr::Prec(_, _, rule) => detach(rule),
            Expr::Blank | Expr::String(_) | Expr::Pattern(_) | Expr::Symbol(_) => {}
        }
    }
}

impl Drop for Rule {
    /// Drops nested rules from a heap worklist instead of recursively, so a
    /// rule nested arbitrarily deep cannot overflow the stack when it is
    /// freed. Rules at most two levels deep — almost all of them — take the
    /// early return and allocate nothing.
    fn drop(&mut self) {
        if self
            .children()
            .iter()
            .all(|child| child.children().is_empty())
        {
            return;
        }
        let mut stack = Vec::new();
        self.detach_into(&mut stack);
        while let Some(mut rule) = stack.pop() {
            rule.detach_into(&mut stack);
            // `rule` now holds only leaves, so dropping it here goes one
            // level deep at most.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optional_is_choice_with_trailing_blank() {
        let rule = Rule::optional(Rule::string("x"));
        let inner = rule
            .as_optional()
            .map(|r| matches!(&r.0, Expr::String(s) if s == "x"));
        assert_eq!(inner, Some(true));
        assert!(
            Rule::choice([Rule::blank(), Rule::string("x")])
                .as_optional()
                .is_none()
        );
        assert!(Rule::choice([Rule::string("x")]).as_optional().is_none());
    }

    #[test]
    fn test_children_cover_every_shape() {
        assert!(Rule::blank().children().is_empty());
        assert!(Rule::symbol("a").children().is_empty());
        assert_eq!(
            Rule::seq([Rule::blank(), Rule::blank()]).children().len(),
            2
        );
        assert_eq!(Rule::prec_left(1, Rule::blank()).children().len(), 1);
        assert_eq!(Rule::field("f", Rule::blank()).children().len(), 1);
    }

    #[test]
    fn test_is_terminal_only_for_unwrapped_tokens() {
        assert!(Rule::string("a").is_terminal());
        assert!(Rule::pattern("a").is_terminal());
        assert!(Rule::token(Rule::seq([Rule::string("a")])).is_terminal());
        assert!(Rule::immediate(Rule::string("a")).is_terminal());
        assert!(!Rule::prec(1, Rule::string("a")).is_terminal());
        assert!(!Rule::symbol("a").is_terminal());
        assert!(!Rule::seq([Rule::string("a")]).is_terminal());
    }

    #[test]
    fn test_drop_deeply_nested_rule_does_not_overflow() {
        let mut rule = Rule::string("x");
        for i in 0..200_000 {
            rule = if i % 2 == 0 {
                Rule::seq([rule, Rule::string("y")])
            } else {
                Rule::repeat(rule)
            };
        }
        drop(rule);
    }

    #[test]
    fn test_clone_is_independent() {
        let a = Rule::seq([Rule::symbol("a"), Rule::string("b")]);
        let b = a.clone();
        drop(a);
        assert_eq!(b.children().len(), 2);
    }
}
