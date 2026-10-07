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
            // `/*` would open a comment. No regular expression starts with a
            // quantifier, so the constructor form only makes tree-sitter
            // report the pattern's real error instead of a syntax error.
            Expr::Pattern(pattern) if pattern.starts_with('*') => {
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
