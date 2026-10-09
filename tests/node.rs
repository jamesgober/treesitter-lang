//! Emitted `grammar.js` held to a real JavaScript runtime.
//!
//! Node.js is a development-only test oracle here, never a dependency, so
//! the test is ignored by default. Run it with `node` on `PATH`:
//!
//! ```text
//! cargo test --test node -- --ignored
//! ```
//!
//! The CI `node` job runs it on Ubuntu. It fuzzes grammars whose strings are
//! hostile — text that would run as code if a regular expression literal
//! swallowed it — and whose patterns are whatever validation lets through,
//! including patterns only JavaScript checks, in rules tree-sitter drops.
//! Every emitted `grammar.js` must then:
//!
//! 1. pass `node --check`, so it parses;
//! 2. evaluate, in a sandbox with no `require` and no `process`, through a
//!    minimal stand-in for tree-sitter's DSL, to exactly the rules and
//!    extras the `grammar.json` emitted beside it holds.
//!
//! The second step is what proves no pattern or string can become code: an
//! injected token would either throw in the sandbox or change what the
//! file evaluates to.

use std::path::{Path, PathBuf};
use std::process::Command;

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use treesitter_lang::{Grammar, Rule};

/// The stand-in for tree-sitter's DSL, and the comparison. Rules are built
/// the way tree-sitter's `dsl.js` builds them, so the result is what
/// tree-sitter would write to `src/grammar.json`.
const CHECK_JS: &str = r#"'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const isRegExp = (v) => Object.prototype.toString.call(v) === '[object RegExp]';
function normalize(v) {
  if (typeof v === 'string') return { type: 'STRING', value: v };
  if (isRegExp(v)) {
    return v.flags ? { type: 'PATTERN', value: v.source, flags: v.flags } : { type: 'PATTERN', value: v.source };
  }
  if (v && typeof v.type === 'string') return v;
  throw new TypeError('invalid rule: ' + String(v));
}
function dsl() {
  const blank = () => ({ type: 'BLANK' });
  const choice = (...m) => ({ type: 'CHOICE', members: m.map(normalize) });
  const prec = (value, content) => ({ type: 'PREC', value, content: normalize(content) });
  prec.left = (value, content) => ({ type: 'PREC_LEFT', value, content: normalize(content) });
  prec.right = (value, content) => ({ type: 'PREC_RIGHT', value, content: normalize(content) });
  prec.dynamic = (value, content) => ({ type: 'PREC_DYNAMIC', value, content: normalize(content) });
  const token = (content) => ({ type: 'TOKEN', content: normalize(content) });
  token.immediate = (content) => ({ type: 'IMMEDIATE_TOKEN', content: normalize(content) });
  const $ = new Proxy({}, { get: (_, name) => ({ type: 'SYMBOL', name }) });
  return {
    blank,
    choice,
    prec,
    token,
    seq: (...m) => ({ type: 'SEQ', members: m.map(normalize) }),
    optional: (v) => choice(v, blank()),
    repeat: (v) => ({ type: 'REPEAT', content: normalize(v) }),
    repeat1: (v) => ({ type: 'REPEAT1', content: normalize(v) }),
    field: (name, v) => ({ type: 'FIELD', name, content: normalize(v) }),
    alias: (v, sym) => ({ type: 'ALIAS', content: normalize(v), named: true, value: sym.name }),
    grammar: (def) => ({
      rules: Object.fromEntries(Object.entries(def.rules).map(([n, f]) => [n, normalize(f($))])),
      extras: def.extras ? def.extras($).map(normalize) : [{ type: 'PATTERN', value: '\\s' }],
    }),
  };
}
const canon = (v) =>
  Array.isArray(v) ? '[' + v.map(canon).join(',') + ']'
  : v && typeof v === 'object' ? '{' + Object.keys(v).sort().map((k) => JSON.stringify(k) + ':' + canon(v[k])).join(',') + '}'
  : JSON.stringify(v);

let failures = 0;
const root = process.argv[2];
for (const dir of fs.readdirSync(root)) {
  const file = path.join(root, dir, 'grammar.js');
  const expected = JSON.parse(fs.readFileSync(path.join(root, dir, 'grammar.json'), 'utf8'));
  const sandbox = dsl();
  sandbox.module = { exports: undefined };
  try {
    vm.runInNewContext(fs.readFileSync(file, 'utf8'), sandbox, { filename: file, timeout: 5000 });
    const got = canon({ rules: sandbox.module.exports.rules, extras: sandbox.module.exports.extras });
    const want = canon({ rules: expected.rules, extras: expected.extras });
    if (got !== want) {
      failures++;
      console.error(`${file}: evaluates to\n${got}\nbut grammar.json holds\n${want}`);
    }
  } catch (e) {
    failures++;
    console.error(`${file}: ${e}`);
  }
}
process.exit(failures === 0 ? 0 : 1);
"#;

/// Strings that would run as code if the literal before them swallowed
/// them, and arbitrary text.
fn hostile_string() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::from(r#"]/,require("fs").rmSync("x")),//"#)),
        Just(String::from("*/,process.exit(1),/*")),
        Just(String::from("]/;throw new Error('injected');//")),
        Just(String::from("'+(()=>{throw 1})()+'")),
        prop::collection::vec(
            prop_oneof![
                prop::sample::select(vec![
                    '\'', '"', '\\', '/', '[', ']', '\n', '\u{2028}', '`', '$', '{'
                ]),
                any::<char>(),
            ],
            1..12,
        )
        .prop_map(String::from_iter),
    ]
}

/// Pattern text with no regard for validity; validation decides which to
/// keep.
fn hostile_pattern() -> impl Strategy<Value = String> {
    let piece = prop::sample::select(vec![
        "[", "]", "\\", "/", "*", "(", ")", "(?:", "(?<n>", "(?=", "{", "}", "{2}", "'", "\"",
        "\n", "\u{2028}", "a", "^", "$", "-", "|", "?", "+", ".", "\\d", "\\w", "\\/", "\\[",
        "\\]", "[^/]", "[/]", "[a-z]", "[\\]/]", "\\u0041", "\\x41", "\\p{L}", "(?i:", "é", "😀",
    ]);
    prop::collection::vec(piece, 1..8).prop_map(|pieces| pieces.concat())
}

/// Whether `pattern` validates where it will be placed: in a rule
/// tree-sitter keeps, or in one it drops (where only JavaScript reads it).
fn validates(pattern: &str, reachable: bool) -> bool {
    let rule = Rule::pattern(String::from(pattern));
    let grammar = if reachable {
        Grammar::new("g").rule("source_file", rule)
    } else {
        Grammar::new("g")
            .rule("source_file", Rule::string("x"))
            .rule("dropped", rule)
    };
    grammar.to_js().is_ok()
}

/// One fuzzed grammar: hostile strings between validated patterns, in a
/// kept rule, a dropped rule, and an extra.
fn grammar(runner: &mut TestRunner) -> Grammar {
    let draw_pattern = |reachable: bool, runner: &mut TestRunner| loop {
        let p = hostile_pattern().new_tree(runner).unwrap().current();
        if validates(&p, reachable) {
            return p;
        }
    };
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for _ in 0..3 {
        kept.push(Rule::pattern(draw_pattern(true, runner)));
        kept.push(Rule::string(
            hostile_string().new_tree(runner).unwrap().current(),
        ));
        dropped.push(Rule::pattern(draw_pattern(false, runner)));
        dropped.push(Rule::string(
            hostile_string().new_tree(runner).unwrap().current(),
        ));
    }
    let extra = Rule::token(Rule::seq([
        Rule::string(hostile_string().new_tree(runner).unwrap().current()),
        Rule::pattern(draw_pattern(true, runner)),
    ]));
    Grammar::new("fuzz")
        .extra(extra)
        .rule("source_file", Rule::repeat1(Rule::symbol("kept")))
        .rule("kept", Rule::seq(kept))
        .rule("dropped", Rule::choice(dropped))
}

fn node(args: &[&std::ffi::OsStr], cwd: &Path) -> std::process::Output {
    Command::new("node")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("node on PATH (this test is a dev-only oracle; see the module docs)")
}

#[test]
#[ignore = "needs Node.js on PATH; run with `cargo test --test node -- --ignored`"]
fn test_emitted_grammar_js_is_valid_javascript_and_means_what_grammar_json_says() {
    const GRAMMARS: usize = 300;
    let root: PathBuf = Path::new(env!("CARGO_TARGET_TMPDIR")).join("node-check");
    let _stale = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut runner = TestRunner::deterministic();
    for i in 0..GRAMMARS {
        let g = grammar(&mut runner);
        let dir = root.join(format!("g{i:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("grammar.js"), g.to_js().unwrap()).unwrap();
        std::fs::write(dir.join("grammar.json"), g.to_json().unwrap()).unwrap();

        let check = node(&["--check".as_ref(), "grammar.js".as_ref()], &dir);
        assert!(
            check.status.success(),
            "node --check {}:\n{}",
            dir.display(),
            String::from_utf8_lossy(&check.stderr)
        );
    }

    std::fs::write(root.join("..").join("node-check.js"), CHECK_JS).unwrap();
    let script = root.join("..").join("node-check.js");
    let eval = node(&[script.as_os_str(), root.as_os_str()], &root);
    assert!(
        eval.status.success(),
        "grammar.js evaluated to something other than grammar.json:\n{}",
        String::from_utf8_lossy(&eval.stderr)
    );
}
