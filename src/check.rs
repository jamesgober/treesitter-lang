//! Grammar validation: the checks `tree-sitter generate` would otherwise fail
//! on (or, in two cases, crash or hang on), run before anything is emitted.
//!
//! Every check mirrors a rule of tree-sitter's own, probed against the CLI.
//! They come in two kinds, as in tree-sitter:
//!
//! - **Load-time checks** apply to every rule, because `grammar.js` evaluates
//!   every rule: names are identifiers, symbols resolve, tokens contain no
//!   symbols, patterns are not empty.
//! - **Reachability checks** apply only to rules reachable from the start
//!   rule or the extras, because tree-sitter drops the rest before building
//!   the parser: strings are not empty, a rule other rules refer to cannot
//!   match nothing, nor can what a `repeat` repeats, no rules form a cycle
//!   of each being just the next, and supertypes produce one node per
//!   alternative.
//!
//! Tree-sitter merges identical tokens, so a rule whose whole body is a token
//! stays a token only if that token appears nowhere else among the reachable
//! rules; the word rule and any rule sharing an external's name must be such
//! a token. Some checks are stricter than tree-sitter, because what they
//! catch is always a mistake: an empty `choice` (accepted silently), an
//! `inline` entry naming nothing (only warned about), and empty patterns and
//! empty strings inside tokens (accepted in some places, but able to match
//! only nothing). Indirect recursion through
//! inlined rules, which makes tree-sitter loop forever, and a nullable extra
//! rule, which makes it crash, are reported as errors.
//!
//! Parse tables are not built here, so what tree-sitter finds while building
//! them — conflicts, and extras whose end it cannot tell — is left to it.
//! Differential fuzzing against the tree-sitter CLI, random grammars run
//! through both, found no other disagreement.
//!
//! Every walk is iterative, so a rule nested arbitrarily deep is checked
//! without recursion.

use alloc::{string::String, vec, vec::Vec};
use core::ptr;

use crate::{
    Error, Grammar, Rule,
    rule::{Expr, Prec, Text},
};

/// What a name refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Def {
    /// The rule at this position in `Grammar::rules`.
    Rule(usize),
    /// An external token.
    External,
}

/// Every rule and external of a grammar, sorted by name for binary search.
///
/// A name that is both a rule and an external resolves to the rule; a name
/// defined as two rules resolves to the first, and is reported by
/// [`Index::duplicate`].
pub(crate) struct Index<'g> {
    entries: Vec<(&'g str, Def)>,
}

impl<'g> Index<'g> {
    pub(crate) fn new(grammar: &'g Grammar) -> Index<'g> {
        let mut entries = Vec::with_capacity(grammar.rules.len() + grammar.externals.len());
        entries.extend(
            grammar
                .rules
                .iter()
                .enumerate()
                .map(|(i, (name, _))| (&**name, Def::Rule(i))),
        );
        entries.extend(
            grammar
                .externals
                .iter()
                .map(|name| (&**name, Def::External)),
        );
        // Rules sort before externals of the same name, and same-named rules
        // in definition order, so the first entry for a name is the one it
        // resolves to.
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0).then(rank(a.1).cmp(&rank(b.1))));
        Index { entries }
    }

    /// What `name` refers to, if anything.
    pub(crate) fn get(&self, name: &str) -> Option<Def> {
        let at = self.entries.partition_point(|(n, _)| *n < name);
        match self.entries.get(at) {
            Some(&(n, def)) if n == name => Some(def),
            _ => None,
        }
    }

    /// The rule name defined more than once whose second definition comes
    /// first, so the report follows definition order.
    fn duplicate(&self) -> Option<&'g str> {
        self.entries
            .windows(2)
            .filter_map(|pair| match (pair[0], pair[1]) {
                ((a, Def::Rule(_)), (b, Def::Rule(second))) if a == b => Some((second, a)),
                _ => None,
            })
            .min_by_key(|&(second, _)| second)
            .map(|(_, name)| name)
    }
}

/// The sort rank of a definition among those sharing a name.
fn rank(def: Def) -> usize {
    match def {
        Def::Rule(i) => i,
        Def::External => usize::MAX,
    }
}

/// Which rules refer to which, outside tokens, in compressed form: the rules
/// rule `i` refers to are `edges[starts[i]..starts[i + 1]]`.
struct Graph {
    starts: Vec<usize>,
    edges: Vec<usize>,
    /// The same form for the rules each rule can be alone — referred to at
    /// an alternative position, through `choice`, `prec`, `field`, and
    /// `alias` — itself excluded.
    unit_starts: Vec<usize>,
    units: Vec<usize>,
    /// Rules named by extras that are a lone symbol, in extras order.
    extra_roots: Vec<usize>,
}

impl Graph {
    fn edges(&self, rule: usize) -> &[usize] {
        &self.edges[self.starts[rule]..self.starts[rule + 1]]
    }

    fn units(&self, rule: usize) -> &[usize] {
        &self.units[self.unit_starts[rule]..self.unit_starts[rule + 1]]
    }
}

/// Which rules tree-sitter keeps, and which of those other rules refer to.
struct Reach {
    reachable: Vec<bool>,
    used: Vec<bool>,
}

/// Validates a grammar, returning the first problem found.
pub(crate) fn validate(grammar: &Grammar) -> Result<(), Error> {
    names(grammar)?;
    let index = Index::new(grammar);
    if let Some(name) = index.duplicate() {
        return Err(Error::DuplicateRule {
            name: String::from(name),
        });
    }
    let graph = load(grammar, &index)?;
    references(grammar, &index)?;
    let reach = reach(grammar, &graph);
    unit_cycles(grammar, &graph, &reach)?;
    reachable_rules(grammar, &reach)?;
    word(grammar, &index, &reach)?;
    externals(grammar, &index, &reach)?;
    inline(grammar, &index, &reach)?;
    supertypes(grammar, &index, &reach)
}

/// The grammar's, rules', and externals' names, and the start rule.
fn names(grammar: &Grammar) -> Result<(), Error> {
    identifier(&grammar.name)?;
    let Some((start, _)) = grammar.rules.first() else {
        return Err(Error::NoRules);
    };
    for name in grammar
        .rules
        .iter()
        .map(|(name, _)| name)
        .chain(&grammar.externals)
    {
        identifier(name)?;
        // `grammar.js` holds rules in an object literal and looks symbols up
        // as properties, where `__proto__` means the prototype, not a key.
        if name == "__proto__" {
            return Err(invalid_name(name));
        }
    }
    if is_hidden(start) {
        return Err(Error::HiddenStart {
            name: String::from(&**start),
        });
    }
    Ok(())
}

/// Whether a name is a valid tree-sitter identifier: an ASCII letter or `_`,
/// then ASCII letters, digits, and `_`.
fn identifier(name: &str) -> Result<(), Error> {
    let bytes = name.as_bytes();
    let valid = matches!(bytes.first(), Some(b) if b.is_ascii_alphabetic() || *b == b'_')
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_');
    if valid {
        Ok(())
    } else {
        Err(invalid_name(name))
    }
}

fn invalid_name(name: &str) -> Error {
    Error::InvalidName {
        name: String::from(name),
    }
}

/// Whether a rule name makes the rule hidden.
#[inline]
pub(crate) fn is_hidden(name: &str) -> bool {
    name.starts_with('_')
}

fn undefined(symbol: &str, rule: &str) -> Error {
    Error::UndefinedSymbol {
        symbol: String::from(symbol),
        rule: String::from(rule),
    }
}

/// The load-time checks on every rule body and every extra, collecting the
/// references between rules on the way.
fn load(grammar: &Grammar, index: &Index<'_>) -> Result<Graph, Error> {
    let count = grammar.rules.len();
    let mut graph = Graph {
        starts: Vec::with_capacity(count + 1),
        edges: Vec::new(),
        unit_starts: Vec::with_capacity(count + 1),
        units: Vec::new(),
        extra_roots: Vec::new(),
    };
    let mut stack = Vec::new();
    for (i, (name, body)) in grammar.rules.iter().enumerate() {
        graph.starts.push(graph.edges.len());
        graph.unit_starts.push(graph.units.len());
        let refs = Refs {
            edges: &mut graph.edges,
            units: &mut graph.units,
            rule: i,
        };
        load_rule(body, name, false, index, &mut stack, refs)?;
    }
    graph.starts.push(graph.edges.len());
    graph.unit_starts.push(graph.units.len());

    let (mut edges, mut units) = (Vec::new(), Vec::new());
    for extra in grammar.extras.iter().flatten() {
        match &extra.0 {
            // A lone symbol names a rule (or external) that may appear
            // anywhere; anything else is lexed as a token.
            Expr::Symbol(name) => match index.get(name) {
                None => return Err(undefined(name, "extras")),
                Some(Def::Rule(i)) => graph.extra_roots.push(i),
                Some(Def::External) => {}
            },
            _ => {
                // A token refers to no rules, so nothing is collected.
                let refs = Refs {
                    edges: &mut edges,
                    units: &mut units,
                    rule: usize::MAX,
                };
                load_rule(extra, "extras", true, index, &mut stack, refs)?;
            }
        }
    }
    Ok(graph)
}

/// Where [`load_rule`] records the rules a rule refers to.
struct Refs<'a> {
    /// Every rule referred to outside tokens.
    edges: &'a mut Vec<usize>,
    /// The rules referred to at an alternative position, other than itself.
    units: &'a mut Vec<usize>,
    /// The rule being loaded.
    rule: usize,
}

/// Checks one rule body in source order: symbols resolve and are not inside
/// a token, patterns are not empty, choices have alternatives, and field and
/// alias names are identifiers. Records the rules it refers to in `refs`.
fn load_rule<'g>(
    root: &'g Rule,
    rule: &str,
    lexical: bool,
    index: &Index<'_>,
    stack: &mut Vec<(&'g Rule, bool, bool)>,
    refs: Refs<'_>,
) -> Result<(), Error> {
    stack.clear();
    stack.push((root, lexical, true));
    while let Some((node, in_token, unit)) = stack.pop() {
        match &node.0 {
            Expr::Symbol(name) => {
                let def = index.get(name).ok_or_else(|| undefined(name, rule))?;
                if in_token {
                    return Err(Error::SymbolInToken {
                        symbol: String::from(&**name),
                        rule: String::from(rule),
                    });
                }
                if let Def::Rule(i) = def {
                    refs.edges.push(i);
                    if unit && i != refs.rule {
                        refs.units.push(i);
                    }
                }
            }
            Expr::Pattern(text) if text.is_empty() => {
                return Err(Error::EmptyString {
                    rule: String::from(rule),
                });
            }
            Expr::Choice(rules) if rules.is_empty() => {
                return Err(Error::EmptyChoice {
                    rule: String::from(rule),
                });
            }
            Expr::Field(name, _) | Expr::Alias(name, _) => identifier(name)?,
            _ => {}
        }
        let in_token = in_token || matches!(node.0, Expr::Token(_) | Expr::Immediate(_));
        let unit = unit
            && matches!(
                node.0,
                Expr::Choice(_) | Expr::Prec(..) | Expr::Field(..) | Expr::Alias(..)
            );
        stack.extend(
            node.children()
                .iter()
                .rev()
                .map(|child| (child, in_token, unit)),
        );
    }
    Ok(())
}

/// The names in `word`, `conflicts`, `inline`, and `supertypes` resolve.
fn references(grammar: &Grammar, index: &Index<'_>) -> Result<(), Error> {
    let defined = |name: &Text, place: &str| match index.get(name) {
        Some(_) => Ok(()),
        None => Err(undefined(name, place)),
    };
    if let Some(word) = &grammar.word {
        defined(word, "word")?;
    }
    for name in grammar.conflicts.iter().flatten() {
        defined(name, "conflicts")?;
    }
    for name in &grammar.inline {
        defined(name, "inline")?;
    }
    for name in &grammar.supertypes {
        defined(name, "supertypes")?;
    }
    Ok(())
}

/// The rules reachable from the start rule and the extras, and which of
/// them other reachable rules (or the extras) refer to.
fn reach(grammar: &Grammar, graph: &Graph) -> Reach {
    let count = grammar.rules.len();
    let mut reachable = vec![false; count];
    let mut used = vec![false; count];
    let mut queue = Vec::with_capacity(count);
    for &root in [0].iter().chain(&graph.extra_roots) {
        if !reachable[root] {
            reachable[root] = true;
            queue.push(root);
        }
    }
    for &root in &graph.extra_roots {
        used[root] = true;
    }
    while let Some(rule) = queue.pop() {
        for &next in graph.edges(rule) {
            used[next] = true;
            if !reachable[next] {
                reachable[next] = true;
                queue.push(next);
            }
        }
    }
    Reach { reachable, used }
}

/// For every reachable rule, in definition order: no empty string, and
/// nothing that can match nothing where tree-sitter needs something — the
/// rule itself if another rule refers to it, and anything a `repeat` or
/// `repeat1` repeats.
fn reachable_rules(grammar: &Grammar, reach: &Reach) -> Result<(), Error> {
    let mut walk = Vec::new();
    let mut frames = Vec::new();
    for (i, (name, body)) in grammar.rules.iter().enumerate() {
        if !reach.reachable[i] {
            continue;
        }
        // Tree-sitter refuses an empty string outside a token, and inside
        // one wherever it decides the token's value; elsewhere in a token it
        // can only match nothing. All are refused.
        walk.clear();
        walk.push(body);
        while let Some(node) = walk.pop() {
            if matches!(&node.0, Expr::String(text) if text.is_empty()) {
                return Err(Error::EmptyString {
                    rule: String::from(&**name),
                });
            }
            walk.extend(node.children());
        }
        let (empty, empty_repeat) = nullability(body, &mut frames);
        if empty_repeat || (empty && reach.used[i]) {
            return Err(Error::MatchesEmpty {
                rule: String::from(&**name),
            });
        }
    }
    Ok(())
}

/// One frame of [`nullability`]: a rule whose parts are being evaluated.
struct Frame<'g> {
    rule: &'g Rule,
    next: usize,
    value: bool,
}

/// Whether tree-sitter would give this rule body an empty production — can
/// it match nothing without consuming any token or symbol? — and whether
/// anything it repeats (outside tokens) could.
///
/// Symbols and tokens consume something by definition; a symbol that can
/// itself match nothing is reported at its own rule. Evaluated bottom-up with
/// an explicit stack, visiting every part once.
fn nullability<'g>(root: &'g Rule, frames: &mut Vec<Frame<'g>>) -> (bool, bool) {
    frames.clear();
    let mut empty_repeat = false;
    let mut current = root;
    loop {
        // Descend until a part's value is known.
        let mut value = match &current.0 {
            Expr::Blank => true,
            Expr::String(_)
            | Expr::Pattern(_)
            | Expr::Symbol(_)
            | Expr::Token(_)
            | Expr::Immediate(_) => false,
            Expr::Seq(rules) | Expr::Choice(rules) => match rules.first() {
                // An empty choice never matches (and is reported separately).
                None => matches!(current.0, Expr::Seq(_)),
                Some(first) => {
                    let value = matches!(current.0, Expr::Seq(_));
                    frames.push(Frame {
                        rule: current,
                        next: 1,
                        value,
                    });
                    current = first;
                    continue;
                }
            },
            Expr::Repeat(inner)
            | Expr::Repeat1(inner)
            | Expr::Field(_, inner)
            | Expr::Alias(_, inner)
            | Expr::Prec(_, _, inner) => {
                frames.push(Frame {
                    rule: current,
                    next: 1,
                    value: false,
                });
                current = inner;
                continue;
            }
        };
        // Return values upward until a parent needs another part.
        loop {
            let Some(frame) = frames.last_mut() else {
                return (value, empty_repeat);
            };
            let parent: &'g Rule = frame.rule;
            match &parent.0 {
                Expr::Seq(rules) | Expr::Choice(rules) => {
                    frame.value = if matches!(parent.0, Expr::Seq(_)) {
                        frame.value && value
                    } else {
                        frame.value || value
                    };
                    if let Some(next) = rules.get(frame.next) {
                        frame.next += 1;
                        current = next;
                        break;
                    }
                    value = frame.value;
                }
                Expr::Repeat(_) => {
                    empty_repeat |= value;
                    value = true;
                }
                Expr::Repeat1(_) => empty_repeat |= value,
                _ => {}
            }
            let _finished = frames.pop();
        }
    }
}

/// No cycle of two or more reachable rules in which each rule can be just the
/// next one — `a` can be `b` alone and `b` can be `a` alone — which
/// tree-sitter refuses as an "indirectly recursive rule". A rule can be
/// another alone when that rule is one of its alternatives, through
/// `choice`, `optional`, and the `prec`, `field`, and `alias` wrappers (not
/// through `seq` or a repetition). A rule that is itself directly is left to
/// tree-sitter, which reports it as a conflict — except a hidden rule that is
/// itself under a precedence, on which tree-sitter hangs.
fn unit_cycles(grammar: &Grammar, graph: &Graph, reach: &Reach) -> Result<(), Error> {
    const WHITE: u8 = 0;
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;
    let count = grammar.rules.len();

    // A hidden rule that can be itself under a precedence makes tree-sitter
    // loop forever.
    let mut walk = Vec::new();
    for (i, (name, body)) in grammar.rules.iter().enumerate() {
        if reach.reachable[i] && is_hidden(name) && itself_under_precedence(body, name, &mut walk) {
            return Err(Error::IndirectRecursion {
                rule: String::from(&**name),
            });
        }
    }

    let mut color = vec![WHITE; count];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in 0..count {
        if color[start] != WHITE || !reach.reachable[start] {
            continue;
        }
        color[start] = GRAY;
        stack.push((start, 0));
        while let Some(top) = stack.last_mut() {
            let (rule, at) = *top;
            let Some(&next) = graph.units(rule).get(at) else {
                color[rule] = BLACK;
                let _finished = stack.pop();
                continue;
            };
            top.1 += 1;
            if color[next] == GRAY {
                return Err(Error::IndirectRecursion {
                    rule: String::from(&*grammar.rules[next].0),
                });
            }
            if color[next] == WHITE {
                color[next] = GRAY;
                stack.push((next, 0));
            }
        }
    }
    Ok(())
}

/// Whether one alternative of a rule body is the rule itself under a
/// non-zero `prec`, `prec.left`, or `prec.right` — seen through `choice` and
/// sequences whose other parts are blank, but not through `field` or `alias`.
/// Tree-sitter never finishes generating a parser for a hidden rule like
/// that; a visible one, precedence zero, or `prec.dynamic` it handles.
fn itself_under_precedence<'g>(
    body: &'g Rule,
    name: &str,
    walk: &mut Vec<(&'g Rule, bool)>,
) -> bool {
    walk.clear();
    walk.push((body, false));
    while let Some((node, ranked)) = walk.pop() {
        match &node.0 {
            Expr::Symbol(symbol) if ranked && symbol == name => return true,
            Expr::Choice(alternatives) => {
                walk.extend(alternatives.iter().map(|a| (a, ranked)));
            }
            Expr::Prec(kind, level, inner) => {
                let ranked = ranked || (*kind != Prec::Dynamic && *level != 0);
                walk.push((inner, ranked));
            }
            Expr::Seq(parts) => {
                let mut solid = parts.iter().filter(|p| !matches!(p.0, Expr::Blank));
                if let (Some(part), None) = (solid.next(), solid.next()) {
                    walk.push((part, ranked));
                }
            }
            _ => {}
        }
    }
    false
}

/// Whether tree-sitter turns rule `i` into a token of its own: it is not the
/// start rule, its whole body is a token, and no reachable rule uses an
/// identical token anywhere else (tree-sitter would merge the two, making
/// the rule a non-terminal wrapper around the shared token). A hidden rule
/// whose token is just a string literal is never one: tree-sitter treats it
/// as that anonymous string. Supertypes count as hidden — tree-sitter hides
/// them before it looks for tokens — except when it decides what may be
/// inlined, which it does by the rule's own name (`supertypes_hidden`).
fn is_token_rule(grammar: &Grammar, reach: &Reach, i: usize, supertypes_hidden: bool) -> bool {
    let (name, target) = &grammar.rules[i];
    let hidden = is_hidden(name) || (supertypes_hidden && grammar.supertypes.contains(name));
    if i == 0 || !target.is_terminal() || (hidden && is_literal(target)) {
        return false;
    }
    let key = token_key(target);
    let mut walk = Vec::new();
    for (j, (_, body)) in grammar.rules.iter().enumerate() {
        if !reach.reachable[j] {
            continue;
        }
        walk.clear();
        walk.push(body);
        while let Some(node) = walk.pop() {
            if node.is_terminal() {
                if !ptr::eq(node, target) && token_key(node).same(key) {
                    return false;
                }
            } else {
                walk.extend(node.children());
            }
        }
    }
    true
}

/// A token as tree-sitter compares it with others: `token` around a single
/// string or pattern is that string or pattern.
fn token_key(rule: &Rule) -> &Rule {
    match &rule.0 {
        Expr::Token(inner) if matches!(inner.0, Expr::String(_) | Expr::Pattern(_)) => inner,
        _ => rule,
    }
}

/// Whether a token is just a string literal: a string, possibly inside one
/// `token` or `token.immediate`, and within that any number of `prec`,
/// `field`, and `alias`, which do not change what is lexed.
fn is_literal(rule: &Rule) -> bool {
    let mut rule = match &rule.0 {
        Expr::Token(inner) | Expr::Immediate(inner) => inner,
        _ => rule,
    };
    loop {
        match &rule.0 {
            Expr::Prec(_, _, inner) | Expr::Field(_, inner) | Expr::Alias(_, inner) => rule = inner,
            Expr::String(_) => return true,
            _ => return false,
        }
    }
}

/// The word rule is a token, or an external.
fn word(grammar: &Grammar, index: &Index<'_>, reach: &Reach) -> Result<(), Error> {
    let Some(word) = &grammar.word else {
        return Ok(());
    };
    match index.get(word) {
        Some(Def::Rule(i)) if !is_token_rule(grammar, reach, i, true) => Err(Error::WordNotToken {
            name: String::from(&**word),
        }),
        _ => Ok(()),
    }
}

/// An external may share its name with a rule only if that rule is a token,
/// which serves as the fallback when the external scanner declines.
fn externals(grammar: &Grammar, index: &Index<'_>, reach: &Reach) -> Result<(), Error> {
    for name in &grammar.externals {
        if let Some(Def::Rule(i)) = index.get(name) {
            if !is_token_rule(grammar, reach, i, true) {
                return Err(Error::InvalidExternal {
                    name: String::from(&**name),
                });
            }
        }
    }
    Ok(())
}

/// Inlined rules: not the start rule, not a token, not an external, and not
/// recursive through inlined rules alone.
fn inline(grammar: &Grammar, index: &Index<'_>, reach: &Reach) -> Result<(), Error> {
    const WHITE: u8 = 0;
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;

    if grammar.inline.is_empty() {
        return Ok(());
    }
    let count = grammar.rules.len();
    let mut inlined = vec![false; count];
    for name in &grammar.inline {
        let invalid = || Error::InvalidInline {
            name: String::from(&**name),
        };
        let Some(Def::Rule(i)) = index.get(name) else {
            return Err(invalid());
        };
        // Tree-sitter refuses to inline the rules it lexes as tokens.
        if i == 0 || is_token_rule(grammar, reach, i, false) {
            return Err(invalid());
        }
        inlined[i] = true;
    }

    // A cycle through inlined rules alone would expand forever: depth-first
    // search over the references between inlined rules, in inline order. A
    // reference inside a repetition does not count: tree-sitter moves it
    // into a helper rule of its own, which is not inlined.
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut walk = Vec::new();
    for (i, (_, body)) in grammar.rules.iter().enumerate() {
        if !inlined[i] {
            continue;
        }
        walk.clear();
        walk.push(body);
        while let Some(node) = walk.pop() {
            match &node.0 {
                Expr::Symbol(name) => {
                    if let Some(Def::Rule(j)) = index.get(name) {
                        if inlined[j] {
                            edges[i].push(j);
                        }
                    }
                }
                Expr::Repeat(_) | Expr::Repeat1(_) | Expr::Token(_) | Expr::Immediate(_) => {}
                _ => walk.extend(node.children()),
            }
        }
    }
    let mut color = vec![WHITE; count];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for name in &grammar.inline {
        let Some(Def::Rule(start)) = index.get(name) else {
            continue;
        };
        if color[start] != WHITE {
            continue;
        }
        color[start] = GRAY;
        stack.push((start, 0));
        while let Some(top) = stack.last_mut() {
            let (rule, at) = *top;
            let Some(&next) = edges[rule].get(at) else {
                color[rule] = BLACK;
                let _finished = stack.pop();
                continue;
            };
            top.1 += 1;
            if color[next] == GRAY {
                return Err(Error::InvalidInline {
                    name: String::from(&*grammar.rules[next].0),
                });
            }
            if color[next] == WHITE {
                color[next] = GRAY;
                stack.push((next, 0));
            }
        }
    }
    Ok(())
}

/// Supertypes: rules, and — when reachable — not a token with a pattern in
/// it, never themselves, and producing a single node in every alternative.
fn supertypes(grammar: &Grammar, index: &Index<'_>, reach: &Reach) -> Result<(), Error> {
    if grammar.supertypes.is_empty() {
        return Ok(());
    }
    let nodes = NodeCounts::new(grammar, index);
    let mut walk = Vec::new();
    let mut expanded = Vec::new();
    for name in &grammar.supertypes {
        let invalid = || Error::InvalidSupertype {
            name: String::from(&**name),
        };
        let Some(Def::Rule(i)) = index.get(name) else {
            return Err(invalid());
        };
        // An inlined supertype is expanded away before supertypes are
        // looked at, and an unreachable one is dropped.
        if !reach.reachable[i] || grammar.inline.contains(name) {
            continue;
        }
        let body = &grammar.rules[i].1;
        // A supertype is hidden, so a literal is not a token here; one that
        // tree-sitter lexes as a token of its own is refused.
        if is_token_rule(grammar, reach, i, true) || nodes.most(body) > 1 {
            return Err(invalid());
        }
        // A supertype that can be itself — directly, or through the hidden
        // and inlined rules tree-sitter expands in place — would make its node
        // types depend on themselves. An alias names something else.
        walk.clear();
        walk.push(body);
        expanded.clear();
        expanded.resize(grammar.rules.len(), false);
        while let Some(node) = walk.pop() {
            match &node.0 {
                Expr::Symbol(symbol) if symbol == name => return Err(invalid()),
                Expr::Symbol(symbol) => {
                    if let Some(Def::Rule(j)) = index.get(symbol) {
                        if nodes.expands[j] && !expanded[j] {
                            expanded[j] = true;
                            walk.push(&grammar.rules[j].1);
                        }
                    }
                }
                Expr::Alias(..) | Expr::Token(_) | Expr::Immediate(_) => {}
                _ => walk.extend(node.children()),
            }
        }
    }
    Ok(())
}

/// The most nodes one alternative of a rule can put in the tree, capped at
/// two ("more than one"), with hidden and inlined rules expanded in place as
/// tree-sitter expands them.
struct NodeCounts<'g, 'i> {
    index: &'i Index<'g>,
    /// Per rule: whether references to it expand, and the count it expands
    /// to.
    expands: Vec<bool>,
    counts: Vec<u8>,
}

impl<'g, 'i> NodeCounts<'g, 'i> {
    fn new(grammar: &'g Grammar, index: &'i Index<'g>) -> Self {
        let expands: Vec<bool> = grammar
            .rules
            .iter()
            .map(|(name, body)| {
                (is_hidden(name) || grammar.inline.contains(name)) && !body.is_terminal()
            })
            .collect();
        let mut counts = NodeCounts {
            index,
            counts: vec![0; expands.len()],
            expands,
        };
        // Counts only grow and are capped, so this settles, including
        // through recursive hidden rules.
        let mut changed = true;
        while changed {
            changed = false;
            for (i, (_, body)) in grammar.rules.iter().enumerate() {
                if counts.expands[i] {
                    let count = counts.most(body);
                    if count > counts.counts[i] {
                        counts.counts[i] = count;
                        changed = true;
                    }
                }
            }
        }
        counts
    }

    /// The most nodes one alternative of `root` produces, capped at two.
    ///
    /// An alias renames each element of what it wraps, so beneath one a
    /// hidden rule is a single (renamed) node rather than expanded.
    fn most(&self, root: &Rule) -> u8 {
        // Each frame: a rule whose parts are being combined, the next part,
        // the value so far, and whether an alias encloses the rule.
        let mut frames: Vec<(&Rule, usize, u8, bool)> = Vec::new();
        let mut current = root;
        let mut aliased = false;
        loop {
            let mut value = match &current.0 {
                Expr::Blank => 0,
                Expr::String(_) | Expr::Pattern(_) | Expr::Token(_) | Expr::Immediate(_) => 1,
                Expr::Symbol(name) => match self.index.get(name) {
                    Some(Def::Rule(j)) if self.expands[j] && !aliased => self.counts[j],
                    _ => 1,
                },
                Expr::Choice(rules) if rules.is_empty() => 0,
                // Tree-sitter turns a repetition into a helper rule that
                // refers to itself twice: several nodes, whatever it repeats.
                Expr::Repeat(_) | Expr::Repeat1(_) => 2,
                _ => {
                    // Sequences, choices, fields, precedences, aliases:
                    // combine their parts. A sequence is never empty (an
                    // empty one is a blank).
                    frames.push((current, 1, 0, aliased));
                    aliased = aliased || matches!(current.0, Expr::Alias(..));
                    current = &current.children()[0];
                    continue;
                }
            };
            loop {
                let Some(frame) = frames.last_mut() else {
                    return value;
                };
                let parent = frame.0;
                frame.2 = match parent.0 {
                    Expr::Seq(_) => frame.2.saturating_add(value).min(2),
                    Expr::Choice(_) => frame.2.max(value),
                    _ => value,
                };
                if let Some(next) = parent.children().get(frame.1) {
                    frame.1 += 1;
                    aliased = frame.3 || matches!(parent.0, Expr::Alias(..));
                    current = next;
                    break;
                }
                value = frame.2;
                let _finished = frames.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty(rule: &Rule) -> bool {
        nullability(rule, &mut Vec::new()).0
    }

    fn empty_repeat(rule: &Rule) -> bool {
        nullability(rule, &mut Vec::new()).1
    }

    #[test]
    fn test_nullability_leaves() {
        assert!(empty(&Rule::blank()));
        assert!(!empty(&Rule::string("a")));
        assert!(!empty(&Rule::pattern("a*")));
        assert!(!empty(&Rule::symbol("a")));
        assert!(!empty(&Rule::token(Rule::blank())));
    }

    #[test]
    fn test_nullability_combinators() {
        assert!(empty(&Rule::seq([])));
        assert!(!empty(&Rule::choice([])));
        assert!(empty(&Rule::repeat(Rule::string("a"))));
        assert!(!empty(&Rule::repeat1(Rule::string("a"))));
        assert!(empty(&Rule::repeat1(Rule::blank())));
        assert!(empty(&Rule::optional(Rule::string("a"))));
        assert!(empty(&Rule::seq([
            Rule::blank(),
            Rule::optional(Rule::string("a"))
        ])));
        assert!(!empty(&Rule::seq([Rule::blank(), Rule::string("a")])));
        assert!(!empty(&Rule::seq([Rule::string("a"), Rule::blank()])));
        assert!(empty(&Rule::choice([Rule::string("a"), Rule::blank()])));
        assert!(!empty(&Rule::choice([
            Rule::string("a"),
            Rule::symbol("b")
        ])));
        assert!(empty(&Rule::field(
            "f",
            Rule::prec(1, Rule::repeat(Rule::string("a")))
        )));
        assert!(!empty(&Rule::alias(Rule::string("a"), "b")));
    }

    #[test]
    fn test_nullability_finds_empty_repeats_anywhere_outside_tokens() {
        assert!(!empty_repeat(&Rule::repeat(Rule::string("a"))));
        assert!(empty_repeat(&Rule::repeat(Rule::optional(Rule::string(
            "a"
        )))));
        assert!(empty_repeat(&Rule::repeat1(Rule::blank())));
        assert!(empty_repeat(&Rule::repeat1(Rule::repeat(Rule::string(
            "a"
        )))));
        let nested = Rule::seq([
            Rule::string("x"),
            Rule::choice([
                Rule::symbol("y"),
                Rule::repeat(Rule::optional(Rule::symbol("z"))),
            ]),
        ]);
        assert!(!empty(&nested));
        assert!(empty_repeat(&nested));
        // Inside a token, repetition is lexical and may match nothing.
        assert!(!empty_repeat(&Rule::token(Rule::repeat(Rule::optional(
            Rule::string("a")
        )))));
    }

    #[test]
    fn test_nullability_deep_chain_is_iterative() {
        let mut rule = Rule::blank();
        for _ in 0..100_000 {
            rule = Rule::seq([Rule::field("f", rule)]);
        }
        assert!(empty(&rule));
    }

    #[test]
    fn test_identifier_rules() {
        for ok in ["a", "_", "_a1", "Snake_Case9", "class"] {
            assert_eq!(identifier(ok), Ok(()), "{ok}");
        }
        for bad in ["", "1a", "a-b", "a b", "é", "a.b", "$a"] {
            assert!(identifier(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_index_resolves_rules_before_externals() {
        let g = Grammar::new("g")
            .rule("b", Rule::string("b"))
            .rule("a", Rule::string("a"))
            .external("a")
            .external("z");
        let index = Index::new(&g);
        assert_eq!(index.get("a"), Some(Def::Rule(1)));
        assert_eq!(index.get("b"), Some(Def::Rule(0)));
        assert_eq!(index.get("z"), Some(Def::External));
        assert_eq!(index.get("c"), None);
        assert_eq!(index.duplicate(), None);
    }

    #[test]
    fn test_duplicate_reports_earliest_second_definition() {
        let g = Grammar::new("g")
            .rule("s", Rule::string("s"))
            .rule("z", Rule::string("1"))
            .rule("a", Rule::string("1"))
            .rule("z", Rule::string("2"))
            .rule("a", Rule::string("2"));
        assert_eq!(Index::new(&g).duplicate(), Some("z"));
    }

    #[test]
    fn test_reach_follows_references_and_extras() {
        let g = Grammar::new("g")
            .extra(Rule::symbol("note"))
            .rule("s", Rule::repeat(Rule::symbol("a")))
            .rule("a", Rule::string("a"))
            .rule("orphan", Rule::symbol("a"))
            .rule("note", Rule::seq([Rule::string("#"), Rule::symbol("text")]))
            .rule("text", Rule::pattern("[a-z]+"));
        let index = Index::new(&g);
        let graph = load(&g, &index);
        assert!(graph.is_ok());
        let Ok(graph) = graph else { return };
        let reach = reach(&g, &graph);
        assert_eq!(reach.reachable, [true, true, false, true, true]);
        assert_eq!(reach.used, [false, true, false, true, true]);
    }

    #[test]
    fn test_node_counts_expand_hidden_rules() {
        let g = Grammar::new("g")
            .rule("s", Rule::blank())
            .rule(
                "_one",
                Rule::choice([Rule::symbol("a"), Rule::symbol("_nested")]),
            )
            .rule("_nested", Rule::symbol("a"))
            .rule("_two", Rule::seq([Rule::symbol("a"), Rule::string(";")]))
            .rule(
                "_list",
                Rule::seq([Rule::symbol("a"), Rule::optional(Rule::symbol("_list"))]),
            )
            .rule("a", Rule::string("a"));
        let index = Index::new(&g);
        let counts = NodeCounts::new(&g, &index);
        let most = |name: &str| match index.get(name) {
            Some(Def::Rule(i)) => counts.most(&g.rules[i].1),
            _ => u8::MAX,
        };
        assert_eq!(most("s"), 0);
        assert_eq!(most("_one"), 1);
        assert_eq!(most("_two"), 2);
        assert_eq!(most("_list"), 2);
        assert_eq!(counts.most(&Rule::repeat(Rule::symbol("a"))), 2);
        assert_eq!(counts.most(&Rule::repeat(Rule::blank())), 2);
        assert_eq!(counts.most(&Rule::alias(Rule::symbol("_two"), "x")), 1);
    }
}
