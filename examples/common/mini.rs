//! `mini`, a small expression language used by the examples, described twice:
//! once as a tree-sitter grammar built with `treesitter_lang`, and once as a
//! hand-written lexer and parser that build `syntax_lang` trees — the way a
//! real language in the `-lang` family is set up.
//!
//! ```text
//! # A comment.
//! let width = 3 * (left + right);
//! area(width, 2) / 4;
//! ```
//!
//! Every node kind of the parser is a rule of the grammar with the same name,
//! so `Grammar::sexp` can render the parser's trees as tree-sitter would and
//! `tree-sitter test` can hold the generated parser to the hand-written one.

#![allow(dead_code)] // Each example uses a different part of this module.

use treesitter_lang::syntax_lang::{Element, Node, Span, Token, TokenKind};
use treesitter_lang::{Grammar, Rule};

/// Every node and token kind of `mini`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    // Nodes.
    SourceFile,
    LetStatement,
    ExpressionStatement,
    BinaryExpression,
    CallExpression,
    Arguments,
    ParenthesizedExpression,
    // Named tokens.
    Identifier,
    Number,
    Comment,
    // Anonymous tokens: keywords and punctuation.
    Let,
    Eq,
    Semicolon,
    Comma,
    LParen,
    RParen,
    Plus,
    Minus,
    Star,
    Slash,
    // Trivia and errors.
    Whitespace,
    Unknown,
}

impl Kind {
    /// The kind's tree-sitter name. Nodes and named tokens use their rule
    /// names; anonymous tokens use their text, which is not a rule, so
    /// `Grammar::sexp` leaves them out exactly as tree-sitter does.
    pub fn name(&self) -> &'static str {
        match self {
            Kind::SourceFile => "source_file",
            Kind::LetStatement => "let_statement",
            Kind::ExpressionStatement => "expression_statement",
            Kind::BinaryExpression => "binary_expression",
            Kind::CallExpression => "call_expression",
            Kind::Arguments => "arguments",
            Kind::ParenthesizedExpression => "parenthesized_expression",
            Kind::Identifier => "identifier",
            Kind::Number => "number",
            Kind::Comment => "comment",
            Kind::Let => "let",
            Kind::Eq => "=",
            Kind::Semicolon => ";",
            Kind::Comma => ",",
            Kind::LParen => "(",
            Kind::RParen => ")",
            Kind::Plus => "+",
            Kind::Minus => "-",
            Kind::Star => "*",
            Kind::Slash => "/",
            Kind::Whitespace => "whitespace",
            Kind::Unknown => "unknown",
        }
    }
}

impl TokenKind for Kind {
    fn is_trivia(&self) -> bool {
        matches!(self, Kind::Whitespace | Kind::Comment)
    }
}

/// Binary operators and their binding power, loosest first.
const OPERATORS: [(&str, i32); 4] = [("+", 1), ("-", 1), ("*", 2), ("/", 2)];

/// The tree-sitter grammar for `mini`.
pub fn grammar() -> Grammar {
    let expr = || Rule::symbol("_expression");

    // One `prec.left` alternative per binding power, built from the same
    // operator table the hand-written parser uses.
    let mut levels: Vec<i32> = OPERATORS.iter().map(|&(_, power)| power).collect();
    levels.dedup();
    let binary = Rule::choice(levels.into_iter().map(|power| {
        let ops = OPERATORS
            .iter()
            .filter(|&&(_, p)| p == power)
            .map(|&(op, _)| Rule::string(op));
        Rule::prec_left(
            power,
            Rule::seq([
                Rule::field("left", expr()),
                Rule::field("operator", Rule::choice(ops)),
                Rule::field("right", expr()),
            ]),
        )
    }));

    Grammar::new("mini")
        .extra(Rule::pattern(r"\s"))
        .extra(Rule::symbol("comment"))
        .supertype("_expression")
        .word("identifier")
        .rule("source_file", Rule::repeat(Rule::symbol("_statement")))
        .rule(
            "_statement",
            Rule::choice([
                Rule::symbol("let_statement"),
                Rule::symbol("expression_statement"),
            ]),
        )
        .rule(
            "let_statement",
            Rule::seq([
                Rule::string("let"),
                Rule::field("name", Rule::symbol("identifier")),
                Rule::string("="),
                Rule::field("value", expr()),
                Rule::string(";"),
            ]),
        )
        .rule(
            "expression_statement",
            Rule::seq([expr(), Rule::string(";")]),
        )
        .rule(
            "_expression",
            Rule::choice([
                Rule::symbol("identifier"),
                Rule::symbol("number"),
                Rule::symbol("binary_expression"),
                Rule::symbol("call_expression"),
                Rule::symbol("parenthesized_expression"),
            ]),
        )
        .rule("binary_expression", binary)
        .rule(
            "call_expression",
            Rule::seq([
                Rule::field("function", Rule::symbol("identifier")),
                Rule::field("arguments", Rule::symbol("arguments")),
            ]),
        )
        .rule(
            "arguments",
            Rule::seq([
                Rule::string("("),
                Rule::optional(Rule::seq([
                    expr(),
                    Rule::repeat(Rule::seq([Rule::string(","), expr()])),
                ])),
                Rule::string(")"),
            ]),
        )
        .rule(
            "parenthesized_expression",
            Rule::seq([Rule::string("("), expr(), Rule::string(")")]),
        )
        .rule("identifier", Rule::pattern("[a-z_][a-z0-9_]*"))
        .rule("number", Rule::pattern(r"\d+"))
        .rule(
            "comment",
            Rule::token(Rule::seq([Rule::string("#"), Rule::pattern(".*")])),
        )
}

/// Splits `source` into tokens, trivia included, covering every byte.
pub fn lex(source: &str) -> Vec<Token<Kind>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let take = |at: &mut usize, keep: fn(u8) -> bool| {
            while *at < bytes.len() && keep(bytes[*at]) {
                *at += 1;
            }
        };
        let kind = match bytes[at] {
            b if b.is_ascii_whitespace() => {
                take(&mut at, |b| b.is_ascii_whitespace());
                Kind::Whitespace
            }
            b'#' => {
                take(&mut at, |b| b != b'\n');
                Kind::Comment
            }
            b if b.is_ascii_digit() => {
                take(&mut at, |b| b.is_ascii_digit());
                Kind::Number
            }
            b if b.is_ascii_lowercase() || b == b'_' => {
                take(&mut at, |b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'
                });
                if &source[start..at] == "let" {
                    Kind::Let
                } else {
                    Kind::Identifier
                }
            }
            b => {
                at += 1;
                match b {
                    b'=' => Kind::Eq,
                    b';' => Kind::Semicolon,
                    b',' => Kind::Comma,
                    b'(' => Kind::LParen,
                    b')' => Kind::RParen,
                    b'+' => Kind::Plus,
                    b'-' => Kind::Minus,
                    b'*' => Kind::Star,
                    b'/' => Kind::Slash,
                    _ => {
                        // Keep the token on a character boundary.
                        while !source.is_char_boundary(at) {
                            at += 1;
                        }
                        Kind::Unknown
                    }
                }
            }
        };
        tokens.push(Token::new(kind, Span::new(start as u32, at as u32)));
    }
    tokens
}

/// Parses `source` into a lossless tree, or describes the first syntax error.
pub fn parse(source: &str) -> Result<Node<Kind>, String> {
    let mut parser = Parser {
        source,
        tokens: lex(source),
        at: 0,
    };
    let mut children = Vec::new();
    while parser.peek().is_some() {
        parser.statement(&mut children)?;
    }
    parser.flush(&mut children);
    Ok(Node::new(Kind::SourceFile, children))
}

/// A recursive-descent parser with a Pratt loop for binary operators. Trees
/// are built bottom-up, so an operator can wrap an operand already parsed.
///
/// Trivia is placed the way tree-sitter places extras: it is held back until
/// the next significant token is consumed, then goes to whichever node that
/// token belongs to. A node therefore never begins or ends with trivia, and
/// a comment between two statements belongs to the file, not a statement.
struct Parser<'s> {
    source: &'s str,
    tokens: Vec<Token<Kind>>,
    /// The next token not yet placed in the tree, trivia included.
    at: usize,
}

impl Parser<'_> {
    /// The next significant token, without consuming anything.
    fn peek(&self) -> Option<Token<Kind>> {
        self.tokens[self.at..]
            .iter()
            .copied()
            .find(|t| !t.is_trivia())
    }

    /// Places held-back trivia into `out`.
    fn flush(&mut self, out: &mut Vec<Element<Kind>>) {
        while let Some(&token) = self.tokens.get(self.at).filter(|t| t.is_trivia()) {
            out.push(Element::Token(token));
            self.at += 1;
        }
    }

    /// Consumes the next significant token into `out`, which must be `kind`.
    fn expect(&mut self, out: &mut Vec<Element<Kind>>, kind: Kind) -> Result<Token<Kind>, String> {
        self.flush(out);
        match self.tokens.get(self.at) {
            Some(&token) if token.kind == kind => {
                out.push(Element::Token(token));
                self.at += 1;
                Ok(token)
            }
            other => Err(self.error(other.copied(), kind.name())),
        }
    }

    fn error(&self, found: Option<Token<Kind>>, wanted: &str) -> String {
        match found {
            Some(token) => {
                let span = token.span;
                let text = &self.source[span.start().to_usize()..span.end().to_usize()];
                format!("expected {wanted}, found `{text}` at byte {}", span.start())
            }
            None => format!("expected {wanted}, found the end of the input"),
        }
    }

    fn statement(&mut self, out: &mut Vec<Element<Kind>>) -> Result<(), String> {
        self.flush(out);
        let mut children = Vec::new();
        let kind = if self.peek().map(|t| t.kind) == Some(Kind::Let) {
            self.expect(&mut children, Kind::Let)?;
            self.expect(&mut children, Kind::Identifier)?;
            self.expect(&mut children, Kind::Eq)?;
            self.expression(&mut children, 0)?;
            Kind::LetStatement
        } else {
            self.expression(&mut children, 0)?;
            Kind::ExpressionStatement
        };
        self.expect(&mut children, Kind::Semicolon)?;
        out.push(Element::Node(Node::new(kind, children)));
        Ok(())
    }

    /// Parses an expression whose operators bind at least `min_power`.
    fn expression(&mut self, out: &mut Vec<Element<Kind>>, min_power: i32) -> Result<(), String> {
        self.flush(out);
        let mut left = self.primary()?;
        while let Some(power) = self.peek().and_then(|t| power(t.kind)) {
            if power < min_power {
                break;
            }
            let mut children = vec![left];
            self.flush(&mut children);
            children.push(Element::Token(self.tokens[self.at]));
            self.at += 1;
            // Left-associative: the right operand binds strictly tighter.
            self.expression(&mut children, power + 1)?;
            left = Element::Node(Node::new(Kind::BinaryExpression, children));
        }
        out.push(left);
        Ok(())
    }

    fn primary(&mut self) -> Result<Element<Kind>, String> {
        let token = self.peek();
        match token.map(|t| t.kind) {
            Some(Kind::Number) => {
                self.at += 1;
                Ok(Element::Token(self.tokens[self.at - 1]))
            }
            Some(Kind::Identifier) => {
                let name = Element::Token(self.tokens[self.at]);
                self.at += 1;
                if self.peek().map(|t| t.kind) != Some(Kind::LParen) {
                    return Ok(name);
                }
                let mut children = vec![name];
                self.arguments(&mut children)?;
                Ok(Element::Node(Node::new(Kind::CallExpression, children)))
            }
            Some(Kind::LParen) => {
                let mut children = Vec::new();
                self.expect(&mut children, Kind::LParen)?;
                self.expression(&mut children, 0)?;
                self.expect(&mut children, Kind::RParen)?;
                Ok(Element::Node(Node::new(
                    Kind::ParenthesizedExpression,
                    children,
                )))
            }
            _ => Err(self.error(token, "an expression")),
        }
    }

    fn arguments(&mut self, out: &mut Vec<Element<Kind>>) -> Result<(), String> {
        self.flush(out);
        let mut children = Vec::new();
        self.expect(&mut children, Kind::LParen)?;
        if self.peek().map(|t| t.kind) != Some(Kind::RParen) {
            self.expression(&mut children, 0)?;
            while self.peek().map(|t| t.kind) == Some(Kind::Comma) {
                self.expect(&mut children, Kind::Comma)?;
                self.expression(&mut children, 0)?;
            }
        }
        self.expect(&mut children, Kind::RParen)?;
        out.push(Element::Node(Node::new(Kind::Arguments, children)));
        Ok(())
    }
}

/// The binding power of a binary operator token, from [`OPERATORS`].
fn power(kind: Kind) -> Option<i32> {
    let text = match kind {
        Kind::Plus | Kind::Minus | Kind::Star | Kind::Slash => kind.name(),
        _ => return None,
    };
    OPERATORS
        .iter()
        .find(|&&(op, _)| op == text)
        .map(|&(_, p)| p)
}
