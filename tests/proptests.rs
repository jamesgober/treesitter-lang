//! Property tests.
//!
//! - Both output formats round-trip: a random rule tree, emitted as
//!   `grammar.js` and as `grammar.json` and read back by independent parsers
//!   written here, is the tree that went in — whatever characters its strings
//!   contain.
//! - A pattern either fails validation or stays inside its own regular
//!   expression literal: for arbitrary pattern text next to a hostile
//!   string, `grammar.js` read by a JavaScript-correct scanner holds exactly
//!   one literal per pattern, equal to the `grammar.json` value, the string
//!   intact after it, and nothing else (ISSUES H06).
//! - The empty-string check agrees with a direct recursive definition of
//!   tree-sitter's rule.
//! - S-expressions agree with a direct recursive renderer, and are indented
//!   two spaces per open parenthesis.

use proptest::prelude::*;
use treesitter_lang::syntax_lang::{Element, Node, Span, Token};
use treesitter_lang::{Error, Grammar, Rule};

// --- A model of rules -------------------------------------------------------

/// A rule tree the tests can build, compare, and inspect.
#[derive(Clone, Debug, PartialEq)]
enum M {
    Blank,
    Str(String),
    Pat(String),
    Sym(String),
    Seq(Vec<M>),
    Choice(Vec<M>),
    Repeat(Box<M>),
    Repeat1(Box<M>),
    Field(String, Box<M>),
    Alias(String, Box<M>),
    Token(Box<M>),
    Immediate(Box<M>),
    Prec(&'static str, i32, Box<M>),
}

fn to_rule(m: &M) -> Rule {
    match m {
        M::Blank => Rule::blank(),
        M::Str(s) => Rule::string(s.clone()),
        M::Pat(p) => Rule::pattern(p.clone()),
        M::Sym(s) => Rule::symbol(s.clone()),
        M::Seq(ms) => Rule::seq(ms.iter().map(to_rule)),
        M::Choice(ms) => Rule::choice(ms.iter().map(to_rule)),
        M::Repeat(m) => Rule::repeat(to_rule(m)),
        M::Repeat1(m) => Rule::repeat1(to_rule(m)),
        M::Field(n, m) => Rule::field(n.clone(), to_rule(m)),
        M::Alias(n, m) => Rule::alias(to_rule(m), n.clone()),
        M::Token(m) => Rule::token(to_rule(m)),
        M::Immediate(m) => Rule::immediate(to_rule(m)),
        M::Prec(kind, n, m) => {
            let f = match *kind {
                "PREC" => Rule::prec,
                "PREC_LEFT" => Rule::prec_left,
                "PREC_RIGHT" => Rule::prec_right,
                _ => Rule::prec_dynamic,
            };
            f(*n, to_rule(m))
        }
    }
}

/// Replaces symbols inside tokens, which validation refuses, with strings.
fn lexical_tokens(m: M, in_token: bool) -> M {
    let b = |m: Box<M>, t| Box::new(lexical_tokens(*m, t));
    let v = |ms: Vec<M>, t| ms.into_iter().map(|m| lexical_tokens(m, t)).collect();
    match m {
        M::Sym(_) if in_token => M::Str("s".into()),
        M::Seq(ms) => M::Seq(v(ms, in_token)),
        M::Choice(ms) => M::Choice(v(ms, in_token)),
        M::Repeat(m) => M::Repeat(b(m, in_token)),
        M::Repeat1(m) => M::Repeat1(b(m, in_token)),
        M::Field(n, m) => M::Field(n, b(m, in_token)),
        M::Alias(n, m) => M::Alias(n, b(m, in_token)),
        M::Token(m) => M::Token(b(m, true)),
        M::Immediate(m) => M::Immediate(b(m, true)),
        M::Prec(k, n, m) => M::Prec(k, n, b(m, in_token)),
        leaf => leaf,
    }
}

/// Tree-sitter's empty-production rule, stated directly.
fn matches_empty(m: &M) -> bool {
    match m {
        M::Blank | M::Repeat(_) => true,
        M::Str(_) | M::Pat(_) | M::Sym(_) | M::Token(_) | M::Immediate(_) => false,
        M::Seq(ms) => ms.iter().all(matches_empty),
        M::Choice(ms) => ms.iter().any(matches_empty),
        M::Repeat1(m) | M::Field(_, m) | M::Alias(_, m) | M::Prec(_, _, m) => matches_empty(m),
    }
}

/// Whether anything a repetition repeats, outside tokens, can be empty.
fn empty_repeat(m: &M, in_token: bool) -> bool {
    match m {
        M::Repeat(c) | M::Repeat1(c) if !in_token && matches_empty(c) => true,
        M::Seq(ms) | M::Choice(ms) => ms.iter().any(|m| empty_repeat(m, in_token)),
        M::Token(c) | M::Immediate(c) => empty_repeat(c, true),
        M::Repeat(c) | M::Repeat1(c) | M::Field(_, c) | M::Alias(_, c) | M::Prec(_, _, c) => {
            empty_repeat(c, in_token)
        }
        _ => false,
    }
}

/// Makes every repetition outside tokens repeat something non-empty, as
/// validation requires, by appending a string to what it repeats.
fn productive_repeats(m: M, in_token: bool) -> M {
    let b = |m: Box<M>, t| Box::new(productive_repeats(*m, t));
    let v = |ms: Vec<M>, t| ms.into_iter().map(|m| productive_repeats(m, t)).collect();
    let fix = |c: Box<M>| {
        let c = productive_repeats(*c, in_token);
        Box::new(if !in_token && matches_empty(&c) {
            M::Seq(vec![c, M::Str("r".into())])
        } else {
            c
        })
    };
    match m {
        M::Repeat(c) => M::Repeat(fix(c)),
        M::Repeat1(c) => M::Repeat1(fix(c)),
        M::Seq(ms) => M::Seq(v(ms, in_token)),
        M::Choice(ms) => M::Choice(v(ms, in_token)),
        M::Field(n, c) => M::Field(n, b(c, in_token)),
        M::Alias(n, c) => M::Alias(n, b(c, in_token)),
        M::Token(c) => M::Token(b(c, true)),
        M::Immediate(c) => M::Immediate(b(c, true)),
        M::Prec(k, n, c) => M::Prec(k, n, b(c, in_token)),
        leaf => leaf,
    }
}

// --- Strategies -------------------------------------------------------------

/// Text that stresses both escapers: quotes, backslashes, control
/// characters, line separators, and arbitrary Unicode.
fn text() -> impl Strategy<Value = String> {
    let ch = prop_oneof![
        prop::sample::select(vec![
            '\'', '"', '\\', '/', '\n', '\r', '\t', '\0', '\u{1b}', '\u{7f}', '\u{2028}',
            '\u{2029}', 'é', '→', '😀', 'a', ' ',
        ]),
        any::<char>(),
    ];
    prop::collection::vec(ch, 1..8).prop_map(String::from_iter)
}

/// A regular expression both JavaScript and tree-sitter accept, with no
/// characters `grammar.js` rewrites, so it validates and reads back
/// unchanged from both formats.
fn pattern() -> impl Strategy<Value = String> {
    let atom = prop::sample::select(vec![
        "a", "z", "0", "é", "_", " ", ".", "'", "\"", "]", "}", ",", r"\d", r"\w", r"\s", r"\.",
        r"\(", r"\)", r"\[", r"\]", r"\{", r"\}", r"\\", r"\*", r"\|", r"\-", r"\'", r"\u0041",
        r"\x41", r"\p{L}",
    ])
    .prop_map(String::from);
    let item = prop::sample::select(vec![
        "a", "z", "0", "é", "_", " ", ".", "$", "'", "\"", "a-z", "0-9", r"\]", r"\[", r"\\",
        r"\-", r"\d", r"\w",
    ]);
    let class = (any::<bool>(), prop::collection::vec(item, 1..4)).prop_map(|(negated, items)| {
        format!("[{}{}]", if negated { "^" } else { "" }, items.concat())
    });
    let quantifier =
        prop::sample::select(vec!["", "", "*", "+", "?", "*?", "{2}", "{1,3}", "{2,}"]);
    let piece = (prop_oneof![atom, class], quantifier.clone()).prop_map(|(a, q)| a + q);
    let leaf = prop::collection::vec(piece, 1..4).prop_map(|pieces| pieces.concat());
    leaf.prop_recursive(3, 24, 3, move |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 2..4).prop_map(|parts| parts.concat()),
            prop::collection::vec(inner.clone(), 2..4)
                .prop_map(|alternatives| format!("(?:{})", alternatives.join("|"))),
            (inner.clone(), quantifier.clone()).prop_map(|(r, q)| format!("({r}){q}")),
            (inner, quantifier.clone()).prop_map(|(r, q)| format!("(?:{r}){q}")),
        ]
    })
}

/// Pattern text with no regard for validity: brackets, slashes, escapes,
/// quantifiers, quotes, and line breaks, plus arbitrary characters.
fn hostile_pattern() -> impl Strategy<Value = String> {
    let ch = prop_oneof![
        4 => prop::sample::select(vec![
            '[', ']', '\\', '/', '*', '(', ')', '{', '}', '\'', '"', '\n', '\u{2028}', 'a', '^',
            '-', '|', '?', '<', '>', ',',
        ]),
        1 => any::<char>(),
    ];
    prop::collection::vec(ch, 0..10).prop_map(String::from_iter)
}

/// Strings that would run as code if a pattern's literal swallowed the text
/// in front of them.
fn payload() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::from(r#"]/,require("fs").rmSync("x")),//"#)),
        Just(String::from("*/,process.exit(1),/*")),
        Just(String::from("]/;throw 1;//")),
        Just(String::from("'+(()=>{throw 1})()+'")),
        text(),
    ]
}

fn ident() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,6}"
}

fn prec_kind() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec!["PREC", "PREC_LEFT", "PREC_RIGHT", "PREC_DYNAMIC"])
}

/// A rule tree whose symbols name `leaf_a` and `leaf_b`, with no symbol
/// inside a token; a repetition may still repeat something empty.
fn model_raw() -> impl Strategy<Value = M> {
    let leaf = prop_oneof![
        Just(M::Blank),
        text().prop_map(M::Str),
        pattern().prop_map(M::Pat),
        prop::sample::select(vec!["leaf_a", "leaf_b"]).prop_map(|s| M::Sym(s.into())),
    ];
    leaf.prop_recursive(5, 48, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..4).prop_map(M::Seq),
            prop::collection::vec(inner.clone(), 1..4).prop_map(M::Choice),
            inner.clone().prop_map(|m| M::Choice(vec![m, M::Blank])),
            inner.clone().prop_map(|m| M::Repeat(Box::new(m))),
            inner.clone().prop_map(|m| M::Repeat1(Box::new(m))),
            (ident(), inner.clone()).prop_map(|(n, m)| M::Field(n, Box::new(m))),
            (ident(), inner.clone()).prop_map(|(n, m)| M::Alias(n, Box::new(m))),
            inner.clone().prop_map(|m| M::Token(Box::new(m))),
            inner.clone().prop_map(|m| M::Immediate(Box::new(m))),
            (prec_kind(), any::<i32>(), inner).prop_map(|(k, n, m)| M::Prec(k, n, Box::new(m))),
        ]
    })
    .prop_map(|m| lexical_tokens(m, false))
}

/// A rule tree that validation accepts as the start rule.
fn model() -> impl Strategy<Value = M> {
    model_raw().prop_map(|m| productive_repeats(m, false))
}

fn grammar_with(start: &M) -> Grammar {
    Grammar::new("prop")
        .rule("start", to_rule(start))
        .rule("leaf_a", Rule::pattern("[a-z]+"))
        .rule("leaf_b", Rule::string("b"))
}

// --- Reading grammar.json back ----------------------------------------------

#[derive(Debug)]
enum Json {
    Str(String),
    Num(i64),
    Bool(bool),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> &Json {
        match self {
            Json::Obj(fields) => &fields.iter().find(|(k, _)| k == key).unwrap().1,
            _ => panic!("not an object"),
        }
    }
    fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            _ => panic!("not a string"),
        }
    }
}

struct Reader<'a> {
    s: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn ws(&mut self) {
        while self.at < self.s.len() && self.s[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }
    fn eat(&mut self, b: u8) {
        self.ws();
        assert_eq!(self.s[self.at], b, "at {}", self.at);
        self.at += 1;
    }
    fn peek(&mut self) -> u8 {
        self.ws();
        self.s[self.at]
    }
    fn hex4(&mut self) -> u32 {
        let h = std::str::from_utf8(&self.s[self.at..self.at + 4]).unwrap();
        self.at += 4;
        u32::from_str_radix(h, 16).unwrap()
    }

    fn json(&mut self) -> Json {
        match self.peek() {
            b'{' => {
                self.eat(b'{');
                let mut fields = Vec::new();
                while self.peek() != b'}' {
                    if !fields.is_empty() {
                        self.eat(b',');
                    }
                    let Json::Str(key) = self.json() else {
                        panic!("key")
                    };
                    self.eat(b':');
                    fields.push((key, self.json()));
                }
                self.eat(b'}');
                Json::Obj(fields)
            }
            b'[' => {
                self.eat(b'[');
                let mut items = Vec::new();
                while self.peek() != b']' {
                    if !items.is_empty() {
                        self.eat(b',');
                    }
                    items.push(self.json());
                }
                self.eat(b']');
                Json::Arr(items)
            }
            b'"' => {
                self.eat(b'"');
                let mut out = String::new();
                loop {
                    let start = self.at;
                    while self.s[self.at] != b'"' && self.s[self.at] != b'\\' {
                        assert!(self.s[self.at] >= 0x20, "raw control character in JSON");
                        self.at += 1;
                    }
                    out.push_str(std::str::from_utf8(&self.s[start..self.at]).unwrap());
                    let b = self.s[self.at];
                    self.at += 1;
                    if b == b'"' {
                        break;
                    }
                    let e = self.s[self.at];
                    self.at += 1;
                    out.push(match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => char::from_u32(self.hex4()).unwrap(),
                        _ => panic!("bad escape"),
                    });
                }
                Json::Str(out)
            }
            b't' | b'f' => {
                let v = self.s[self.at] == b't';
                self.at += if v { 4 } else { 5 };
                Json::Bool(v)
            }
            _ => {
                let start = self.at;
                self.at += 1;
                while self.at < self.s.len() && self.s[self.at].is_ascii_digit() {
                    self.at += 1;
                }
                let n = std::str::from_utf8(&self.s[start..self.at]).unwrap();
                Json::Num(n.parse().unwrap())
            }
        }
    }
}

fn from_json(j: &Json) -> M {
    let content = || Box::new(from_json(j.get("content")));
    match j.get("type").str() {
        "BLANK" => M::Blank,
        "STRING" => M::Str(j.get("value").str().into()),
        "PATTERN" => M::Pat(j.get("value").str().into()),
        "SYMBOL" => M::Sym(j.get("name").str().into()),
        kind @ ("SEQ" | "CHOICE") => {
            let Json::Arr(items) = j.get("members") else {
                panic!("members")
            };
            let ms = items.iter().map(from_json).collect();
            if kind == "SEQ" {
                M::Seq(ms)
            } else {
                M::Choice(ms)
            }
        }
        "REPEAT" => M::Repeat(content()),
        "REPEAT1" => M::Repeat1(content()),
        "TOKEN" => M::Token(content()),
        "IMMEDIATE_TOKEN" => M::Immediate(content()),
        "FIELD" => M::Field(j.get("name").str().into(), content()),
        "ALIAS" => {
            assert!(matches!(j.get("named"), Json::Bool(true)));
            M::Alias(j.get("value").str().into(), content())
        }
        kind => {
            let Json::Num(n) = j.get("value") else {
                panic!("prec value")
            };
            let kind = ["PREC", "PREC_LEFT", "PREC_RIGHT", "PREC_DYNAMIC"]
                .into_iter()
                .find(|k| *k == kind)
                .unwrap();
            M::Prec(kind, i32::try_from(*n).unwrap(), content())
        }
    }
}

fn start_from_json(json: &str) -> M {
    rule_from_json(json, "start")
}

fn rule_from_json(json: &str, name: &str) -> M {
    let doc = Reader {
        s: json.as_bytes(),
        at: 0,
    }
    .json();
    from_json(doc.get("rules").get(name))
}

// --- Reading grammar.js back ------------------------------------------------

/// Reads the rule expressions of the `grammar.js` DSL.
struct Js<'a> {
    s: &'a str,
    at: usize,
}

impl Js<'_> {
    fn ws(&mut self) {
        while self.s[self.at..].starts_with(|c: char| c.is_ascii_whitespace()) {
            self.at += 1;
        }
    }
    fn eat(&mut self, word: &str) -> bool {
        self.ws();
        if self.s[self.at..].starts_with(word) {
            self.at += word.len();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, word: &str) {
        assert!(
            self.eat(word),
            "expected {word:?} at {:?}",
            &self.s[self.at..]
        );
    }
    fn ident(&mut self) -> String {
        self.ws();
        let rest = &self.s[self.at..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        self.at += len;
        rest[..len].to_string()
    }
    fn int(&mut self) -> i32 {
        self.ws();
        let rest = &self.s[self.at..];
        let len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '-'))
            .unwrap();
        self.at += len;
        rest[..len].parse().unwrap()
    }
    fn string(&mut self) -> String {
        self.expect("'");
        let mut out = String::new();
        let mut chars = self.s[self.at..].char_indices();
        loop {
            let (i, c) = chars.next().unwrap();
            match c {
                '\'' => {
                    self.at += i + 1;
                    return out;
                }
                '\n' | '\r' | '\u{2028}' | '\u{2029}' => panic!("raw line break in a string"),
                '\\' => {
                    let (_, e) = chars.next().unwrap();
                    let mut hex = |n: usize| {
                        let h: String = (0..n).map(|_| chars.next().unwrap().1).collect();
                        char::from_u32(u32::from_str_radix(&h, 16).unwrap()).unwrap()
                    };
                    out.push(match e {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'x' => hex(2),
                        'u' => hex(4),
                        other => other,
                    });
                }
                c => out.push(c),
            }
        }
    }
    /// A regular expression literal, read as a JavaScript lexer reads one:
    /// a backslash escapes the next character, and inside a character class
    /// — from `[` to the next unescaped `]` — a `/` does not end the
    /// literal. Returns the text between the slashes.
    fn regex(&mut self) -> String {
        self.expect("/");
        let mut out = String::new();
        let mut chars = self.s[self.at..].char_indices();
        let mut in_class = false;
        loop {
            let (i, c) = chars
                .next()
                .expect("unterminated regular expression literal");
            match c {
                '/' if !in_class => {
                    self.at += i + 1;
                    // Flags would follow straight after; none are written.
                    assert!(
                        !self.s[self.at..]
                            .starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$'),
                        "regular expression flags"
                    );
                    return out;
                }
                '\n' | '\r' | '\u{2028}' | '\u{2029}' => panic!("raw line break in a regex"),
                '\\' => {
                    out.push('\\');
                    let (_, e) = chars.next().expect("lone backslash");
                    assert!(
                        !matches!(e, '\n' | '\r' | '\u{2028}' | '\u{2029}'),
                        "escaped line break"
                    );
                    out.push(e);
                }
                '[' if !in_class => {
                    in_class = true;
                    out.push(c);
                }
                ']' if in_class => {
                    in_class = false;
                    out.push(c);
                }
                c => out.push(c),
            }
        }
    }
    /// The arguments after an opening parenthesis, up to the closing one.
    fn args(&mut self) -> Vec<M> {
        let mut items = Vec::new();
        while !self.eat(")") {
            items.push(self.expr());
            let _ = self.eat(",");
        }
        items
    }
    fn one(&mut self) -> Box<M> {
        let mut args = self.args();
        assert_eq!(args.len(), 1);
        Box::new(args.remove(0))
    }
    fn expr(&mut self) -> M {
        self.ws();
        let rest = &self.s[self.at..];
        if rest.starts_with('\'') {
            return M::Str(self.string());
        }
        if rest.starts_with('/') {
            return M::Pat(self.regex());
        }
        if self.eat("new RegExp(") {
            let pattern = self.string();
            self.expect(")");
            return M::Pat(pattern);
        }
        if self.eat("$.") {
            return M::Sym(self.ident());
        }
        let head = self.ident();
        let head = if self.eat(".") {
            format!("{head}.{}", self.ident())
        } else {
            head
        };
        self.expect("(");
        match head.as_str() {
            "blank" => {
                self.expect(")");
                M::Blank
            }
            "seq" => M::Seq(self.args()),
            "choice" => M::Choice(self.args()),
            "optional" => M::Choice(vec![*self.one(), M::Blank]),
            "repeat" => M::Repeat(self.one()),
            "repeat1" => M::Repeat1(self.one()),
            "token" => M::Token(self.one()),
            "token.immediate" => M::Immediate(self.one()),
            "field" => {
                let name = self.string();
                self.expect(",");
                M::Field(name, self.one())
            }
            "alias" => {
                let inner = self.expr();
                self.expect(",");
                self.expect("$.");
                let name = self.ident();
                self.expect(")");
                M::Alias(name, Box::new(inner))
            }
            prec => {
                let kind = match prec {
                    "prec" => "PREC",
                    "prec.left" => "PREC_LEFT",
                    "prec.right" => "PREC_RIGHT",
                    "prec.dynamic" => "PREC_DYNAMIC",
                    other => panic!("unknown function {other}"),
                };
                let level = self.int();
                self.expect(",");
                M::Prec(kind, level, self.one())
            }
        }
    }
}

fn start_from_js(js: &str) -> M {
    let at = js.find("    start: $ => ").unwrap() + "    start: $ => ".len();
    let mut reader = Js { s: js, at };
    let m = reader.expr();
    reader.expect(",");
    m
}

/// Every rule of a `grammar.js`, in order, read up to the closing lines of
/// the file, which must follow the last rule and end it.
fn rules_from_js(js: &str) -> Vec<(String, M)> {
    let at = js.find("\n  rules: {\n").unwrap() + "\n  rules: {\n".len();
    let mut reader = Js { s: js, at };
    let mut rules = Vec::new();
    loop {
        reader.ws();
        if reader.s[reader.at..].starts_with("},") {
            assert_eq!(&reader.s[reader.at..], "},\n});\n", "text after the rules");
            return rules;
        }
        let name = reader.ident();
        reader.expect(": $ =>");
        let m = reader.expr();
        reader.expect(",");
        rules.push((name, m));
    }
}

// --- S-expressions ----------------------------------------------------------

/// Names a random tree may use, and how tree-sitter would show each.
const NODE_NAMES: [&str; 11] = [
    "pair", "list", "_hidden", "inl", "word", "_htok", "ext", "_hext", "lex", "sup", "al",
];
const TOKEN_NAMES: [&str; 10] = [
    "word", "pair", "_htok", "ext", "_hext", "punct", "al", "cmt", "sup", "lex",
];

fn sexp_grammar() -> Grammar {
    let item = || Rule::choice([Rule::symbol("pair"), Rule::symbol("word")]);
    Grammar::new("shapes")
        .extra(Rule::pattern(r"\s"))
        .extra(Rule::symbol("cmt"))
        .external("ext")
        .external("_hext")
        .inline("inl")
        .supertype("sup")
        .rule("file", Rule::repeat(item()))
        .rule(
            "pair",
            Rule::seq([
                Rule::string("("),
                item(),
                Rule::optional(Rule::alias(Rule::symbol("word"), "al")),
                Rule::string(")"),
            ]),
        )
        .rule(
            "list",
            Rule::seq([Rule::string("["), Rule::repeat(item()), Rule::string("]")]),
        )
        .rule("_hidden", Rule::seq([Rule::string("{"), item()]))
        .rule("inl", Rule::seq([Rule::string("<"), item()]))
        .rule(
            "sup",
            Rule::choice([Rule::symbol("pair"), Rule::symbol("word")]),
        )
        .rule("lex", Rule::seq([Rule::string("a"), Rule::pattern("b+")]))
        .rule("word", Rule::pattern("[a-z]+"))
        .rule(
            "cmt",
            Rule::token(Rule::seq([Rule::string("#"), Rule::pattern(".*")])),
        )
        .rule("_htok", Rule::string("~"))
}

/// The direct recursive renderer: one line, single spaces.
fn reference(node: &Node<&'static str>, out: &mut Vec<String>) {
    let name = *node.kind();
    match name {
        "word" | "ext" | "cmt" => out.push(format!("({name})")),
        "_htok" | "_hext" => {}
        "_hidden" | "inl" | "sup" => children(node, false, out),
        // `lex` names no other node, so only the extras inside it show.
        "lex" => branch(name, node, true, out),
        _ => branch(name, node, false, out),
    }
}

fn branch(name: &str, node: &Node<&'static str>, extras_only: bool, out: &mut Vec<String>) {
    let mut inner = Vec::new();
    children(node, extras_only, &mut inner);
    if inner.is_empty() {
        out.push(format!("({name})"));
    } else {
        out.push(format!("({name} {})", inner.join(" ")));
    }
}

fn children(node: &Node<&'static str>, extras_only: bool, out: &mut Vec<String>) {
    for child in node.children() {
        let name = *child.kind();
        if extras_only && name != "cmt" {
            continue;
        }
        match child {
            Element::Node(n) => reference(n, out),
            Element::Token(_) => match name {
                "word" | "pair" | "ext" | "al" | "cmt" | "lex" => out.push(format!("({name})")),
                _ => {}
            },
        }
    }
}

fn tree() -> impl Strategy<Value = Node<&'static str>> {
    let token = prop::sample::select(TOKEN_NAMES.to_vec())
        .prop_map(|k| Element::Token(Token::new(k, Span::new(0, 1))));
    let element = token.prop_recursive(6, 64, 5, |inner| {
        (
            prop::sample::select(NODE_NAMES.to_vec()),
            prop::collection::vec(inner, 0..5),
        )
            .prop_map(|(k, cs)| Element::Node(Node::new(k, cs)))
    });
    prop::collection::vec(element, 0..6).prop_map(|cs| Node::new("file", cs))
}

// --- Properties -------------------------------------------------------------

/// Cases per property: 512, or `PROPTEST_CASES` for a longer soak.
fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(512)
}

/// The generators reach the cases the properties are about: rules that can
/// and cannot match nothing, strings that need escaping, and trees deep
/// enough to nest.
#[test]
fn test_generators_are_not_vacuous() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    fn strings(m: &M, out: &mut Vec<String>) {
        match m {
            M::Str(s) => out.push(s.clone()),
            M::Seq(ms) | M::Choice(ms) => ms.iter().for_each(|m| strings(m, out)),
            M::Repeat(m)
            | M::Repeat1(m)
            | M::Field(_, m)
            | M::Alias(_, m)
            | M::Token(m)
            | M::Immediate(m)
            | M::Prec(_, _, m) => strings(m, out),
            _ => {}
        }
    }

    let mut runner = TestRunner::deterministic();
    let (mut empty, mut special, mut deep) = (0, 0, 0);
    const N: usize = 1000;
    for _ in 0..N {
        let m = model().new_tree(&mut runner).unwrap().current();
        empty += usize::from(matches_empty(&m));
        let mut found = Vec::new();
        strings(&m, &mut found);
        special += usize::from(
            found
                .iter()
                .any(|s| s.contains(['\'', '\\', '\n', '\u{2028}'])),
        );
        let root = tree().new_tree(&mut runner).unwrap().current();
        let rendered = sexp_grammar().sexp(&root, |k| *k).unwrap();
        deep += usize::from(rendered.contains("\n    ("));
    }
    assert!(
        empty > N / 10 && empty < N * 9 / 10,
        "matches_empty in {empty}/{N}"
    );
    assert!(special > N / 10, "escapable strings in {special}/{N}");
    assert!(deep > N / 10, "nested trees in {deep}/{N}");

    // Hostile patterns are sometimes valid — some with a class or a slash,
    // which the scanner must read correctly — and usually not; valid
    // patterns are always valid.
    let (mut valid, mut tricky) = (0, 0);
    for _ in 0..N {
        let p = hostile_pattern().new_tree(&mut runner).unwrap().current();
        let ok = Grammar::new("g")
            .rule("start", Rule::pattern(p.clone()))
            .to_js()
            .is_ok();
        valid += usize::from(ok);
        tricky += usize::from(ok && p.contains(['[', '/']));
        let p = pattern().new_tree(&mut runner).unwrap().current();
        let result = Grammar::new("g")
            .rule("start", Rule::pattern(p.clone()))
            .to_js();
        assert!(result.is_ok(), "{p:?}: {result:?}");
    }
    assert!(
        valid > N / 20 && valid < N / 2,
        "valid hostile patterns {valid}/{N}"
    );
    assert!(
        tricky > N / 100,
        "valid hostile patterns with `[` or `/` {tricky}/{N}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    #[test]
    fn prop_json_round_trips(m in model()) {
        let json = grammar_with(&m).to_json().unwrap();
        prop_assert_eq!(start_from_json(&json), m);
    }

    #[test]
    fn prop_js_round_trips(m in model()) {
        let js = grammar_with(&m).to_js().unwrap();
        prop_assert_eq!(start_from_js(&js), m);
    }

    #[test]
    fn prop_patterns_are_refused_or_stay_inside_their_literal(
        pattern in hostile_pattern(),
        text in payload(),
        reachable in any::<bool>(),
    ) {
        let body = Rule::seq([Rule::pattern(pattern.clone()), Rule::string(text.clone())]);
        let grammar = if reachable {
            Grammar::new("prop").rule("start", body)
        } else {
            Grammar::new("prop").rule("start", Rule::string("x")).rule("other", body)
        };
        let name = if reachable { "start" } else { "other" };
        match (grammar.to_js(), grammar.to_json()) {
            (Ok(js), Ok(json)) => {
                // grammar.json records the literal's text as tree-sitter would
                // read it back; grammar.js must hold exactly that literal, then
                // the string, then nothing but the rest of the file.
                let expected = rule_from_json(&json, name);
                let M::Seq(parts) = &expected else {
                    return Err(TestCaseError::fail("not a sequence"));
                };
                prop_assert!(matches!(&parts[..], [M::Pat(_), M::Str(s)] if *s == text));
                let rules = rules_from_js(&js);
                let read = rules.iter().find(|(n, _)| n == name).map(|(_, m)| m);
                prop_assert_eq!(read, Some(&expected));
                prop_assert_eq!(rules.len(), if reachable { 1 } else { 2 });
            }
            (Err(a), Err(b)) => {
                prop_assert_eq!(&a, &b);
                let Error::EmptyString { rule } = &a else {
                    return Err(TestCaseError::fail(format!("unexpected {a:?}")));
                };
                prop_assert!(pattern.is_empty() || rule.starts_with(&format!("{name}: ")), "{rule}");
                // The offset in the report is a character boundary of the
                // pattern.
                if let Some(at) = rule.split(" at byte ").nth(1).and_then(|r| r.split(' ').next()) {
                    let at: usize = at.parse().unwrap();
                    prop_assert!(pattern.is_char_boundary(at), "{rule}");
                }
            }
            (js, json) => prop_assert!(false, "formats disagree: {js:?} / {json:?}"),
        }
    }

    #[test]
    fn prop_matches_empty_agrees_with_definition(m in model_raw()) {
        // `start` refers to `r`, so `r` may not match nothing.
        let grammar = Grammar::new("prop")
            .rule("start", Rule::seq([Rule::string("<"), Rule::symbol("r"), Rule::string(">")]))
            .rule("r", to_rule(&m))
            .rule("leaf_a", Rule::pattern("[a-z]+"))
            .rule("leaf_b", Rule::string("b"));
        let expected = if matches_empty(&m) || empty_repeat(&m, false) {
            Err(Error::MatchesEmpty { rule: "r".into() })
        } else {
            Ok(())
        };
        prop_assert_eq!(grammar.to_json().map(|_| ()), expected);
    }

    #[test]
    fn prop_sexp_agrees_with_reference(root in tree()) {
        let ours = sexp_grammar().sexp(&root, |k| *k).unwrap();

        let mut expected = Vec::new();
        reference(&root, &mut expected);
        let flattened = ours.split('\n').map(str::trim_start).collect::<Vec<_>>().join(" ");
        prop_assert_eq!(flattened, expected.join(" "));

        // Each line is indented two spaces per parenthesis still open.
        let mut open = 0usize;
        for line in ours.split('\n') {
            let indent = line.len() - line.trim_start().len();
            prop_assert_eq!(indent, open * 2);
            open += line.matches('(').count();
            open -= line.matches(')').count();
        }
    }
}
