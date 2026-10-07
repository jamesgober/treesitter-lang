//! Why a grammar could not be emitted, or a tree could not be rendered.

use alloc::string::String;
use core::fmt;

/// Why [`Grammar`](crate::Grammar) refused to emit a grammar, or to render a
/// tree as an S-expression.
///
/// Emitting validates the whole grammar first and reports the first problem,
/// checking in this order:
///
/// 1. names (the grammar's, then each rule's, then each external's), the
///    start rule, and duplicate rules;
/// 2. every rule body in definition order, then the extras: symbols resolve
///    and are not inside tokens, patterns and choices are not empty, field
///    and alias names are identifiers;
/// 3. the names in `word`, `conflicts`, `inline`, and `supertypes` resolve;
/// 4. the rules reachable from the start rule and the extras: no cycle of
///    rules that can each be just the next; then, in definition order, no
///    empty strings, and nothing that can match the empty string where
///    tree-sitter needs a token;
/// 5. the word token, externals sharing a rule's name, inlined rules, and
///    supertypes.
///
/// Every variant names a mistake that would otherwise surface later, as an
/// error from `tree-sitter generate` — in two cases as a crash or a hang — or
/// as a `grammar.js` that does not load. Rules that cannot be reached from the
/// start rule or the extras are checked only as far as `grammar.js` itself
/// evaluates them (step 2), because tree-sitter drops them.
///
/// What validation does not do is build parse tables. The problems
/// tree-sitter finds while building them — conflicts, and extras made of
/// rules whose end it cannot tell — are reported by `tree-sitter generate`.
///
/// Where a variant has a `rule` field, it holds the name of the rule the
/// problem was found in, or — for a problem outside any rule — the name of the
/// grammar field it was found in: `extras`, `word`, `conflicts`, `inline`, or
/// `supertypes`.
///
/// # Examples
///
/// ```
/// use treesitter_lang::{Error, Grammar, Rule};
///
/// let grammar = Grammar::new("demo").rule("source_file", Rule::symbol("item"));
///
/// match grammar.to_js() {
///     Err(Error::UndefinedSymbol { symbol, rule }) => {
///         assert_eq!(symbol, "item");
///         assert_eq!(rule, "source_file");
///     }
///     other => panic!("unexpected {other:?}"),
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The grammar name, a rule name, an external, a field name, or an alias
    /// name is not an identifier. Names must start with an ASCII letter or
    /// `_` and continue with ASCII letters, digits, or `_`; tree-sitter turns
    /// them into JavaScript property names and C identifiers. A rule or an
    /// external may not be called `__proto__`, which JavaScript reserves.
    ///
    /// Rename it. Tree-sitter convention is `snake_case`.
    InvalidName {
        /// The offending name.
        name: String,
    },

    /// The grammar has no rules. Tree-sitter needs at least one: the first
    /// rule is the start rule.
    NoRules,

    /// Two rules share a name. The second definition would silently replace
    /// the first in `grammar.js`, so it is refused instead.
    ///
    /// Rename one of them, or merge them with [`Rule::choice`](crate::Rule::choice).
    DuplicateRule {
        /// The name defined more than once.
        name: String,
    },

    /// The first rule — the start rule, the root of every tree — is hidden
    /// (its name starts with `_`). Tree-sitter requires it to be visible.
    ///
    /// Rename it, or add a visible rule in front of it.
    HiddenStart {
        /// The start rule's name.
        name: String,
    },

    /// A symbol names neither a rule nor an external token.
    ///
    /// Usually a typo; otherwise define the rule with
    /// [`Grammar::rule`](crate::Grammar::rule) or declare it with
    /// [`Grammar::external`](crate::Grammar::external).
    UndefinedSymbol {
        /// The undefined name.
        symbol: String,
        /// Where it was referred to.
        rule: String,
    },

    /// A string literal in a reachable rule is empty — tree-sitter refuses
    /// these outside tokens and in most places inside them, and the rest can
    /// only match nothing — or a pattern anywhere is empty, which cannot be
    /// written as a JavaScript regular expression literal.
    ///
    /// Use [`Rule::blank`](crate::Rule::blank) or
    /// [`Rule::optional`](crate::Rule::optional) to express "nothing".
    EmptyString {
        /// Where the empty string or pattern was found.
        rule: String,
    },

    /// A [`Rule::choice`](crate::Rule::choice) has no alternatives, so it can
    /// never match. Tree-sitter does not report this; it is always a mistake.
    EmptyChoice {
        /// Where the empty choice was found.
        rule: String,
    },

    /// A symbol appears inside [`Rule::token`](crate::Rule::token),
    /// [`Rule::immediate`](crate::Rule::immediate), or an extra that is not a
    /// lone symbol — all of which are lexed as one token, so their contents
    /// may only be strings, patterns, and the combinators over them, not
    /// references to other rules.
    ///
    /// Inline the referenced rule's strings and patterns into the token, or
    /// make the extra a symbol naming a rule.
    SymbolInToken {
        /// The symbol found inside the token.
        symbol: String,
        /// The rule containing the token, or `extras`.
        rule: String,
    },

    /// Something tree-sitter needs to match at least one token can match
    /// the empty string: a rule that another reachable rule (or an extra)
    /// refers to, or the contents of a [`Rule::repeat`](crate::Rule::repeat)
    /// or [`Rule::repeat1`](crate::Rule::repeat1) in a reachable rule. Only a
    /// rule nothing refers to — in practice the start rule — may match
    /// nothing.
    ///
    /// Move the optionality outward: make the rule (or the repeated part)
    /// match at least one token, and wrap its uses in
    /// [`Rule::optional`](crate::Rule::optional), or use
    /// [`Rule::repeat1`](crate::Rule::repeat1) instead of
    /// [`Rule::repeat`](crate::Rule::repeat).
    MatchesEmpty {
        /// The rule that can match nothing, or that contains the repetition.
        rule: String,
    },

    /// Reachable rules form a cycle in which each can be just the next —
    /// `a` can be `b` alone and `b` can be `a` alone — so one piece of text
    /// would have endlessly many trees. Tree-sitter refuses this as an
    /// "indirectly recursive rule". A rule can be another alone when that
    /// rule is one of its alternatives, through
    /// [`choice`](crate::Rule::choice), [`optional`](crate::Rule::optional),
    /// [`prec`](crate::Rule::prec), [`field`](crate::Rule::field), and
    /// [`alias`](crate::Rule::alias). Also reported: a hidden rule with an
    /// alternative that is itself under a non-zero `prec`, `prec.left`, or
    /// `prec.right`, on which tree-sitter loops forever.
    ///
    /// Break the cycle: usually one of the rules should require more than
    /// the other (a delimiter, an operator), or the two should be one rule.
    IndirectRecursion {
        /// A rule on the cycle.
        rule: String,
    },

    /// The [`word`](crate::Grammar::word) rule is not a token. Tree-sitter
    /// requires it to be a single terminal: not the start rule, its body a
    /// string, a pattern, [`Rule::token`](crate::Rule::token), or
    /// [`Rule::immediate`](crate::Rule::immediate) — with no
    /// [`prec`](crate::Rule::prec), [`field`](crate::Rule::field), or
    /// [`alias`](crate::Rule::alias) around it — and that token used nowhere
    /// else in the reachable rules (tree-sitter would merge the two uses,
    /// leaving the rule a non-terminal). A hidden rule whose token is only a
    /// string literal is not a token either: tree-sitter treats it as that
    /// anonymous string. Or the word may be an external.
    ///
    /// Refer to the word rule by symbol wherever its token is needed.
    WordNotToken {
        /// The word rule's name.
        name: String,
    },

    /// An [external](crate::Grammar::external) shares its name with a rule
    /// that is not a token. A rule may share an external's name only as the
    /// fallback lexer for it, so it must meet the same conditions as the
    /// [word token](Error::WordNotToken).
    ///
    /// Rename one of them, or make the rule a token.
    InvalidExternal {
        /// The external's name.
        name: String,
    },

    /// An [inlined](crate::Grammar::inline) rule cannot be inlined: it is
    /// the start rule, an external, a rule tree-sitter lexes as a token (by
    /// the same test as the [word token](Error::WordNotToken)), or it refers
    /// to itself through inlined rules alone, which tree-sitter would expand
    /// forever.
    ///
    /// Remove it from the inline list, or break the cycle by leaving one of
    /// the rules in it un-inlined.
    InvalidInline {
        /// The rule that cannot be inlined.
        name: String,
    },

    /// A [supertype](crate::Grammar::supertype) is an external, or a
    /// reachable rule tree-sitter cannot use as one: a token with a pattern
    /// in it, a rule one of whose alternatives is the supertype itself, or a
    /// rule with an alternative that can produce more than one node. Every
    /// alternative of a supertype must be a single node, counted through
    /// hidden and inlined rules, which tree-sitter expands in place.
    ///
    /// Make the supertype a choice of single symbols.
    InvalidSupertype {
        /// The supertype's name.
        name: String,
    },

    /// [`Grammar::sexp`](crate::Grammar::sexp) met a tree node whose name is
    /// not a rule, external, or alias name of the grammar, so the grammar and
    /// the parser that built the tree have drifted apart.
    ///
    /// Add the rule, or correct the naming function passed to `sexp`.
    UnknownNode {
        /// The name the naming function gave the node.
        name: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name } => write!(
                f,
                "`{name}` is not a valid name: names start with an ASCII letter or `_` \
                 and continue with ASCII letters, digits, or `_`"
            ),
            Self::NoRules => {
                f.write_str("the grammar has no rules; tree-sitter needs at least one")
            }
            Self::DuplicateRule { name } => write!(f, "rule `{name}` is defined more than once"),
            Self::HiddenStart { name } => write!(
                f,
                "the start rule `{name}` is hidden; tree-sitter requires the first rule to be visible"
            ),
            Self::UndefinedSymbol { symbol, rule } => {
                write!(f, "`{rule}` refers to undefined symbol `{symbol}`")
            }
            Self::EmptyString { rule } => write!(f, "`{rule}` contains an empty string or pattern"),
            Self::EmptyChoice { rule } => {
                write!(f, "`{rule}` contains a choice with no alternatives")
            }
            Self::SymbolInToken { symbol, rule } => write!(
                f,
                "`{rule}` uses symbol `{symbol}` inside a token; token contents must be \
                 strings and patterns"
            ),
            Self::MatchesEmpty { rule } => write!(
                f,
                "rule `{rule}` can match the empty string where tree-sitter needs a token: \
                 it is referred to by another rule, or it repeats something that can match nothing"
            ),
            Self::IndirectRecursion { rule } => write!(
                f,
                "rule `{rule}` is part of a cycle of rules that can each be just the next one"
            ),
            Self::WordNotToken { name } => write!(
                f,
                "word rule `{name}` is not a token: its body must be a string, a pattern, \
                 or a token used nowhere else"
            ),
            Self::InvalidExternal { name } => write!(
                f,
                "external `{name}` shares its name with a rule that is not a token"
            ),
            Self::InvalidInline { name } => write!(f, "rule `{name}` cannot be inlined"),
            Self::InvalidSupertype { name } => write!(
                f,
                "supertype `{name}` must be a rule whose every alternative is a single node"
            ),
            Self::UnknownNode { name } => write!(
                f,
                "the tree has a node named `{name}`, which is not in the grammar"
            ),
        }
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test]
    fn test_display_names_the_offending_items() {
        let e = Error::UndefinedSymbol {
            symbol: "item".into(),
            rule: "list".into(),
        };
        assert_eq!(e.to_string(), "`list` refers to undefined symbol `item`");
        let e = Error::MatchesEmpty {
            rule: "args".into(),
        };
        assert!(
            e.to_string()
                .starts_with("rule `args` can match the empty string")
        );
        assert!(Error::NoRules.to_string().contains("no rules"));
    }

    #[test]
    fn test_every_variant_displays_something() {
        let s = || String::from("x");
        let all = [
            Error::InvalidName { name: s() },
            Error::NoRules,
            Error::DuplicateRule { name: s() },
            Error::HiddenStart { name: s() },
            Error::UndefinedSymbol {
                symbol: s(),
                rule: s(),
            },
            Error::EmptyString { rule: s() },
            Error::EmptyChoice { rule: s() },
            Error::SymbolInToken {
                symbol: s(),
                rule: s(),
            },
            Error::MatchesEmpty { rule: s() },
            Error::IndirectRecursion { rule: s() },
            Error::WordNotToken { name: s() },
            Error::InvalidExternal { name: s() },
            Error::InvalidInline { name: s() },
            Error::InvalidSupertype { name: s() },
            Error::UnknownNode { name: s() },
        ];
        for e in all {
            assert!(e.to_string().contains('x') || matches!(e, Error::NoRules));
        }
    }
}
