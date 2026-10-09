# treesitter-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../../_strategy/LANG_COLLECTION.md
>
> **Anti-deferral rule:** no listed hard task moves to a later phase unless this file records the move and the reason.

## v0.1.0 - Scaffold (DONE)
Compiles, CI green, structure correct, no domain logic.
- [x] Manifest, README, CHANGELOG, REPS, dual license, CI, deny, clippy, rustfmt.

## v0.2.0 - Core (THE HARD PART, NOT DEFERRED) (DONE)
Tree-sitter grammar generation from the syntax/CST grammar.
Dependencies (wires syntax) are wired here, when first used.
Exit criteria:
- [x] Every public item has rustdoc + a runnable example.
- [x] Core invariants property-tested (full DIRECTIVES + API authored at this stage).

Delivered 2026-10-07: `Rule` (tree-sitter's rule language, one constructor
per DSL function), `Grammar` (builder, validation, `grammar.js` and
`grammar.json` emission, `sexp` rendering), `Error`, and the `syntax_lang`
re-export. Validation mirrors tree-sitter's own refusals, each probed against
the tree-sitter 0.27 CLI before it was written down; the emitted
`grammar.json` is byte-identical to the one tree-sitter derives from the
emitted `grammar.js`, both generate the same `parser.c`, and a corpus built
from a hand-written parser's trees passes `tree-sitter test`. All three are
checked in CI. Both formats round-trip through independent parsers in the
property tests. Scaffold defects fixed on the way in: unquoted `keywords` /
`categories` in `Cargo.toml` (the manifest did not parse), clippy MSRV
`1.87` → `1.85`, a stale project name in `deny.toml`, byte-order marks in
docs, and a dead link to `dev/DIRECTIVES.md`.

How "from the syntax/CST grammar" was realized, recorded under the
anti-deferral rule:

- **The CST does not carry a grammar, so none is inferred from it.**
  `syntax-lang` trees record what was parsed, not the rules that allow it;
  inferring a grammar from sample trees cannot be done reliably. The grammar
  is written in Rust instead, rule names matching the CST's kind names, and
  the two are held together by corpus tests: `Grammar::sexp` renders the
  hand-written parser's trees as tree-sitter's S-expressions, applying
  tree-sitter's rules for hidden, inlined, token, and external rules, and
  `tree-sitter test` compares. This is a design decision, not a deferral.
- **`syntax-lang` — wired.** `Grammar::sexp` reads `syntax_lang::Node`, and
  the crate is re-exported whole so callers build trees with the same
  version.
- **`serde` and `loom` — removed.** The scaffold declared both with no code
  behind them. There is no shared mutable state for `loom` to check, and the
  emitted formats are the serialization.

## v1.0.0 - API freeze (DONE)
Public surface stable and frozen until 2.0.
- [x] docs/API.md marked stable; SemVer promise recorded.
- [x] Full test + benchmark suite green on all three platforms.

Shipped 2026-10-07, with the 0.2.0 surface unchanged. Before freezing, an
adversarial review attacked the crate's claims with hand-built grammars, and
a differential fuzzer ran tens of thousands of random grammars through both
validation and `tree-sitter generate`. Every disagreement was traced to its
cause in tree-sitter and fixed in 0.2.0 before release: checks that depend on
how rules are used now apply only to reachable rules; the word, external,
inline, and supertype checks follow tree-sitter's rules for which rules are
tokens (identical tokens merge, hidden literal tokens are anonymous strings,
supertypes count as hidden); supertype shapes are counted the way tree-sitter
expands them; cycles of rules that are each just the next, and three grammar
shapes that crash or hang tree-sitter, are refused; duplicate `inline`
entries, control characters in patterns, and empty `seq()` inside tokens are
emitted the way tree-sitter needs. On the final code the fuzzer found no
disagreement beyond the documented ones: the stricter checks (empty
`choice`, an `inline` entry naming nothing, empty patterns and strings) and
what only tree-sitter's parse tables reveal (conflicts, and extras whose end
it cannot tell). The surface, the validation and output contracts, the
`sexp` rendering rules, `syntax-lang` 1 as a public dependency, and MSRV 1.85
are recorded as the contract in `docs/API.md#stability`. Tests and
benchmarks green on Windows and Linux (WSL2) locally; macOS through the CI
matrix.

Additive 1.x candidates (not commitments):

- Named precedences: tree-sitter's `precedences` field, and a rule
  constructor for a named level.
- Reserved-word sets (tree-sitter 0.25's `reserved`).
- Pattern flags (`/.../i`).
- Anonymous aliases (`alias(rule, 'text')`).
- A `Grammar::check` that validates without emitting.

## v1.0.1 - Security patch (DONE)
Fixes ISSUES H06 and the validation part of M79 in `lang-collection/_lexersketch/ISSUES.md`.
No public API added or removed.
- [x] Every pattern validated against both parsers that read it; invalid ones refused with the reason and byte offset.
- [x] Emission safe without validation: no regular expression literal can extend past its closing slash.
- [x] Class-aware test reader, H06 regression test, hostile-pattern properties, Node.js oracle job.

Delivered 2026-10-08:

- **H06, code injection through `grammar.js`.** A pattern with an unclosed
  `[` kept its `/.../` literal open, so the text after it in `grammar.js`
  became code when tree-sitter evaluated the file. Fixed in validation and,
  independently, in emission (`src/text.rs`: from the first class that would
  never close, every `[` is written `\[`; `src/js.rs`: an empty pattern is
  written `new RegExp('')`).
- **Pattern validation** (`src/pattern.rs`): two recognizers, one per parser
  a pattern meets. The JavaScript one follows ECMAScript Annex B (no flags,
  ES2025 modifier groups and named references included) and runs on every
  rule, because `grammar.js` evaluates every rule. The tree-sitter one
  follows regex-syntax 0.8's parser branch for branch, after tree-sitter's
  textual `\w`/`\s`/`\d`/`\W`/`\S`/`\D` rewrite, including its nesting limit
  of 250, plus tree-sitter's refusal of assertions that survive into the
  compiled expression; it runs on reachable rules and extras, the patterns
  tree-sitter reads. Both are single forward passes with explicit stacks.
  Reports reuse `Error::EmptyString` (the error type is frozen): `rule` is
  the rule's name, `: `, then reason, offset, and the start of the pattern.
- **How it was checked.** Against V8 (Node.js 24) and a replica of
  tree-sitter 0.27's pattern pipeline on regex-syntax 0.8.11, 500,000 random
  patterns (structured and chaotic) agreed except in the documented
  unchecked places; 60,000 deeply nested ones agreed on the nesting limit;
  the replica and the verdicts were confirmed against the real tree-sitter
  0.27.0 CLI (`--js-runtime node` and `native`) on 633 patterns, which
  disagreed only on unknown `\p{…}` names and on one huge counted repetition
  tree-sitter timed out on, and showed that QuickJS refuses modifier groups.
  These oracles are development tools: none is a dependency, and the only
  one in CI is Node.js, in the `node` job.
- **Left unchecked, documented in `docs/API.md#accepted-patterns`:**
  property names in `\p{…}` (they need Unicode tables), the
  `ID_Start`/`ID_Continue` class of non-ASCII group-name characters, and
  repeated group names in rules tree-sitter drops (engines differ: V8 in
  Node.js 24 accepts `(?<a>x(?<a>y)|z)`, which ES2025 refuses). Each is
  accepted rather than refused, so no working grammar is refused; at worst
  the error appears when `grammar.js` loads or tree-sitter generates, as
  before. None can end a literal early.

Dependency wiring: none. No dependency was added; `syntax-lang` 1 remains
the only one. regex-syntax, the tree-sitter CLI, and Node.js were used as
test oracles outside the crate; Node.js runs in CI as a dev-only oracle
(`actions/setup-node`), per LexerSketch decision D3.

## v1.1.0 - Planned (additive)
The rest of ISSUES M79, all additive to the frozen 1.0 surface:
- [ ] Named precedences: tree-sitter's `precedences` field, and a rule constructor for a named level.
- [ ] Reserved-word sets (tree-sitter 0.25's `reserved`).
- [ ] Anonymous aliases (`alias(rule, 'text')`).
- [ ] `Grammar::check`, validating without emitting.

Still candidates, not scheduled: pattern flags (`/.../i`). M79 also lists
external scanner generation, which is outside 1.1.0's scope until its design
is decided.
