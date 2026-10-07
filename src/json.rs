//! Emitting `grammar.json`.
//!
//! The output is the file `tree-sitter generate` writes to `src/grammar.json`:
//! the same `$schema` line, rule objects, and field order, in `serde_json`'s
//! pretty layout — two-space indentation, `"key": value`, empty arrays as
//! `[]`, no trailing newline. For a grammar this crate emits, the file
//! tree-sitter 0.25 and later write from the `grammar.js` is byte-identical to
//! the one emitted here; 0.24 omits the final, empty `reserved` field, which it
//! and older versions accept. Rules are printed from an explicit work stack
//! rather than by recursion.

use alloc::{string::String, vec::Vec};

use crate::{
    Grammar, Rule,
    rule::{Expr, Prec, Text},
    text::{indent, int, js_regex, js_regex_rewrites, json_string},
};

/// The JSON schema tree-sitter names at the top of `grammar.json`.
const SCHEMA: &str = "https://tree-sitter.github.io/tree-sitter/assets/schemas/grammar.schema.json";

/// Appends the whole grammar to `out`. The grammar must already be valid.
pub(crate) fn write(grammar: &Grammar, out: &mut String) {
    out.reserve(estimate(grammar));
    let mut p = Printer::default();
    out.push_str("{\n  \"$schema\": \"");
    out.push_str(SCHEMA);
    out.push_str("\",\n  \"name\": ");
    json_string(out, &grammar.name);
    if let Some(word) = &grammar.word {
        out.push_str(",\n  \"word\": ");
        json_string(out, word);
    }

    out.push_str(",\n  \"rules\": {");
    for (i, (name, body)) in grammar.rules.iter().enumerate() {
        out.push_str(if i == 0 { "\n    " } else { ",\n    " });
        json_string(out, name);
        out.push_str(": ");
        p.rule(out, body, 2);
    }
    out.push_str("\n  },\n  \"extras\": [");
    match &grammar.extras {
        Some(extras) => {
            for (i, extra) in extras.iter().enumerate() {
                out.push_str(if i == 0 { "\n    " } else { ",\n    " });
                p.rule(out, extra, 2);
            }
            out.push_str(if extras.is_empty() { "]" } else { "\n  ]" });
        }
        // `grammar.json` has no implicit default: spell out the one
        // `grammar.js` would apply.
        None => out.push_str(
            "\n    {\n      \"type\": \"PATTERN\",\n      \"value\": \"\\\\s\"\n    }\n  ]",
        ),
    }

    out.push_str(",\n  \"conflicts\": [");
    for (i, set) in grammar.conflicts.iter().enumerate() {
        out.push_str(if i == 0 { "\n    " } else { ",\n    " });
        string_array(out, set, 2);
    }
    out.push_str(if grammar.conflicts.is_empty() {
        "]"
    } else {
        "\n  ]"
    });

    out.push_str(",\n  \"precedences\": [],\n  \"externals\": [");
    for (i, name) in grammar.externals.iter().enumerate() {
        out.push_str(if i == 0 { "\n    " } else { ",\n    " });
        out.push_str("{\n      \"type\": \"SYMBOL\",\n      \"name\": ");
        json_string(out, name);
        out.push_str("\n    }");
    }
    out.push_str(if grammar.externals.is_empty() {
        "]"
    } else {
        "\n  ]"
    });

    out.push_str(",\n  \"inline\": ");
    // Tree-sitter keeps the first of repeated inline entries; so does this.
    string_array(out, grammar.inline_names(), 1);
    out.push_str(",\n  \"supertypes\": ");
    string_array(out, &grammar.supertypes, 1);
    // Reserved-word sets are not supported; tree-sitter writes the empty
    // object when a grammar has none, and so does this.
    out.push_str(",\n  \"reserved\": {}\n}");
}

/// A rough output size; JSON is about three times as long as `grammar.js`.
fn estimate(grammar: &Grammar) -> usize {
    512 + grammar.rules.len() * 320
}

/// Appends an array of strings opened on a line indented `level` levels.
fn string_array<'a>(out: &mut String, names: impl IntoIterator<Item = &'a Text>, level: usize) {
    let mut any = false;
    for name in names {
        out.push_str(if any { ",\n" } else { "[\n" });
        any = true;
        indent(out, level + 1);
        json_string(out, name);
    }
    if any {
        out.push('\n');
        indent(out, level);
        out.push(']');
    } else {
        out.push_str("[]");
    }
}

/// One step of printing a rule.
enum Work<'g> {
    /// Print this rule.
    Rule(&'g Rule),
    /// Append fixed text.
    Text(&'static str),
    /// Append text from the grammar as a JSON string.
    Quoted(&'g str),
    /// Start a new line at the current indentation.
    Line,
    /// Close an indentation level opened earlier.
    Dedent,
}

/// Prints rules, reusing its work stack and scratch buffer from one rule to
/// the next.
#[derive(Default)]
struct Printer<'g> {
    work: Vec<Work<'g>>,
    level: usize,
    /// Holds a pattern while it is rewritten into `grammar.js` form.
    scratch: String,
}

impl<'g> Printer<'g> {
    /// Appends `rule` as a JSON object that opens on a line indented `level`
    /// levels.
    fn rule(&mut self, out: &mut String, rule: &'g Rule, level: usize) {
        self.level = level;
        self.work.push(Work::Rule(rule));
        while let Some(step) = self.work.pop() {
            match step {
                Work::Rule(rule) => self.visit(out, rule),
                Work::Text(text) => out.push_str(text),
                Work::Quoted(text) => json_string(out, text),
                Work::Line => self.line(out),
                Work::Dedent => self.level -= 1,
            }
        }
    }

    fn line(&self, out: &mut String) {
        out.push('\n');
        indent(out, self.level);
    }

    /// `{` and the `"type"` field.
    fn open(&mut self, out: &mut String, kind: &str) {
        out.push('{');
        self.level += 1;
        self.line(out);
        out.push_str("\"type\": \"");
        out.push_str(kind);
        out.push('"');
    }

    /// `,` and the next field's key.
    fn key(&self, out: &mut String, key: &str) {
        out.push(',');
        self.line(out);
        out.push('"');
        out.push_str(key);
        out.push_str("\": ");
    }

    /// The closing `}` of an object whose fields are all written.
    fn close(&mut self, out: &mut String) {
        self.level -= 1;
        self.line(out);
        out.push('}');
    }

    /// Schedules the `}` that follows a nested rule.
    fn close_after(&mut self) {
        self.work.push(Work::Text("}"));
        self.work.push(Work::Line);
        self.work.push(Work::Dedent);
    }

    /// An object whose last field is `"content"`: the wrapped rule.
    fn content(&mut self, out: &mut String, inner: &'g Rule) {
        self.key(out, "content");
        self.close_after();
        self.work.push(Work::Rule(inner));
    }

    /// Appends the start of `rule` and schedules the rest.
    fn visit(&mut self, out: &mut String, rule: &'g Rule) {
        match &rule.0 {
            Expr::Blank => {
                self.open(out, "BLANK");
                self.close(out);
            }
            Expr::String(text) => self.leaf(out, "STRING", "value", text),
            Expr::Pattern(pattern) => self.pattern(out, pattern),
            Expr::Symbol(name) => self.leaf(out, "SYMBOL", "name", name),
            Expr::Seq(rules) => self.members(out, "SEQ", rules),
            Expr::Choice(rules) => self.members(out, "CHOICE", rules),
            Expr::Repeat(inner) => {
                self.open(out, "REPEAT");
                self.content(out, inner);
            }
            Expr::Repeat1(inner) => {
                self.open(out, "REPEAT1");
                self.content(out, inner);
            }
            Expr::Token(inner) => {
                self.open(out, "TOKEN");
                self.content(out, inner);
            }
            Expr::Immediate(inner) => {
                self.open(out, "IMMEDIATE_TOKEN");
                self.content(out, inner);
            }
            Expr::Field(name, inner) => {
                self.open(out, "FIELD");
                self.key(out, "name");
                json_string(out, name);
                self.content(out, inner);
            }
            Expr::Prec(kind, level, inner) => {
                self.open(
                    out,
                    match kind {
                        Prec::Plain => "PREC",
                        Prec::Left => "PREC_LEFT",
                        Prec::Right => "PREC_RIGHT",
                        Prec::Dynamic => "PREC_DYNAMIC",
                    },
                );
                self.key(out, "value");
                int(out, *level);
                self.content(out, inner);
            }
            Expr::Alias(name, inner) => {
                // Tree-sitter's order: content, then named, then value.
                self.open(out, "ALIAS");
                self.key(out, "content");
                self.close_after();
                self.work.push(Work::Quoted(name));
                self.work.push(Work::Text("\"value\": "));
                self.work.push(Work::Line);
                self.work.push(Work::Text("\"named\": true,"));
                self.work.push(Work::Line);
                self.work.push(Work::Text(","));
                self.work.push(Work::Rule(inner));
            }
        }
    }

    /// A `PATTERN` object. The value is the pattern as `grammar.js` writes
    /// it between the slashes — what tree-sitter reads back as the regular
    /// expression's source — so the two formats agree byte for byte. Most
    /// patterns contain nothing `grammar.js` rewrites and are copied directly.
    fn pattern(&mut self, out: &mut String, pattern: &str) {
        if !js_regex_rewrites(pattern) {
            return self.leaf(out, "PATTERN", "value", pattern);
        }
        let mut source = core::mem::take(&mut self.scratch);
        source.clear();
        js_regex(&mut source, pattern);
        self.leaf(out, "PATTERN", "value", &source);
        self.scratch = source;
    }

    /// An object with a type and one string field.
    fn leaf(&mut self, out: &mut String, kind: &str, key: &str, value: &str) {
        self.open(out, kind);
        self.key(out, key);
        json_string(out, value);
        self.close(out);
    }

    /// A `SEQ` or `CHOICE` object with its `"members"` array.
    fn members(&mut self, out: &mut String, kind: &str, rules: &'g [Rule]) {
        self.open(out, kind);
        self.key(out, "members");
        if rules.is_empty() {
            out.push_str("[]");
            return self.close(out);
        }
        out.push('[');
        self.level += 1;
        self.close_after();
        self.work.push(Work::Text("]"));
        self.work.push(Work::Line);
        self.work.push(Work::Dedent);
        for (i, rule) in rules.iter().enumerate().rev() {
            if i + 1 < rules.len() {
                self.work.push(Work::Text(","));
            }
            self.work.push(Work::Rule(rule));
            self.work.push(Work::Line);
        }
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
    fn test_leaf_objects() {
        assert_eq!(print(&Rule::blank()), "{\n  \"type\": \"BLANK\"\n}");
        assert_eq!(
            print(&Rule::pattern(r"\d")),
            "{\n  \"type\": \"PATTERN\",\n  \"value\": \"\\\\d\"\n}"
        );
        assert_eq!(
            print(&Rule::symbol("a")),
            "{\n  \"type\": \"SYMBOL\",\n  \"name\": \"a\"\n}"
        );
    }

    #[test]
    fn test_members_and_content() {
        let rule = Rule::seq([Rule::repeat(Rule::string("a")), Rule::blank()]);
        let expected = "{
  \"type\": \"SEQ\",
  \"members\": [
    {
      \"type\": \"REPEAT\",
      \"content\": {
        \"type\": \"STRING\",
        \"value\": \"a\"
      }
    },
    {
      \"type\": \"BLANK\"
    }
  ]
}";
        assert_eq!(print(&rule), expected);
        assert_eq!(
            print(&Rule::choice([])),
            "{\n  \"type\": \"CHOICE\",\n  \"members\": []\n}"
        );
    }

    #[test]
    fn test_prec_field_alias_layout() {
        let rule = Rule::prec_right(-3, Rule::field("f", Rule::alias(Rule::symbol("x"), "y")));
        let expected = "{
  \"type\": \"PREC_RIGHT\",
  \"value\": -3,
  \"content\": {
    \"type\": \"FIELD\",
    \"name\": \"f\",
    \"content\": {
      \"type\": \"ALIAS\",
      \"content\": {
        \"type\": \"SYMBOL\",
        \"name\": \"x\"
      },
      \"named\": true,
      \"value\": \"y\"
    }
  }
}";
        assert_eq!(print(&rule), expected);
    }

    #[test]
    fn test_pattern_value_is_the_grammar_js_source() {
        let printed = print(&Rule::pattern("a/b\n"));
        assert!(printed.contains(r#""value": "a\\/b\\n""#), "{printed}");
        let printed = print(&Rule::pattern(r"[^/]"));
        assert!(printed.contains(r#""value": "[^\\/]""#), "{printed}");
        let printed = print(&Rule::pattern(r"\d"));
        assert!(printed.contains(r#""value": "\\d""#), "{printed}");
    }

    #[test]
    fn test_token_kinds() {
        assert!(print(&Rule::token(Rule::string("a"))).contains("\"type\": \"TOKEN\""));
        assert!(
            print(&Rule::immediate(Rule::string("a"))).contains("\"type\": \"IMMEDIATE_TOKEN\"")
        );
        assert!(print(&Rule::repeat1(Rule::string("a"))).contains("\"type\": \"REPEAT1\""));
        assert!(print(&Rule::prec(1, Rule::string("a"))).contains("\"type\": \"PREC\""));
        assert!(
            print(&Rule::prec_dynamic(1, Rule::string("a"))).contains("\"type\": \"PREC_DYNAMIC\"")
        );
        assert!(print(&Rule::prec_left(1, Rule::string("a"))).contains("\"type\": \"PREC_LEFT\""));
    }
}
