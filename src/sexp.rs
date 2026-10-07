//! Rendering a `syntax_lang` tree as the S-expression tree-sitter prints.
//!
//! Tree-sitter shows only named nodes. A node for a hidden, inlined, or
//! supertype rule is replaced by its children; a rule that lexes a single
//! token is a leaf even if the hand-written parser gave it structure; a rule
//! that names no other node shows only the extras (comments) inside it;
//! anonymous tokens (punctuation, keywords written as strings, whitespace)
//! are left out. The walk keeps an explicit stack of child iterators, so
//! trees of any depth render without recursion.

use alloc::{string::String, vec, vec::Vec};

use syntax_lang::{Element, Node};

use crate::{
    Error, Grammar,
    check::{Def, Index, is_hidden},
    rule::Expr,
    text::indent,
};

/// How a name is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Show {
    /// Left out, with everything beneath it.
    Skip,
    /// Left out, its children shown in its place.
    Splice,
    /// `(name)`, with nothing beneath it shown.
    Leaf,
    /// `(name ...)` around its children.
    Branch,
    /// `(name ...)` around only the extras among its children: the rule
    /// names no other node, so nothing else beneath it can be one.
    Lexical,
}

/// How each name of a grammar is shown.
struct Classes<'g> {
    index: Index<'g>,
    /// Indexed like `Grammar::rules`.
    rules: Vec<Show>,
    /// Names given by `alias`, sorted.
    aliases: Vec<&'g str>,
    /// Names of the rules and externals the extras name, sorted.
    extras: Vec<&'g str>,
}

impl<'g> Classes<'g> {
    fn new(grammar: &'g Grammar) -> Classes<'g> {
        let index = Index::new(grammar);
        let mut rules: Vec<Show> = grammar
            .rules
            .iter()
            .map(|(name, body)| {
                let single = body.is_single_token();
                match (is_hidden(name), single) {
                    (true, true) => Show::Skip,
                    (true, false) => Show::Splice,
                    (false, true) => Show::Leaf,
                    (false, false) if body.names_nodes() => Show::Branch,
                    (false, false) => Show::Lexical,
                }
            })
            .collect();
        // Inlined rules and supertypes never appear in the tree either.
        for name in grammar.inline.iter().chain(&grammar.supertypes) {
            if let Some(Def::Rule(i)) = index.get(name) {
                rules[i] = match rules[i] {
                    Show::Leaf | Show::Skip => Show::Skip,
                    Show::Branch | Show::Lexical | Show::Splice => Show::Splice,
                };
            }
        }

        let mut aliases = Vec::new();
        let mut walk: Vec<&'g crate::Rule> = grammar.rules.iter().map(|(_, body)| body).collect();
        walk.extend(grammar.extras.iter().flatten());
        while let Some(rule) = walk.pop() {
            if let Expr::Alias(name, _) = &rule.0 {
                aliases.push(&**name);
            }
            walk.extend(rule.children());
        }
        aliases.sort_unstable();
        aliases.dedup();

        let mut extras: Vec<&'g str> = grammar
            .extras
            .iter()
            .flatten()
            .filter_map(|extra| match &extra.0 {
                Expr::Symbol(name) => Some(&**name),
                _ => None,
            })
            .collect();
        extras.sort_unstable();

        Classes {
            index,
            rules,
            aliases,
            extras,
        }
    }

    fn is_alias(&self, name: &str) -> bool {
        self.aliases.binary_search(&name).is_ok()
    }

    /// Whether a child of a node that names no other node can still appear:
    /// only an extra can.
    fn is_extra(&self, name: &str) -> bool {
        self.extras.binary_search(&name).is_ok()
    }

    /// How a node of the tree called `name` is shown.
    fn node(&self, name: &str) -> Result<Show, Error> {
        match self.index.get(name) {
            Some(Def::Rule(i)) => Ok(self.rules[i]),
            Some(Def::External) => Ok(visible_or(name, Show::Leaf, Show::Skip)),
            None if self.is_alias(name) => Ok(visible_or(name, Show::Branch, Show::Splice)),
            None => Err(Error::UnknownNode {
                name: String::from(name),
            }),
        }
    }

    /// How a token of the tree called `name` is shown. Tokens never have
    /// children, so a rule that would be a branch is a leaf here.
    fn token(&self, name: &str) -> Show {
        match self.index.get(name) {
            Some(Def::Rule(i)) => match self.rules[i] {
                Show::Leaf | Show::Branch | Show::Lexical => Show::Leaf,
                Show::Skip | Show::Splice => Show::Skip,
            },
            Some(Def::External) => visible_or(name, Show::Leaf, Show::Skip),
            None if self.is_alias(name) => visible_or(name, Show::Leaf, Show::Skip),
            None => Show::Skip,
        }
    }
}

/// `visible` for a visible name, `hidden` for one starting with `_`.
fn visible_or(name: &str, visible: Show, hidden: Show) -> Show {
    if is_hidden(name) { hidden } else { visible }
}

/// Writes nodes one per line, indented by depth, closing parentheses on the
/// last line of each node.
struct Writer<'o> {
    out: &'o mut String,
    first: bool,
}

impl Writer<'_> {
    fn open(&mut self, name: &str, depth: usize) {
        if !self.first {
            self.out.push('\n');
            indent(self.out, depth);
        }
        self.first = false;
        self.out.push('(');
        self.out.push_str(name);
    }

    fn leaf(&mut self, name: &str, depth: usize) {
        self.open(name, depth);
        self.out.push(')');
    }
}

/// The remaining children of a node being rendered.
struct Frame<I> {
    children: I,
    /// The depth the children print at.
    depth: usize,
    /// Whether the node printed an opening parenthesis to close.
    close: bool,
    /// Whether only extras among the children can be shown.
    extras_only: bool,
}

/// Appends the S-expression for `tree`. On error the caller discards what
/// was written.
pub(crate) fn write<'n, K>(
    grammar: &Grammar,
    tree: &Node<K>,
    name: &impl Fn(&K) -> &'n str,
    out: &mut String,
) -> Result<(), Error> {
    let classes = Classes::new(grammar);
    let mut w = Writer { out, first: true };

    let root_name = name(tree.kind());
    let frame = |depth, close, extras_only| Frame {
        children: tree.children(),
        depth,
        close,
        extras_only,
    };
    let mut stack = match classes.node(root_name)? {
        Show::Skip => return Ok(()),
        Show::Leaf => {
            w.leaf(root_name, 0);
            return Ok(());
        }
        Show::Branch | Show::Lexical => {
            w.open(root_name, 0);
            let extras_only = classes.node(root_name)? == Show::Lexical;
            vec![frame(1, true, extras_only)]
        }
        Show::Splice => vec![frame(0, false, false)],
    };

    while let Some(top) = stack.last_mut() {
        let (depth, extras_only) = (top.depth, top.extras_only);
        let Some(child) = top.children.next() else {
            if let Some(Frame { close: true, .. }) = stack.pop() {
                w.out.push(')');
            }
            continue;
        };
        match child {
            Element::Token(token) => {
                let label = name(token.kind());
                if (!extras_only || classes.is_extra(label)) && classes.token(label) == Show::Leaf {
                    w.leaf(label, depth);
                }
            }
            Element::Node(node) => {
                let label = name(node.kind());
                if extras_only && !classes.is_extra(label) {
                    continue;
                }
                let show = classes.node(label)?;
                let mut push = |depth, close, extras_only| {
                    stack.push(Frame {
                        children: node.children(),
                        depth,
                        close,
                        extras_only,
                    });
                };
                match show {
                    Show::Skip => {}
                    Show::Leaf => w.leaf(label, depth),
                    Show::Branch | Show::Lexical => {
                        w.open(label, depth);
                        push(depth + 1, true, show == Show::Lexical);
                    }
                    Show::Splice => push(depth, false, false),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use syntax_lang::{Span, Token};

    use super::*;
    use crate::Rule;

    fn tok(kind: &'static str, at: u32) -> Element<&'static str> {
        Element::Token(Token::new(kind, Span::new(at, at + 1)))
    }

    fn node(kind: &'static str, children: Vec<Element<&'static str>>) -> Element<&'static str> {
        Element::Node(Node::new(kind, children))
    }

    fn render(grammar: &Grammar, tree: &Node<&'static str>) -> Result<String, Error> {
        let mut out = String::new();
        write(grammar, tree, &|k: &&'static str| *k, &mut out)?;
        Ok(out)
    }

    fn grammar() -> Grammar {
        Grammar::new("g")
            .extra(Rule::pattern(r"\s"))
            .extra(Rule::symbol("comment"))
            .rule("file", Rule::repeat(Rule::symbol("_item")))
            .rule(
                "_item",
                Rule::choice([Rule::symbol("call"), Rule::symbol("name")]),
            )
            .rule(
                "call",
                Rule::seq([Rule::symbol("name"), Rule::symbol("args")]),
            )
            .rule("args", Rule::seq([Rule::string("("), Rule::string(")")]))
            .rule("name", Rule::pattern("[a-z]+"))
            .rule(
                "string",
                Rule::token(Rule::seq([Rule::string("\""), Rule::string("\"")])),
            )
            .rule("number", Rule::prec(1, Rule::pattern(r"\d+")))
            .rule("_newline", Rule::string("\n"))
            .rule("_group", Rule::seq([Rule::string("("), Rule::string(")")]))
            .rule(
                "_value",
                Rule::choice([Rule::symbol("name"), Rule::symbol("number")]),
            )
            .rule(
                "value",
                Rule::choice([Rule::symbol("name"), Rule::symbol("number")]),
            )
            .rule("label", Rule::alias(Rule::symbol("name"), "tag"))
            .rule(
                "comment",
                Rule::token(Rule::seq([Rule::string("#"), Rule::pattern(".*")])),
            )
            .inline("_item")
            .supertype("value")
            .external("indent")
            .external("_dedent")
    }

    #[test]
    fn test_named_nodes_and_tokens_show_anonymous_tokens_do_not() {
        let tree = Node::new(
            "file",
            vec![node(
                "call",
                vec![tok("name", 0), node("args", vec![tok("(", 1), tok(")", 2)])],
            )],
        );
        assert_eq!(
            render(&grammar(), &tree),
            Ok(String::from("(file\n  (call\n    (name)\n    (args)))"))
        );
    }

    #[test]
    fn test_hidden_inlined_and_supertype_nodes_are_spliced() {
        let tree = Node::new(
            "file",
            vec![
                node("_item", vec![tok("name", 0)]),
                node("_group", vec![tok("name", 1)]),
                node("value", vec![tok("number", 2)]),
                tok("_newline", 3),
            ],
        );
        assert_eq!(
            render(&grammar(), &tree),
            Ok(String::from("(file\n  (name)\n  (name)\n  (number))"))
        );
    }

    #[test]
    fn test_single_token_rules_hide_their_structure() {
        let tree = Node::new(
            "file",
            vec![
                node("string", vec![tok("\"", 0), tok("name", 1), tok("\"", 2)]),
                node("number", vec![tok("name", 3)]),
            ],
        );
        assert_eq!(
            render(&grammar(), &tree),
            Ok(String::from("(file\n  (string)\n  (number))"))
        );
    }

    #[test]
    fn test_rules_naming_no_nodes_show_only_extras() {
        // `args` names no other node: a named child inside it cannot come
        // from the grammar, but a comment can.
        let tree = Node::new(
            "file",
            vec![node(
                "args",
                vec![tok("(", 0), tok("name", 1), tok("comment", 2), tok(")", 3)],
            )],
        );
        assert_eq!(
            render(&grammar(), &tree),
            Ok(String::from("(file\n  (args\n    (comment)))"))
        );
    }

    #[test]
    fn test_alias_names_are_visible_nodes() {
        let tree = Node::new(
            "file",
            vec![
                node("label", vec![tok("tag", 0)]),
                node("tag", vec![tok("name", 1)]),
            ],
        );
        assert_eq!(
            render(&grammar(), &tree),
            Ok(String::from(
                "(file\n  (label\n    (tag))\n  (tag\n    (name)))"
            ))
        );
    }

    #[test]
    fn test_externals_show_unless_hidden() {
        let tree = Node::new("file", vec![tok("indent", 0), tok("_dedent", 1)]);
        assert_eq!(
            render(&grammar(), &tree),
            Ok(String::from("(file\n  (indent))"))
        );
    }

    #[test]
    fn test_unknown_node_is_an_error() {
        let tree = Node::new("file", vec![node("mystery", vec![])]);
        assert_eq!(
            render(&grammar(), &tree),
            Err(Error::UnknownNode {
                name: String::from("mystery")
            })
        );
        let tree = Node::new("nope", vec![]);
        assert!(render(&grammar(), &tree).is_err());
    }

    #[test]
    fn test_empty_and_spliced_roots() {
        assert_eq!(
            render(&grammar(), &Node::new("file", vec![])),
            Ok(String::from("(file)"))
        );
        let tree = Node::new("_group", vec![tok("name", 0), tok("name", 1)]);
        assert_eq!(
            render(&grammar(), &tree),
            Ok(String::from("(name)\n(name)"))
        );
        assert_eq!(
            render(&grammar(), &Node::new("name", vec![])),
            Ok(String::from("(name)"))
        );
    }

    #[test]
    fn test_deep_tree_is_iterative() {
        let mut inner = Node::new("call", vec![tok("name", 0)]);
        for _ in 0..50_000 {
            inner = Node::new("_item", vec![Element::Node(inner)]);
        }
        let tree = Node::new("file", vec![Element::Node(inner)]);
        assert_eq!(
            render(&grammar(), &tree),
            Ok(String::from("(file\n  (call\n    (name)))"))
        );
        // `syntax_lang::Node` drops iteratively, so freeing it is safe too.
        drop(tree);
    }
}
