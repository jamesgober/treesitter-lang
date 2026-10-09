//! Pattern validation: which regular expressions tree-sitter can use.
//!
//! A pattern travels twice before it becomes part of a lexer. First the
//! JavaScript runtime that evaluates `grammar.js` parses it as a regular
//! expression literal, with no flags, under the web-compatibility grammar of
//! ECMAScript Annex B (B.1.2). Then tree-sitter reads the literal's source
//! back, rewrites the Perl classes `\w`, `\s`, `\d`, `\W`, `\S`, and `\D`
//! textually into bracketed classes, and parses the result with the Rust
//! `regex-syntax` crate (0.8, default parser settings: a nesting limit of
//! 250, no octal escapes, no whitespace mode, Unicode on). It then refuses
//! any assertion — `^`, `$`, `\b`, `\B`, `\A`, `\z`, `\<`, `\>`, `\b{...}` —
//! that survives into the compiled expression, which only happens to one
//! inside a repetition of exactly zero.
//!
//! A pattern works only if both parsers accept it, and the two disagree in
//! many places: JavaScript reads `[` inside a class as a plain character
//! where regex-syntax opens a nested class; JavaScript takes `\x{41}` as `x`
//! repeated 41 times where regex-syntax takes it as `A`; regex-syntax refuses
//! `\c`, `\0`, `\e`, and `{` without a count that JavaScript accepts. So the
//! checks here are two recognizers, one per parser, each following its own
//! grammar, and a pattern is valid when both accept it:
//!
//! - [`Checker::javascript`] — the JavaScript parse. A load-time check: it
//!   applies to every pattern, because `grammar.js` evaluates every rule.
//! - [`Checker::tree_sitter`] — the regex-syntax parse and the assertion
//!   rule. Tree-sitter only parses the patterns of rules it keeps, so this
//!   applies only to reachable rules and to the extras.
//!
//! Both recognizers read the pattern exactly as `grammar.js` writes it
//! between the slashes (see [`crate::text::js_regex`]), so a rewritten `/`,
//! control character, or trailing backslash is checked in the form the
//! parsers see — but without the `\[` that `js_regex` writes for a class
//! that never closes, so such a pattern is refused rather than repaired.
//!
//! The JavaScript recognizer is applied alone to rules tree-sitter drops, so
//! it must not refuse anything a runtime accepts. Where it cannot be exact it
//! accepts instead (see [`Js`]): non-ASCII group-name characters, and
//! repeated group names. The regex-syntax recognizer does not look up the
//! names in `\p{...}` and `\P{...}`, which needs Unicode tables this crate
//! does not carry; `tree-sitter generate` reports an unknown one. Neither
//! bounds counted repetitions: `a{100000}` is valid, and slow for
//! tree-sitter to expand.
//!
//! Both recognizers are forward passes with explicit stacks, so nesting
//! depth costs no recursion. Each character is read a bounded number of
//! times (a look-ahead that fails is never repeated over the same text), and
//! group names go through ordered sets: `O(n log n)` in the pattern's length
//! at worst, linear without named groups.

use alloc::{collections::BTreeSet, string::String, vec::Vec};

/// Why a pattern is not a regular expression tree-sitter can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Invalid {
    /// The byte offset in the pattern, as given, where the problem is.
    pub(crate) at: usize,
    /// What is wrong.
    pub(crate) reason: &'static str,
}

/// One character of the pattern as a parser sees it: a UTF-16 code unit for
/// JavaScript, a code point for regex-syntax. `at` is the byte offset in the
/// original pattern of the character it came from.
#[derive(Clone, Copy, Debug)]
struct Unit {
    c: u32,
    at: usize,
}

/// Reusable buffers, so validating a grammar allocates once for all its
/// patterns rather than once per pattern.
#[derive(Debug, Default)]
pub(crate) struct Checker {
    units: Vec<Unit>,
    /// JavaScript: the open groups, and the group names as code points.
    js_groups: Vec<JsGroup>,
    js_names: BTreeSet<Vec<u32>>,
    /// regex-syntax: the open groups and the open classes.
    groups: Vec<Frame>,
    brackets: Vec<Bracket>,
    names: BTreeSet<String>,
}

impl Checker {
    /// Whether a JavaScript runtime accepts the pattern as a `/.../`
    /// literal: ECMAScript Annex B syntax, without flags.
    pub(crate) fn javascript(&mut self, pattern: &str) -> Result<(), Invalid> {
        let units = &mut self.units;
        units.clear();
        literal_body(pattern, &mut |c, at| {
            let c = u32::from(c);
            // Without the `u` flag a pattern is a sequence of UTF-16 code
            // units: a character outside the Basic Multilingual Plane is two
            // atoms, which matters to quantifiers and class ranges.
            if c > 0xFFFF {
                let c = c - 0x1_0000;
                units.push(Unit {
                    c: 0xD800 | (c >> 10),
                    at,
                });
                units.push(Unit {
                    c: 0xDC00 | (c & 0x3FF),
                    at,
                });
            } else {
                units.push(Unit { c, at });
            }
        });
        Js {
            u: &self.units,
            end: pattern.len(),
            groups: &mut self.js_groups,
            names: &mut self.js_names,
            named: false,
        }
        .run()
    }

    /// Whether tree-sitter accepts the pattern: regex-syntax parses it after
    /// tree-sitter's Perl-class rewrite, and no assertion survives.
    pub(crate) fn tree_sitter(&mut self, pattern: &str) -> Result<(), Invalid> {
        let units = &mut self.units;
        units.clear();
        // Tree-sitter's rewrite is textual: a `\` followed by one of the six
        // letters is replaced wherever it occurs, even when that `\` is
        // itself escaped (`\\d`). The replacements contain no `\` that could
        // start another match.
        let mut backslash: Option<usize> = None;
        literal_body(pattern, &mut |c, at| {
            if let Some(slash) = backslash.take() {
                if let Some(text) = perl_class(c) {
                    units.extend(text.chars().map(|c| Unit {
                        c: u32::from(c),
                        at: slash,
                    }));
                    return;
                }
                units.push(Unit {
                    c: u32::from('\\'),
                    at: slash,
                });
            }
            if c == '\\' {
                backslash = Some(at);
            } else {
                units.push(Unit {
                    c: u32::from(c),
                    at,
                });
            }
        });
        if let Some(slash) = backslash {
            units.push(Unit {
                c: u32::from('\\'),
                at: slash,
            });
        }
        self.groups.clear();
        self.brackets.clear();
        self.names.clear();
        Rx {
            u: &self.units,
            end: pattern.len(),
            groups: &mut self.groups,
            brackets: &mut self.brackets,
            names: &mut self.names,
        }
        .run()
    }
}

/// What tree-sitter writes in place of `\` followed by `letter`.
fn perl_class(letter: char) -> Option<&'static str> {
    Some(match letter {
        'w' => "[0-9A-Za-z_]",
        's' => "[\\t-\\r ]",
        'd' => "[0-9]",
        'W' => "[^0-9A-Za-z_]",
        'S' => "[^\\t-\\r ]",
        'D' => "[^0-9]",
        _ => return None,
    })
}

/// The pattern as [`crate::text::js_regex`] writes it between the slashes,
/// one character at a time, each tagged with the offset of the one it came
/// from: `/` gains a backslash, a control character or line separator — raw
/// or escaped — becomes its escape, and a trailing lone backslash is doubled.
fn literal_body(pattern: &str, emit: &mut impl FnMut(char, usize)) {
    fn text(emit: &mut impl FnMut(char, usize), text: &str, at: usize) {
        for c in text.chars() {
            emit(c, at);
        }
    }
    fn control(emit: &mut impl FnMut(char, usize), c: char, at: usize) {
        match c {
            '\n' => text(emit, "\\n", at),
            '\r' => text(emit, "\\r", at),
            '\t' => text(emit, "\\t", at),
            '\u{2028}' => text(emit, "\\u2028", at),
            '\u{2029}' => text(emit, "\\u2029", at),
            _ => {
                let b = u32::from(c);
                text(emit, "\\x", at);
                for nibble in [b >> 4, b & 0xF] {
                    // Both nibbles are below 16, so the digit always exists.
                    emit(char::from_digit(nibble, 16).unwrap_or('0'), at);
                }
            }
        }
    }
    let rewritten = |c: char| c < ' ' || c == '\u{2028}' || c == '\u{2029}';

    let mut chars = pattern.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '/' => text(emit, "\\/", at),
            '\\' => match chars.peek().copied() {
                None => text(emit, "\\\\", at),
                Some((_, n)) if rewritten(n) => {
                    let _escaped = chars.next();
                    control(emit, n, at);
                }
                Some((next_at, n)) => {
                    let _escaped = chars.next();
                    emit('\\', at);
                    emit(n, next_at);
                }
            },
            c if rewritten(c) => control(emit, c, at),
            c => emit(c, at),
        }
    }
}

// --- Reasons ------------------------------------------------------------------

const NOTHING_TO_REPEAT: &str = "a quantifier has nothing to repeat";
const QUANTIFIER_ORDER: &str = "the numbers in a `{}` quantifier are out of order";
const UNMATCHED_CLOSE: &str = "unmatched `)`";
const UNCLOSED_GROUP: &str = "unterminated group";
const UNCLOSED_CLASS: &str = "unterminated character class";
const CLASS_ORDER: &str = "a character class range is out of order";
const INVALID_GROUP: &str = "invalid group: `(?` must start `(?:`, `(?<name>`, or a modifier group";
const INVALID_MODIFIERS: &str = "invalid modifier group: a flag is repeated or none is given";
const GROUP_NAME: &str = "invalid capture group name";
const GROUP_NAME_UNCLOSED: &str = "unterminated capture group name";
const GROUP_NAME_EMPTY: &str = "empty capture group name";
const GROUP_NAME_DUPLICATE: &str = "duplicate capture group name";
const NAMED_REFERENCE: &str = "invalid named reference: `\\k` must be `\\k<name>` naming a group";
const LOOK_AROUND: &str = "look-ahead and look-behind are not supported by tree-sitter";
const ASSERTION: &str = "assertions (`^`, `$`, `\\b`, `\\B`, ...) are not supported by tree-sitter";
const BACKREFERENCE: &str =
    "backreferences and octal escapes (`\\0` to `\\9`) are not supported by tree-sitter";
const UNKNOWN_ESCAPE: &str = "unrecognized escape sequence";
const ESCAPE_EOF: &str = "incomplete escape sequence";
const HEX_DIGIT: &str = "invalid hexadecimal digit in an escape";
const HEX_VALUE: &str = "the escape is not a Unicode scalar value (surrogates are not allowed)";
const HEX_EMPTY: &str = "empty hexadecimal escape";
const UNICODE_CLASS: &str = "invalid Unicode class `\\p`";
const WORD_BOUNDARY: &str = "invalid special word boundary `\\b{...}`";
const REPEAT_UNCLOSED: &str = "unclosed counted repetition `{`";
const REPEAT_EMPTY: &str = "a counted repetition needs a number";
const REPEAT_TOO_BIG: &str = "a repetition count does not fit in 32 bits";
const REPEAT_ORDER: &str = "the numbers in a counted repetition are out of order";
const CLASS_RANGE_ENDPOINT: &str = "a character class range must run between two characters";
const CLASS_ASSERTION: &str = "an assertion cannot appear in a character class";
const FLAG_UNKNOWN: &str = "unrecognized flag";
const FLAG_REPEATED: &str = "a flag or `-` is repeated";
const FLAG_DANGLING: &str = "a `-` in a flag group must be followed by a flag";
const FLAG_EMPTY: &str = "an empty flag group `(?)` has nothing to repeat";
const NESTING: &str = "nested more than 250 levels deep";

fn invalid(at: usize, reason: &'static str) -> Invalid {
    Invalid { at, reason }
}

fn is_ascii_char(c: u32, ch: u8) -> bool {
    c == u32::from(ch)
}

fn hex_value(c: u32) -> Option<u32> {
    char::from_u32(c).and_then(|c| c.to_digit(16))
}

// --- JavaScript ---------------------------------------------------------------

/// A recognizer for ECMAScript regular expression patterns without the `u`
/// or `v` flag, Annex B grammar (the one every web-compatible engine
/// implements). It checks syntax and the early errors: quantifier targets,
/// `{n,m}` order, class range order, group syntax, named references, and
/// modifier groups (ES2025).
///
/// It is exact but for two rules, where it accepts rather than risk
/// refusing what a JavaScript runtime accepts:
///
/// - A non-ASCII character of a group name is not held to `ID_Start` or
///   `ID_Continue`, for which this crate carries no Unicode tables.
/// - Repeated group names are not looked for. ES2025 allows a name to repeat
///   in different alternatives, and engines differ on exactly where: V8 in
///   Node.js 24 accepts `(?<a>x(?<a>y)|z)`, which the specification refuses,
///   while older engines refuse every repeat.
///
/// Neither can make the literal end early: at worst the runtime reports the
/// error when it loads `grammar.js`. And in the rules tree-sitter keeps,
/// regex-syntax refuses repeated names, and its own name rule applies.
struct Js<'a> {
    u: &'a [Unit],
    end: usize,
    groups: &'a mut Vec<JsGroup>,
    names: &'a mut BTreeSet<Vec<u32>>,
    /// Whether the pattern has a named group, which makes every `\k` a
    /// named reference (Annex B).
    named: bool,
}

/// An open JavaScript group: where it opened, and whether it is a
/// lookbehind, which no quantifier may follow.
#[derive(Clone, Copy, Debug)]
struct JsGroup {
    at: usize,
    lookbehind: bool,
}

impl Js<'_> {
    fn c(&self, i: usize) -> Option<u32> {
        self.u.get(i).map(|u| u.c)
    }

    fn is(&self, i: usize, ch: u8) -> bool {
        self.c(i).is_some_and(|c| is_ascii_char(c, ch))
    }

    fn at(&self, i: usize) -> usize {
        self.u.get(i).map_or(self.end, |u| u.at)
    }

    fn err(&self, i: usize, reason: &'static str) -> Invalid {
        invalid(self.at(i), reason)
    }

    fn hex(&self, i: usize, n: usize) -> Option<u32> {
        (i..i + n).try_fold(0, |v, j| Some(v * 16 + hex_value(self.c(j)?)?))
    }

    fn run(&mut self) -> Result<(), Invalid> {
        self.groups.clear();
        self.collect_names();
        let mut i = 0;
        // Whether the term just read may take a quantifier: atoms and
        // lookaheads may; nothing, assertions, and quantifiers may not.
        let mut quantifiable = false;
        while let Some(c) = self.c(i) {
            match char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER) {
                '|' => {
                    quantifiable = false;
                    i += 1;
                }
                '(' => {
                    i = self.open_group(i)?;
                    quantifiable = false;
                }
                ')' => {
                    let Some(group) = self.groups.pop() else {
                        return Err(self.err(i, UNMATCHED_CLOSE));
                    };
                    quantifiable = !group.lookbehind;
                    i += 1;
                }
                '[' => {
                    i = self.class(i)?;
                    quantifiable = true;
                }
                '\\' => (i, quantifiable) = self.escape(i)?,
                '^' | '$' => {
                    quantifiable = false;
                    i += 1;
                }
                '*' | '+' | '?' => {
                    if !quantifiable {
                        return Err(self.err(i, NOTHING_TO_REPEAT));
                    }
                    i = self.lazy(i + 1);
                    quantifiable = false;
                }
                '{' => match self.braced_quantifier(i) {
                    Some((next, in_order)) => {
                        if !quantifiable {
                            return Err(self.err(i, NOTHING_TO_REPEAT));
                        }
                        if !in_order {
                            return Err(self.err(i, QUANTIFIER_ORDER));
                        }
                        i = self.lazy(next);
                        quantifiable = false;
                    }
                    // Not a quantifier: in Annex B a `{` is then a literal.
                    None => {
                        quantifiable = true;
                        i += 1;
                    }
                },
                // Everything else, `]` and `}` included, is a literal.
                _ => {
                    quantifiable = true;
                    i += 1;
                }
            }
        }
        match self.groups.last() {
            Some(group) => Err(invalid(group.at, UNCLOSED_GROUP)),
            None => Ok(()),
        }
    }

    /// Skips the `?` that makes a quantifier lazy.
    fn lazy(&self, i: usize) -> usize {
        if self.is(i, b'?') { i + 1 } else { i }
    }

    /// `{n}`, `{n,}`, or `{n,m}` at `i`: where it ends and whether `n <= m`.
    fn braced_quantifier(&self, i: usize) -> Option<(usize, bool)> {
        let digits = |mut j: usize| {
            let start = j;
            while self.c(j).is_some_and(|c| (0x30..=0x39).contains(&c)) {
                j += 1;
            }
            (start < j).then_some((start, j))
        };
        let (min_start, j) = digits(i + 1)?;
        if self.is(j, b'}') {
            return Some((j + 1, true));
        }
        if !self.is(j, b',') {
            return None;
        }
        if self.is(j + 1, b'}') {
            return Some((j + 2, true));
        }
        let (max_start, k) = digits(j + 1)?;
        if !self.is(k, b'}') {
            return None;
        }
        Some((k + 1, self.decimal_le(min_start, j, max_start, k)))
    }

    /// Whether the decimal digits in `a..a_end` are at most those in
    /// `b..b_end`, compared as numbers of any size.
    fn decimal_le(&self, a: usize, a_end: usize, b: usize, b_end: usize) -> bool {
        let skip_zeros = |mut j: usize, end: usize| {
            while j + 1 < end && self.is(j, b'0') {
                j += 1;
            }
            j
        };
        let (a, b) = (skip_zeros(a, a_end), skip_zeros(b, b_end));
        let (a_len, b_len) = (a_end - a, b_end - b);
        if a_len != b_len {
            return a_len < b_len;
        }
        let a_digits = self.u[a..a_end].iter().map(|u| u.c);
        let b_digits = self.u[b..b_end].iter().map(|u| u.c);
        a_digits.le(b_digits)
    }

    /// Every group name in the pattern, before the main pass: a named
    /// reference may come before the group it names.
    fn collect_names(&mut self) {
        self.names.clear();
        self.named = false;
        let mut i = 0;
        while let Some(c) = self.c(i) {
            if is_ascii_char(c, b'\\') {
                i += 2;
            } else if is_ascii_char(c, b'[') {
                i += 1;
                while let Some(c) = self.c(i) {
                    if is_ascii_char(c, b']') {
                        break;
                    }
                    i += if is_ascii_char(c, b'\\') { 2 } else { 1 };
                }
                i += 1;
            } else if is_ascii_char(c, b'(')
                && self.is(i + 1, b'?')
                && self.is(i + 2, b'<')
                && !self.is(i + 3, b'=')
                && !self.is(i + 3, b'!')
            {
                self.named = true;
                // A malformed name is reported by the main pass.
                match self.group_name(i + 3) {
                    Ok((name, next)) => {
                        let _new = self.names.insert(name);
                        i = next;
                    }
                    Err(_) => i += 3,
                }
            } else {
                i += 1;
            }
        }
    }

    /// An escape outside a class: where it ends, and whether a quantifier
    /// may follow it.
    fn escape(&self, i: usize) -> Result<(usize, bool), Invalid> {
        let Some(n) = self.c(i + 1) else {
            // The literal body never ends in a lone backslash.
            return Ok((i + 1, true));
        };
        Ok(
            match char::from_u32(n).unwrap_or(char::REPLACEMENT_CHARACTER) {
                'b' | 'B' => (i + 2, false),
                // `\c` before a non-letter is a literal backslash; the `c` is
                // read next as a character of its own.
                'c' if !self.c(i + 2).is_some_and(is_ascii_letter) => (i + 1, true),
                'c' => (i + 3, true),
                'x' if self.hex(i + 2, 2).is_some() => (i + 4, true),
                'u' if self.hex(i + 2, 4).is_some() => (i + 6, true),
                // With a named group anywhere, `\k` must name one of them.
                'k' if self.named => {
                    let reference = self
                        .is(i + 2, b'<')
                        .then(|| self.group_name(i + 3).ok())
                        .flatten()
                        .filter(|(name, _)| self.names.contains(name));
                    match reference {
                        Some((_, next)) => (next, true),
                        None => return Err(self.err(i, NAMED_REFERENCE)),
                    }
                }
                // Backreferences, octal escapes, and identity escapes: each is
                // one atom; any further digits are atoms of their own.
                _ => (i + 2, true),
            },
        )
    }

    /// The group opened at `i`; returns where its contents start.
    fn open_group(&mut self, i: usize) -> Result<usize, Invalid> {
        let mut lookbehind = false;
        let next = if !self.is(i + 1, b'?') {
            i + 1
        } else {
            match self.c(i + 2).and_then(char::from_u32) {
                Some(':' | '=' | '!') => i + 3,
                Some('<') if self.is(i + 3, b'=') || self.is(i + 3, b'!') => {
                    lookbehind = true;
                    i + 4
                }
                Some('<') => self.group_name(i + 3)?.1,
                _ => self.modifiers(i)?,
            }
        };
        self.groups.push(JsGroup {
            at: self.at(i),
            lookbehind,
        });
        Ok(next)
    }

    /// A group name starting at `i`, just after `<`: the name as code
    /// points, and the position after the closing `>`. A character may be
    /// written as `\uXXXX`, a surrogate pair of those, or `\u{X...}`.
    fn group_name(&self, i: usize) -> Result<(Vec<u32>, usize), Invalid> {
        let mut name = Vec::new();
        let mut j = i;
        loop {
            let Some(c) = self.c(j) else {
                return Err(self.err(i, GROUP_NAME_UNCLOSED));
            };
            if is_ascii_char(c, b'>') {
                break;
            }
            let start = j;
            let (c, next) = if is_ascii_char(c, b'\\') {
                self.name_escape(j).ok_or_else(|| self.err(j, GROUP_NAME))?
            } else {
                (c, j + 1)
            };
            // A surrogate pair, written or escaped, is one character.
            let (c, next) = match self.name_unit(next) {
                Some((low, after)) if is_lead(c) && is_trail(low) => (pair(c, low), after),
                _ => (c, next),
            };
            if !name_char(c, name.is_empty()) {
                return Err(self.err(start, GROUP_NAME));
            }
            name.push(c);
            j = next;
        }
        if name.is_empty() {
            return Err(self.err(i, GROUP_NAME_EMPTY));
        }
        Ok((name, j + 1))
    }

    /// The code unit of a group name at `j`, escaped or not, if it is a
    /// trail surrogate's candidate: used only to join surrogate pairs.
    fn name_unit(&self, j: usize) -> Option<(u32, usize)> {
        let c = self.c(j)?;
        if is_ascii_char(c, b'\\') {
            self.name_escape(j).filter(|&(c, _)| c <= 0xFFFF)
        } else {
            Some((c, j + 1))
        }
    }

    /// `\uXXXX` or `\u{X...}` at `j` in a group name.
    fn name_escape(&self, j: usize) -> Option<(u32, usize)> {
        if !self.is(j + 1, b'u') {
            return None;
        }
        if !self.is(j + 2, b'{') {
            return Some((self.hex(j + 2, 4)?, j + 6));
        }
        let mut k = j + 3;
        let mut value = 0u32;
        while let Some(d) = self.c(k).and_then(hex_value) {
            value = value.saturating_mul(16).saturating_add(d);
            k += 1;
        }
        (k > j + 3 && self.is(k, b'}') && value <= 0x10_FFFF).then_some((value, k + 1))
    }

    /// A modifier group `(?ims-ims:` at `i`; returns where its contents
    /// start.
    fn modifiers(&self, i: usize) -> Result<usize, Invalid> {
        let flag = |c: Option<u32>| match c.and_then(char::from_u32) {
            Some('i') => Some(1u8),
            Some('m') => Some(2),
            Some('s') => Some(4),
            _ => None,
        };
        let mut j = i + 2;
        let (mut added, mut removed) = (0u8, 0u8);
        while let Some(bit) = flag(self.c(j)) {
            if added & bit != 0 {
                return Err(self.err(j, INVALID_MODIFIERS));
            }
            added |= bit;
            j += 1;
        }
        if self.is(j, b'-') {
            j += 1;
            while let Some(bit) = flag(self.c(j)) {
                if (added | removed) & bit != 0 {
                    return Err(self.err(j, INVALID_MODIFIERS));
                }
                removed |= bit;
                j += 1;
            }
            if added | removed == 0 {
                return Err(self.err(i, INVALID_MODIFIERS));
            }
        }
        if !self.is(j, b':') {
            return Err(self.err(i, INVALID_GROUP));
        }
        Ok(j + 1)
    }

    /// The class opened at `i`; returns the position after its `]`.
    fn class(&self, i: usize) -> Result<usize, Invalid> {
        let mut j = i + 1;
        if self.is(j, b'^') {
            j += 1;
        }
        loop {
            match self.c(j) {
                None => return Err(self.err(i, UNCLOSED_CLASS)),
                Some(c) if is_ascii_char(c, b']') => return Ok(j + 1),
                Some(_) => {}
            }
            let start = j;
            let (low, next) = self.class_atom(j)?;
            j = next;
            // A `-` between two atoms makes a range, unless the class ends
            // right after it.
            if self.is(j, b'-') && self.c(j + 1).is_some() && !self.is(j + 1, b']') {
                let (high, next) = self.class_atom(j + 1)?;
                j = next;
                // A range with a class escape at either end is a union in
                // Annex B, not an error.
                if let (Some(low), Some(high)) = (low, high) {
                    if low > high {
                        return Err(self.err(start, CLASS_ORDER));
                    }
                }
            }
        }
    }

    /// One atom of a class at `i`: its code unit (`None` for a class escape
    /// such as `\d`) and where it ends.
    fn class_atom(&self, i: usize) -> Result<(Option<u32>, usize), Invalid> {
        let Some(c) = self.c(i) else {
            return Ok((None, i));
        };
        if !is_ascii_char(c, b'\\') {
            return Ok((Some(c), i + 1));
        }
        let Some(n) = self.c(i + 1) else {
            return Ok((Some(c), i + 1));
        };
        Ok(
            match char::from_u32(n).unwrap_or(char::REPLACEMENT_CHARACTER) {
                'b' => (Some(8), i + 2),
                'd' | 'D' | 's' | 'S' | 'w' | 'W' => (None, i + 2),
                'f' => (Some(0x0C), i + 2),
                'n' => (Some(0x0A), i + 2),
                'r' => (Some(0x0D), i + 2),
                't' => (Some(0x09), i + 2),
                'v' => (Some(0x0B), i + 2),
                'c' => match self.c(i + 2) {
                    // In a class, `\c` also takes a digit or `_` (Annex B).
                    Some(l) if is_ascii_letter(l) || (0x30..=0x39).contains(&l) || l == 0x5F => {
                        (Some(l & 0x1F), i + 3)
                    }
                    _ => (Some(c), i + 1),
                },
                'x' => match self.hex(i + 2, 2) {
                    Some(v) => (Some(v), i + 4),
                    None => (Some(n), i + 2),
                },
                'u' => match self.hex(i + 2, 4) {
                    Some(v) => (Some(v), i + 6),
                    None => (Some(n), i + 2),
                },
                '0'..='7' => self.legacy_octal(i + 1),
                // With a named group anywhere, `\k` is no identity escape, and a
                // class cannot hold a named reference.
                'k' if self.named => return Err(self.err(i, NAMED_REFERENCE)),
                // Identity escapes, `\8` and `\9` included.
                _ => (Some(n), i + 2),
            },
        )
    }

    /// `\0` or a legacy octal escape whose first digit is at `i`: up to
    /// three octal digits when the first is 0 to 3, two otherwise.
    fn legacy_octal(&self, i: usize) -> (Option<u32>, usize) {
        let octal = |j: usize| {
            self.c(j)
                .filter(|c| (0x30..=0x37).contains(c))
                .map(|c| c - 0x30)
        };
        let Some(first) = octal(i) else {
            return (None, i + 1);
        };
        let most = if first <= 3 { 3 } else { 2 };
        let mut value = first;
        let mut j = i + 1;
        while j < i + most {
            let Some(d) = octal(j) else { break };
            value = value * 8 + d;
            j += 1;
        }
        (Some(value), j)
    }
}

fn is_ascii_letter(c: u32) -> bool {
    char::from_u32(c).is_some_and(|c| c.is_ascii_alphabetic())
}

fn is_lead(c: u32) -> bool {
    (0xD800..0xDC00).contains(&c)
}

fn is_trail(c: u32) -> bool {
    (0xDC00..0xE000).contains(&c)
}

fn pair(lead: u32, trail: u32) -> u32 {
    0x1_0000 + ((lead - 0xD800) << 10) + (trail - 0xDC00)
}

/// Whether `c` may be in a JavaScript group name, first or later: `$`, `_`,
/// ASCII letters, and (not first) digits. Any non-ASCII character passes;
/// see [`Js`].
fn name_char(c: u32, first: bool) -> bool {
    match char::from_u32(c) {
        Some(c) if c.is_ascii() => {
            c == '$' || c == '_' || c.is_ascii_alphabetic() || (!first && c.is_ascii_digit())
        }
        _ => true,
    }
}

// --- regex-syntax -------------------------------------------------------------

/// The deepest regex-syntax lets an expression nest: `ParserBuilder`'s
/// default `nest_limit`.
const NEST_LIMIT: u32 = 250;

/// A finished part of the expression, as much as the checks need of it.
#[derive(Clone, Copy, Debug, Default)]
struct Elem {
    /// How many nesting levels regex-syntax counts on the deepest path
    /// through it: groups, repetitions, alternations, concatenations of two
    /// or more, classes, unions of two or more class items, and class set
    /// operations.
    height: u32,
    /// The first assertion in it that would reach the compiled expression.
    look: Option<usize>,
    /// A `(?flags)` directive, which cannot be repeated.
    flags: bool,
}

/// A concatenation being built.
#[derive(Clone, Copy, Debug, Default)]
struct Concat {
    count: usize,
    /// The tallest element and the first assertion, excluding `last`.
    height: u32,
    look: Option<usize>,
    last: Option<Elem>,
}

impl Concat {
    fn push(&mut self, elem: Elem) {
        if let Some(prev) = self.last.replace(elem) {
            self.height = self.height.max(prev.height);
            self.look = self.look.or(prev.look);
        }
        self.count += 1;
    }

    /// The concatenation as one element: nothing, its one element, or a
    /// concatenation node one level above its tallest element.
    fn finish(self, at: usize) -> Result<Elem, Invalid> {
        let Some(last) = self.last else {
            return Ok(Elem::default());
        };
        if self.count == 1 {
            return Ok(Elem {
                flags: false,
                ..last
            });
        }
        Ok(Elem {
            height: nest(self.height.max(last.height), at)?,
            look: self.look.or(last.look),
            flags: false,
        })
    }
}

/// One level of the expression: the current concatenation, and the
/// alternatives before it if there was a `|`.
#[derive(Clone, Copy, Debug, Default)]
struct Level {
    concat: Concat,
    /// The tallest earlier alternative and the first assertion in them.
    alternation: Option<(u32, Option<usize>)>,
}

impl Level {
    fn alternate(&mut self, at: usize) -> Result<(), Invalid> {
        let branch = core::mem::take(&mut self.concat).finish(at)?;
        self.alternation = Some(match self.alternation {
            Some((height, look)) => (height.max(branch.height), look.or(branch.look)),
            None => (branch.height, branch.look),
        });
        Ok(())
    }

    fn finish(mut self, at: usize) -> Result<Elem, Invalid> {
        if self.alternation.is_none() {
            return self.concat.finish(at);
        }
        self.alternate(at)?;
        let (height, look) = self.alternation.unwrap_or_default();
        Ok(Elem {
            height: nest(height, at)?,
            look,
            flags: false,
        })
    }
}

/// An open group: the level around it, and where it opened.
#[derive(Clone, Copy, Debug)]
struct Frame {
    outer: Level,
    at: usize,
}

/// An open class: where it opened, the left side of a pending set
/// operation, and the items of the current union.
#[derive(Clone, Copy, Debug)]
struct Bracket {
    at: usize,
    lhs: Option<u32>,
    count: usize,
    height: u32,
}

impl Bracket {
    fn add(&mut self, height: u32) {
        self.count += 1;
        self.height = self.height.max(height);
    }

    /// The current union as one item.
    fn union(&self, at: usize) -> Result<u32, Invalid> {
        match self.count {
            0 | 1 => Ok(self.height),
            _ => nest(self.height, at),
        }
    }

    /// The class set so far: the union, combined with a pending operation.
    fn set(&self, at: usize) -> Result<u32, Invalid> {
        let union = self.union(at)?;
        match self.lhs {
            Some(lhs) => nest(lhs.max(union), at),
            None => Ok(union),
        }
    }
}

/// One level above `height`, if that is within regex-syntax's limit.
fn nest(height: u32, at: usize) -> Result<u32, Invalid> {
    let height = height.saturating_add(1);
    if height > NEST_LIMIT {
        Err(invalid(at, NESTING))
    } else {
        Ok(height)
    }
}

/// What an escape or character denotes, as far as the checks care.
#[derive(Clone, Copy, Debug)]
enum Prim {
    Literal(u32),
    Class,
    Assertion,
}

/// A recognizer for regex-syntax 0.8 with tree-sitter's settings, plus
/// tree-sitter's refusal of assertions. Follows `regex_syntax::ast::parse`
/// branch for branch; `x` (whitespace) mode is not modelled, because
/// JavaScript has no way to write a flag group, so it never reaches here.
struct Rx<'a> {
    u: &'a [Unit],
    end: usize,
    groups: &'a mut Vec<Frame>,
    brackets: &'a mut Vec<Bracket>,
    names: &'a mut BTreeSet<String>,
}

impl Rx<'_> {
    fn ch(&self, i: usize) -> Option<char> {
        self.u.get(i).and_then(|u| char::from_u32(u.c))
    }

    fn is(&self, i: usize, ch: char) -> bool {
        self.ch(i) == Some(ch)
    }

    fn at(&self, i: usize) -> usize {
        self.u.get(i).map_or(self.end, |u| u.at)
    }

    fn err(&self, i: usize, reason: &'static str) -> Invalid {
        invalid(self.at(i), reason)
    }

    fn run(&mut self) -> Result<(), Invalid> {
        let mut level = Level::default();
        let mut i = 0;
        while let Some(c) = self.ch(i) {
            match c {
                '(' => i = self.group(i, &mut level)?,
                ')' => {
                    let Some(frame) = self.groups.pop() else {
                        return Err(self.err(i, UNMATCHED_CLOSE));
                    };
                    let inner = level.finish(self.at(i))?;
                    let group = Elem {
                        height: nest(inner.height, frame.at)?,
                        look: inner.look,
                        flags: false,
                    };
                    level = frame.outer;
                    level.concat.push(group);
                    i += 1;
                }
                '|' => {
                    level.alternate(self.at(i))?;
                    i += 1;
                }
                '[' => {
                    let (height, next) = self.class(i)?;
                    level.concat.push(Elem {
                        height,
                        ..Elem::default()
                    });
                    i = next;
                }
                '?' | '*' | '+' => {
                    let target = self.repeat_target(i, &mut level.concat)?;
                    let mut next = i + 1;
                    if self.is(next, '?') {
                        next += 1;
                    }
                    level.concat.push(Elem {
                        height: nest(target.height, self.at(i))?,
                        look: target.look,
                        flags: false,
                    });
                    i = next;
                }
                '{' => i = self.counted(i, &mut level.concat)?,
                '.' => {
                    level.concat.push(Elem::default());
                    i += 1;
                }
                '^' | '$' => {
                    level.concat.push(Elem {
                        look: Some(self.at(i)),
                        ..Elem::default()
                    });
                    i += 1;
                }
                '\\' => {
                    let (prim, next) = self.escape(i)?;
                    level.concat.push(Elem {
                        look: matches!(prim, Prim::Assertion).then(|| self.at(i)),
                        ..Elem::default()
                    });
                    i = next;
                }
                _ => {
                    level.concat.push(Elem::default());
                    i += 1;
                }
            }
        }
        if let Some(frame) = self.groups.last() {
            return Err(invalid(frame.at, UNCLOSED_GROUP));
        }
        let root = level.finish(self.end)?;
        match root.look {
            Some(at) => Err(invalid(at, ASSERTION)),
            None => Ok(()),
        }
    }

    /// Takes the element a repetition at `i` applies to.
    fn repeat_target(&self, i: usize, concat: &mut Concat) -> Result<Elem, Invalid> {
        match concat.last.take() {
            Some(last) if !last.flags => {
                concat.count -= 1;
                Ok(last)
            }
            other => {
                concat.last = other;
                Err(self.err(i, NOTHING_TO_REPEAT))
            }
        }
    }

    /// A counted repetition `{n}`, `{n,}`, or `{n,m}` at `i`, optionally
    /// lazy; returns where it ends. Spaces are allowed around the numbers.
    fn counted(&self, i: usize, concat: &mut Concat) -> Result<usize, Invalid> {
        let target = self.repeat_target(i, concat)?;
        let unclosed = || self.err(i, REPEAT_UNCLOSED);
        let mut j = i + 1;
        if self.ch(j).is_none() {
            return Err(unclosed());
        }
        let min = self.decimal(&mut j);
        if self.ch(j).is_none() {
            return Err(unclosed());
        }
        let (min, max) = if self.is(j, ',') {
            j += 1;
            if self.ch(j).is_none() {
                return Err(unclosed());
            }
            if self.is(j, '}') {
                (min?, None)
            } else {
                let min = min?;
                (min, Some(self.decimal(&mut j)?))
            }
        } else {
            let min = min?;
            (min, Some(min))
        };
        if !self.is(j, '}') {
            return Err(unclosed());
        }
        j += 1;
        if self.is(j, '?') {
            j += 1;
        }
        if max.is_some_and(|max| min > max) {
            return Err(self.err(i, REPEAT_ORDER));
        }
        // A repetition of exactly zero compiles to nothing, assertions
        // included.
        let erased = max == Some(0);
        concat.push(Elem {
            height: nest(target.height, self.at(i))?,
            look: if erased { None } else { target.look },
            flags: false,
        });
        Ok(j)
    }

    /// A decimal number with optional surrounding whitespace at `*j`.
    fn decimal(&self, j: &mut usize) -> Result<u32, Invalid> {
        while self.ch(*j).is_some_and(char::is_whitespace) {
            *j += 1;
        }
        let start = *j;
        let mut value = Some(0u32);
        while let Some(d) = self.ch(*j).and_then(|c| c.to_digit(10)) {
            value = value.and_then(|v| v.checked_mul(10)?.checked_add(d));
            *j += 1;
        }
        let digits_end = *j;
        while self.ch(*j).is_some_and(char::is_whitespace) {
            *j += 1;
        }
        if start == digits_end {
            return Err(self.err(start, REPEAT_EMPTY));
        }
        value.ok_or_else(|| self.err(start, REPEAT_TOO_BIG))
    }

    /// A group or flag directive at `i`; returns where its contents start.
    fn group(&mut self, i: usize, level: &mut Level) -> Result<usize, Invalid> {
        let open = self.at(i);
        let j = i + 1;
        if self.is(j, '?')
            && (self.is(j + 1, '=')
                || self.is(j + 1, '!')
                || (self.is(j + 1, '<') && (self.is(j + 2, '=') || self.is(j + 2, '!'))))
        {
            return Err(self.err(i, LOOK_AROUND));
        }
        let start = if self.is(j, '?') && self.is(j + 1, 'P') && self.is(j + 2, '<') {
            self.capture_name(j + 3)?
        } else if self.is(j, '?') && self.is(j + 1, '<') {
            self.capture_name(j + 2)?
        } else if self.is(j, '?') {
            if self.ch(j + 1).is_none() {
                return Err(invalid(open, UNCLOSED_GROUP));
            }
            let (any, end) = self.flags(j + 1)?;
            if self.is(end, ')') {
                if !any {
                    return Err(self.err(j, FLAG_EMPTY));
                }
                level.concat.push(Elem {
                    flags: true,
                    ..Elem::default()
                });
                return Ok(end + 1);
            }
            end + 1
        } else {
            j
        };
        self.groups.push(Frame {
            outer: core::mem::take(level),
            at: open,
        });
        Ok(start)
    }

    /// Flags up to `:` or `)` from `j`: whether there were any, and where
    /// the `:` or `)` is.
    fn flags(&self, mut j: usize) -> Result<(bool, usize), Invalid> {
        let mut seen = [false; 8];
        let mut any = false;
        let mut last_negation = false;
        loop {
            let Some(c) = self.ch(j) else {
                return Err(self.err(j, UNCLOSED_GROUP));
            };
            if c == ':' || c == ')' {
                break;
            }
            let slot = match c {
                '-' => 0,
                'i' => 1,
                'm' => 2,
                's' => 3,
                'U' => 4,
                'u' => 5,
                'R' => 6,
                'x' => 7,
                _ => return Err(self.err(j, FLAG_UNKNOWN)),
            };
            if seen[slot] {
                return Err(self.err(j, FLAG_REPEATED));
            }
            seen[slot] = true;
            any = true;
            last_negation = slot == 0;
            j += 1;
        }
        if last_negation {
            return Err(self.err(j, FLAG_DANGLING));
        }
        Ok((any, j))
    }

    /// A capture group name from `j` (just after `<`) to `>`; returns the
    /// position after the `>`.
    fn capture_name(&mut self, j: usize) -> Result<usize, Invalid> {
        let mut k = j;
        let mut name = String::new();
        loop {
            match self.ch(k) {
                None => return Err(self.err(k, GROUP_NAME_UNCLOSED)),
                Some('>') => break,
                Some(c) => {
                    let valid = if k == j {
                        c == '_' || c.is_alphabetic()
                    } else {
                        matches!(c, '_' | '.' | '[' | ']') || c.is_alphanumeric()
                    };
                    if !valid {
                        return Err(self.err(k, GROUP_NAME));
                    }
                    name.push(c);
                    k += 1;
                }
            }
        }
        if name.is_empty() {
            return Err(self.err(j, GROUP_NAME_EMPTY));
        }
        if !self.names.insert(name) {
            return Err(self.err(j, GROUP_NAME_DUPLICATE));
        }
        Ok(k + 1)
    }

    /// An escape at `i`; returns what it denotes and where it ends.
    fn escape(&self, i: usize) -> Result<(Prim, usize), Invalid> {
        let Some(c) = self.ch(i + 1) else {
            return Err(self.err(i, ESCAPE_EOF));
        };
        let prim = match c {
            '0'..='9' => return Err(self.err(i, BACKREFERENCE)),
            'x' | 'u' | 'U' => return self.hex(i + 1),
            'p' | 'P' => return self.unicode_class(i + 1),
            'd' | 's' | 'w' | 'D' | 'S' | 'W' => Prim::Class,
            'a' => Prim::Literal(0x07),
            'f' => Prim::Literal(0x0C),
            't' => Prim::Literal(0x09),
            'n' => Prim::Literal(0x0A),
            'r' => Prim::Literal(0x0D),
            'v' => Prim::Literal(0x0B),
            'b' => return self.word_boundary(i + 2),
            'A' | 'z' | 'B' | '<' | '>' => Prim::Assertion,
            // Meta characters and every other ASCII character that is not a
            // letter or digit may be escaped.
            c if c.is_ascii() && !c.is_ascii_alphanumeric() => Prim::Literal(u32::from(c)),
            _ => return Err(self.err(i, UNKNOWN_ESCAPE)),
        };
        Ok((prim, i + 2))
    }

    /// `\b`, possibly `\b{start}`, `\b{end}`, `\b{start-half}`, or
    /// `\b{end-half}`; `j` is just after the `b`.
    fn word_boundary(&self, j: usize) -> Result<(Prim, usize), Invalid> {
        if !self.is(j, '{') {
            return Ok((Prim::Assertion, j));
        }
        let valid = |c: char| c.is_ascii_alphabetic() || c == '-';
        match self.ch(j + 1) {
            None => return Err(self.err(j, WORD_BOUNDARY)),
            // Not a word: `\b{5}` is a counted repetition of `\b`.
            Some(c) if !valid(c) => return Ok((Prim::Assertion, j)),
            Some(_) => {}
        }
        let mut k = j + 1;
        let mut name = String::new();
        while let Some(c) = self.ch(k).filter(|&c| valid(c)) {
            name.push(c);
            k += 1;
        }
        if !self.is(k, '}') || !matches!(&*name, "start" | "end" | "start-half" | "end-half") {
            return Err(self.err(j, WORD_BOUNDARY));
        }
        Ok((Prim::Assertion, k + 1))
    }

    /// A hexadecimal escape whose `x`, `u`, or `U` is at `j`: two, four, or
    /// eight digits, or any number in braces.
    fn hex(&self, j: usize) -> Result<(Prim, usize), Invalid> {
        let digits = match self.ch(j) {
            Some('x') => 2,
            Some('u') => 4,
            _ => 8,
        };
        let mut k = j + 1;
        let mut value = Some(0u32);
        let mut add = |d: u32| value = value.and_then(|v| v.checked_mul(16)?.checked_add(d));
        if self.is(k, '{') {
            let open = k;
            k += 1;
            loop {
                match self.ch(k) {
                    None => return Err(self.err(k, ESCAPE_EOF)),
                    Some('}') => break,
                    Some(c) => match c.to_digit(16) {
                        Some(d) => add(d),
                        None => return Err(self.err(k, HEX_DIGIT)),
                    },
                }
                k += 1;
            }
            if k == open + 1 {
                return Err(self.err(open, HEX_EMPTY));
            }
            k += 1;
        } else {
            for _ in 0..digits {
                match self.ch(k) {
                    None => return Err(self.err(k, ESCAPE_EOF)),
                    Some(c) => match c.to_digit(16) {
                        Some(d) => add(d),
                        None => return Err(self.err(k, HEX_DIGIT)),
                    },
                }
                k += 1;
            }
        }
        match value.and_then(char::from_u32) {
            Some(c) => Ok((Prim::Literal(u32::from(c)), k)),
            None => Err(self.err(j, HEX_VALUE)),
        }
    }

    /// `\pX`, `\p{...}`, or the `\P` forms, with the `p` or `P` at `j`. The
    /// name is not looked up; see the module documentation.
    fn unicode_class(&self, j: usize) -> Result<(Prim, usize), Invalid> {
        match self.ch(j + 1) {
            None | Some('\\') => Err(self.err(j, UNICODE_CLASS)),
            Some('{') => {
                let mut k = j + 2;
                while let Some(c) = self.ch(k) {
                    if c == '}' {
                        // No property has an empty name.
                        if k == j + 2 {
                            return Err(self.err(j, UNICODE_CLASS));
                        }
                        return Ok((Prim::Class, k + 1));
                    }
                    k += 1;
                }
                Err(self.err(j, UNICODE_CLASS))
            }
            Some(_) => Ok((Prim::Class, j + 2)),
        }
    }

    /// The class opened at `i`, nested classes and set operations included;
    /// returns its nesting height and where it ends.
    fn class(&mut self, i: usize) -> Result<(u32, usize), Invalid> {
        self.brackets.clear();
        let mut j = self.class_open(i)?;
        loop {
            let Some(c) = self.ch(j) else {
                let at = self.brackets.last().map_or(self.end, |b| b.at);
                return Err(invalid(at, UNCLOSED_CLASS));
            };
            match c {
                '[' => match self.ascii_class(j) {
                    Some(next) => {
                        self.top(|b| b.add(0));
                        j = next;
                    }
                    None => j = self.class_open(j)?,
                },
                ']' => {
                    let Some(bracket) = self.brackets.pop() else {
                        return Err(self.err(j, UNCLOSED_CLASS));
                    };
                    let height = nest(bracket.set(bracket.at)?, bracket.at)?;
                    j += 1;
                    if self.brackets.is_empty() {
                        return Ok((height, j));
                    }
                    self.top(|b| b.add(height));
                }
                '&' | '-' | '~' if self.is(j + 1, c) => {
                    let at = self.at(j);
                    let Some(bracket) = self.brackets.last_mut() else {
                        return Err(invalid(at, UNCLOSED_CLASS));
                    };
                    let lhs = bracket.set(at)?;
                    *bracket = Bracket {
                        lhs: Some(lhs),
                        count: 0,
                        height: 0,
                        ..*bracket
                    };
                    j += 2;
                }
                _ => {
                    j = self.class_range(j)?;
                    self.top(|b| b.add(0));
                }
            }
        }
    }

    fn top(&mut self, f: impl FnOnce(&mut Bracket)) {
        if let Some(bracket) = self.brackets.last_mut() {
            f(bracket);
        }
    }

    /// Opens a class at `i`: `[`, an optional `^`, any number of leading
    /// `-`, and a leading `]` if there was no `-`; all of them literals.
    fn class_open(&mut self, i: usize) -> Result<usize, Invalid> {
        let unclosed = || self.err(i, UNCLOSED_CLASS);
        let mut j = i + 1;
        if self.ch(j).is_none() {
            return Err(unclosed());
        }
        if self.is(j, '^') {
            j += 1;
            if self.ch(j).is_none() {
                return Err(unclosed());
            }
        }
        let mut count = 0;
        while self.is(j, '-') {
            count += 1;
            j += 1;
            if self.ch(j).is_none() {
                return Err(unclosed());
            }
        }
        if count == 0 && self.is(j, ']') {
            count = 1;
            j += 1;
            if self.ch(j).is_none() {
                return Err(unclosed());
            }
        }
        self.brackets.push(Bracket {
            at: self.at(i),
            lhs: None,
            count,
            height: 0,
        });
        Ok(j)
    }

    /// `[:name:]` or `[:^name:]` at `j` inside a class, if it is one of the
    /// fourteen ASCII classes; returns where it ends.
    fn ascii_class(&self, j: usize) -> Option<usize> {
        const NAMES: [&str; 14] = [
            "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print",
            "punct", "space", "upper", "word", "xdigit",
        ];
        if !self.is(j + 1, ':') {
            return None;
        }
        let mut k = j + 2;
        if self.is(k, '^') {
            k += 1;
        }
        // regex-syntax looks for the next `:` however far away; a name
        // longer than the longest class name cannot match, so looking a
        // few characters ahead decides the same and keeps this constant
        // time.
        let mut name = String::new();
        let mut end = k;
        loop {
            let c = self.ch(end)?;
            if c == ':' {
                break;
            }
            if end - k >= 6 {
                return None;
            }
            name.push(c);
            end += 1;
        }
        (self.is(end + 1, ']') && NAMES.contains(&&*name)).then_some(end + 2)
    }

    /// One class item at `j` — a character, an escape, or a range of two —
    /// returning where it ends.
    fn class_range(&self, j: usize) -> Result<usize, Invalid> {
        let unclosed = |k: usize| {
            let at = self.brackets.last().map_or(self.at(k), |b| b.at);
            invalid(at, UNCLOSED_CLASS)
        };
        let (low, k) = self.class_item(j)?;
        let Some(c) = self.ch(k) else {
            return Err(unclosed(k));
        };
        if c != '-' || self.is(k + 1, ']') || self.is(k + 1, '-') {
            if matches!(low, Prim::Assertion) {
                return Err(self.err(j, CLASS_ASSERTION));
            }
            return Ok(k);
        }
        if self.ch(k + 1).is_none() {
            return Err(unclosed(k + 1));
        }
        let (high, end) = self.class_item(k + 1)?;
        match (low, high) {
            (Prim::Literal(low), Prim::Literal(high)) if low <= high => Ok(end),
            (Prim::Literal(_), Prim::Literal(_)) => Err(self.err(j, CLASS_ORDER)),
            _ => Err(self.err(j, CLASS_RANGE_ENDPOINT)),
        }
    }

    fn class_item(&self, j: usize) -> Result<(Prim, usize), Invalid> {
        match self.ch(j) {
            Some('\\') => self.escape(j),
            Some(c) => Ok((Prim::Literal(u32::from(c)), j + 1)),
            None => Err(self.err(j, UNCLOSED_CLASS)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::js_regex;

    fn js(pattern: &str) -> Result<(), Invalid> {
        Checker::default().javascript(pattern)
    }

    fn ts(pattern: &str) -> Result<(), Invalid> {
        Checker::default().tree_sitter(pattern)
    }

    /// Both checks, as validation applies them to a reachable rule.
    fn ok(pattern: &str) -> bool {
        let mut checker = Checker::default();
        checker.javascript(pattern).is_ok() && checker.tree_sitter(pattern).is_ok()
    }

    #[test]
    fn test_literal_body_matches_js_regex() {
        for pattern in [
            "",
            "a/b",
            r"a\/b",
            "[/]",
            "a\nb",
            "a\\\nb",
            "x\u{2028}",
            "x\\\u{2029}",
            r"end\",
            "a\tb\0",
            "\\\0\\\t\u{1}",
            r"\\/",
            "\\é/",
            "\\\u{2192}",
            "😀/\\😀",
        ] {
            let mut body = Vec::new();
            literal_body(pattern, &mut |c, at| body.push((c, at)));
            let mut expected = String::new();
            js_regex(&mut expected, pattern);
            assert_eq!(String::from_iter(body.iter().map(|&(c, _)| c)), expected);
            assert!(body.iter().all(|&(_, at)| at < pattern.len().max(1)));
        }
    }

    #[test]
    fn test_common_patterns_are_valid() {
        for pattern in [
            r"\d+",
            r"\s",
            r"[a-zA-Z_][a-zA-Z0-9_]*",
            r"\d+(\.\d+)?([eE][+-]?\d+)?",
            r#""([^"\\]|\\.)*""#,
            r"//[^\n]*",
            r"/\*[^*]*\*+([^/*][^*]*\*+)*/",
            "[^\n]+",
            r"\\\r?\n",
            r"0[xX][0-9a-fA-F]+",
            r"[\u0041-\u005A]",
            r"\p{L}+",
            r"\p{XID_Start}\p{XID_Continue}*",
            r"[\p{L}_]",
            "a/b",
            "[/]",
            r"[^/\\s]?",
            "]",
            "}",
            "a{ 2 }",
            "[]a]",
            "[^]a]",
            "[[:alpha:]]",
            "[a&&b]",
            r"\x{41}",
            r"\u{1F600}",
            r"(?<name>a)",
            r"(?:^){0}",
            r"(?i:abc)",
            r"(?-i:a)",
            "é+",
            "😀+",
            "[a-😀]",
            "a|",
            "|",
            "()",
            r"end\",
            "a\tb\0",
        ] {
            assert!(
                ok(pattern),
                "{pattern:?}: {:?} {:?}",
                js(pattern),
                ts(pattern)
            );
        }
    }

    #[test]
    fn test_javascript_refusals() {
        let cases: &[(&str, usize, &str)] = &[
            ("[", 0, UNCLOSED_CLASS),
            ("a[b", 1, UNCLOSED_CLASS),
            ("(a", 0, UNCLOSED_GROUP),
            ("a)", 1, UNMATCHED_CLOSE),
            ("*a", 0, NOTHING_TO_REPEAT),
            ("a**", 2, NOTHING_TO_REPEAT),
            ("a|*", 2, NOTHING_TO_REPEAT),
            ("^*", 1, NOTHING_TO_REPEAT),
            (r"\b+", 2, NOTHING_TO_REPEAT),
            ("(?<=a)*", 6, NOTHING_TO_REPEAT),
            ("{1}", 0, NOTHING_TO_REPEAT),
            ("a{1}{2}", 4, NOTHING_TO_REPEAT),
            (r"\x{41}{2}", 6, NOTHING_TO_REPEAT),
            ("a{2,1}", 1, QUANTIFIER_ORDER),
            ("a{10,9}", 1, QUANTIFIER_ORDER),
            ("[z-a]", 1, CLASS_ORDER),
            ("[a--]", 1, CLASS_ORDER),
            ("[😀-😁]", 1, CLASS_ORDER),
            ("(?a)", 0, INVALID_GROUP),
            ("(?i)a", 0, INVALID_GROUP),
            ("(?-:a)", 0, INVALID_MODIFIERS),
            ("(?ii:a)", 3, INVALID_MODIFIERS),
            ("(?i-i:a)", 4, INVALID_MODIFIERS),
            ("(?<a.b>x)", 4, GROUP_NAME),
            ("(?<1a>x)", 3, GROUP_NAME),
            ("(?<>x)", 3, GROUP_NAME_EMPTY),
            ("(?<ab", 3, GROUP_NAME_UNCLOSED),
            ("a//[", 3, UNCLOSED_CLASS),
        ];
        for &(pattern, at, reason) in cases {
            assert_eq!(js(pattern), Err(Invalid { at, reason }), "{pattern:?}");
        }
        // JavaScript accepts what only regex-syntax refuses.
        for pattern in [
            "{", "a{", "[]", "[[a]", r"\c", r"\0", r"\e", "^a", "(?=a)", "a{,5}",
        ] {
            assert_eq!(js(pattern), Ok(()), "{pattern:?}");
        }
        assert_eq!(js("a{4294967296}"), Ok(()));
        // Named references: `\k` is an identity escape until the pattern has
        // a named group; then it must name one, before or after it.
        for pattern in [
            r"\k<n>",
            r"(?<n>a)\k<n>",
            r"\k<n>(?<n>a)",
            r"(?<\u0061>x)\k<a>",
            r"(?<a\u{62}>x)\k<ab>",
            r"(?<\uD835\uDC9C>x)",
            "(?<$é1>x)",
        ] {
            assert_eq!(js(pattern), Ok(()), "{pattern:?}");
        }
        assert_eq!(
            js(r"(?<n>a)\k<m>"),
            Err(Invalid {
                at: 7,
                reason: NAMED_REFERENCE
            })
        );
        assert_eq!(
            js(r"(?<n>a)\k"),
            Err(Invalid {
                at: 7,
                reason: NAMED_REFERENCE
            })
        );
        assert_eq!(
            js(r"(?<n>a)[\k]"),
            Err(Invalid {
                at: 8,
                reason: NAMED_REFERENCE
            })
        );
        assert_eq!(
            js(r"(?<\x61>x)"),
            Err(Invalid {
                at: 3,
                reason: GROUP_NAME
            })
        );
        // Repeated names are left to the runtime (see `Js`); regex-syntax
        // refuses them.
        for pattern in ["(?<a>x)|(?<a>y)", "(?<a>x)(?<a>y)", "(?<a>x(?<a>y)|z)"] {
            assert_eq!(js(pattern), Ok(()), "{pattern:?}");
            assert_eq!(ts(pattern).map_err(|e| e.reason), Err(GROUP_NAME_DUPLICATE));
        }
        // regex-syntax refuses any escape in a name.
        assert_eq!(ts(r"(?<\u0061>x)").map_err(|e| e.reason), Err(GROUP_NAME));
        assert_eq!(
            js("a{99999999999999999999,1}"),
            Err(Invalid {
                at: 1,
                reason: QUANTIFIER_ORDER
            })
        );
    }

    #[test]
    fn test_tree_sitter_refusals() {
        let cases: &[(&str, usize, &str)] = &[
            ("{", 0, NOTHING_TO_REPEAT),
            ("a{", 1, REPEAT_UNCLOSED),
            ("a{x}", 2, REPEAT_EMPTY),
            ("a{,5}", 2, REPEAT_EMPTY),
            ("a{4294967296}", 2, REPEAT_TOO_BIG),
            ("[]", 0, UNCLOSED_CLASS),
            ("[^]", 0, UNCLOSED_CLASS),
            ("[[a]", 0, UNCLOSED_CLASS),
            ("[a[]", 2, UNCLOSED_CLASS),
            (r"\c", 0, UNKNOWN_ESCAPE),
            (r"\e", 0, UNKNOWN_ESCAPE),
            (r"\k<n>", 0, UNKNOWN_ESCAPE),
            ("\\é", 0, UNKNOWN_ESCAPE),
            (r"\0", 0, BACKREFERENCE),
            (r"(a)\1", 3, BACKREFERENCE),
            ("^a", 0, ASSERTION),
            ("a$", 1, ASSERTION),
            (r"a\b", 1, ASSERTION),
            // Rewritten to `\[^0-9]`; the `^` comes from the second `\`.
            (r"\\D", 1, ASSERTION),
            ("(?:^){0,1}", 3, ASSERTION),
            ("(?=a)", 0, LOOK_AROUND),
            ("(?<!a)", 0, LOOK_AROUND),
            (r"\x4", 3, ESCAPE_EOF),
            (r"\xg0", 2, HEX_DIGIT),
            (r"\x{110000}", 1, HEX_VALUE),
            (r"\uD83D\uDE00", 1, HEX_VALUE),
            (r"\x{}", 2, HEX_EMPTY),
            (r"\p{}", 1, UNICODE_CLASS),
            (r"\b{foo}", 2, WORD_BOUNDARY),
            // `\s` is rewritten to `[\t-\r ]`: the range ends at `[`.
            ("[a-\\s]", 1, CLASS_ORDER),
            (r"[a-\pL]", 1, CLASS_RANGE_ENDPOINT),
            ("[ü-é]", 1, CLASS_ORDER),
            (r"[\b]", 1, CLASS_ASSERTION),
            ("(?<a1>x)(?<a1>y)", 11, GROUP_NAME_DUPLICATE),
            ("(?q:a)", 2, FLAG_UNKNOWN),
            ("(?)", 1, FLAG_EMPTY),
            ("(?i-)", 4, FLAG_DANGLING),
            ("(?i-i:a)", 4, FLAG_REPEATED),
            ("(?i)*", 4, NOTHING_TO_REPEAT),
            ("a)", 1, UNMATCHED_CLOSE),
            ("((a)", 0, UNCLOSED_GROUP),
        ];
        for &(pattern, at, reason) in cases {
            assert_eq!(ts(pattern), Err(Invalid { at, reason }), "{pattern:?}");
        }
        // regex-syntax accepts what only JavaScript refuses.
        for pattern in ["a**", r"\x{41}{2}", "[😀-😁]", "^{0}", "(?i)a", "a{1}{2}"] {
            assert_eq!(ts(pattern), Ok(()), "{pattern:?}");
        }
    }

    #[test]
    fn test_perl_class_rewrite_is_textual() {
        // `\\w` keeps working, `\\D` becomes `\[^0-9]`: an escaped `[`, then
        // the assertion `^`.
        assert!(ok(r"\\w"));
        assert!(ok(r"\\s"));
        assert!(ok(r"\\d"));
        assert!(ts(r"\\D").is_err());
        assert!(ts(r"\\S").is_err());
        assert!(ts(r"\\W").is_err());
        // In a class, the rewrite nests a class, so a range with `\w` at its
        // start is a nested class and two literals.
        assert!(ok(r"[\w-a]"));
        assert!(ok(r"[\d-z]"));
    }

    #[test]
    fn test_assertions_in_zero_repetitions_are_erased() {
        for pattern in [
            "(?:^){0}",
            "(^){0}",
            "(?:^){0,0}",
            r"(?:\b){0}",
            "(?:$|a){0}",
        ] {
            assert!(ok(pattern), "{pattern:?}");
        }
        for pattern in ["(?:^){0,1}", "(?:^)?", "(?:^)*", "(?:^){1}"] {
            assert_eq!(
                ts(pattern).map_err(|e| e.reason),
                Err(ASSERTION),
                "{pattern:?}"
            );
        }
    }

    #[test]
    fn test_nesting_limit_matches_regex_syntax() {
        let groups = |n: usize| "(".repeat(n) + "a" + &")".repeat(n);
        assert!(ts(&groups(250)).is_ok());
        assert_eq!(ts(&groups(251)).map_err(|e| e.reason), Err(NESTING));
        let classes = |n: usize| "[".repeat(n) + "a" + &"]".repeat(n);
        assert!(ts(&classes(250)).is_ok());
        assert_eq!(ts(&classes(251)).map_err(|e| e.reason), Err(NESTING));
        // A concatenation of two is a level of its own.
        let pair = |n: usize| "(".repeat(n) + "ab" + &")".repeat(n);
        assert!(ts(&pair(249)).is_ok());
        assert_eq!(ts(&pair(250)).map_err(|e| e.reason), Err(NESTING));
        // Repetitions nest too.
        assert!(ts(&(String::from("a") + &"*".repeat(250))).is_ok());
        assert_eq!(
            ts(&(String::from("a") + &"*".repeat(251))).map_err(|e| e.reason),
            Err(NESTING)
        );
    }

    #[test]
    fn test_deep_and_long_patterns_are_linear_and_iterative() {
        let deep = "(".repeat(200_000) + &")".repeat(200_000);
        assert!(js(&deep).is_ok());
        assert_eq!(ts(&deep).map_err(|e| e.reason), Err(NESTING));
        let brackets = "[".repeat(200_000);
        assert!(js(&brackets).is_err());
        assert!(ts(&brackets).is_err());
        let braces = String::from("a") + &"{1".repeat(200_000);
        assert!(js(&braces).is_ok());
        assert!(ts(&braces).is_err());
        let ascii = String::from("[") + &"[:".repeat(200_000);
        assert!(ts(&ascii).is_err());
        // 100,000 named groups, each referred to: names go through ordered
        // sets, not a scan per name.
        let mut named = String::new();
        for i in 0..100_000 {
            named.push_str(&alloc::format!("(?<n{i}>x)"));
        }
        named.push_str(r"\k<n99999>");
        assert!(js(&named).is_ok());
        let named = named.replace(r"\k<n99999>", "");
        assert!(ts(&named).is_ok());
        // A run of unclosed named references fails at the first.
        let references = String::from("(?<a>x)") + &r"\k<aaaa".repeat(100_000);
        assert!(js(&references).is_err());
    }

    #[test]
    fn test_offsets_point_into_the_original_pattern() {
        // The `/` is written `\/`, and the tab `\t`, but offsets count the
        // pattern as given.
        assert_eq!(js("/\t[").map_err(|e| e.at), Err(2));
        assert_eq!(ts("é^").map_err(|e| e.at), Err(2));
        assert_eq!(ts(r"a\D^").map_err(|e| e.at), Err(3));
    }
}
