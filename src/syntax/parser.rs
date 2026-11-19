//! Recursive-descent parser (INTERNALS 4.1, 4.2).
//!
//! Parsing stops at the first error (4.5). Checks that need the whole tree (labels, scopes,
//! `break`/`return` placement) run afterwards in `check.rs` and report every violation.

use std::rc::Rc;

use super::ast::*;
use super::directives;
use super::lexer::{decimal_comma_hint, format_hint, KWARGS_HINT, LABEL_HINT, MERGE_HINT, ONLY_FIXED};
use super::span::{SourceMap, Span};
use super::token::{BraceKind, FPartTok, Kw, Op, Tok, Token};
use super::Diag;
use crate::ffi::cdecl;

/// Past this many nested expressions, bodies and interpolations the parser stops (4.1).
const MAX_DEPTH: u32 = 256;

/// Past this height of operator and postfix nodes in one expression tree (counting the
/// expression it sits in) the parser stops (4.1): `a + b + …` builds a tree as deep as the chain
/// is long, and later passes recurse over it.
const MAX_LINKS: u32 = 10_000;

/// The fixed compiled tags: no block args, exports by name only (4.5, 15.2).
pub const COMPILED_TAGS: &[&str] = &["c", "cpp", "cxx", "rust", "rs"];

pub fn parse_program(sm: &SourceMap, toks: Vec<Token>) -> Result<Vec<Stmt>, Diag> {
    let mut p = Parser { sm, toks, pos: 0, depth: 0, links: 0, peak: 0 };
    let result = p.stmts(false).and_then(|stmts| match p.peek() {
        Tok::Eof => Ok(stmts),
        other => Err(Diag::new(format!("unexpected {other}"), p.span())),
    });
    result.map_err(|d| match d.hint {
        None => match habit(&p.toks, d.span, sm) {
            Some(h) => d.hint(h),
            None => d,
        },
        Some(_) => d,
    })
}

/// Python's `def f(a, **kwargs)`, written as `func f(a, **kw)` or `func(**kw)` (2.4).
const KWARGS_PARAM_HINT: &str = "V2 has no **kwargs parameter: take the options as one dict parameter, as in func f(a, opts) { … }";

/// Python's `for root, dirs, files in os.walk(d)` and `for i, (a, b) in …`: V2's loop names one
/// or two (2.4).
const FOR_NAMES_HINT: &str = "a for loop takes one name, or two (index or key, then item): for e in walk(d) { let root = e[0] … }, or loop in a @py block";

/// What V2 writes for Python or JavaScript written out of habit, from the tokens where a syntax
/// error stopped (4.3): `def f(`, `var x`, `and`, `x is None`, `x => …`, `i++`, `i--`, `===`,
/// `if x:`, `function(x) {`, `class P {`, `switch x {`.
fn habit(toks: &[Token], at: Span, sm: &SourceMap) -> Option<String> {
    let i = toks.iter().position(|t| t.span.file == at.file && t.span.lo == at.lo)?;
    let tok = |back: usize| i.checked_sub(back).map(|j| &toks[j].tok);
    let word = |back: usize| match tok(back) {
        Some(Tok::Ident(w)) => Some(&**w),
        _ => None,
    };
    let op = |back: usize| match tok(back) {
        Some(Tok::Op(o)) => Some(*o),
        _ => None,
    };
    let ends_line = matches!(tok(0), Some(Tok::Newline | Tok::Semi | Tok::Eof | Tok::RBrace));
    // `Let x = 1`, `} Else {`: a keyword written with a capital, at a statement's start.
    let starts = |back: usize| i.checked_sub(back + 1).is_none_or(|j| matches!(toks[j].tok, Tok::Newline | Tok::Semi | Tok::RBrace | Tok::LBrace(_)));
    let capital = |w: Option<&str>| {
        const KEYWORDS: [&str; 15] = ["let", "const", "func", "return", "if", "elif", "else", "while", "for", "break", "continue", "try", "catch", "finally", "throw"];
        w.filter(|w| w.chars().next().is_some_and(char::is_uppercase)).map(str::to_lowercase).filter(|l| KEYWORDS.contains(&l.as_str()))
    };
    if let Some(kw) = capital(word(1)).filter(|_| starts(1)).or_else(|| capital(word(0)).filter(|_| starts(0))) {
        return Some(format!("V2's own words are written in small letters: {kw}"));
    }
    // Python's tuples, comprehensions and unpacking.
    // The bracket still open where the parser stopped (`idxmax()` before it is closed).
    let mut depth = 0usize;
    let mut open_at = None;
    for j in (0..i).rev() {
        match toks[j].tok {
            Tok::Op(Op::RParen | Op::RBracket) => depth += 1,
            Tok::Op(Op::LParen | Op::LBracket) if depth > 0 => depth -= 1,
            Tok::Op(Op::LParen | Op::LBracket) | Tok::Newline => {
                open_at = Some(j);
                break;
            }
            _ => {}
        }
    }
    let opener = open_at.map(|j| &toks[j].tok);
    let after_let = (0..i).rev().take_while(|&j| !matches!(toks[j].tok, Tok::Newline | Tok::Semi)).last().is_some_and(|j| matches!(toks[j].tok, Tok::Kw(Kw::Let | Kw::Const)));
    let after_return = (0..i).rev().take_while(|&j| !matches!(toks[j].tok, Tok::Newline | Tok::Semi | Tok::LBrace(_))).last().is_some_and(|j| matches!(toks[j].tok, Tok::Kw(Kw::Return)));
    match tok(0) {
        Some(Tok::Op(Op::Comma)) if matches!(opener, Some(Tok::Op(Op::LParen))) && !matches!(tok(1), Some(Tok::Op(Op::LParen))) => {
            return Some("V2 has no tuples: write a list, as in [1, 2]".into())
        }
        // `df.loc[r, "col"]`, `a[i, j]`: an index takes one key.
        Some(Tok::Op(Op::Comma))
            if matches!(opener, Some(Tok::Op(Op::LBracket)))
                && open_at.and_then(|j| j.checked_sub(1)).is_some_and(|j| matches!(toks[j].tok, Tok::Ident(_) | Tok::Op(Op::RParen | Op::RBracket) | Tok::Str(_))) =>
        {
            return Some("an index takes one key: write x[a][b], or, for a pandas or numpy handle, do it in a block, as in @py(df) { df.loc[r, \"col\"] }".into())
        }
        // `let preis = 19,99`, `x = 2,5`, `if x > 2,5 {`: a decimal comma where one expression ends
        // (`1,500`: or thousands).
        Some(Tok::Op(Op::Comma)) if decimal_comma(toks, i, sm).is_some() => return decimal_comma(toks, i, sm),
        Some(Tok::Op(Op::Comma)) if after_let => return Some("V2 declares one name per let: let a = 1, then let b = 2 (or one list, let ab = [1, 2])".into()),
        Some(Tok::Op(Op::Comma)) if after_return => return Some("V2 returns one value: return a list, as in return [a, b]".into()),
        Some(Tok::Kw(Kw::For)) if matches!(opener, Some(Tok::Op(Op::LBracket))) => {
            return Some("V2 has no comprehensions: write xs.map(func(x) => x * 2), or xs.filter(func(x) => x > 0)".into())
        }
        _ => {}
    }
    let hint = match (word(1), word(0), op(0)) {
        (Some("fn" | "def" | "function" | "fun" | "sub"), Some(name), _) => format!("V2 writes functions as func {name}(a, b) {{ ... }}"),
        (Some("var"), Some(_), _) => "V2 declares variables with let: let x = 1".into(),
        // `let mein name = 1`, `let mein-name = 1`: a name is one word.
        (Some(first), Some(_), _) if matches!(tok(2), Some(Tok::Kw(Kw::Let | Kw::Const))) => {
            format!("a name is one word, without spaces: {first}_{}", word(0).unwrap_or("name"))
        }
        (Some(first), _, Some(Op::Minus)) if matches!(tok(2), Some(Tok::Kw(Kw::Let | Kw::Const))) && matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Ident(_))) => {
            let second = match toks.get(i + 1).map(|t| &t.tok) {
                Some(Tok::Ident(w)) => w.to_string(),
                _ => "name".into(),
            };
            format!("a name can't have a - in it: {first}_{second}")
        }
        // `elif` or `else` with no `if` before it, or a second `else`.
        _ if matches!(tok(0), Some(Tok::Kw(Kw::Elif))) && starts(0) => "elif goes right after an if's }: if x { ... } elif y { ... }".into(),
        _ if matches!(tok(0), Some(Tok::Kw(Kw::Else))) && matches!(tok(1), Some(Tok::RBrace)) => {
            "an if has one else, at its end: for more cases, put elif x { ... } before it".into()
        }
        _ if matches!(tok(0), Some(Tok::Kw(Kw::Else))) && starts(0) => "else goes right after an if's }: if x { ... } else { ... }".into(),
        // `quadrat(x) {`: a function written without func.
        _ if matches!(tok(0), Some(Tok::LBrace(_))) && matches!(tok(1), Some(Tok::Op(Op::RParen))) && {
            let j = line_start(toks, i);
            // Other languages' statements that look the same keep their own hints.
            let other = matches!(&toks[j].tok, Tok::Ident(w) if matches!(&**w, "switch" | "match" | "foreach" | "elseif" | "elsif" | "unless" | "until" | "with" | "using" | "lock" | "synchronized"));
            matches!(toks[j].tok, Tok::Ident(_)) && !other && matches!(toks.get(j + 1).map(|t| &t.tok), Some(Tok::Op(Op::LParen)))
        } =>
        {
            let name = first_word_of_line(toks, i).unwrap_or("name");
            format!("a function is written with func: func {name}(…) {{ ... }}")
        }
        // JavaScript's and Python's `await f()`.
        (Some("await"), Some(name), _) => format!("V2 has no await: when a call returns a Promise or a coroutine, V2 waits for its result, so write {name}(…)"),
        (_, Some("and" | "or"), _) => "V2 writes and, or and not as &&, || and !".into(),
        (_, Some("is"), _) => "V2 compares with == and !=, as in x == null".into(),
        (Some("lambda"), Some(_), _) => "V2 writes an anonymous function as func(x) => x * 2".into(),
        (Some("class"), Some(_), _) => "V2 has no classes: keep the data in a dict and write functions for the rest, or write the class in a @py or @js block".into(),
        // `switch x {`, Python's `match x:`; `switch (x) {` parses as a call, then stops at `{`.
        (Some("switch" | "match"), Some(_), _) => "V2 has no switch or match: write if x == 1 { ... } elif x == 2 { ... } else { ... }".into(),
        _ if matches!(tok(0), Some(Tok::LBrace(_))) && first_word_of_line(toks, i) == Some("switch") => "V2 has no switch or match: write if x == 1 { ... } elif x == 2 { ... } else { ... }".into(),
        // `raise ValueError("bad")`, `throw new Error("bad")`, `new Foo()`.
        (Some("raise"), Some(kind), _) => format!("V2 throws an error with throw error(\"{kind}\", {})", message_after(toks, i)),
        (Some("new"), Some(kind), _) if matches!(tok(2), Some(Tok::Kw(Kw::Throw))) => {
            let message = message_after(toks, i);
            format!("V2 has no new: throw error(\"{kind}\", {message}), or throw {message} for a plain Error")
        }
        (Some("new"), Some(kind), _) => format!("V2 has no new: call it, as in {kind}(…)"),
        // Python's `"big" if x > 2 else "small"`.
        // `if x = 2 {`, `while x = 1 {`: a comparison written with one `=`.
        (_, _, Some(Op::Eq)) if matches!(toks[line_start(toks, i)].tok, Tok::Kw(Kw::If | Kw::Elif | Kw::While)) => {
            "V2 compares with ==, as in if x == 2 { ... }".into()
        }
        _ if matches!(tok(0), Some(Tok::Kw(Kw::If))) && !matches!(tok(1), None | Some(Tok::Newline | Tok::Semi | Tok::LBrace(_) | Tok::RBrace | Tok::Kw(Kw::Else | Kw::Func))) => {
            "V2 writes a conditional value as cond ? a : b, as in x > 2 ? \"big\" : \"small\"".into()
        }
        (Some("print"), _, _) if matches!(tok(0), Some(Tok::Str(_) | Tok::FStr(_) | Tok::Ident(_) | Tok::Int(_) | Tok::Float(_))) => {
            "print is a function in V2: print(\"hello\")".into()
        }
        _ if matches!(tok(0), Some(Tok::Kw(Kw::From))) && matches!(tok(1), None | Some(Tok::Newline | Tok::Semi)) => {
            from_import_hint(&toks[i + 1..]).unwrap_or_else(|| "V2 imports names with @import { mean } from py.statistics".into())
        }
        // `js.date-fns`: a package name with `-` goes in quotes, the whole name as written.
        (_, _, Some(Op::Minus)) if word(1).is_some() && op(2) == Some(Op::Dot) && matches!(word(3), Some("js" | "py" | "ts")) => {
            let name = dashed_name(&sm.file(toks[i - 1].span.file).text, toks[i - 1].span.lo as usize);
            format!("a package name with - goes in quotes: {}.\"{name}\"", word(3).unwrap_or("js"))
        }
        (_, _, Some(Op::FatArrow)) => "V2 writes an anonymous function as func(x) => x * 2".into(),
        // Python's `xs[::2]`, `xs[1:5:2]`.
        (_, _, Some(Op::Colon)) if slice_step(toks, i) => {
            "V2 slices have no step: xs[a:b] takes the items from a up to b (a @py block has xs[::2])".into()
        }
        // JavaScript's `[0, ...a]`.
        (_, _, Some(Op::Ellipsis)) if matches!(op(1), Some(Op::Comma | Op::LBracket)) => "... spreads a list into a call's arguments only: join lists with +, as in [0] + a".into(),
        // Python's `f(x, **opts)` and `def f(a, **kwargs)`; a grouping's `(**x)` gets neither.
        (_, _, Some(Op::StarStar)) if matches!(op(1), Some(Op::Comma | Op::LParen)) && matches!(opener, Some(Tok::Op(Op::LParen))) => {
            let before = |back: usize| open_at.and_then(|j| j.checked_sub(back)).map(|j| &toks[j].tok);
            match (before(2), before(1)) {
                (_, Some(Tok::Kw(Kw::Func))) | (Some(Tok::Kw(Kw::Func)), Some(Tok::Ident(_))) => KWARGS_PARAM_HINT.into(),
                (_, Some(Tok::Ident(_) | Tok::Op(Op::RParen | Op::RBracket) | Tok::Str(_))) => KWARGS_HINT.into(),
                _ => return None,
            }
        }
        (_, _, Some(Op::Plus)) if op(1) == Some(Op::Plus) => "V2 has no ++: write x += 1".into(),
        // JavaScript's `a ??= 5`, Ruby's and Perl's `elsif`, Python's and Go's `x := 1`.
        (_, _, Some(Op::Eq)) if op(1) == Some(Op::QQ) => format!("V2 has no ??=: write {0} = {0} ?? …", word(2).unwrap_or("x")),
        (_, Some("elsif" | "elseif"), _) if matches!(tok(1), Some(Tok::RBrace)) => "V2 writes elif: } elif x > 0 { ... }".into(),
        (_, _, Some(Op::Colon)) if word(1).is_some() && matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Op(Op::Eq))) => {
            format!("V2 declares a name with let {0} = …, and assigns to it with {0} = …", word(1).unwrap_or("x"))
        }
        _ if ends_line && op(1) == Some(Op::Minus) && op(2) == Some(Op::Minus) => "V2 has no --: write x -= 1".into(),
        (_, _, Some(Op::Eq)) if op(1) == Some(Op::DotDot) => "a..b leaves out b: for 1, 2 and 3 write 1..4".into(),
        (_, _, Some(Op::Eq)) if word(1).is_some() && matches!(op(2), Some(Op::Comma | Op::LParen)) => {
            format!("V2 writes a named argument as name: value, as in f(x, {}: 2)", word(1).unwrap_or("name"))
        }
        (_, Some(_), _) if matches!(tok(1), Some(Tok::Kw(Kw::Catch))) => "V2 writes catch (e) { ... }".into(),
        (_, _, Some(Op::Eq)) if op(1) == Some(Op::EqEq) => "V2 compares with == (there is no ===)".into(),
        (_, _, Some(Op::Eq)) if op(1) == Some(Op::Ne) => "V2 compares with != (there is no !==)".into(),
        // `:` continues its line (3.3), so the next token is on a later line.
        (_, _, Some(Op::Colon)) if toks.get(i + 1).is_none_or(|t| matches!(t.tok, Tok::Eof) || t.span.line > at.line) => {
            "V2 puts a block in { }, not after a colon: if x > 1 { ... }".into()
        }
        _ if matches!(tok(0), Some(Tok::LBrace(_))) && anonymous_function_before(toks, i) => {
            "V2 writes an anonymous function as func(x) { ... }".into()
        }
        _ => return None,
    };
    Some(hint)
}

/// The `,` at `toks[i]` between two number literals, with no space on either side (`19,99`): a
/// decimal comma, or thousands (`1,500`, `1,500,250`, `1,234,567.89`: the whole run of them), and
/// the hint for the number (2.4).
fn decimal_comma(toks: &[Token], i: usize, sm: &SourceMap) -> Option<String> {
    let text = &sm.file(toks.get(i)?.span.file).text;
    let literal = |t: &Token| match t.tok {
        Tok::Int(_) | Tok::Float(_) => text.get(t.span.lo as usize..t.span.hi as usize),
        _ => None,
    };
    // The `,` at `toks[k]` joins two such literals.
    let joins = |k: usize| match (k.checked_sub(1).and_then(|j| toks.get(j)), toks.get(k), toks.get(k + 1)) {
        (Some(a), Some(comma), Some(b)) => {
            matches!(comma.tok, Tok::Op(Op::Comma)) && a.span.hi == comma.span.lo && comma.span.hi == b.span.lo && literal(a).is_some() && literal(b).is_some()
        }
        _ => false,
    };
    if !joins(i) {
        return None;
    }
    let (mut first, mut last) = (i, i);
    while first >= 2 && joins(first - 2) {
        first -= 2;
    }
    while joins(last + 2) {
        last += 2;
    }
    let groups: Vec<&str> = (first - 1..=last + 1).step_by(2).filter_map(|k| literal(&toks[k])).collect();
    decimal_comma_hint(&groups)
}

/// Python's `from os.path import join, exists as e` as V2 writes it:
/// `V2 imports names with @import { join, exists as e } from py.os.path`; `None` for another shape.
fn from_import_hint(toks: &[Token]) -> Option<String> {
    let mut it = toks.iter().map(|t| &t.tok).peekable();
    let Some(Tok::Ident(first)) = it.next() else { return None };
    let mut module = first.to_string();
    while matches!(it.peek(), Some(Tok::Op(Op::Dot))) {
        it.next();
        let Some(Tok::Ident(seg)) = it.next() else { return None };
        module.push('.');
        module.push_str(seg);
    }
    if !matches!(it.next(), Some(Tok::Kw(Kw::Import))) {
        return None;
    }
    let mut names = Vec::new();
    while let Some(Tok::Ident(n)) = it.next() {
        let mut item = n.to_string();
        if matches!(it.peek(), Some(Tok::Kw(Kw::As))) {
            it.next();
            let Some(Tok::Ident(alias)) = it.next() else { return None };
            item = format!("{item} as {alias}");
        }
        names.push(item);
        if !matches!(it.peek(), Some(Tok::Op(Op::Comma))) {
            break;
        }
        it.next();
    }
    if names.is_empty() {
        return None;
    }
    Some(format!("V2 imports names with @import {{ {} }} from py.{module}", names.join(", ")))
}

/// The message of `raise ValueError("bad")` as written (the string after `(`), else `"text"`.
fn message_after(toks: &[Token], i: usize) -> String {
    match (toks.get(i + 1).map(|t| &t.tok), toks.get(i + 2).map(|t| &t.tok)) {
        (Some(Tok::Op(Op::LParen)), Some(Tok::Str(m))) => format!("{:?}", &**m),
        _ => "\"text\"".into(),
    }
}

/// The package name written at byte `lo` of `text`, with its dashes and digits (`date-fns`,
/// `utf-8-validate`, `is-in-browser`), up to a space or a character no package name has.
fn dashed_name(text: &str, lo: usize) -> String {
    let rest = &text[lo..];
    let end = rest.find(|c: char| c.is_whitespace() || "{}()[],;\"'".contains(c)).unwrap_or(rest.len());
    rest[..end].trim_end_matches('.').to_string()
}

/// A package selector as V2 writes it, from the text after `from` (2.4): `py.os.path`,
/// `js."date-fns"` (a name V2 can't read bare goes in quotes), and a quoted name, JavaScript's
/// `from "dayjs"` or a selector quoted whole, as `js.dayjs`; `None` for anything else (a block
/// label, `@py`).
fn package_selector(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"').and_then(|t| t.strip_suffix('"'));
    let (lang, rest) = match (inner, inner.unwrap_or(text).split_once('.')) {
        (_, Some((lang @ ("py" | "js" | "ts"), rest))) if !rest.is_empty() => (lang, rest),
        (Some(name), _) if !name.is_empty() => ("js", name),
        _ => return None,
    };
    let quote = |name: &str| {
        if name.starts_with('"') || super::lexer::is_plain_name(name) {
            name.to_string()
        } else {
            format!("\"{name}\"")
        }
    };
    if lang != "py" {
        // An npm name is one segment, dots and all.
        return Some(format!("{lang}.{}", quote(rest)));
    }
    let (mut segments, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in rest.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            '.' if !quoted => segments.push(quote(&std::mem::take(&mut cur))),
            _ => cur.push(c),
        }
    }
    segments.push(quote(&cur));
    Some(format!("py.{}", segments.join(".")))
}

/// Another .v2 file, named where a package goes: V2's own import of it, with `alias` or the file's
/// name, and how a name from it is reached (2.4).
fn v2_file_hint(path: &str, alias: Option<&str>, name: Option<&str>) -> String {
    let stem = path.rsplit(['/', '\\']).next().unwrap_or(path).trim_end_matches(".v2");
    let alias = alias.map_or_else(|| alias_for(stem), str::to_string);
    let then = name.map(|n| format!(", then {alias}.{n}(…)")).unwrap_or_default();
    format!("another .v2 file is imported with import \"{path}\" as {alias}{then}")
}

/// A name for `@import js."date-fns" as …`: the package's last part, with other characters as `_`.
fn alias_for(package: &str) -> String {
    let last = package.rsplit('/').next().unwrap_or(package);
    let alias: String = last.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if alias.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        alias
    } else {
        format!("m_{alias}")
    }
}

/// `{name}` (or `{a.b}`, `{x:.2f}`) in an f-string's text, Python's placeholder for V2's
/// `${name}`: what is inside, unless the braces are doubled (`{{name}}`, a template for something
/// else).
fn python_placeholder(parts: &[FPartTok]) -> Option<String> {
    for part in parts {
        let FPartTok::Lit(s) = part else { continue };
        let b = s.as_bytes();
        let mut i = 0;
        while let Some(off) = s[i..].find('{') {
            let open = i + off;
            let Some(len) = s[open + 1..].find('}') else { break };
            let inner = &s[open + 1..open + 1 + len];
            // `{{x}}` is a template for something else, and `${x}` here was written `\${x}`.
            let doubled = (open > 0 && matches!(b[open - 1], b'{' | b'$')) || b.get(open + 2 + len) == Some(&b'}');
            // A format after the name, as Python writes one: `.2f`, `>10`, `,`.
            let (name, spec) = inner.split_once(':').unwrap_or((inner, ""));
            let spec_ok = spec.chars().all(|c| c.is_ascii_alphanumeric() || ".,<>^=+-#_%".contains(c)) && (spec.is_empty() || !inner.ends_with(':'));
            if !doubled && !name.is_empty() && name.split('.').all(super::lexer::is_plain_name) && spec_ok {
                return Some(inner.to_string());
            }
            i = open + 1;
        }
    }
    None
}

/// Whether `toks[i]`, a `:`, is a second one inside `[ ]` on its line: a slice with a step.
fn slice_step(toks: &[Token], i: usize) -> bool {
    let (mut depth, mut colons) = (0usize, 0usize);
    for t in toks[..i].iter().rev() {
        match t.tok {
            Tok::Op(Op::RParen | Op::RBracket) | Tok::RBrace => depth += 1,
            Tok::Op(Op::LParen) | Tok::LBrace(_) if depth == 0 => return false,
            Tok::Op(Op::LBracket) if depth == 0 => return colons > 0,
            Tok::Op(Op::LParen | Op::LBracket) | Tok::LBrace(_) => depth -= 1,
            Tok::Op(Op::Colon) if depth == 0 => colons += 1,
            Tok::Newline | Tok::Semi => return false,
            _ => {}
        }
    }
    false
}

/// The index of the token that starts the statement of token `i` on its line.
fn line_start(toks: &[Token], i: usize) -> usize {
    let line = toks[i].span.line;
    let mut j = i;
    while j > 0 && toks[j - 1].span.line == line && !matches!(toks[j - 1].tok, Tok::Newline | Tok::Semi) {
        j -= 1;
    }
    j
}

/// The name that starts the line of token `i`, if a name does.
fn first_word_of_line(toks: &[Token], i: usize) -> Option<&str> {
    let j = line_start(toks, i);
    match &toks[j].tok {
        Tok::Ident(w) => Some(w),
        _ => None,
    }
}

/// Whether the line before `toks[i]` (a `{`) is `function(…)` or `fn(…)`, another language's
/// anonymous function.
fn anonymous_function_before(toks: &[Token], i: usize) -> bool {
    let line = toks[i].span.line;
    let mut j = i;
    while j > 0 && toks[j - 1].span.line == line {
        j -= 1;
        if let (Tok::Ident(w), Some(Tok::Op(Op::LParen))) = (&toks[j].tok, toks.get(j + 1).map(|t| &t.tok)) {
            if matches!(&**w, "function" | "fn") {
                return true;
            }
        }
    }
    false
}

/// A parameter list: the parameters with their defaults, and `...rest`.
type Params = (Vec<(Name, Option<Expr>)>, Option<Name>);

struct Parser<'a> {
    sm: &'a SourceMap,
    toks: Vec<Token>,
    pos: usize,
    depth: u32,
    /// The height, in chain links (binary operators, postfix steps), at which the node being
    /// parsed will sit: the chains it is an operand of, with their operands so far.
    links: u32,
    /// The highest `links` reached under the operand being measured; a chain's first operand
    /// and each later operand are measured alone, so a finished operand's height is
    /// `peak - links` (see `chain_begin`).
    peak: u32,
}

/// Where a chain started: `links` there, and the caller's `peak`.
struct ChainMark {
    links: u32,
    peak: u32,
}

fn name_of(t: &Token) -> Option<Name> {
    match &t.tok {
        Tok::Ident(n) => Some(Name::new(n.clone(), t.span)),
        _ => None,
    }
}

impl<'a> Parser<'a> {
    // -- token helpers ------------------------------------------------------------------------

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    /// At `(` after the name `lambda`: does a parenthesized list follow that ends in `=>` or a
    /// body `{`, as in another language's `lambda(x) => …`? (16: V2 writes `func(x) => …`.)
    fn lambda_follows(&self) -> bool {
        if !matches!(self.peek(), Tok::Op(Op::LParen)) {
            return false;
        }
        let mut depth = 0usize;
        for n in 0.. {
            match self.peek_at(n) {
                Tok::Op(Op::LParen) => depth += 1,
                Tok::Op(Op::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.peek_at(n + 1), Tok::Op(Op::FatArrow) | Tok::LBrace(_));
                    }
                }
                Tok::Eof => return false,
                _ => {}
            }
        }
        false
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }

    fn bump(&mut self) -> Token {
        // An f-string token holds the tokens of its interpolations: move them out, so nested
        // f-strings are not copied once per level.
        let t = match &mut self.toks[self.pos].tok {
            Tok::FStr(parts) => Token { tok: Tok::FStr(std::mem::take(parts)), span: self.toks[self.pos].span },
            _ => self.toks[self.pos].clone(),
        };
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn at_op(&self, op: Op) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn at_kw(&self, kw: Kw) -> bool {
        matches!(self.peek(), Tok::Kw(k) if *k == kw)
    }

    fn expect_op(&mut self, op: Op) -> Result<Span, Diag> {
        if self.at_op(op) {
            Ok(self.bump().span)
        } else {
            Err(Diag::new(format!("expected `{}`, found {}", op.as_str(), self.peek()), self.span()))
        }
    }

    fn expect_name(&mut self, what: &str) -> Result<Name, Diag> {
        let t = self.bump();
        name_of(&t).ok_or_else(|| {
            let d = Diag::new(format!("expected {what}, found {}", t.tok), t.span);
            match &t.tok {
                Tok::Kw(k) => d.hint(format!("{} is one of V2's own words: pick another name, as in {}_", k.as_str(), k.as_str())),
                _ => d,
            }
        })
    }

    fn skip_separators(&mut self) {
        while matches!(self.peek(), Tok::Newline | Tok::Semi) {
            self.bump();
        }
    }

    /// Whether the current token starts on the line where the previous token ends. A token
    /// never ends with its own line break, so its `hi` (a char boundary) is on its last line.
    fn on_previous_tokens_line(&self) -> bool {
        let prev = self.prev_span();
        let cur = self.span();
        let (end_line, _) = self.sm.line_col(prev.file, prev.hi);
        prev.file != cur.file || end_line == cur.line
    }

    /// Skip line breaks if the next real token is `kw` (`elif`, `else`, `catch`, `finally`).
    fn newlines_before(&mut self, kw: Kw) -> bool {
        let mut k = self.pos;
        while matches!(self.toks[k].tok, Tok::Newline) && k + 1 < self.toks.len() {
            k += 1;
        }
        if matches!(&self.toks[k].tok, Tok::Kw(x) if *x == kw) {
            self.pos = k;
            true
        } else {
            false
        }
    }

    fn deeper(&mut self) -> Result<(), Diag> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Diag::new("nested too deeply", self.span()));
        }
        Ok(())
    }

    // A left-associative chain builds its tree bottom-up: the chain so far becomes the left
    // child of each new node. So the height of what is already built (`cur`) is measured, and
    // every node checks its own height on top of where the chain sits.

    /// Start a chain; its first operand is measured alone.
    fn chain_begin(&mut self) -> ChainMark {
        let mark = ChainMark { links: self.links, peak: self.peak };
        self.peak = self.links;
        mark
    }

    /// The height of the operand just parsed.
    fn operand_height(&self) -> u32 {
        self.peak - self.links
    }

    /// A new node above the chain so far (height `cur`): check the limit, and measure the next
    /// operand alone, at the node's depth.
    fn chain_link(&mut self, mark: &ChainMark, cur: u32) -> Result<(), Diag> {
        self.links = mark.links + cur + 1;
        if self.links > MAX_LINKS {
            return Err(Diag::new("expression too long", self.span()).hint("split it into several statements with let"));
        }
        self.peak = self.links;
        Ok(())
    }

    /// The chain's height after a node and its operand (if any).
    fn after_link(&self, cur: u32) -> u32 {
        self.operand_height().max(cur) + 1
    }

    /// End a chain of height `cur`; the caller sees its height through `peak`.
    fn chain_end(&mut self, mark: ChainMark, cur: u32) {
        self.links = mark.links;
        self.peak = mark.peak.max(mark.links + cur);
    }

    /// A binary-only operator: one that can't start an expression (3.3).
    fn binary_only(t: &Tok, next: &Tok) -> bool {
        matches!(
            t,
            Tok::Op(
                Op::Plus
                    | Op::Star
                    | Op::Slash
                    | Op::Percent
                    | Op::StarStar
                    | Op::EqEq
                    | Op::Ne
                    | Op::Lt
                    | Op::Gt
                    | Op::Le
                    | Op::Ge
                    | Op::AndAnd
                    | Op::OrOr
                    | Op::QQ
                    | Op::Question
            ) | Tok::Kw(Kw::In)
        ) || (matches!(t, Tok::Kw(Kw::Not)) && matches!(next, Tok::Kw(Kw::In)))
    }

    /// A line that starts with a binary-only operator does not continue the previous one (3.3).
    fn leading_operator(&self, what: String) -> Diag {
        Diag::new(what, self.span()).hint("end the previous line with the operator")
    }

    // -- statements ---------------------------------------------------------------------------

    /// Statements up to `}` (when `in_body`) or the end of the file (4.1 `stmts`).
    fn stmts(&mut self, in_body: bool) -> Result<Vec<Stmt>, Diag> {
        let mut out = Vec::new();
        loop {
            self.skip_separators();
            match self.peek() {
                Tok::Eof => return Ok(out),
                Tok::RBrace if in_body => return Ok(out),
                _ => {}
            }
            out.push(self.statement()?);
            match self.peek() {
                Tok::Newline | Tok::Semi | Tok::Eof => {}
                Tok::RBrace if in_body => {}
                other => {
                    let mut d = Diag::new(format!("expected end of line, found {other}"), self.span());
                    if self.at_op(Op::DotDot) {
                        d = Diag::new("a..b is only allowed in a for head; use range(a, b)", self.span());
                    }
                    return Err(d);
                }
            }
        }
    }

    /// `{ stmts }` after a header (`what` names it for the hint).
    fn body(&mut self, what: &str) -> Result<Body, Diag> {
        self.deeper()?;
        let misplaced = |p: &Parser| Diag::new("expected {", p.span()).hint(format!("put {{ at the end of the {what} line"));
        let open = match self.peek() {
            // Inside ( [ or a dict no Newline is emitted (3.3), so compare lines directly.
            Tok::LBrace(BraceKind::Body) if self.pos > 0 && !self.on_previous_tokens_line() => return Err(misplaced(self)),
            Tok::LBrace(BraceKind::Body) => self.bump().span,
            Tok::Newline if matches!(self.peek_at(1), Tok::LBrace(_)) => {
                self.bump();
                return Err(misplaced(self));
            }
            Tok::Newline if Self::binary_only(self.peek_at(1), self.peek_at(2)) => {
                self.bump();
                let found = self.peek().clone();
                return Err(self.leading_operator(format!("expected {{, found {found} on the next line")));
            }
            // `if (x == 1) print(x)`: a body on the header's line, without braces.
            Tok::Ident(w) if self.on_previous_tokens_line() && !matches!(&**w, "and" | "or" | "is" | "not" | "then" | "do") && matches!(self.peek_at(1), Tok::Op(Op::LParen | Op::Eq | Op::Dot | Op::PlusEq)) => {
                let d = Diag::new(format!("expected {{, found {}", self.peek()), self.span());
                return Err(d.hint(format!("the body goes in braces, even for one statement: {what} … {{ ... }}")));
            }
            Tok::Kw(Kw::Return | Kw::Break | Kw::Continue | Kw::Throw | Kw::Let) if self.on_previous_tokens_line() => {
                let d = Diag::new(format!("expected {{, found {}", self.peek()), self.span());
                return Err(d.hint(format!("the body goes in braces, even for one statement: {what} … {{ ... }}")));
            }
            other => return Err(Diag::new(format!("expected {{, found {other}"), self.span())),
        };
        let stmts = self.stmts(true)?;
        if !matches!(self.peek(), Tok::RBrace) {
            return Err(Diag::new(format!("expected }}, found {}", self.peek()), self.span()));
        }
        let close = self.bump().span;
        self.depth -= 1;
        let funcs = stmts.iter().enumerate().filter(|(_, s)| matches!(s.kind, StmtKind::Func(_))).map(|(i, _)| i).collect();
        Ok(Body { stmts, funcs, layout: None, reusable: false, span: open.to(close) })
    }

    fn statement(&mut self) -> Result<Stmt, Diag> {
        let start = self.span();
        let kind = match self.peek() {
            Tok::Kw(Kw::Let) => {
                self.bump();
                let name = self.expect_name("a name after `let`")?;
                let value = if self.at_op(Op::Eq) {
                    self.bump();
                    Some(self.expr()?)
                } else {
                    None
                };
                StmtKind::Let(name, value)
            }
            Tok::Kw(Kw::Const) => {
                self.bump();
                let name = self.expect_name("a name after `const`")?;
                if !self.at_op(Op::Eq) {
                    return Err(Diag::new("a const needs a value: const NAME = value", self.span()));
                }
                self.bump();
                StmtKind::Const(name, self.expr()?)
            }
            Tok::Kw(Kw::If) => {
                self.bump();
                let mut arms = vec![(self.condition("if")?, self.body("if")?)];
                let mut else_ = None;
                loop {
                    if self.newlines_before(Kw::Elif) {
                        self.bump();
                        arms.push((self.condition("elif")?, self.body("elif")?));
                    } else if self.newlines_before(Kw::Else) {
                        self.bump();
                        if self.at_kw(Kw::If) {
                            return Err(Diag::new("V2 writes `else if` as `elif`", self.span()));
                        }
                        else_ = Some(self.body("else")?);
                        break;
                    } else {
                        break;
                    }
                }
                StmtKind::If { arms, else_ }
            }
            Tok::Kw(Kw::While) => {
                self.bump();
                let cond = self.condition("while")?;
                StmtKind::While { cond, body: self.body("while")? }
            }
            Tok::Kw(Kw::For) => self.for_stmt()?,
            Tok::Kw(Kw::Break) => {
                self.bump();
                StmtKind::Break
            }
            Tok::Kw(Kw::Continue) => {
                self.bump();
                StmtKind::Continue
            }
            Tok::Kw(Kw::Return) => {
                self.bump();
                if matches!(self.peek(), Tok::Newline | Tok::Semi | Tok::RBrace | Tok::Eof) {
                    StmtKind::Return(None)
                } else {
                    StmtKind::Return(Some(self.expr()?))
                }
            }
            Tok::Kw(Kw::Throw) => {
                self.bump();
                StmtKind::Throw(self.expr()?)
            }
            Tok::Kw(Kw::Try) => {
                self.bump();
                let body = self.body("try")?;
                let mut catch = None;
                let mut finally = None;
                if self.newlines_before(Kw::Catch) {
                    self.bump();
                    let name = if self.at_op(Op::LParen) {
                        self.bump();
                        let n = self.expect_name("a name for the error")?;
                        // TypeScript's `catch (e: Error)`, Java's `catch (Exception e)`.
                        if matches!(self.peek(), Tok::Op(Op::Colon) | Tok::Ident(_)) {
                            let d = Diag::new(format!("expected `)`, found {}", self.peek()), self.span());
                            return Err(d.hint("V2 catches every error: catch (e) { … }, then test e.type"));
                        }
                        self.expect_op(Op::RParen)?;
                        Some(n)
                    } else {
                        None
                    };
                    catch = Some((name, self.body("catch")?));
                }
                if self.newlines_before(Kw::Finally) {
                    self.bump();
                    finally = Some(self.body("finally")?);
                }
                if catch.is_none() && finally.is_none() {
                    let d = Diag::new("try needs a catch or a finally", start);
                    // Python's `except`, on the same line or the next.
                    let mut k = 0;
                    while matches!(self.peek_at(k), Tok::Newline) {
                        k += 1;
                    }
                    let except = matches!(self.peek_at(k), Tok::Ident(w) if &**w == "except");
                    return Err(if except { d.hint("V2 writes except as catch: try { ... } catch (e) { ... }") } else { d });
                }
                StmtKind::Try { body, catch, finally }
            }
            Tok::Kw(Kw::Func) if !matches!(self.peek_at(1), Tok::Op(Op::LParen)) => {
                self.bump();
                let name = self.expect_name("a function name after `func`")?;
                let f = self.func_rest(Some(name), start)?;
                StmtKind::Func(Rc::new(f))
            }
            Tok::Kw(Kw::Import) => {
                self.bump();
                let t = self.bump();
                let Tok::Str(path) = t.tok else {
                    let hint = self.import_habit(&t.tok);
                    return Err(Diag::new(format!("expected a path string after import, found {}", t.tok), t.span).hint(hint));
                };
                if !self.at_kw(Kw::As) {
                    return Err(Diag::new("expected `as NAME` after the import path", self.span()).hint("import \"./util.v2\" as util"));
                }
                self.bump();
                StmtKind::Import { path, alias: self.expect_name("a name after `as`")? }
            }
            Tok::DirImport => StmtKind::EngineImport(Rc::new(self.engine_import()?)),
            Tok::DirExport => {
                return Err(Diag::new("@export is only valid inside an embedded block", self.span())
                    .hint("modules export every top-level name not starting with _"))
            }
            Tok::Kw(Kw::Extern) => StmtKind::Extern(Rc::new(self.extern_stmt()?)),
            Tok::BlockHead { .. } => {
                let b = self.block()?;
                self.no_postfix_after_block()?;
                StmtKind::Block(b)
            }
            _ => self.expr_or_assign()?,
        };
        Ok(Stmt { kind, span: start.to(self.prev_span()) })
    }

    fn for_stmt(&mut self) -> Result<StmtKind, Diag> {
        self.bump();
        if self.at_op(Op::LParen) {
            // Python's `for (a, b) in pairs`: no unpacking (2.4).
            let names = matches!(self.peek_at(1), Tok::Ident(_)) && matches!(self.peek_at(2), Tok::Op(Op::Comma));
            return Err(Diag::new("a for head takes no parentheses", self.span()).hint(if names { FOR_NAMES_HINT } else { "for x in xs { … }" }));
        }
        // `for [a, b] in …`, `for i, (a, b) in …`: a loop variable that would unpack.
        let unpacks = |at: bool| move |d: Diag| if at { d.hint(FOR_NAMES_HINT) } else { d };
        let at = self.at_op(Op::LBracket);
        let first = self.expect_name("a loop variable after `for`").map_err(unpacks(at))?;
        let second = if self.at_op(Op::Comma) {
            self.bump();
            let at = self.at_op(Op::LParen) || self.at_op(Op::LBracket);
            Some(self.expect_name("a second loop variable").map_err(unpacks(at))?)
        } else {
            None
        };
        if !self.at_kw(Kw::In) {
            let d = Diag::new(format!("expected `in`, found {}", self.peek()), self.span());
            // Python's `for root, dirs, files in os.walk(d)`: a third name.
            return Err(if second.is_some() && self.at_op(Op::Comma) { d.hint(FOR_NAMES_HINT) } else { d });
        }
        self.bump();
        self.deeper()?;
        let start = self.ternary()?;
        self.depth -= 1;
        let iter = if self.at_op(Op::DotDot) {
            let dots = self.bump().span;
            if second.is_some() {
                return Err(Diag::new("a range loop takes one variable", dots));
            }
            ForIter::Range(start, self.expr()?)
        } else {
            ForIter::Value(start)
        };
        let body = self.body("for")?;
        Ok(StmtKind::For { first, second, iter, body })
    }

    /// `( params ) body` of a `func` statement, or `( params ) ( => expr | body )` of an
    /// anonymous `func` expression.
    /// The condition of `if`, `elif` or `while`; a `{` right away means there is none.
    fn condition(&mut self, what: &str) -> Result<Expr, Diag> {
        if matches!(self.peek(), Tok::LBrace(_)) {
            return Err(Diag::new(format!("{what} needs a condition before its {{"), self.span()).hint(format!("as in {what} x < 3 {{ ... }}")));
        }
        self.expr()
    }

    fn func_rest(&mut self, name: Option<Name>, start: Span) -> Result<FuncDecl, Diag> {
        // `func hallo {`: the brackets are needed, empty or not.
        if let (Tok::LBrace(_), Some(n)) = (self.peek(), &name) {
            return Err(Diag::new("expected `(`, found `{`", self.span()).hint(format!("a function has brackets for its parameters, even none: func {}() {{ ... }}", n.text)));
        }
        self.expect_op(Op::LParen)?;
        let (params, rest) = self.params()?;
        if let (Some(n), true) = (&name, self.at_op(Op::FatArrow)) {
            let names: Vec<&str> = params.iter().map(|(p, _)| &*p.text).collect();
            let (f, args) = (&n.text, names.join(", "));
            return Err(Diag::new("expected {, found `=>`", self.span())
                .hint(format!("a named function has a {{ }} body: func {f}({args}) {{ return … }}, or write let {f} = func({args}) => …")));
        }
        let body = if name.is_none() && self.at_op(Op::FatArrow) {
            self.bump();
            // `=> {` starts a dict: the JS habit of a block after `=>` lands here.
            let dict = matches!(self.peek(), Tok::LBrace(_));
            let hint = "`=>` takes one expression, so `{` starts a dict; for statements write func(…) { … }";
            // Unless the key itself has a hint: `=> {1: "x"}` is a dict, with a number for a key.
            FuncBody::Expr(self.expr().map_err(|d| if dict && d.message.contains("dict key") && d.hint.is_none() { d.hint(hint) } else { d })?)
        } else {
            FuncBody::Block(self.body("func")?)
        };
        Ok(FuncDecl { name, params, rest, body, layout: Layout::from([]), span: start.to(self.prev_span()), anon_name: std::cell::OnceCell::new() })
    }

    /// Parameters through the closing `)` (4.1, 4.5).
    fn params(&mut self) -> Result<Params, Diag> {
        let mut params: Vec<(Name, Option<Expr>)> = Vec::new();
        let mut rest: Option<Name> = None;
        loop {
            if self.at_op(Op::RParen) {
                self.bump();
                return Ok((params, rest));
            }
            if let Some(r) = &rest {
                return Err(Diag::new("...rest must be the last parameter", r.span));
            }
            if self.at_op(Op::Ellipsis) {
                self.bump();
                let n = self.expect_name("a name after `...`")?;
                self.check_dup_param(&params, &n)?;
                rest = Some(n);
            } else {
                // Python's `def f(*args)`; its keyword-only `*` alone (`def f(a, *, b)`) isn't one.
                let star = match (self.peek(), self.peek_at(1)) {
                    (Tok::Op(Op::Star), Tok::Ident(w)) => Some(w.to_string()),
                    _ => None,
                };
                let n = self.expect_name("a parameter name").map_err(|d| match &star {
                    Some(rest) => d.hint(format!("V2 collects extra arguments with ...{rest}")),
                    None => d,
                })?;
                self.check_dup_param(&params, &n)?;
                let default = if self.at_op(Op::Eq) {
                    self.bump();
                    Some(self.expr()?)
                } else {
                    if params.iter().any(|(_, d)| d.is_some()) {
                        return Err(Diag::new("a parameter without a default can't follow one with a default", n.span));
                    }
                    None
                };
                params.push((n, default));
            }
            if self.at_op(Op::Comma) {
                self.bump();
            } else if self.at_op(Op::Colon) {
                return Err(Diag::new(format!("expected `,` or `)`, found {}", self.peek()), self.span())
                    .hint("V2 has no type annotations; a default is written with =, as in func f(x, key = 1)"));
            } else if !self.at_op(Op::RParen) {
                return Err(Diag::new(format!("expected `,` or `)`, found {}", self.peek()), self.span()));
            }
        }
    }

    fn check_dup_param(&self, params: &[(Name, Option<Expr>)], n: &Name) -> Result<(), Diag> {
        match params.iter().find(|(p, _)| p.text == n.text) {
            Some((p, _)) => Err(Diag::new(format!("duplicate parameter '{}'", n.text), n.span).note("first declared here", p.span)),
            None => Ok(()),
        }
    }

    fn expr_or_assign(&mut self) -> Result<StmtKind, Diag> {
        let target = self.expr()?;
        let op = match self.peek() {
            Tok::Op(Op::Eq) => None,
            Tok::Op(Op::PlusEq) => Some(BinOp::Add),
            Tok::Op(Op::MinusEq) => Some(BinOp::Sub),
            Tok::Op(Op::StarEq) => Some(BinOp::Mul),
            Tok::Op(Op::SlashEq) => Some(BinOp::Div),
            Tok::Op(Op::PercentEq) => Some(BinOp::Rem),
            _ => return Ok(StmtKind::Expr(target)),
        };
        let op = op.map(|o| (o, self.span()));
        self.bump();
        let span = target.span;
        let target = match target.kind {
            ExprKind::Name(n, _) => Target::Name(Name::new(n, span), Loc::Unresolved),
            ExprKind::Member { obj, name, optional: false } => Target::Member(obj, name),
            ExprKind::Index { obj, index, bracket } => Target::Index(obj, index, bracket),
            ExprKind::OptChain(_) => return Err(Diag::new("cannot assign through ?.", span)),
            _ => return Err(Diag::new("cannot assign to this expression", span)),
        };
        let value = self.expr()?;
        Ok(StmtKind::Assign { target, op, value })
    }

    // -- @import, extern ----------------------------------------------------------------------

    /// `@import { items } from SELECTOR` or `@import PACKAGE as NAME` at V2 level (4.1).
    fn engine_import(&mut self) -> Result<EngineImport, Diag> {
        let start = self.bump().span;
        // Python's `from math import *`, written as `@import * from py.math`.
        if self.at_op(Op::Star) {
            return Err(Diag::new("@import needs a list of names, or a module and a name for it", self.span())
                .hint("V2 has no import *: list the names, @import { sqrt, pi } from py.math, or import the module, @import py.math as math"));
        }
        if matches!(self.peek(), Tok::LBrace(_)) {
            let open = self.bump().span;
            if self.at_op(Op::Star) {
                return Err(Diag::new("@import needs a list of names", self.span()).hint("list the names: @import { mean } from …"));
            }
            let items = {
                let mut items = Vec::new();
                loop {
                    let t = self.bump();
                    let (name, quoted) = match &t.tok {
                        Tok::Ident(n) => (n.clone(), false),
                        Tok::Str(s) => (s.clone(), true),
                        Tok::Kw(k) => {
                            return Err(Diag::new(format!("expected a name in the @import list, found keyword `{}`", k.as_str()), t.span)
                                .hint(format!("quote it and give it a name: \"{}\" as NAME", k.as_str())))
                        }
                        Tok::RBrace if items.is_empty() => return Err(Diag::new("an @import list can't be empty", open)),
                        other => return Err(Diag::new(format!("expected a name in the @import list, found {other}"), t.span)),
                    };
                    let alias = if self.at_kw(Kw::As) {
                        self.bump();
                        Some(self.expect_name("a name after `as`")?)
                    } else {
                        None
                    };
                    if quoted && alias.is_none() && !super::lexer::is_plain_name(&name) {
                        return Err(Diag::new(format!("quoted name '{name}' needs an alias: \"{name}\" as NAME"), t.span));
                    }
                    items.push(ImportItem { name, alias, span: t.span });
                    if self.at_op(Op::Comma) {
                        self.bump();
                        if matches!(self.peek(), Tok::RBrace) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                ImportItems::Names(items)
            };
            if !matches!(self.peek(), Tok::RBrace) {
                return Err(Diag::new(format!("expected `,` or `}}` in the @import list, found {}", self.peek()), self.span()));
            }
            self.bump();
            if !self.at_kw(Kw::From) {
                return Err(Diag::new(format!("expected `from` after the @import list, found {}", self.peek()), self.span()));
            }
            self.bump();
            let selector = self.selector()?;
            Ok(EngineImport { items, selector, span: start.to(self.prev_span()) })
        } else {
            let selector = self.selector()?;
            // `@import math from py.math`: names go in braces (Python's `from m import x` habit).
            if let (Selector::Label { label }, true) = (&selector, self.at_kw(Kw::From)) {
                let name = label.text.clone();
                self.bump();
                // The selector after `from` as written: a package also has the whole-module form.
                let written = self.written_selector();
                let hint = match package_selector(&written) {
                    Some(sel) => {
                        let js = sel.starts_with("js.") || sel.starts_with("ts.");
                        let default = if js { format!(" (a default export: @import {{ \"default\" as {name} }} from {sel})") } else { String::new() };
                        format!("write @import {{ {name} }} from {sel}{default}, or @import {sel} as {name} for the whole module")
                    }
                    None if written.is_empty() => format!("write @import {{ {name} }} from a block's label or a package (py.module, js.package)"),
                    None => format!("write @import {{ {name} }} from {written}"),
                };
                return Err(Diag::new("expected `{` before the names to import", label.span).hint(hint));
            }
            if !matches!(selector, Selector::Module { .. }) {
                return Err(Diag::new("only a package selector (py.name, js.name) can be imported as a whole module", self.prev_span()));
            }
            if !self.at_kw(Kw::As) {
                let d = Diag::new("expected `as NAME` after the package selector", self.span());
                return Err(match &selector {
                    // `js.date-fns`: the whole name, in quotes.
                    Selector::Module { lang, path } if self.at_op(Op::Minus) && path.len() == 1 => {
                        let name = dashed_name(&self.sm.file(path[0].span.file).text, path[0].span.lo as usize);
                        // The user's own `as NAME` later on the line, if there is one.
                        let line = path[0].span.line;
                        let alias = self.toks[self.pos..]
                            .windows(2)
                            .take_while(|w| w[0].span.line == line)
                            .find_map(|w| match (&w[0].tok, &w[1].tok) {
                                (Tok::Kw(Kw::As), Tok::Ident(a)) => Some(a.to_string()),
                                _ => None,
                            })
                            .unwrap_or_else(|| alias_for(&name));
                        d.hint(format!("a package name with - goes in quotes: @import {}.\"{name}\" as {alias}", lang.text))
                    }
                    Selector::Module { lang, path } => {
                        let alias = path.last().map(|n| n.text.to_string()).filter(|n| super::lexer::is_plain_name(n)).unwrap_or_else(|| "m".into());
                        let dotted: Vec<&str> = path.iter().map(|n| &*n.text).collect();
                        d.hint(format!("@import {}.{} as {alias}", lang.text, dotted.join(".")))
                    }
                    _ => d,
                });
            }
            self.bump();
            let alias = self.expect_name("a name after `as`")?;
            Ok(EngineImport { items: ImportItems::Module(alias), selector, span: start.to(self.prev_span()) })
        }
    }

    /// The hint for `import` followed by `t`, something other than a path (2.4): Python's
    /// `import numpy as np`, JavaScript's `import dayjs from "dayjs"`, `import { a } from "x"` and
    /// `import * as m from "x"`, as V2 writes them. The parser stands after `t`.
    fn import_habit(&self, t: &Tok) -> String {
        // The package of a JavaScript `from "x"` at `k` tokens ahead.
        let from_at = |k: usize| match (self.peek_at(k), self.peek_at(k + 1)) {
            (Tok::Kw(Kw::From), Tok::Str(p)) => package_selector(&format!("\"{p}\"")),
            _ => None,
        };
        // A `from "./x.v2"`: another V2 file, not a package.
        let v2_file = |k: usize| match (self.peek_at(k), self.peek_at(k + 1)) {
            (Tok::Kw(Kw::From), Tok::Str(p)) if p.ends_with(".v2") => Some(p.to_string()),
            _ => None,
        };
        match t {
            Tok::Ident(m) if self.at_kw(Kw::From) && v2_file(0).is_some() => v2_file_hint(&v2_file(0).unwrap_or_default(), Some(&**m), None),
            Tok::Ident(m) if self.at_kw(Kw::From) => {
                let sel = from_at(0).unwrap_or_else(|| "js.package".into());
                format!("V2 imports a JavaScript default export with @import {{ \"default\" as {m} }} from {sel}")
            }
            Tok::Ident(m) => {
                let mut module = m.to_string();
                let mut k = 0;
                while let (Tok::Op(Op::Dot), Tok::Ident(seg)) = (self.peek_at(k), self.peek_at(k + 1)) {
                    module.push('.');
                    module.push_str(seg);
                    k += 2;
                }
                let alias = match (self.peek_at(k), self.peek_at(k + 1)) {
                    (Tok::Kw(Kw::As), Tok::Ident(a)) => a.to_string(),
                    _ => module.rsplit('.').next().unwrap_or_default().to_string(),
                };
                format!("a Python module is imported with @import py.{module} as {alias}; another .v2 file with import \"./util.v2\" as util")
            }
            Tok::LBrace(_) => {
                let mut names = Vec::new();
                let mut k = 0;
                while let Tok::Ident(n) = self.peek_at(k) {
                    let mut item = n.to_string();
                    k += 1;
                    if let (Tok::Kw(Kw::As), Tok::Ident(a)) = (self.peek_at(k), self.peek_at(k + 1)) {
                        item = format!("{item} as {a}");
                        k += 2;
                    }
                    names.push(item);
                    if !matches!(self.peek_at(k), Tok::Op(Op::Comma)) {
                        break;
                    }
                    k += 1;
                }
                if let Some(path) = matches!(self.peek_at(k), Tok::RBrace).then(|| v2_file(k + 1)).flatten() {
                    return v2_file_hint(&path, None, names.first().and_then(|n| n.split(" as ").next()));
                }
                let sel = matches!(self.peek_at(k), Tok::RBrace).then(|| from_at(k + 1)).flatten();
                match (names.is_empty(), sel) {
                    (false, Some(sel)) => format!("V2 imports names with @import {{ {} }} from {sel}", names.join(", ")),
                    _ => "V2 imports names with @import { a, b } from js.package (or py.module, or a block's label)".into(),
                }
            }
            Tok::Op(Op::Star) if matches!((self.peek_at(0), self.peek_at(1)), (Tok::Kw(Kw::As), Tok::Ident(_))) && v2_file(2).is_some() => {
                let alias = if let Tok::Ident(a) = self.peek_at(1) { a.to_string() } else { String::new() };
                v2_file_hint(&v2_file(2).unwrap_or_default(), Some(&alias), None)
            }
            Tok::Op(Op::Star) => match (self.peek_at(0), self.peek_at(1), from_at(2)) {
                (Tok::Kw(Kw::As), Tok::Ident(a), Some(sel)) => format!("V2 imports a whole package with @import {sel} as {a}"),
                _ => "V2 imports a whole package with @import js.package as name".into(),
            },
            _ => "import \"./util.v2\" as util".into(),
        }
    }

    /// What was written from the next token to the first space outside quotes (or a `//`
    /// comment): the selector as the user typed it, for hints.
    fn written_selector(&self) -> String {
        let t = &self.toks[self.pos.min(self.toks.len() - 1)];
        if matches!(t.tok, Tok::Eof | Tok::Newline | Tok::Semi) {
            return String::new();
        }
        let text = &self.sm.file(t.span.file).text;
        let rest = &text[t.span.lo as usize..];
        let mut quoted = false;
        let mut end = rest.len();
        for (i, c) in rest.char_indices() {
            if c == '"' {
                quoted = !quoted;
            } else if !quoted && (c.is_whitespace() || rest[i..].starts_with("//")) {
                end = i;
                break;
            }
        }
        rest[..end].to_string()
    }

    /// `@tag`, `@tag.label`, `label`, `lang.seg.seg`, `v2` (4.1).
    fn selector(&mut self) -> Result<Selector, Diag> {
        let t = self.bump();
        match t.tok {
            Tok::AtTag(tag) => {
                let tag = Name::new(tag, t.span);
                if self.at_op(Op::Dot) {
                    self.bump();
                    if let Tok::Str(_) = self.peek() {
                        return Err(Diag::new("a block label can't be a string", self.span()).hint(LABEL_HINT));
                    }
                    let label = self.segment()?;
                    Ok(Selector::Exact { tag, label })
                } else {
                    Ok(Selector::Union { tag })
                }
            }
            Tok::Ident(n) => {
                if self.at_op(Op::Dot) {
                    let lang = Name::new(n, t.span);
                    let mut path = Vec::new();
                    while self.at_op(Op::Dot) {
                        self.bump();
                        path.push(self.segment()?);
                    }
                    // `js."./util.v2"`: Node would read a V2 file as JavaScript.
                    if let [seg] = &path[..] {
                        if seg.text.ends_with(".v2") {
                            return Err(Diag::new(format!("a .v2 file isn't a {} module", if &*lang.text == "py" { "Python" } else { "JavaScript" }), seg.span)
                                .hint(v2_file_hint(&seg.text, None, None) + " (a statement of its own, not @import)"));
                        }
                    }
                    Ok(Selector::Module { lang, path })
                } else if &*n == "v2" {
                    Ok(Selector::V2 { span: t.span })
                } else {
                    Ok(Selector::Label { label: Name::new(n, t.span) })
                }
            }
            // `from "py.statistics"`: the package selector, quoted whole by mistake.
            // `@import { f } from "./util.v2"`: V2 files have their own import.
            Tok::Str(s) if s.ends_with(".v2") => Err(Diag::new("a block label can't be a string", t.span).hint(v2_file_hint(&s, None, None) + " (a statement of its own, not @import)")),
            Tok::Str(s) if ["py.", "js.", "ts."].iter().any(|p| s.starts_with(p)) => {
                let sel = package_selector(&format!("\"{s}\"")).unwrap_or_else(|| s.to_string());
                let hint = if sel.contains('"') {
                    format!("a package is written {sel}: the quotes go around the name only")
                } else {
                    format!("a package is written {sel}, without quotes")
                };
                Err(Diag::new("a block label can't be a string", t.span).hint(hint))
            }
            Tok::Str(_) => Err(Diag::new("a block label can't be a string", t.span).hint(LABEL_HINT)),
            other => Err(Diag::new(format!("expected a selector (@py, a block label, or py.module), found {other}"), t.span)),
        }
    }

    fn segment(&mut self) -> Result<Name, Diag> {
        let t = self.bump();
        match t.tok {
            Tok::Ident(n) | Tok::Str(n) => Ok(Name::new(n, t.span)),
            other => Err(Diag::new(format!("expected a name after `.`, found {other}"), t.span)),
        }
    }

    /// `extern "lib" [as name] { C declarations }`: the declarations are read now (3.9, 16.1).
    fn extern_stmt(&mut self) -> Result<Extern, Diag> {
        self.bump();
        let t = self.bump();
        let Tok::Str(lib) = t.tok else { unreachable!("the lexer checked the library string") };
        let lib = Name::new(lib, t.span);
        let alias = if self.at_kw(Kw::As) {
            self.bump();
            Some(self.expect_name("a name after `as`")?)
        } else {
            None
        };
        let t = self.bump();
        let Tok::ExternBody(raw) = t.tok else { unreachable!("the lexer captured the extern body") };
        let base = raw.text_span.lo;
        let file = raw.text_span.file;
        let at = |lo: usize, hi: usize| self.sm.span(file, base + lo as u32, base + hi.max(lo + 1) as u32);
        let parsed = cdecl::parse_extern(&raw.text).map_err(|e| {
            // The body holds C declarations, and extern itself comes with compiled blocks (7.3).
            let d = Diag::new(e.message, at(e.lo, e.hi))
                .hint("an extern body holds C declarations, as in double cos(double x); (extern itself isn't in this version of V2 yet)");
            match e.note {
                Some((text, lo, hi)) => d.note(text, at(lo, hi)),
                None => d,
            }
        })?;
        let decl_spans = parsed.iter().map(|(_, lo, hi)| self.sm.span(file, base + *lo as u32, base + *hi as u32)).collect();
        let decls = parsed.into_iter().map(|(s, _, _)| s).collect();
        Ok(Extern { lib, alias, decls, decl_spans, body_span: raw.text_span })
    }

    // -- blocks -------------------------------------------------------------------------------

    /// After a block value: an operator or postfix needs parentheses (4.1).
    fn no_postfix_after_block(&self) -> Result<(), Diag> {
        if self.at_op(Op::LParen) || self.at_op(Op::Dot) || self.at_op(Op::LBracket) || self.at_op(Op::QDot) {
            return Err(Diag::new("a block value must be in parentheses here: (@py { … }).m()", self.span()));
        }
        Ok(())
    }

    /// `@tag [label] [( args )] body` (4.1).
    fn block(&mut self) -> Result<Rc<EngineBlock>, Diag> {
        let head = self.bump();
        let Tok::BlockHead { tag, label } = head.tok else { unreachable!("called at a block head") };
        let at = head.span;
        let tag_name = Name::new(tag.clone(), Span { hi: at.lo + 1 + tag.len() as u32, ..at });
        let label = label.map(|(text, span)| Name::new(text, span));
        let mut args: Vec<(Name, Expr)> = Vec::new();
        if self.at_op(Op::LParen) {
            let open = self.bump().span;
            loop {
                if self.at_op(Op::RParen) {
                    self.bump();
                    break;
                }
                let t = self.bump();
                // `@py(1 + 2)`, `@py(data.copy())`: a value goes after a name for it (4.1).
                let args_hint = format!("block args are `name` or `name: expr`, as in @{tag}(n: len(xs))");
                let Some(name) = name_of(&t) else {
                    return Err(Diag::new(format!("expected a block arg name, found {}", t.tok), t.span).hint(args_hint));
                };
                let value = if self.at_op(Op::Colon) {
                    self.bump();
                    self.expr()?
                } else {
                    Expr { kind: ExprKind::Name(name.text.clone(), Loc::Unresolved), span: name.span }
                };
                if let Some((prev, _)) = args.iter().find(|(n, _)| n.text == name.text) {
                    return Err(Diag::new(format!("block arg '{}' is given twice", name.text), name.span)
                        .note("first given here", prev.span));
                }
                args.push((name, value));
                if self.at_op(Op::Comma) {
                    self.bump();
                } else if !self.at_op(Op::RParen) {
                    return Err(Diag::new(format!("expected `,` or `)`, found {}", self.peek()), self.span()).hint(args_hint));
                }
            }
            if COMPILED_TAGS.contains(&&*tag) && !args.is_empty() {
                return Err(Diag::new("compiled blocks take no args — @export a function and call it with arguments", open));
            }
        }
        let body_tok = self.bump();
        let Tok::BlockBody(raw) = body_tok.tok else {
            return Err(Diag::new(format!("expected a block body, found {}", body_tok.tok), body_tok.span));
        };
        let processed = directives::process(self.sm, &raw, &tag)?;
        // A block arg may not share a name with what an inner @import binds (12.1, 13.7).
        for imp in &processed.directives.imports {
            let bound: Vec<(Rc<str>, Span)> = match &imp.items {
                ImportItems::Names(items) => items
                    .iter()
                    .map(|i| match &i.alias {
                        Some(a) => (a.text.clone(), a.span),
                        None => (i.name.clone(), i.span),
                    })
                    .collect(),
                ImportItems::Module(alias) => vec![(alias.text.clone(), alias.span)],
            };
            for (name, at) in bound {
                if let Some((arg, _)) = args.iter().find(|(n, _)| n.text == name) {
                    return Err(Diag::new(format!("'{name}' is already declared in this block"), arg.span)
                        .note("also bound by this @import", at));
                }
            }
        }
        Ok(Rc::new(EngineBlock {
            unit_id: UnitId { file: at.file, offset: at.lo },
            tag: tag_name,
            label,
            args,
            body: processed.body,
            loc: processed.loc,
            body_span: raw.text_span,
            delimiter: raw.delim,
            directives: processed.directives,
            span: at.to(raw.close),
        }))
    }

    // -- expressions (4.2) --------------------------------------------------------------------

    fn expr(&mut self) -> Result<Expr, Diag> {
        let e = self.expr_before_range()?;
        if self.at_op(Op::DotDot) {
            return Err(Diag::new("a..b is only allowed in a for head; use range(a, b)", self.span()));
        }
        Ok(e)
    }

    /// An `expr` that leaves a following `..` to the caller: the else branch of a ternary
    /// that starts a for range (`for i in c ? 0 : 1..10`).
    fn expr_before_range(&mut self) -> Result<Expr, Diag> {
        self.deeper()?;
        let e = if matches!(self.peek(), Tok::BlockHead { .. }) { self.block_value()? } else { self.ternary()? };
        self.depth -= 1;
        Ok(e)
    }

    /// A block used as a value must be an action block (4.5).
    fn block_value(&mut self) -> Result<Expr, Diag> {
        let b = self.block()?;
        if let Some(ex) = &b.directives.exports {
            return Err(Diag::new("a block with @export is a definition block and cannot be used as a value", b.tag.span)
                .note("the @export that makes it one", ex.at.unwrap_or(b.span)));
        }
        self.no_postfix_after_block()?;
        let span = b.span;
        Ok(Expr { kind: ExprKind::Block(b), span })
    }

    fn ternary(&mut self) -> Result<Expr, Diag> {
        let cond = self.coalesce()?;
        if !self.at_op(Op::Question) {
            return Ok(cond);
        }
        self.bump();
        let a = self.expr()?;
        self.expect_op(Op::Colon)?;
        let b = self.expr_before_range()?;
        let span = cond.span.to(b.span);
        Ok(Expr { kind: ExprKind::Ternary(Box::new(cond), Box::new(a), Box::new(b)), span })
    }

    fn coalesce(&mut self) -> Result<Expr, Diag> {
        let mark = self.chain_begin();
        let mut e = self.or()?;
        let mut h = self.operand_height();
        while self.at_op(Op::QQ) {
            self.chain_link(&mark, h)?;
            self.bump();
            let r = self.or()?;
            h = self.after_link(h);
            let span = e.span.to(r.span);
            e = Expr { kind: ExprKind::Coalesce(Box::new(e), Box::new(r)), span };
        }
        self.chain_end(mark, h);
        Ok(e)
    }

    fn or(&mut self) -> Result<Expr, Diag> {
        let mark = self.chain_begin();
        let mut e = self.and()?;
        let mut h = self.operand_height();
        while self.at_op(Op::OrOr) {
            self.chain_link(&mark, h)?;
            self.bump();
            let r = self.and()?;
            h = self.after_link(h);
            let span = e.span.to(r.span);
            e = Expr { kind: ExprKind::Or(Box::new(e), Box::new(r)), span };
        }
        self.chain_end(mark, h);
        Ok(e)
    }

    fn and(&mut self) -> Result<Expr, Diag> {
        let mark = self.chain_begin();
        let mut e = self.equality()?;
        let mut h = self.operand_height();
        while self.at_op(Op::AndAnd) {
            self.chain_link(&mark, h)?;
            self.bump();
            let r = self.equality()?;
            h = self.after_link(h);
            let span = e.span.to(r.span);
            e = Expr { kind: ExprKind::And(Box::new(e), Box::new(r)), span };
        }
        self.chain_end(mark, h);
        Ok(e)
    }

    fn binary(&mut self, op: BinOp, op_span: Span, l: Expr, r: Expr) -> Expr {
        let span = l.span.to(r.span);
        Expr { kind: ExprKind::Binary(op, op_span, Box::new(l), Box::new(r)), span }
    }

    fn equality(&mut self) -> Result<Expr, Diag> {
        let mark = self.chain_begin();
        let mut e = self.compare()?;
        let mut h = self.operand_height();
        let mut linked = false;
        loop {
            let op = match self.peek() {
                Tok::Op(Op::EqEq) => BinOp::Eq,
                Tok::Op(Op::Ne) => BinOp::Ne,
                _ => break,
            };
            if linked {
                return Err(Diag::new("comparisons cannot be chained; write a == b && b == c", self.span()));
            }
            linked = true;
            self.chain_link(&mark, h)?;
            let s = self.bump().span;
            let r = self.compare()?;
            h = self.after_link(h);
            e = self.binary(op, s, e, r);
        }
        self.chain_end(mark, h);
        Ok(e)
    }

    /// The comparison operator at the cursor, if any (`not in` takes two tokens).
    fn compare_op(&self) -> Option<(BinOp, usize)> {
        Some(match self.peek() {
            Tok::Op(Op::Lt) => (BinOp::Lt, 1),
            Tok::Op(Op::Gt) => (BinOp::Gt, 1),
            Tok::Op(Op::Le) => (BinOp::Le, 1),
            Tok::Op(Op::Ge) => (BinOp::Ge, 1),
            Tok::Kw(Kw::In) => (BinOp::In, 1),
            Tok::Kw(Kw::Not) if matches!(self.peek_at(1), Tok::Kw(Kw::In)) => (BinOp::NotIn, 2),
            _ => return None,
        })
    }

    fn compare(&mut self) -> Result<Expr, Diag> {
        let l = self.additive()?;
        if self.at_kw(Kw::Not) && !matches!(self.peek_at(1), Tok::Kw(Kw::In)) {
            return Err(Diag::new("not is only used in 'not in'; negate with !x", self.span()));
        }
        let Some((op, n)) = self.compare_op() else { return Ok(l) };
        let s = self.span();
        for _ in 0..n {
            self.bump();
        }
        let r = self.additive()?;
        if self.compare_op().is_some() {
            return Err(Diag::new("comparisons cannot be chained; write a < b && b < c", self.span()));
        }
        Ok(self.binary(op, s, l, r))
    }

    fn additive(&mut self) -> Result<Expr, Diag> {
        let mark = self.chain_begin();
        let mut e = self.term()?;
        let mut h = self.operand_height();
        loop {
            let op = match self.peek() {
                Tok::Op(Op::Plus) => BinOp::Add,
                Tok::Op(Op::Minus) => BinOp::Sub,
                _ => break,
            };
            self.chain_link(&mark, h)?;
            let s = self.bump().span;
            let r = self.term()?;
            h = self.after_link(h);
            e = self.binary(op, s, e, r);
        }
        self.chain_end(mark, h);
        Ok(e)
    }

    fn term(&mut self) -> Result<Expr, Diag> {
        let mark = self.chain_begin();
        let mut e = self.unary()?;
        let mut h = self.operand_height();
        loop {
            let op = match self.peek() {
                Tok::Op(Op::Star) => BinOp::Mul,
                Tok::Op(Op::Slash) => BinOp::Div,
                Tok::Op(Op::Percent) => BinOp::Rem,
                _ => break,
            };
            self.chain_link(&mark, h)?;
            let s = self.bump().span;
            let r = self.unary()?;
            h = self.after_link(h);
            e = self.binary(op, s, e, r);
        }
        self.chain_end(mark, h);
        Ok(e)
    }

    fn unary(&mut self) -> Result<Expr, Diag> {
        let op = match self.peek() {
            Tok::Op(Op::Minus) => UnOp::Neg,
            Tok::Op(Op::Bang) => UnOp::Not,
            Tok::Kw(Kw::Not) if matches!(self.peek_at(1), Tok::Kw(Kw::In)) => {
                let d = Diag::new("expected an expression, found keyword `not`", self.span());
                let line_start = self.pos > 0 && matches!(self.toks[self.pos - 1].tok, Tok::Newline);
                return Err(if line_start { d.hint("end the previous line with the operator") } else { d });
            }
            Tok::Kw(Kw::Not) => return Err(Diag::new("not is only used in 'not in'; negate with !x", self.span())),
            _ => return self.power(),
        };
        let s = self.bump().span;
        // -9223372036854775808 is i64::MIN, valid only as the whole operand of `-` (3.4).
        if op == UnOp::Neg && matches!(self.peek(), Tok::Int(n) if *n == 1u64 << 63) {
            let t = self.bump();
            let continued = matches!(self.peek(), Tok::Op(Op::StarStar | Op::LParen | Op::LBracket | Op::Dot | Op::QDot));
            if continued {
                return Err(Diag::new("integer literal does not fit in 64 bits", t.span));
            }
            return Ok(Expr { kind: ExprKind::Int(i64::MIN), span: s.to(t.span) });
        }
        self.deeper()?;
        let operand = self.unary()?;
        self.depth -= 1;
        let span = s.to(operand.span);
        Ok(Expr { kind: ExprKind::Unary(op, s, Box::new(operand)), span })
    }

    fn power(&mut self) -> Result<Expr, Diag> {
        let base = self.postfix()?;
        if !self.at_op(Op::StarStar) {
            return Ok(base);
        }
        let s = self.bump().span;
        self.deeper()?;
        let exp = self.unary()?;
        self.depth -= 1;
        Ok(self.binary(BinOp::Pow, s, base, exp))
    }

    fn postfix(&mut self) -> Result<Expr, Diag> {
        let mark = self.chain_begin();
        let mut e = self.primary()?;
        let mut h = self.operand_height();
        let mut optional = false;
        loop {
            if matches!(self.peek(), Tok::Op(Op::LParen | Op::LBracket | Op::Dot | Op::QDot)) {
                self.chain_link(&mark, h)?;
            }
            match self.peek() {
                Tok::Op(Op::LParen) => {
                    let paren = self.bump().span;
                    let args = self.args()?;
                    let span = e.span.to(self.prev_span());
                    e = Expr { kind: ExprKind::Call { callee: Box::new(e), args, paren }, span };
                }
                Tok::Op(Op::LBracket) => {
                    let bracket = self.bump().span;
                    e = self.subscript(e, bracket)?;
                }
                Tok::Op(op @ (Op::Dot | Op::QDot)) => {
                    let is_opt = *op == Op::QDot;
                    self.bump();
                    let name = self.expect_name("a member name after `.`")?;
                    optional |= is_opt;
                    let span = e.span.to(name.span);
                    e = Expr { kind: ExprKind::Member { obj: Box::new(e), name, optional: is_opt }, span };
                }
                _ => break,
            }
            h = self.after_link(h);
        }
        if optional {
            let span = e.span;
            e = Expr { kind: ExprKind::OptChain(Box::new(e)), span };
        }
        self.chain_end(mark, h);
        Ok(e)
    }

    /// After `[`: an index `expr` or a slice `[lo]:[hi]`, through `]`.
    fn subscript(&mut self, obj: Expr, bracket: Span) -> Result<Expr, Diag> {
        let lo = if self.at_op(Op::Colon) { None } else { Some(Box::new(self.expr()?)) };
        if self.at_op(Op::Colon) {
            self.bump();
            let hi = if self.at_op(Op::RBracket) { None } else { Some(Box::new(self.expr()?)) };
            self.expect_op(Op::RBracket)?;
            let span = obj.span.to(self.prev_span());
            return Ok(Expr { kind: ExprKind::Slice { obj: Box::new(obj), lo, hi, bracket }, span });
        }
        self.expect_op(Op::RBracket)?;
        let span = obj.span.to(self.prev_span());
        let index = lo.expect("an index expression");
        Ok(Expr { kind: ExprKind::Index { obj: Box::new(obj), index, bracket }, span })
    }

    /// Arguments after `(`, through the closing `)`: positional and spread args, then named
    /// args (4.1).
    fn args(&mut self) -> Result<Vec<Arg>, Diag> {
        let mut args: Vec<Arg> = Vec::new();
        let mut named_seen: Option<Span> = None;
        loop {
            if self.at_op(Op::RParen) {
                self.bump();
                return Ok(args);
            }
            // A keyword before `:` is a name too (`df.agg(func: "sum")`): nothing else starts so.
            let is_named = matches!(self.peek(), Tok::Ident(_) | Tok::Kw(_)) && matches!(self.peek_at(1), Tok::Op(Op::Colon));
            if is_named {
                let t = self.bump();
                let name = match &t.tok {
                    Tok::Kw(k) => Name::new(k.as_str().into(), t.span),
                    _ => name_of(&t).expect("an identifier"),
                };
                self.bump(); // :
                let value = self.expr()?;
                if let Some(Arg::Named(prev, _)) = args.iter().find(|a| matches!(a, Arg::Named(n, _) if n.text == name.text)) {
                    return Err(Diag::new(format!("argument '{}' is given twice", name.text), name.span).note("first given here", prev.span));
                }
                named_seen.get_or_insert(name.span);
                args.push(Arg::Named(name, value));
            } else {
                let spread = self.at_op(Op::Ellipsis);
                if spread {
                    self.bump();
                }
                let value = self.expr()?;
                if let Some(first) = named_seen {
                    return Err(Diag::new("a positional argument can't follow a named one", value.span).note("the first named argument", first));
                }
                args.push(if spread { Arg::Spread(value) } else { Arg::Pos(value) });
            }
            if self.at_op(Op::Comma) {
                self.bump();
            } else if !self.at_op(Op::RParen) {
                return Err(Diag::new(format!("expected `,` or `)`, found {}", self.peek()), self.span()));
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, Diag> {
        let line_start = self.pos > 0 && matches!(self.toks[self.pos - 1].tok, Tok::Newline);
        let t = self.bump();
        let kind = match t.tok {
            Tok::Int(n) => match i64::try_from(n) {
                Ok(v) => ExprKind::Int(v),
                Err(_) => return Err(Diag::new("integer literal does not fit in 64 bits", t.span)),
            },
            Tok::Float(x) => ExprKind::Float(x),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::Bytes(b) => ExprKind::Bytes(b),
            Tok::FStr(parts) => {
                if let Some(name) = python_placeholder(&parts) {
                    return Err(Diag::new(format!("{{{name}}} in an f-string is Python's placeholder, which V2 doesn't use"), t.span)
                        .hint(format!("write ${{{name}}} for its value, or ${{\"{{{name}}}\"}} for the text {{{name}}}")));
                }
                ExprKind::FStr(self.fstring(parts)?)
            }
            Tok::Ident(n) if &*n == "lambda" && self.lambda_follows() => {
                return Err(Diag::new("V2 has no lambda keyword", t.span).hint("anonymous functions are written func(x) => …"));
            }
            Tok::Ident(n) => ExprKind::Name(n, Loc::Unresolved),
            Tok::Kw(Kw::True) => ExprKind::Bool(true),
            Tok::Kw(Kw::False) => ExprKind::Bool(false),
            Tok::Kw(Kw::Null) => ExprKind::Null,
            Tok::Kw(Kw::Func) => {
                let f = self.func_rest(None, t.span)?;
                let span = f.span;
                return Ok(Expr { kind: ExprKind::Func(Rc::new(f)), span });
            }
            Tok::Op(Op::LParen) => {
                let inner = self.expr()?;
                self.expect_op(Op::RParen)?;
                // The inner expression keeps its own span, so error carets land on the name or
                // operator inside the parentheses (2.2), not on the `(`.
                return Ok(inner);
            }
            Tok::Op(Op::LBracket) => {
                let mut items = Vec::new();
                loop {
                    if self.at_op(Op::RBracket) {
                        self.bump();
                        break;
                    }
                    items.push(self.expr()?);
                    if self.at_op(Op::Comma) {
                        self.bump();
                    } else if !self.at_op(Op::RBracket) {
                        return Err(Diag::new(format!("expected `,` or `]`, found {}", self.peek()), self.span()));
                    }
                }
                ExprKind::List(items)
            }
            Tok::LBrace(BraceKind::Dict) => ExprKind::Dict(self.dict_entries()?),
            // A block is a whole `expr`; as an operand it needs parentheses (4.1, DOCS 4.5).
            Tok::BlockHead { tag, .. } => {
                return Err(Diag::new("a block value must be in parentheses here", t.span)
                    .hint(format!("wrap the block: (@{tag} {{ … }})")))
            }
            other => {
                let d = Diag::new(format!("expected an expression, found {other}"), t.span);
                // A line that starts with a binary-only operator does not continue the previous
                // one (3.3).
                return Err(if line_start && Self::binary_only(&other, self.peek()) {
                    d.hint("end the previous line with the operator")
                } else {
                    d
                });
            }
        };
        Ok(Expr { kind, span: t.span.to(self.prev_span()) })
    }

    /// `{ key: value, … }` after a dict `{`.
    fn dict_entries(&mut self) -> Result<Vec<(DictKey, Expr)>, Diag> {
        let mut entries = Vec::new();
        // The `{` just read: at the start of a statement, JavaScript's habit of a bare block.
        let statement = self.pos < 2 || matches!(self.toks[self.pos - 2].tok, Tok::Newline | Tok::Semi | Tok::RBrace | Tok::LBrace(BraceKind::Body));
        loop {
            if matches!(self.peek(), Tok::RBrace) {
                self.bump();
                return Ok(entries);
            }
            let t = self.bump();
            let key = match t.tok {
                Tok::Ident(n) | Tok::Str(n) => DictKey::Str(n, t.span),
                Tok::Op(Op::LBracket) => {
                    let k = self.expr()?;
                    self.expect_op(Op::RBracket)?;
                    DictKey::Computed(k)
                }
                Tok::Kw(k) if self.at_op(Op::Colon) => DictKey::Str(k.as_str().into(), t.span),
                Tok::Kw(_) if statement && entries.is_empty() => {
                    return Err(Diag::new("V2 has no bare { } blocks: a { that starts a statement is a dict", t.span)
                        .hint("write the statements without the braces, or in a function"))
                }
                Tok::Kw(_) => return Err(Diag::new(format!("expected a dict key, found {}", t.tok), t.span)),
                // Python's `{1: "x"}`: V2's keys are strings.
                Tok::Int(n) => {
                    return Err(Diag::new("dict keys are names, strings or [expr]", t.span).hint(format!("dict keys are strings: write \"{n}\": or [str({n})]:")))
                }
                // JavaScript's `{...a, ...b}`, Python's `{**a, **b}`: a merge.
                Tok::Op(Op::Ellipsis | Op::StarStar) => return Err(Diag::new("dict keys are names, strings or [expr]", t.span).hint(MERGE_HINT)),
                _ => return Err(Diag::new("dict keys are names, strings or [expr]", t.span)),
            };
            if statement && entries.is_empty() && !self.at_op(Op::Colon) {
                return Err(Diag::new("V2 has no bare { } blocks: a { that starts a statement is a dict", self.span())
                    .hint("write the statements without the braces, or in a function"));
            }
            self.expect_op(Op::Colon)?;
            entries.push((key, self.expr()?));
            if self.at_op(Op::Comma) {
                self.bump();
            } else if !matches!(self.peek(), Tok::RBrace) {
                return Err(Diag::new(format!("expected `,` or `}}`, found {}", self.peek()), self.span()));
            }
        }
    }

    /// Each `${…}` holds exactly one expression (3.5).
    fn fstring(&mut self, parts: Vec<FPartTok>) -> Result<Vec<FPart>, Diag> {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                FPartTok::Lit(s) => out.push(FPart::Lit(s)),
                FPartTok::Expr { mut toks, fixed, span } => {
                    // The closing `}` of `${…}` ends the token list (a lone `}` can't appear
                    // inside it: the lexer balanced the braces).
                    let close = self.sm.span(span.file, span.hi - 1, span.hi);
                    toks.push(Token { tok: Tok::RBrace, span: close });
                    let mut sub =
                        Parser { sm: self.sm, toks, pos: 0, depth: self.depth, links: self.links, peak: self.links };
                    let expr = sub.expr()?;
                    self.peak = self.peak.max(sub.peak);
                    if matches!(sub.peek(), Tok::Op(Op::Colon)) {
                        // The hint fits the format as written: from the `:` to the closing `}`.
                        let colon = sub.span();
                        let text = &self.sm.file(span.file).text;
                        let spec = text.get(colon.hi as usize..close.lo as usize).unwrap_or_default();
                        let written = text.get(expr.span.lo as usize..expr.span.hi as usize).unwrap_or_default();
                        return Err(Diag::new(ONLY_FIXED, colon).hint(format_hint(spec, written)));
                    }
                    if !matches!(sub.peek(), Tok::RBrace) || sub.pos + 1 != sub.toks.len() {
                        let d = Diag::new(format!("expected `}}` to close the ${{…}}, found {}", sub.peek()), sub.span());
                        // `${3,5:.2f}`: a decimal comma.
                        return Err(match decimal_comma(&sub.toks, sub.pos, self.sm) {
                            Some(h) => d.hint(h),
                            None => d,
                        });
                    }
                    out.push(FPart::Expr { expr, fixed });
                }
            }
        }
        Ok(out)
    }
}

/// Convenience for tests holding source text.
#[cfg(test)]
pub fn parse_str(src: &str) -> Result<Vec<Stmt>, String> {
    let mut sm = SourceMap::new();
    let id = sm.add(std::path::PathBuf::from("t.v2"), "t.v2".into(), src);
    let toks = super::lexer::lex(&sm, id).map_err(|d| d.message)?;
    let mut program = parse_program(&sm, toks).map_err(|d| d.message)?;
    super::resolve::resolve(&mut program);
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(src: &str) -> String {
        parse_str(src).unwrap_err()
    }

    fn expr_of(src: &str) -> Expr {
        let mut prog = parse_str(src).unwrap();
        match prog.remove(0).kind {
            StmtKind::Expr(e) => e,
            other => panic!("not an expression statement: {other:?}"),
        }
    }

    /// A compact rendering of an expression's structure, for precedence tests.
    fn show(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(n) => n.to_string(),
            ExprKind::Name(n, _) => n.to_string(),
            ExprKind::Str(s) => format!("{s:?}"),
            ExprKind::Null => "null".into(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Unary(op, _, x) => format!("({} {})", if *op == UnOp::Neg { "-" } else { "!" }, show(x)),
            ExprKind::Binary(op, _, a, b) => format!("({op:?} {} {})", show(a), show(b)),
            ExprKind::And(a, b) => format!("(&& {} {})", show(a), show(b)),
            ExprKind::Or(a, b) => format!("(|| {} {})", show(a), show(b)),
            ExprKind::Coalesce(a, b) => format!("(?? {} {})", show(a), show(b)),
            ExprKind::Ternary(c, a, b) => format!("(? {} {} {})", show(c), show(a), show(b)),
            ExprKind::Call { callee, args, .. } => format!("{}({})", show(callee), args.len()),
            ExprKind::Member { obj, name, optional } => format!("{}{}{}", show(obj), if *optional { "?." } else { "." }, name.text),
            ExprKind::OptChain(x) => format!("opt[{}]", show(x)),
            ExprKind::Index { obj, index, .. } => format!("{}[{}]", show(obj), show(index)),
            ExprKind::Slice { obj, lo, hi, .. } => format!(
                "{}[{}:{}]",
                show(obj),
                lo.as_ref().map(|x| show(x)).unwrap_or_default(),
                hi.as_ref().map(|x| show(x)).unwrap_or_default()
            ),
            ExprKind::List(xs) => format!("[{}]", xs.iter().map(show).collect::<Vec<_>>().join(", ")),
            ExprKind::Dict(es) => format!("{{{}}}", es.len()),
            ExprKind::Func(_) => "func".into(),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn precedence_table() {
        assert_eq!(show(&expr_of("a ? b : c ?? d || e && f == g < h + i * -j ** k")), "(? a b (?? c (|| d (&& e (Eq f (Lt g (Add h (Mul i (- (Pow j k))))))))))");
        assert_eq!(show(&expr_of("-2 ** 2")), "(- (Pow 2 2))");
        assert_eq!(show(&expr_of("2 ** -1")), "(Pow 2 (- 1))");
        assert_eq!(show(&expr_of("2 ** 3 ** 2")), "(Pow 2 (Pow 3 2))");
        assert_eq!(show(&expr_of("a - b - c")), "(Sub (Sub a b) c)");
        assert_eq!(show(&expr_of("x not in xs")), "(NotIn x xs)");
        assert_eq!(show(&expr_of("!a && b")), "(&& (! a) b)");
        assert_eq!(show(&expr_of("-9223372036854775808")), "-9223372036854775808");
    }

    #[test]
    fn postfix_and_optional_chains() {
        assert_eq!(show(&expr_of("a.b.c(1)[2]")), "a.b.c(1)[2]");
        assert_eq!(show(&expr_of("a?.b.c(f())")), "opt[a?.b.c(1)]");
        assert_eq!(show(&expr_of("(a?.b).c")), "opt[a?.b].c");
        assert_eq!(show(&expr_of("s[1:3]")), "s[1:3]");
        assert_eq!(show(&expr_of("s[:2]")), "s[:2]");
        assert_eq!(show(&expr_of("s[1:]")), "s[1:]");
        assert_eq!(show(&expr_of("h.from")), "h.from");
    }

    #[test]
    fn literals_lambdas_and_calls() {
        assert_eq!(show(&expr_of("[1, 2, 3,]")), "[1, 2, 3]");
        assert_eq!(show(&expr_of("{name: \"Ada\", \"x y\": 1, [k]: 2}")), "{3}");
        assert_eq!(show(&expr_of("func(x) => x * 2")), "func");
        assert!(parse_str("let f = func(s) {\n  return s + \"!\"\n}").is_ok());
        let e = expr_of("f(1, ...xs, sep: \"-\", end: \"\")");
        let ExprKind::Call { args, .. } = e.kind else { panic!() };
        assert!(matches!(args[1], Arg::Spread(_)) && matches!(args[3], Arg::Named(..)));
    }

    #[test]
    fn statements() {
        let src = r#"let a = 1
const B = 2
let c
a += 1
d.x = 3
e[0] = 4
if a > 1 {
    print("big")
} elif a == 1 {
    print("one")
}
else {
    print("small")
}
while false { break }
for i in 0..10 { continue }
for k, v in {x: 1} { print(k, v) }
func f(x, y = 2, ...rest) {
    return x
}
try {
    throw "e"
}
catch (e) {
} finally {
}
import "./util.v2" as util
@import { mean, "odd-name" as odd } from py.statistics
@import py.numpy as np
@import { mean as m2 } from @py.stats
extern "m" as m { double cos(double x); }
"#;
        let prog = parse_str(src).unwrap();
        assert_eq!(prog.len(), 17);
        assert!(matches!(prog[6].kind, StmtKind::If { ref arms, else_: Some(_) } if arms.len() == 2));
        assert!(matches!(prog[8].kind, StmtKind::For { iter: ForIter::Range(..), .. }));
        let StmtKind::Func(f) = &prog[10].kind else { panic!() };
        assert_eq!((f.params.len(), f.rest.as_ref().map(|r| &*r.text)), (2, Some("rest")));
        let StmtKind::Extern(x) = &prog[16].kind else { panic!() };
        assert_eq!((x.decls.len(), x.alias.as_ref().map(|a| &*a.text)), (1, Some("m")));
    }

    #[test]
    fn syntax_errors_with_4_5_messages() {
        assert_eq!(err("a < b < c"), "comparisons cannot be chained; write a < b && b < c");
        assert_eq!(err("not x"), "not is only used in 'not in'; negate with !x");
        assert_eq!(err("f() = 1"), "cannot assign to this expression");
        assert_eq!(err("a?.b = 1"), "cannot assign through ?.");
        assert_eq!(err("let d = {1: 2}"), "dict keys are names, strings or [expr]");
        assert_eq!(err("let d = {true 2}"), "expected a dict key, found keyword `true`");
        assert_eq!(err("try { }"), "try needs a catch or a finally");
        assert_eq!(err("let r = 0..3"), "a..b is only allowed in a for head; use range(a, b)");
        assert_eq!(err("for i, x in 0..3 { }"), "a range loop takes one variable");
        assert_eq!(err("for (x in xs) { }"), "a for head takes no parentheses");
        assert_eq!(err("func f(a, a) { }"), "duplicate parameter 'a'");
        assert_eq!(err("func f(a = 1, b) { }"), "a parameter without a default can't follow one with a default");
        assert_eq!(err("func f(...r, b) { }"), "...rest must be the last parameter");
        assert_eq!(err("f(a: 1, 2)"), "a positional argument can't follow a named one");
        assert_eq!(err("f(a: 1, a: 2)"), "argument 'a' is given twice");
        assert_eq!(err("if x\n{\n}"), "expected {");
        assert_eq!(err("@export { f }"), "@export is only valid inside an embedded block");
        assert_eq!(err("@import { \"from\" } from py.x"), "quoted name 'from' needs an alias: \"from\" as NAME");
        assert_eq!(err("@import { from } from py.x"), "expected a name in the @import list, found keyword `from`");
        assert_eq!(err("@import stats as s"), "only a package selector (py.name, js.name) can be imported as a whole module");
        assert_eq!(err("extern \"c\" { int printf(const char* f, ...); }"), "variadic functions (like printf) are not supported — wrap the call in a @c block");
        assert_eq!(err("@c fast {\n  @export { \"int f(...)\" }\n}"), "compiled blocks export functions by name");
        assert_eq!(err("@import { f } from \"odd name\""), "a block label can't be a string");
        assert_eq!(err("@import { f } from @py.\"odd name\""), "a block label can't be a string");
        assert_eq!(diag("@import { f } from \"odd name\"").hint.as_deref(), Some(LABEL_HINT));
        assert_eq!(err("let x = -9223372036854775808 ** 1"), "integer literal does not fit in 64 bits");
        assert_eq!(err("} else if x {"), "unmatched `}`");
        assert_eq!(err("if a {\n} else if b {\n}"), "V2 writes `else if` as `elif`");
        assert_eq!(err("print(1)\n+ 2"), "expected an expression, found `+`");
    }

    #[test]
    fn blocks() {
        let prog = parse_str("@py stats {\n    def f(): pass\n    @export { f }\n}\nprint(@py(n, k: 3) { n * k })").unwrap();
        let StmtKind::Block(b) = &prog[0].kind else { panic!("statement block") };
        assert_eq!(&*b.tag.text, "py");
        assert_eq!(b.label.as_ref().unwrap().text.as_ref(), "stats");
        assert!(b.directives.exports.is_some());
        let StmtKind::Expr(Expr { kind: ExprKind::Call { args, .. }, .. }) = &prog[1].kind else { panic!() };
        let Arg::Pos(Expr { kind: ExprKind::Block(v), .. }) = &args[0] else { panic!("block value") };
        assert_eq!(v.args.len(), 2);
        assert!(matches!(&v.args[0].1.kind, ExprKind::Name(n, _) if &**n == "n"));
    }

    #[test]
    fn block_static_checks() {
        assert_eq!(
            err("print(@py {\n  def f(): pass\n  @export { f }\n})"),
            "a block with @export is a definition block and cannot be used as a value"
        );
        assert_eq!(err("@c(x) { int y; }"), "compiled blocks take no args — @export a function and call it with arguments");
        assert_eq!(err("@py(a, a: 1) { a }"), "block arg 'a' is given twice");
        assert_eq!(err("@py(m) {\n  @import { mean as m } from py.statistics\n}"), "'m' is already declared in this block");
        assert_eq!(err("@py(np) {\n  @import py.numpy as np\n}"), "the whole-module form of @import is only valid in V2 code");
        assert_eq!(err("@py { 1 }\n.x"), "a block value must be in parentheses here: (@py { … }).m()");
    }

    /// The whole diagnostic, for tests that check the hint or the position.
    fn diag(src: &str) -> Diag {
        let mut sm = SourceMap::new();
        let id = sm.add(std::path::PathBuf::from("t.v2"), "t.v2".into(), src);
        let toks = super::super::lexer::lex(&sm, id).unwrap();
        parse_program(&sm, toks).unwrap_err()
    }

    #[test]
    fn a_decimal_comma_gets_the_number_with_a_point() {
        // 2.4 (round 20): `19,99` with no spaces where one expression ends, in the user's number.
        let hint = |src: &str| diag(src).hint;
        let point = |n: &str| Some(format!("decimal numbers use a point: write {}, not {n}", n.replace(',', ".")));
        assert_eq!(hint("let preis = 19,99"), point("19,99"));
        assert_eq!(hint("let x = 1\nx = 2,5"), point("2,5"));
        assert_eq!(hint("let x = 1\nif x > 2,5 { print(1) }"), point("2,5"));
        assert_eq!(hint("print(f\"${3,5:.2f}\")"), point("3,5"));
        assert_eq!(hint("print(f\"${1_000,5}\")"), point("1_000,5"));
        assert_eq!(hint("func f() {\n    return 2,5\n}"), point("2,5"));
        // Round 20: three digits after the `,` may be thousands, so both readings; more groups of
        // three are (`1,000` and `1,000,000` stop at the lexer's leading zeros first).
        let thousands = |n: &str, rest: &str| Some(format!("numbers are written without thousands commas: {} or {}{rest}", n.replace(',', ""), n.replace(',', "_")));
        assert_eq!(hint("let x = 1,500"), thousands("1,500", " (a decimal one has a point: 1.5)"));
        assert_eq!(hint("let x = 12,345"), thousands("12,345", " (a decimal one has a point: 12.345)"));
        assert_eq!(hint("print(f\"${1,500}\")"), thousands("1,500", " (a decimal one has a point: 1.5)"));
        assert_eq!(hint("let n = 1,500,250"), thousands("1,500,250", ""));
        assert_eq!(hint("let x = 1234,567"), point("1234,567"));
        assert_eq!(hint("let x = 0,500"), point("0,500"));
        assert_eq!(hint("let x = 1,5,2"), Some("V2 declares one name per let: let a = 1, then let b = 2 (or one list, let ab = [1, 2])".into()));
        // A float literal ends a run, thousands only; another run with one gets no number hint.
        assert_eq!(hint("let total = 1,234,567.89"), thousands("1,234,567.89", ""));
        assert_eq!(hint("let x = 1,234.5"), thousands("1,234.5", ""));
        for no_number in ["let x = 19,99.5", "let x = 1,234.5,678", "let x = 1.5,234"] {
            assert_eq!(hint(no_number), Some("V2 declares one name per let: let a = 1, then let b = 2 (or one list, let ab = [1, 2])".into()), "{no_number}");
        }
        // With a space it is a list of values, and a tuple keeps its own hint.
        assert_eq!(hint("let x = 1\nif x > 2, 5 { print(1) }"), None);
        assert_eq!(hint("let p = (2,5)").as_deref(), Some("V2 has no tuples: write a list, as in [1, 2]"));
        assert!(parse_str("print(round(2,5))").is_ok());
    }

    #[test]
    fn python_and_javascript_habits_get_hints() {
        // 2.4: what V2 writes instead, from the tokens where the parser stopped.
        let hint = |src: &str| diag(src).hint.unwrap_or_else(|| panic!("no hint for {src:?}"));
        assert_eq!(hint("def add(a, b):\n    return a"), "V2 writes functions as func add(a, b) { ... }");
        assert_eq!(hint("function add(a, b) {\n}"), "V2 writes functions as func add(a, b) { ... }");
        assert_eq!(hint("fn add(a) {\n}"), "V2 writes functions as func add(a, b) { ... }");
        assert_eq!(hint("var x = 1"), "V2 declares variables with let: let x = 1");
        assert_eq!(hint("print(a and b)"), "V2 writes and, or and not as &&, || and !");
        assert_eq!(hint("if a or b {\n}"), "V2 writes and, or and not as &&, || and !");
        assert_eq!(hint("if x is null {\n}"), "V2 compares with == and !=, as in x == null");
        assert_eq!(hint("let f = x => x * 2"), "V2 writes an anonymous function as func(x) => x * 2");
        assert_eq!(hint("i++"), "V2 has no ++: write x += 1");
        assert_eq!(hint("i--\n"), "V2 has no --: write x -= 1");
        assert_eq!(hint("print(1 === 1)"), "V2 compares with == (there is no ===)");
        assert_eq!(hint("print(1 !== 2)"), "V2 compares with != (there is no !==)");
        assert_eq!(hint("for i in 1..=3 {\n}"), "a..b leaves out b: for 1, 2 and 3 write 1..4");
        assert_eq!(hint("dumps(d, indent=2)"), "V2 writes a named argument as name: value, as in f(x, indent: 2)");
        assert_eq!(hint("try {\n} catch e {\n}"), "V2 writes catch (e) { ... }");
        // Round 21 (L2): a typed catch, Python's unpacking loops and *args.
        let any = "V2 catches every error: catch (e) { … }, then test e.type";
        assert_eq!(diag("try {\n} catch (e: Error) {\n}").message, "expected `)`, found `:`");
        assert_eq!(hint("try {\n} catch (e: Error) {\n}"), any);
        assert_eq!(hint("try {\n} catch (Exception e) {\n}"), any);
        let names = "a for loop takes one name, or two (index or key, then item): for e in walk(d) { let root = e[0] … }, or loop in a @py block";
        assert_eq!(diag("for root, dirs, files in walk(\".\") {\n}").message, "expected `in`, found `,`");
        for src in ["for root, dirs, files in walk(\".\") {\n}", "for i, (a, b) in [[1, 2]] {\n}", "for i, [a, b] in xs {\n}", "for [a, b] in xs {\n}", "for (a, b) in xs {\n}"] {
            assert_eq!(hint(src), names, "{src}");
        }
        assert_eq!(hint("for (let i = 0; i < 3; i++) {\n}"), "for x in xs { … }");
        assert_eq!(diag("func f(*args) {\n}").message, "expected a parameter name, found `*`");
        assert_eq!(hint("func f(*args) {\n}"), "V2 collects extra arguments with ...args");
        assert_eq!(hint("let f = func(a, *rest) => a"), "V2 collects extra arguments with ...rest");
        // Python's keyword-only `*` alone isn't a rest parameter (review of round 21).
        assert_eq!(diag("func f(a, *, b) {\n}").hint, None);
        assert_eq!(hint("if x > 1:\n    print(x)"), "V2 puts a block in { }, not after a colon: if x > 1 { ... }");
        assert_eq!(hint("let f = function(x) {\n    return x\n}"), "V2 writes an anonymous function as func(x) { ... }");
        // Python's f-string braces, and the import forms.
        let d = diag("let n = 1\nprint(f\"hello {n}\")");
        assert_eq!((d.message.as_str(), d.hint.as_deref()), ("{n} in an f-string is Python's placeholder, which V2 doesn't use", Some("write ${n} for its value, or ${\"{n}\"} for the text {n}")));
        assert!(parse_str("print(f\"{{n}} and \\${n} and ${1}\")").is_ok(), "doubled and escaped braces are text");
        let d = diag("@import math from py.math");
        assert_eq!(d.message, "expected `{` before the names to import");
        assert_eq!(d.hint.as_deref(), Some("write @import { math } from py.math, or @import py.math as math for the whole module"));
        assert_eq!(
            hint("@import slugify from js.\"@sindresorhus/slugify\""),
            "write @import { slugify } from js.\"@sindresorhus/slugify\" (a default export: @import { \"default\" as slugify } from js.\"@sindresorhus/slugify\"), or @import js.\"@sindresorhus/slugify\" as slugify for the whole module"
        );
        assert_eq!(hint("let f = lambda x: x * 2"), "V2 writes an anonymous function as func(x) => x * 2");
        assert_eq!(hint("raise ValueError(\"bad\")"), "V2 throws an error with throw error(\"ValueError\", \"bad\")");
        assert_eq!(hint("throw new Error(\"bad\")"), "V2 has no new: throw error(\"Error\", \"bad\"), or throw \"bad\" for a plain Error");
        assert_eq!(hint("let d = new Date()"), "V2 has no new: call it, as in Date(…)");
        let step = "V2 slices have no step: xs[a:b] takes the items from a up to b (a @py block has xs[::2])";
        assert_eq!(hint("print(\"abcdef\"[::2])"), step);
        assert_eq!(hint("let xs = [1]\nprint(xs[1:5:2])"), step);
        assert_eq!(hint("let a = [1]\nlet b = [0, ...a]"), "... spreads a list into a call's arguments only: join lists with +, as in [0] + a");
        assert_eq!(hint("let f = func() => {1: \"x\"}"), "dict keys are strings: write \"1\": or [str(1)]:");
        assert_eq!(hint("let a = null\na ??= 5"), "V2 has no ??=: write a = a ?? …");
        assert_eq!(hint("let x = 1\nif x > 2 {\n} elsif x > 0 {\n}"), "V2 writes elif: } elif x > 0 { ... }");
        assert_eq!(hint("x := 1"), "V2 declares a name with let x = …, and assigns to it with x = …");
        assert_eq!(hint("@import * from py.math"), "V2 has no import *: list the names, @import { sqrt, pi } from py.math, or import the module, @import py.math as math");
        assert_eq!(hint("let y = await foo()"), "V2 has no await: when a call returns a Promise or a coroutine, V2 waits for its result, so write foo(…)");
        // A body on the header's line, without braces; `and` keeps its own hint.
        assert_eq!(hint("let x = 1
if (x == 1) print(x)"), "the body goes in braces, even for one statement: if … { ... }");
        assert_eq!(hint("for x in [1] print(x)"), "the body goes in braces, even for one statement: for … { ... }");
        assert_eq!(hint("func f() {
    while true break
}"), "the body goes in braces, even for one statement: while … { ... }");
        assert_eq!(hint("let x = 1
if x and x {
}"), "V2 writes and, or and not as &&, || and !");
        assert_eq!(hint("let if = 1"), "if is one of V2's own words: pick another name, as in if_");
        assert_eq!(hint("Let x = 1"), "V2's own words are written in small letters: let");
        assert_eq!(hint("let x = 1\nIf x > 0 {\n}"), "V2's own words are written in small letters: if");
        assert_eq!(hint("if true {\n} Else {\n}"), "V2's own words are written in small letters: else");
        assert_eq!(hint("let p = (1, 2)"), "V2 has no tuples: write a list, as in [1, 2]");
        // A comma in an index, also after a call that is closed (`idxmax()`).
        let index = "an index takes one key: write x[a][b], or, for a pandas or numpy handle, do it in a block, as in @py(df) { df.loc[r, \"col\"] }";
        assert_eq!(hint("let df = 1\nprint(df.loc[df.idxmax(), \"x\"])"), index);
        assert_eq!(hint("let a = [1]\nprint(a[0, 1])"), index);
        assert_eq!(hint("let xs = [1]\nlet ys = [x * 2 for x in xs]"), "V2 has no comprehensions: write xs.map(func(x) => x * 2), or xs.filter(func(x) => x > 0)");
        assert_eq!(hint("let a, b = 1, 2"), "V2 declares one name per let: let a = 1, then let b = 2 (or one list, let ab = [1, 2])");
        assert_eq!(hint("func f() {\n    return 1, 2\n}"), "V2 returns one value: return a list, as in return [a, b]");
        assert_eq!(hint("{\n    print(\"x\")\n}"), "write the statements without the braces, or in a function");
        // One that starts a statement after `=>` is a block body (a keyword before `:` is a key).
        assert_eq!(hint("let f = func(x) => { return x }"), "`=>` takes one expression, so `{` starts a dict; for statements write func(…) { … }");
        assert_eq!(hint("let x = 1\nif x = 2 {\n    print(x)\n}"), "V2 compares with ==, as in if x == 2 { ... }");
        assert_eq!(hint("let x = 1\nwhile x = 1 {\n}"), "V2 compares with ==, as in if x == 2 { ... }");
        assert_eq!(hint("{\n    let a = 1\n}"), "write the statements without the braces, or in a function");
        let class = "V2 has no classes: keep the data in a dict and write functions for the rest, or write the class in a @py or @js block";
        assert_eq!(hint("class Point {\n}"), class);
        assert_eq!(hint("class Point:\n    pass"), class);
        let switch = "V2 has no switch or match: write if x == 1 { ... } elif x == 2 { ... } else { ... }";
        for src in ["switch x {\n}", "let x = 1\nswitch (x) {\n}", "match x:\n    case 1: pass"] {
            assert_eq!(hint(src), switch, "{src}");
        }
        let args = "block args are `name` or `name: expr`, as in @py(n: len(xs))";
        assert_eq!(hint("let data = [1]\n@py(data.copy()) { 1 }"), args);
        assert_eq!(hint("@py(1 + 2) { 1 }"), args);
        assert_eq!(hint("let x = 2\nprint(f\"{x:.2f}\")"), "write ${x:.2f} for its value, or ${\"{x:.2f}\"} for the text {x:.2f}");
        assert_eq!(hint("let s = \"big\" if x > 2 else \"small\""), "V2 writes a conditional value as cond ? a : b, as in x > 2 ? \"big\" : \"small\"");
        assert_eq!(hint("try {\n} except (e) {\n}"), "V2 writes except as catch: try { ... } catch (e) { ... }");
        assert_eq!(hint("try {\n}\nexcept (e) {\n}"), "V2 writes except as catch: try { ... } catch (e) { ... }");
        assert_eq!(hint("func by_len(s) => len(s)"), "a named function has a { } body: func by_len(s) { return … }, or write let by_len = func(s) => …");
        assert_eq!(hint("print \"hi\""), "print is a function in V2: print(\"hello\")");
        assert_eq!(hint("import numpy as np"), "a Python module is imported with @import py.numpy as np; another .v2 file with import \"./util.v2\" as util");
        assert_eq!(hint("import os.path"), "a Python module is imported with @import py.os.path as path; another .v2 file with import \"./util.v2\" as util");
        // JavaScript's import forms.
        assert_eq!(hint("import dayjs from \"dayjs\""), "V2 imports a JavaScript default export with @import { \"default\" as dayjs } from js.dayjs");
        assert_eq!(hint("import x from \"./x.js\""), "V2 imports a JavaScript default export with @import { \"default\" as x } from js.\"./x.js\"");
        assert_eq!(hint("import { chunk, zip as z } from \"lodash\""), "V2 imports names with @import { chunk, zip as z } from js.lodash");
        assert_eq!(hint("import * as fs from \"node:fs\""), "V2 imports a whole package with @import js.\"node:fs\" as fs");
        // Another .v2 file, in each form.
        assert_eq!(hint("import { bump } from \"./lib/side.v2\""), "another .v2 file is imported with import \"./lib/side.v2\" as side, then side.bump(…)");
        assert_eq!(hint("import util from \"./util.v2\""), "another .v2 file is imported with import \"./util.v2\" as util");
        assert_eq!(hint("import * as u from \"./util.v2\""), "another .v2 file is imported with import \"./util.v2\" as u");
        assert_eq!(hint("@import { bump } from \"./lib/side.v2\""), "another .v2 file is imported with import \"./lib/side.v2\" as side (a statement of its own, not @import)");
        assert_eq!(hint("@import { f } from js.\"./util.v2\""), "another .v2 file is imported with import \"./util.v2\" as util (a statement of its own, not @import)");
        assert_eq!(hint("from statistics import mean"), "V2 imports names with @import { mean } from py.statistics");
        assert_eq!(hint("from os.path import join, exists as e"), "V2 imports names with @import { join, exists as e } from py.os.path");
        // The selector as written: a label, a tag, JavaScript's quoted package, a dashed name.
        assert_eq!(hint("@import mean from stats"), "write @import { mean } from stats");
        assert_eq!(hint("@import mean from @py.stats // the block"), "write @import { mean } from @py.stats");
        assert_eq!(
            hint("@import dayjs from \"dayjs\""),
            "write @import { dayjs } from js.dayjs (a default export: @import { \"default\" as dayjs } from js.dayjs), or @import js.dayjs as dayjs for the whole module"
        );
        assert!(hint("@import format from js.date-fns").starts_with("write @import { format } from js.\"date-fns\" (a default export"));
        assert_eq!(hint("@import { f } from \"py.statistics\""), "a package is written py.statistics, without quotes");
        assert_eq!(hint("@import { f } from \"js.date-fns\""), "a package is written js.\"date-fns\": the quotes go around the name only");
        assert_eq!(hint("@import { format } from js.date-fns"), "a package name with - goes in quotes: js.\"date-fns\"");
        assert_eq!(hint("@import { x } from js.left-pad-x\n"), "a package name with - goes in quotes: js.\"left-pad-x\"");
        for (src, name) in [("js.utf-8-validate", "utf-8-validate"), ("js.is-in-browser", "is-in-browser"), ("js.deep-for-each", "deep-for-each"), ("js.lodash-4", "lodash-4")] {
            assert_eq!(hint(&format!("@import {{ f }} from {src}")), format!("a package name with - goes in quotes: js.\"{name}\""), "{src}");
        }
        assert_eq!(hint("@import js.date-fns as dfns"), "a package name with - goes in quotes: @import js.\"date-fns\" as dfns");
        assert_eq!(hint("@import js.date-fns"), "a package name with - goes in quotes: @import js.\"date-fns\" as date_fns");
        assert_eq!(hint("func greet(name, greeting: \"hello\") {\n}"), "V2 has no type annotations; a default is written with =, as in func f(x, key = 1)");
        assert_eq!(diag("@import py.os.path").hint.as_deref(), Some("@import py.os.path as path"));
        // An error with nothing of the kind keeps no hint.
        assert_eq!(diag("let = 1").hint, None);
    }

    #[test]
    fn named_argument_spreads_and_dict_merges_get_v2s_ways() {
        // 2.4 habits, 4.5, round 17: Python's f(**opts), JavaScript's {...a} and Python's {**a}.
        let hint = |src: &str| diag(src).hint.unwrap_or_else(|| panic!("no hint for {src:?}"));
        assert_eq!(hint("print(dumps(d, **opts))"), KWARGS_HINT);
        assert_eq!(hint("f(**opts)"), KWARGS_HINT);
        // Only a call's `(`: after a name, `)`, `]` or a string.
        for src in ["obj.run(1, **opts)", "fs[0](**opts)", "make()(**opts)", "\"x\"(**opts)", "f(g(1), **opts)"] {
            assert_eq!(hint(src), KWARGS_HINT, "{src}");
        }
        // A parameter list's `(` gets the parameter's way; a grouping's, none.
        for src in ["func f(a, **kw) {\n}", "func f(**kw) {\n}", "let g = func(**kw) => 1", "let g = func(a, **kw) { return a }", "func f(a = g(1), **kw) {\n}"] {
            assert_eq!(hint(src), KWARGS_PARAM_HINT, "{src}");
        }
        for src in ["let y = (**x)", "if (**x) {\n}", "print((**x))", "let y = 2 * (**x)"] {
            assert_eq!(diag(src).hint, None, "{src}");
        }
        for src in ["let c = {...a, ...b}", "let c = {**a, **b}", "let c = {x: 1, ...a}"] {
            let d = diag(src);
            assert_eq!((d.message.as_str(), d.hint.as_deref()), ("dict keys are names, strings or [expr]", Some(MERGE_HINT)), "{src}");
        }
        // A `**` that isn't an argument gets none.
        assert_eq!(diag("let r = [**o]").hint, None);
        // A format left after the expression (3.5): the hint fits the format, and passes the
        // expression in.
        let d = diag("print(f\"${d.rate:%}\")");
        assert_eq!((d.message.as_str(), d.hint.as_deref()), (ONLY_FIXED, Some("format it in a block, as in @py(x: d.rate) { f\"{x:%}\" }")));
        assert_eq!(hint("print(f\"${n:,}\")"), "format it in a block, as in @py(n) { f\"{n:,}\" }");
        assert_eq!(hint("print(f\"${name:>8}\")"), super::super::lexer::PAD_HINT);
    }

    #[test]
    fn review_0_5_regressions() {
        // A block in operand position is a SyntaxError at the `@`, never a panic.
        for src in [
            "let x = 1 + @py { 2 }",
            "let x = -@py { 2 }",
            "let x = !@py { 2 }",
            "let y = x == @py { 2 }",
            "print(2 ** @py { 1 })",
            "let x = a ?? @py { 1 }",
            "let s = f\"${1 + @py { 1 }}\"",
            "func f(a = 1 + @py { 1 }) { }",
            "for x in @py ```\n[1]\n```\n{\n}",
        ] {
            let d = diag(src);
            assert_eq!(d.message, "a block value must be in parentheses here", "{src}");
            assert_eq!(&src[d.span.lo as usize..d.span.lo as usize + 3], "@py", "{src}");
        }
        assert!(parse_str("let x = (@py { 2 }) + 1\nlet y = c ? 1 : @py { 2 }").is_ok());

        // A line that starts with any binary-only operator gets the 3.3 hint; mid-line, none.
        for src in ["print(1)\n?? 2", "print(1)\n!= 2", "let a = 2\n** 3", "let a = 2\n    >= 3", "let a = 2\n? 3 : 4", "let a = [1]\nin a", "x\n+ 2"] {
            assert_eq!(diag(src).hint.as_deref(), Some("end the previous line with the operator"), "{src}");
        }
        for src in ["let y = *", "let x = 1 + * 2", "f(a, == b)", "let x = (* 2)"] {
            assert_eq!(diag(src).hint, None, "{src}");
        }

        // A ternary may start a for range: `(c ? 0 : 1)..10`.
        let prog = parse_str("for i in c ? 0 : 1..10 { }").unwrap();
        let StmtKind::For { iter: ForIter::Range(a, _), .. } = &prog[0].kind else { panic!("range loop") };
        assert_eq!(show(a), "(? c 0 1)");
        assert_eq!(err("let r = c ? 0 : 1..3"), "a..b is only allowed in a for head; use range(a, b)");

        // A body `{` must be on its header's line, inside brackets too.
        for src in ["print(func(x)\n{\n    return x\n})", "let xs = [func()\n{ }]", "let d = {f: func()\n{ }}"] {
            let d = diag(src);
            assert_eq!((d.message.as_str(), d.hint.as_deref()), ("expected {", Some("put { at the end of the func line")), "{src}");
        }
        assert!(parse_str("print(func(x) {\n    return x\n})").is_ok());
        assert!(parse_str("if \"a\" == \"\"\"\n    a\n    \"\"\" { }").is_ok(), "a multi-line token ends on the brace's line");

        // A compiled function exported twice (15.2); compiled blocks export names only (4.4, 4.5).
        assert_eq!(err("@c fast {\n    int f(int a) { return a; }\n    @export { f, f }\n}"), "'f' is exported twice");
        assert!(parse_str("@c fast {\n    @export { f, g }\n}").is_ok());
        for p in ["\"\"", "\"int g(int a)\"", "\"std::string greet(const std::string& name)\""] {
            assert_eq!(err(&format!("@cpp fast {{\n    @export {{ {p} }}\n}}")), "compiled blocks export functions by name", "{p}");
        }

        // Duplicate extern functions point at the name, with a note at the first.
        let d = diag("extern \"m\" {\n    double cos(double x);\n    double cos(double y);\n}");
        assert_eq!((d.span.line, d.span.col, d.notes[0].1.line, d.notes[0].1.col), (3, 12, 2, 12));
    }

    #[test]
    fn rereview_0_5_regressions() {
        // A body `{` after a name ending in a multi-byte char.
        for src in ["let café = true\nif café {\n    print(1)\n}", "if é {\n}", "while 日本 { }", "for x in é { }", "if a { } elif 𝔸 { }"] {
            assert!(parse_str(src).is_ok(), "{src}");
        }
        assert_eq!(err("print(func(é)\n{ })"), "expected {");
        // `not in` starting a line is a leading operator; elsewhere `not` gets its own message.
        let d = diag("let a = 1\nnot in xs");
        assert_eq!((d.message.as_str(), d.hint.as_deref()), ("expected an expression, found keyword `not`", Some("end the previous line with the operator")));
        assert_eq!(diag("let x = not in xs").hint, None);
        assert_eq!(err("let x = not y"), "not is only used in 'not in'; negate with !x");
        // A header continued by a leading operator on the next line.
        for (src, found) in [("if a\n    && b {\n}", "`&&`"), ("while a\n    || b {\n}", "`||`"), ("for x in xs\n    + ys {\n}", "`+`"), ("if a { } elif b\n    not in c {\n}", "keyword `not`")] {
            let d = diag(src);
            assert_eq!(d.message, format!("expected {{, found {found} on the next line"), "{src}");
            assert_eq!(d.hint.as_deref(), Some("end the previous line with the operator"), "{src}");
            assert_eq!(d.span.line, 2, "{src}");
        }
        // An unfinished `${…}` is reported at its `}`.
        let src = "let a = 1\nprint(f\"total: ${a +}\")";
        let d = diag(src);
        assert_eq!((d.message.as_str(), d.span.line, d.span.col), ("expected an expression, found `}`", 2, 21));
        assert_eq!(err("print(f\"x ${a ? 1} y\")"), "expected `:`, found `}`");
        assert!(parse_str("print(f\"${ {a: 1} }\", f\"${func() { }}\")").is_ok());
        // Quoted export names stay in py, js and ts blocks (4.4).
        assert!(parse_str("@js words {\n    @export { \"default\" }\n}").is_ok());
        // The whole-module form stays at V2 level (4.1).
        assert!(parse_str("@import py.numpy as np\n@import js.\"date-fns\" as dfns").is_ok());
    }

    #[test]
    fn chains_are_limited_to_10_000_links() {
        // Run on a big stack: dropping a 10,000-deep tree recurses.
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(|| {
                let chain = |op: &str, n: usize| format!("let x = a{}", op.repeat(n));
                for op in [" + a", " && a", " ?? a", " * a", ".b", "()", "[0]"] {
                    assert!(parse_str(&chain(op, 10_000)).is_ok(), "{op}");
                    assert_eq!(err(&chain(op, 10_001)), "expression too long", "{op}");
                }
                // Links add up along one path, through parentheses and interpolations.
                assert_eq!(err(&format!("let x = a{}(a{})", " + a".repeat(5_000), " + a".repeat(5_001))), "expression too long");
                assert!(parse_str(&format!("let x = a{}\nlet y = a{}", " + a".repeat(10_000), " + a".repeat(10_000))).is_ok());
                // The height of a finished operand counts too: a group or a tighter chain sits
                // under the chain it starts.
                let mut nested = "a".to_string();
                for _ in 0..2 {
                    nested = format!("({nested}{})", " + a".repeat(4_999));
                }
                assert!(parse_str(&format!("let x = {nested}")).is_ok(), "two groups of 4,999 stay under 10,000");
                nested = format!("({nested}{})", " + a".repeat(4_999));
                assert_eq!(err(&format!("let x = {nested}")), "expression too long");
                assert_eq!(err(&format!("let x = (a{}).b{}", ".b".repeat(6_000), ".b".repeat(5_000))), "expression too long");
                assert_eq!(err(&format!("let x = a{} * a{}", ".b".repeat(6_000), " * a".repeat(5_000))), "expression too long");
                assert_eq!(err(&format!("let x = f\"${{a{}}}\"{}", " + a".repeat(6_000), " + a".repeat(5_000))), "expression too long");
                assert_eq!(err(&format!("let f = func() {{ return a{} }}{}", " + a".repeat(6_000), ".b".repeat(5_000))), "expression too long");
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
