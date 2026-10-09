//! Emitting `grammar.js`.
//!
//! The layout follows how tree-sitter grammars are written by hand: two-space
//! indentation, trailing commas, and a sequence or choice kept on one line
//! when its members are short and simple, otherwise one member per line. A
//! wrapper (`prec.left(1, seq(` ...) opens on the line it starts on, so the
//! members of what it wraps sit one level in and the closers stack up at the
//! end: `)),`.
//!
//! Rules are printed from an explicit work stack rather than by recursion, so
//! nesting depth is limited only by memory.

use alloc::{string::String, vec::Vec};

use crate::{
    Grammar, Rule,
    rule::{Expr, Prec, Text},
    text::{indent, int, js_regex, js_string},
};

/// The widest a sequence or choice may be and still go on one line, counting
/// only its members and separators.
const FLAT_WIDTH: usize = 60;

/// Appends the whole grammar to `out`. The grammar must already be valid.
pub(crate) fn write(grammar: &Grammar, out: &mut String) {
    out.reserve(estimate(grammar));
    out.push_str("/// <reference types=\"tree-sitter-cli/dsl\" />\n// @ts-check\n\n");
    out.push_str("module.exports = grammar({\n  name: ");
    js_string(out, &grammar.name);
    out.push_str(",\n");

    let mut printer = Printer::default();
    if let Some(extras) = &grammar.extras {
        out.push_str("\n  extras: $ => [\n");
        for extra in extras {
            indent(out, 2);
            printer.rule(out, extra, 2);
            out.push_str(",\n");
        }
        out.push_str("  ],\n");
    }
    names(out, "externals", &grammar.externals);
    names(out, "supertypes", &grammar.supertypes);
    // Tree-sitter warns about repeated inline entries and keeps the first.
    names(out, "inline", grammar.inline_names());
    if !grammar.conflicts.is_empty() {
        out.push_str("\n  conflicts: $ => [\n");
        for set in &grammar.conflicts {
            out.push_str("    [");
            for (i, name) in set.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str("$.");
                out.push_str(name);
            }
            out.push_str("],\n");
        }
        out.push_str("  ],\n");
    }
    if let Some(word) = &grammar.word {
        out.push_str("\n  word: $ => $.");
        out.push_str(word);
        out.push_str(",\n");
    }

    out.push_str("\n  rules: {\n");
    for (i, (name, body)) in grammar.rules.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str("    ");
        out.push_str(name);
        out.push_str(": $ => ");
        printer.rule(out, body, 2);
        out.push_str(",\n");
    }
    out.push_str("  },\n});\n");
}

/// A rough output size, so the buffer grows once or twice rather than a
/// dozen times.
fn estimate(grammar: &Grammar) -> usize {
    256 + grammar.rules.len() * 96 + grammar.extras.as_ref().map_or(0, |e| e.len() * 32)
}

/// Appends a `field: $ => [ $.a, $.b ],` section, or nothing for an empty
/// list.
fn names<'a>(out: &mut String, field: &str, names: impl IntoIterator<Item = &'a Text>) {
    let mut any = false;
    for name in names {
        if !any {
            out.push_str("\n  ");
            out.push_str(field);
            out.push_str(": $ => [\n");
            any = true;
        }
        out.push_str("    $.");
        out.push_str(name);
        out.push_str(",\n");
    }
    if any {
        out.push_str("  ],\n");
    }
}

/// One step of printing a rule.
enum Work<'g> {
    /// Print this rule.
    Rule(&'g Rule),
    /// Append fixed text.
    Text(&'static str),
    /// Append a name taken from the grammar.
    Name(&'g str),
    /// Start a new line at the current indentation.
    Line,
    /// Close an indentation level opened earlier.
    Dedent,
}

/// Prints rules, reusing its work stack from one rule to the next.
#[derive(Default)]
struct Printer<'g> {
    work: Vec<Work<'g>>,
    level: usize,
}

impl<'g> Printer<'g> {
    /// Appends `rule`, which starts on a line indented `level` levels.
    fn rule(&mut self, out: &mut String, rule: &'g Rule, level: usize) {
        self.level = level;
        self.work.push(Work::Rule(rule));
        while let Some(step) = self.work.pop() {
            match step {
                Work::Rule(rule) => self.visit(out, rule),
                Work::Text(text) => out.push_str(text),
                Work::Name(name) => out.push_str(name),
                Work::Line => {
                    out.push('\n');
                    indent(out, self.level);
                }
                Work::Dedent => self.level -= 1,
            }
        }
    }

    /// Appends the start of `rule` and schedules the rest.
    fn visit(&mut self, out: &mut String, rule: &'g Rule) {
        if let Some(inner) = rule.as_optional() {
            return self.wrap(out, "optional(", inner);
        }
        match &rule.0 {
            Expr::Blank => out.push_str("blank()"),
            Expr::String(text) => js_string(out, text),
            // `/*` would open a block comment and `//` a line comment.
            // Validation refuses both patterns — no regular expression is
            // empty or starts with a quantifier — so this is a second line
            // of defence: the constructor form keeps the pattern inside a
            // string literal, where it cannot change what the code around it
            // means.
            Expr::Pattern(pattern) if pattern.is_empty() || pattern.starts_with('*') => {
                out.push_str("new RegExp(");
                js_string(out, pattern);
                out.push(')');
            }
            Expr::Pattern(pattern) => {
                out.push('/');
                js_regex(out, pattern);
                out.push('/');
            }
            Expr::Symbol(name) => {
                out.push_str("$.");
                out.push_str(name);
            }
            Expr::Seq(rules) => self.list(out, "seq(", rules),
            Expr::Choice(rules) => self.list(out, "choice(", rules),
            Expr::Repeat(inner) => self.wrap(out, "repeat(", inner),
            Expr::Repeat1(inner) => self.wrap(out, "repeat1(", inner),
            Expr::Token(inner) => self.wrap(out, "token(", inner),
            Expr::Immediate(inner) => self.wrap(out, "token.immediate(", inner),
            Expr::Field(name, inner) => {
                out.push_str("field(");
                js_string(out, name);
                out.push_str(", ");
                self.work.push(Work::Text(")"));
                self.work.push(Work::Rule(inner));
            }
            Expr::Alias(name, inner) => {
                out.push_str("alias(");
                self.work.push(Work::Text(")"));
                self.work.push(Work::Name(name));
                self.work.push(Work::Text(", $."));
                self.work.push(Work::Rule(inner));
            }
            Expr::Prec(kind, level, inner) => {
                out.push_str(match kind {
                    Prec::Plain => "prec(",
                    Prec::Left => "prec.left(",
                    Prec::Right => "prec.right(",
                    Prec::Dynamic => "prec.dynamic(",
                });
                int(out, *level);
                out.push_str(", ");
                self.work.push(Work::Text(")"));
                self.work.push(Work::Rule(inner));
            }
        }
    }

    /// `head` + the one wrapped rule + `)`.
    fn wrap(&mut self, out: &mut String, head: &str, inner: &'g Rule) {
        out.push_str(head);
        self.work.push(Work::Text(")"));
        self.work.push(Work::Rule(inner));
    }

    /// A sequence or choice: on one line when every member is simple and the
    /// whole fits, otherwise one member per line, indented one level.
    fn list(&mut self, out: &mut String, head: &str, rules: &'g [Rule]) {
        out.push_str(head);
        if fits_on_one_line(rules) {
            self.work.push(Work::Text(")"));
            for (i, rule) in rules.iter().enumerate().rev() {
                self.work.push(Work::Rule(rule));
                if i > 0 {
                    self.work.push(Work::Text(", "));
                }
            }
            return;
        }
        // Scheduled in reverse: each member on its own line with a trailing
        // comma, then the closer back at the outer level.
        self.work.push(Work::Text(")"));
        self.work.push(Work::Line);
        self.work.push(Work::Dedent);
        for rule in rules.iter().rev() {
            self.work.push(Work::Text(","));
            self.work.push(Work::Rule(rule));
            self.work.push(Work::Line);
        }
        self.level += 1;
    }
}

/// Whether a list's members are all simple — a leaf, possibly wrapped in
/// single-rule wrappers — and fit within [`FLAT_WIDTH`] together. An empty
/// list fits.
fn fits_on_one_line(rules: &[Rule]) -> bool {
    let mut total = rules.len().saturating_sub(1) * 2;
    for rule in rules {
        match simple_width(rule) {
            Some(width) => total += width,
            None => return false,
        }
        if total > FLAT_WIDTH {
            return false;
        }
    }
    true
}

/// The printed width of a leaf behind a chain of single-rule wrappers, or
/// `None` if the chain ends in a sequence or choice. Escaping is not counted;
/// the width only decides layout.
fn simple_width(mut rule: &Rule) -> Option<usize> {
    let mut width = 0;
    loop {
        if let Some(inner) = rule.as_optional() {
            width += "optional()".len();
            rule = inner;
            continue;
        }
        let (overhead, inner) = match &rule.0 {
            Expr::Blank => return Some(width + "blank()".len()),
            Expr::String(text) | Expr::Pattern(text) => return Some(width + text.len() + 2),
            Expr::Symbol(name) => return Some(width + name.len() + 2),
            Expr::Seq(_) | Expr::Choice(_) => return None,
            Expr::Repeat(inner) => ("repeat()".len(), inner),
            Expr::Repeat1(inner) => ("repeat1()".len(), inner),
            Expr::Token(inner) => ("token()".len(), inner),
            Expr::Immediate(inner) => ("token.immediate()".len(), inner),
            Expr::Field(name, inner) => ("field('', )".len() + name.len(), inner),
            Expr::Alias(name, inner) => ("alias(, $.)".len() + name.len(), inner),
            Expr::Prec(_, _, inner) => ("prec.dynamic(0, )".len(), inner),
        };
        width += overhead;
        rule = inner;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print(rule: &Rule) -> String {
        let mut out = String::new();
        Printer::default().rule(&mut out, rule, 0);
        out
    }

    #[test]
    fn test_leaves() {
        assert_eq!(print(&Rule::blank()), "blank()");
        assert_eq!(print(&Rule::symbol("a")), "$.a");
        assert_eq!(print(&Rule::string("a'b")), r"'a\'b'");
        assert_eq!(print(&Rule::pattern("a/b")), r"/a\/b/");
    }

    #[test]
    fn test_wrappers() {
        let rule = Rule::prec_left(-2, Rule::field("f", Rule::alias(Rule::string("x"), "y")));
        assert_eq!(print(&rule), "prec.left(-2, field('f', alias('x', $.y)))");
        assert_eq!(
            print(&Rule::immediate(Rule::string("a"))),
            "token.immediate('a')"
        );
        assert_eq!(
            print(&Rule::prec_dynamic(1, Rule::blank())),
            "prec.dynamic(1, blank())"
        );
        assert_eq!(print(&Rule::optional(Rule::symbol("a"))), "optional($.a)");
    }

    #[test]
    fn test_short_list_stays_flat() {
        let rule = Rule::seq([Rule::symbol("a"), Rule::string("+"), Rule::symbol("b")]);
        assert_eq!(print(&rule), "seq($.a, '+', $.b)");
        assert_eq!(print(&Rule::seq([])), "blank()");
    }

    #[test]
    fn test_nested_list_breaks_lines() {
        let rule = Rule::choice([
            Rule::seq([Rule::string("("), Rule::symbol("e"), Rule::string(")")]),
            Rule::symbol("n"),
        ]);
        assert_eq!(print(&rule), "choice(\n  seq('(', $.e, ')'),\n  $.n,\n)");
    }

    #[test]
    fn test_wide_list_breaks_lines() {
        let long = "a_rather_long_rule_name_for_testing";
        let rule = Rule::seq([Rule::symbol(long), Rule::symbol(long)]);
        let printed = print(&rule);
        assert!(printed.starts_with("seq(\n  $.a_rather"));
        assert!(printed.ends_with(",\n)"));
    }

    #[test]
    fn test_wrapper_around_multiline_list_stacks_closers() {
        let rule = Rule::prec(
            1,
            Rule::seq([Rule::seq([Rule::symbol("a")]), Rule::symbol("b")]),
        );
        assert_eq!(print(&rule), "prec(1, seq(\n  seq($.a),\n  $.b,\n))");
    }

    /// The length of the JavaScript regular expression literal at the start
    /// of `s`, read as a JavaScript lexer reads one: a backslash escapes the
    /// next character, a `[` opens a class that the next unescaped `]`
    /// closes, and only a `/` outside a class ends the literal. `None` if
    /// there is no literal there.
    fn regex_literal(s: &str) -> Option<usize> {
        let terminator = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
        let rest = s.strip_prefix('/')?;
        // `//` and `/*` open comments.
        if rest.starts_with(['/', '*']) {
            return None;
        }
        let mut chars = rest.char_indices();
        let mut in_class = false;
        while let Some((i, c)) = chars.next() {
            match c {
                c if terminator(c) => return None,
                '\\' => match chars.next() {
                    Some((_, e)) if !terminator(e) => {}
                    _ => return None,
                },
                '[' => in_class = true,
                ']' => in_class = false,
                '/' if !in_class => return Some(i + 2),
                _ => {}
            }
        }
        None
    }

    /// The single-quoted JavaScript string literal at the start of `s`:
    /// its value and its length.
    fn string_literal(s: &str) -> Option<(String, usize)> {
        let rest = s.strip_prefix('\'')?;
        let mut value = String::new();
        let mut chars = rest.char_indices();
        let hex = |chars: &mut core::str::CharIndices<'_>, n: usize| {
            let mut v = 0;
            for _ in 0..n {
                v = v * 16 + chars.next()?.1.to_digit(16)?;
            }
            char::from_u32(v)
        };
        while let Some((i, c)) = chars.next() {
            match c {
                '\'' => return Some((value, i + 2)),
                '\n' | '\r' | '\u{2028}' | '\u{2029}' => return None,
                '\\' => value.push(match chars.next()?.1 {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'x' => hex(&mut chars, 2)?,
                    'u' => hex(&mut chars, 4)?,
                    e => e,
                }),
                c => value.push(c),
            }
        }
        None
    }

    /// Reads `seq(<pattern>, <string>)` as a JavaScript lexer would: the
    /// pattern as one regular expression literal or one `new RegExp(...)`
    /// call on a string, then one string literal. The string's value, if
    /// that is all the text holds.
    fn pattern_then_string(printed: &str) -> Option<String> {
        let rest = printed.strip_prefix("seq(")?.trim_start();
        let rest = match rest.strip_prefix("new RegExp(") {
            Some(inner) => {
                let (_, n) = string_literal(inner)?;
                inner[n..].strip_prefix(')')?
            }
            None => &rest[regex_literal(rest)?..],
        };
        let rest = rest.strip_prefix(',')?.trim_start();
        let (value, n) = string_literal(rest)?;
        let rest = rest[n..].trim_start();
        let rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
        (rest == ")").then_some(value)
    }

    #[test]
    fn test_hostile_patterns_cannot_leave_their_literal_even_unvalidated() {
        // ISSUES H06, second line of defence. Printed without validation,
        // every pattern of up to five characters from a hostile alphabet
        // stays one literal (or one `new RegExp` string), and the string
        // after it survives intact: nothing it holds can become code.
        const ALPHABET: [char; 10] = ['[', ']', '\\', '/', '*', 'a', '\n', '\u{2028}', '(', '\''];
        let payload = r#"]/,require("fs").rmSync("x")),//"#;
        let mut digits: Vec<usize> = Vec::new();
        loop {
            let pattern: String = digits.iter().map(|&d| ALPHABET[d]).collect();
            let printed = print(&Rule::seq([
                Rule::pattern(pattern.clone()),
                Rule::string(payload),
            ]));
            assert_eq!(
                pattern_then_string(&printed).as_deref(),
                Some(payload),
                "{pattern:?} printed as {printed:?}"
            );

            // The next pattern, counting in base 10 over the alphabet.
            match digits.iter().rposition(|&d| d + 1 < ALPHABET.len()) {
                Some(at) => {
                    digits[at] += 1;
                    digits[at + 1..].iter_mut().for_each(|d| *d = 0);
                }
                None if digits.len() < 5 => {
                    digits.iter_mut().for_each(|d| *d = 0);
                    digits.push(0);
                }
                None => break,
            }
        }
    }

    #[test]
    fn test_empty_pattern_is_printed_as_a_constructor() {
        // `//` would open a comment; validation refuses empty patterns, so
        // this is only the second line of defence.
        assert_eq!(print(&Rule::pattern("")), "new RegExp('')");
        assert_eq!(print(&Rule::pattern("*a")), "new RegExp('*a')");
    }

    #[test]
    fn test_deep_nesting_is_iterative() {
        let mut rule = Rule::string("x");
        for _ in 0..100_000 {
            rule = Rule::repeat(rule);
        }
        let printed = print(&rule);
        assert_eq!(printed.len(), 100_000 * "repeat()".len() + 3);
        assert!(printed.starts_with("repeat(repeat("));
        assert!(printed.contains("(repeat('x'))"));
    }
}
