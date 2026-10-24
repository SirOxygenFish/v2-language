//! The C declaration parser (INTERNALS 16.3).
//!
//! Reads `extern` bodies (a list of prototypes) at parse time, and single definition heads (`;`
//! optional). Errors carry byte offsets into the text; the caller turns them into `SyntaxError`
//! spans. The C++ dialect, for `@cpp` definition heads (15.3), adds `std::string`,
//! `const std::string&`, `[[…]]`, `noexcept` and `extern "C"`. Definition heads of compiled
//! blocks arrive with roadmap 2.3 (C) and 2.4 (C++).

use std::rc::Rc;

use super::types::{CType, Param, Sig, CHAR_SIGNED, LONG_SIZE, PTR_SIZE};

/// A declaration V2 can't read, at bytes `lo..hi` of the text, with an optional note.
#[derive(Debug, PartialEq)]
pub struct CdeclError {
    pub message: String,
    pub lo: usize,
    pub hi: usize,
    pub note: Option<(String, usize, usize)>,
}

impl CdeclError {
    fn new(message: impl Into<String>, lo: usize, hi: usize) -> CdeclError {
        CdeclError { message: message.into(), lo, hi, note: None }
    }
}

/// C, or C with the additions of 16.3's C++ dialect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    C,
    Cpp,
}

/// Every prototype of an `extern` body (each ends with `;`), with the byte range of each
/// function's name.
pub fn parse_extern(text: &str) -> Result<Vec<(Sig, usize, usize)>, CdeclError> {
    let mut p = P::new(text, Dialect::C)?;
    let mut sigs: Vec<(Sig, usize, usize)> = Vec::new();
    while !p.at_end() {
        let (sig, lo, hi) = p.proto()?;
        match p.peek() {
            Some(CT::Punct(';')) => {
                p.pos += 1;
            }
            Some(CT::Punct(',')) => return Err(p.err_here("declare one function per declaration")),
            Some(_) => {
                p.check_unreadable(p.pos)?;
                return Err(p.err_here("expected `;` after the declaration"));
            }
            None => return Err(p.err_here("expected `;` after the declaration")),
        }
        if let Some((_, first_lo, first_hi)) = sigs.iter().find(|(s, _, _)| s.name == sig.name) {
            let mut e = CdeclError::new(format!("'{}' is declared twice in this extern block", sig.name), lo, hi);
            e.note = Some(("first declared here".into(), *first_lo, *first_hi));
            return Err(e);
        }
        sigs.push((sig, lo, hi));
    }
    Ok(sigs)
}

/// One definition head or prototype (`;` optional), for compiled C/C++ functions (15.3).
#[allow(dead_code)] // read by compiled-block signature extraction (roadmap 2.3)
pub fn parse_definition_head(text: &str, dialect: Dialect) -> Result<Sig, CdeclError> {
    let mut p = P::new(text, dialect)?;
    if p.at_end() {
        return Err(p.err_here("expected a C prototype, like \"int add(int a, int b)\""));
    }
    let (sig, _, _) = p.proto()?;
    if matches!(p.peek(), Some(CT::Punct(','))) {
        return Err(p.err_here("declare one function per declaration"));
    }
    if matches!(p.peek(), Some(CT::Punct(';'))) {
        p.pos += 1;
    }
    if !p.at_end() {
        p.check_unreadable(p.pos)?;
        return Err(p.err_here("expected the end of the prototype"));
    }
    Ok(sig)
}

// ---------------------------------------------------------------------------------------------
// Tokens

#[derive(Clone, Copy, PartialEq, Debug)]
enum CT<'a> {
    Ident(&'a str),
    Int,
    Punct(char),
    Ellipsis,
    ColonColon,
    Literal,
    Preproc,
}

#[derive(Clone, Copy, Debug)]
struct Tok<'a> {
    kind: CT<'a>,
    lo: usize,
    hi: usize,
}

fn tokenize(src: &str) -> Vec<Tok<'_>> {
    let b = src.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut line_start = true;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        if src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if src[i..].starts_with("/*") {
            i = src[i + 2..].find("*/").map(|k| i + 2 + k + 2).unwrap_or(b.len());
            continue;
        }
        let lo = i;
        if c == b'#' && line_start {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            toks.push(Tok { kind: CT::Preproc, lo, hi: i });
            continue;
        }
        line_start = false;
        if c == b'_' || c.is_ascii_alphabetic() {
            while i < b.len() && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            toks.push(Tok { kind: CT::Ident(&src[lo..i]), lo, hi: i });
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            toks.push(Tok { kind: CT::Int, lo, hi: i });
        } else if src[i..].starts_with("...") {
            i += 3;
            toks.push(Tok { kind: CT::Ellipsis, lo, hi: i });
        } else if src[i..].starts_with("::") {
            i += 2;
            toks.push(Tok { kind: CT::ColonColon, lo, hi: i });
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            toks.push(Tok { kind: CT::Literal, lo, hi: i });
        } else {
            let ch = src[i..].chars().next().unwrap_or(' ');
            i += ch.len_utf8();
            toks.push(Tok { kind: CT::Punct(ch), lo, hi: i });
        }
    }
    toks
}

// ---------------------------------------------------------------------------------------------
// Types

/// The base type before pointers.
#[derive(Clone, Debug)]
enum Base {
    Void,
    Bool,
    Int { size: u8, signed: bool, plain_char: bool },
    F32,
    F64,
    /// A typedef that is itself a pointer (`HANDLE`, `LPSTR`) or `const char*` (`LPCSTR`).
    PtrTypedef(CType),
    /// An unknown identifier: valid only before `*`.
    Opaque(String),
    Struct,
    Enum,
    /// C++ `std::string` (`&` in `Specs::reference`).
    CppString,
    Unsupported(String),
}

fn typedef(name: &str) -> Option<Base> {
    let int = |size: u8, signed: bool| Some(Base::Int { size, signed, plain_char: false });
    match name {
        "int8_t" => int(1, true),
        "uint8_t" => int(1, false),
        "int16_t" => int(2, true),
        "uint16_t" => int(2, false),
        "int32_t" => int(4, true),
        "uint32_t" => int(4, false),
        "int64_t" => int(8, true),
        "uint64_t" => int(8, false),
        "size_t" | "uintptr_t" => int(PTR_SIZE, false),
        "ssize_t" | "intptr_t" | "ptrdiff_t" => int(PTR_SIZE, true),
        "BOOL" | "INT" | "LONG" | "HRESULT" => int(4, true),
        "DWORD" | "UINT" | "ULONG" => int(4, false),
        "BYTE" | "UCHAR" => int(1, false),
        "CHAR" => Some(Base::Int { size: 1, signed: CHAR_SIGNED, plain_char: true }),
        "WORD" | "USHORT" => int(2, false),
        "SHORT" => int(2, true),
        "LONGLONG" => int(8, true),
        "ULONGLONG" | "DWORD64" => int(8, false),
        "SIZE_T" | "WPARAM" | "UINT_PTR" | "ULONG_PTR" | "DWORD_PTR" => int(PTR_SIZE, false),
        "LPARAM" | "LRESULT" | "INT_PTR" | "LONG_PTR" => int(PTR_SIZE, true),
        "HANDLE" | "HWND" | "HMODULE" | "HINSTANCE" | "LPVOID" | "LPCVOID" | "LPWSTR" | "LPCWSTR" | "LPSTR" => {
            Some(Base::PtrTypedef(CType::Ptr))
        }
        "LPCSTR" => Some(Base::PtrTypedef(CType::Str)),
        _ => None,
    }
}

const QUALS: &[&str] = &["const", "volatile", "restrict", "__restrict", "__restrict__"];
const CALLCONV: &[&str] = &["__cdecl", "_cdecl", "__stdcall", "_stdcall", "__fastcall", "_fastcall", "WINAPI", "WINAPIV", "APIENTRY", "CALLBACK"];
const IGNORED: &[&str] = &["extern", "static", "inline", "__inline", "__extension__", "WINBASEAPI", "WINUSERAPI"];
const INT_WORDS: &[&str] = &["signed", "unsigned", "char", "short", "int", "long"];
/// C keywords that are not type words: never a function name.
const C_KEYWORDS: &[&str] = &[
    "auto", "break", "case", "continue", "default", "do", "else", "for", "goto", "if", "register", "return", "sizeof",
    "switch", "typedef", "while", "_Alignas", "_Alignof", "_Atomic", "_Generic", "_Noreturn", "_Static_assert",
    "_Thread_local",
];
const UNSUPPORTED: &[&str] = &["wchar_t", "char16_t", "char32_t", "__int128", "_Complex"];

struct P<'a> {
    src: &'a str,
    toks: Vec<Tok<'a>>,
    pos: usize,
    cpp: bool,
}

/// Specifiers read before a declarator: the base type, whether `const` appeared, the type's
/// spelling without ignored words, and the bytes of the base type's own tokens (for errors).
struct Specs {
    base: Base,
    is_const: bool,
    spelling: String,
    lo: usize,
    hi: usize,
}

/// A word that is part of a type, so never a K&R parameter name.
fn type_word(w: &str) -> bool {
    INT_WORDS.contains(&w)
        || QUALS.contains(&w)
        || UNSUPPORTED.contains(&w)
        || matches!(w, "void" | "bool" | "_Bool" | "float" | "double" | "struct" | "union" | "enum")
        || typedef(w).is_some()
}

impl<'a> P<'a> {
    fn new(src: &'a str, dialect: Dialect) -> Result<P<'a>, CdeclError> {
        Ok(P { src, toks: tokenize(src), pos: 0, cpp: dialect == Dialect::Cpp })
    }

    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn peek(&self) -> Option<CT<'a>> {
        self.toks.get(self.pos).map(|t| t.kind)
    }

    fn peek_at(&self, n: usize) -> Option<CT<'a>> {
        self.toks.get(self.pos + n).map(|t| t.kind)
    }

    fn err_here(&self, message: &str) -> CdeclError {
        match self.toks.get(self.pos) {
            Some(t) => CdeclError::new(message, t.lo, t.hi),
            None => CdeclError::new(message, self.src.len(), self.src.len()),
        }
    }

    fn err_tok(&self, i: usize, message: impl Into<String>) -> CdeclError {
        let t = self.toks[i];
        CdeclError::new(message, t.lo, t.hi)
    }

    fn kind_at(&self, i: usize) -> Option<CT<'a>> {
        self.toks.get(i).map(|t| t.kind)
    }

    /// Reject constructs that can never be read, wherever they appear.
    fn check_unreadable(&self, i: usize) -> Result<(), CdeclError> {
        match self.toks[i].kind {
            CT::Preproc => Err(self.err_tok(i, "#include/#define are not supported in extern — copy the prototypes")),
            CT::Ident("typedef") => Err(self.err_tok(i, "typedef is not supported in extern — use the underlying type")),
            CT::Punct('{') => Err(self.err_tok(i, "structs are not supported in extern — wrap the function in a @c block")),
            CT::Punct('&') | CT::ColonColon => Err(self.err_tok(i, "cannot read this declaration")),
            _ => Ok(()),
        }
    }

    /// At a `(` where a name belongs: `(*f)(…)` and `(WINAPI *f)(…)` are function pointers.
    fn paren_declarator(&self) -> CdeclError {
        let mut k = self.pos + 1;
        while matches!(self.kind_at(k), Some(CT::Ident(w)) if CALLCONV.contains(&w)) {
            k += 1;
        }
        if matches!(self.kind_at(k), Some(CT::Punct('*'))) {
            self.err_here("function pointers (callbacks) are not supported")
        } else {
            self.err_here("cannot read this declaration")
        }
    }

    /// After `__declspec` / `__attribute__` at `k`: the index past its parenthesized group.
    fn after_group(&self, k: usize) -> Option<usize> {
        if !matches!(self.kind_at(k), Some(CT::Punct('('))) {
            return None;
        }
        let mut depth = 0;
        for (i, t) in self.toks.iter().enumerate().skip(k) {
            match t.kind {
                CT::Punct('(') => depth += 1,
                CT::Punct(')') => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Whether `*` comes next, past qualifiers, ignored words and attribute groups: an unknown
    /// name is a valid pointee only there (16.3).
    fn star_follows(&self, mut k: usize) -> bool {
        loop {
            match self.kind_at(k) {
                Some(CT::Ident("__declspec" | "__attribute__")) => match self.after_group(k + 1) {
                    Some(next) => k = next,
                    None => return false,
                },
                Some(CT::Ident(w)) if QUALS.contains(&w) || IGNORED.contains(&w) || CALLCONV.contains(&w) => k += 1,
                Some(CT::Punct('*')) => return true,
                _ => return false,
            }
        }
    }

    /// Whether `&` comes next, past qualifiers.
    fn amp_follows(&self, mut k: usize) -> bool {
        while matches!(self.kind_at(k), Some(CT::Ident(w)) if QUALS.contains(&w)) {
            k += 1;
        }
        matches!(self.kind_at(k), Some(CT::Punct('&')))
    }

    /// C++ `[[…]]` at the cursor: skip it.
    fn skip_attribute(&mut self) -> Result<(), CdeclError> {
        let start = self.pos;
        let mut depth = 0;
        while let Some(k) = self.peek() {
            self.pos += 1;
            match k {
                CT::Punct('[') => depth += 1,
                CT::Punct(']') => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err(self.err_tok(start, "cannot read this declaration"))
    }

    /// C++ after the parameter list: `noexcept`, `noexcept(…)` and `[[…]]`.
    fn cpp_trailer(&mut self) -> Result<(), CdeclError> {
        loop {
            match self.peek() {
                Some(CT::Ident("noexcept")) => {
                    self.pos += 1;
                    if matches!(self.peek(), Some(CT::Punct('('))) {
                        self.skip_parens()?;
                    }
                }
                Some(CT::Punct('[')) if matches!(self.peek_at(1), Some(CT::Punct('['))) => self.skip_attribute()?,
                _ => return Ok(()),
            }
        }
    }

    /// Whether a `(` comes before the next `;` (or the end).
    fn paren_before_semicolon(&self) -> bool {
        self.toks[self.pos..]
            .iter()
            .take_while(|t| t.kind != CT::Punct(';'))
            .any(|t| t.kind == CT::Punct('('))
    }

    /// `specs declarator` → (signature, byte range of the function's name).
    fn proto(&mut self) -> Result<(Sig, usize, usize), CdeclError> {
        let specs = self.specs()?;
        let stars = self.stars();
        while matches!(self.peek(), Some(CT::Ident(w)) if CALLCONV.contains(&w)) {
            self.pos += 1;
        }
        if matches!(self.peek(), Some(CT::Punct('('))) {
            return Err(self.paren_declarator());
        }
        let name_i = self.pos;
        let name = match self.peek() {
            Some(CT::Ident(n)) if C_KEYWORDS.contains(&n) => return Err(self.err_here("cannot read this declaration")),
            Some(CT::Ident(n)) => n,
            // `struct tm;`, `int;`: no function here.
            Some(CT::Punct(';')) | None => return Err(CdeclError::new("only functions can be declared in extern", specs.lo, specs.hi)),
            Some(_) => {
                self.check_unreadable(self.pos)?;
                return Err(self.err_here("cannot read this declaration"));
            }
        };
        self.pos += 1;
        match self.peek() {
            Some(CT::Punct('(')) => {}
            Some(CT::ColonColon) => return Err(self.err_here("cannot read this declaration")),
            // `int SQLITE_STDCALL f(void)`: a macro between the type and the name.
            Some(CT::Ident(_)) if self.paren_before_semicolon() => {
                return Err(self.err_tok(name_i, format!("unknown type '{name}' — remove macros from copied declarations")))
            }
            _ => return Err(self.err_tok(name_i, "only functions can be declared in extern")),
        }
        self.pos += 1;
        let params = self.params()?;
        if self.cpp {
            self.cpp_trailer()?;
        }
        let ret = resolve(&specs, stars, true).map_err(|m| CdeclError::new(m, specs.lo, specs.hi))?;
        let n = self.toks[name_i];
        let sig = Sig { name: name.into(), ret, ret_spelling: spelling(&specs, stars).into(), params, err_slot: false };
        Ok((sig, n.lo, n.hi))
    }

    /// Qualifiers, ignored words and exactly one base type.
    fn specs(&mut self) -> Result<Specs, CdeclError> {
        let mut is_const = false;
        let mut words: Vec<&str> = Vec::new();
        let mut base: Option<Base> = None;
        // The qualifiers and type words as written, and the bytes of the type words.
        let mut spelled: Vec<&str> = Vec::new();
        let mut span: Option<(usize, usize)> = None;
        let mut reference = false;
        while let Some(kind) = self.peek() {
            let t = self.toks[self.pos];
            if self.cpp {
                match kind {
                    // `const std::string&`
                    CT::Punct('&') if matches!(base, Some(Base::CppString)) => {
                        if !is_const || matches!(self.peek_at(1), Some(CT::Punct('&'))) {
                            // `std::string&`, `std::string&&`: the references row (16.3).
                            return Err(self.err_tok(self.pos + usize::from(is_const), "cannot read this declaration"));
                        }
                        reference = true;
                        span = span.map(|(lo, _)| (lo, t.hi));
                        self.pos += 1;
                        break;
                    }
                    CT::Punct('[') if matches!(self.peek_at(1), Some(CT::Punct('['))) => {
                        self.skip_attribute()?;
                        continue;
                    }
                    // `extern "C"`
                    CT::Literal if self.pos > 0 && self.toks[self.pos - 1].kind == CT::Ident("extern") => {
                        self.pos += 1;
                        continue;
                    }
                    _ => {}
                }
            }
            self.check_unreadable(self.pos)?;
            let CT::Ident(w) = kind else { break };
            if QUALS.contains(&w) {
                is_const |= w == "const";
                spelled.push(w);
                self.pos += 1;
                continue;
            }
            if IGNORED.contains(&w) || CALLCONV.contains(&w) {
                self.pos += 1;
                continue;
            }
            if w == "__declspec" || w == "__attribute__" {
                self.pos += 1;
                self.skip_parens()?;
                continue;
            }
            let unsupported = matches!(base, Some(Base::Unsupported(_)));
            let float_word = w == "float" || w == "double";
            if UNSUPPORTED.contains(&w) && (base.is_none() || unsupported || matches!(base, Some(Base::F32 | Base::F64))) {
                // `__int128`, `unsigned __int128`, `double _Complex`: the whole type is unsupported.
                if !unsupported {
                    base = Some(Base::Unsupported(w.into()));
                }
                words.clear();
            } else if unsupported && (INT_WORDS.contains(&w) || float_word) {
                // `_Complex double`, `__int128 unsigned`: absorbed.
            } else if (w == "double" && base.is_none() && words == ["long"])
                || (w == "long" && matches!(base, Some(Base::F64)) && words.is_empty())
            {
                base = Some(Base::Unsupported("long double".into()));
                words.clear();
            } else if INT_WORDS.contains(&w) && base.is_none() {
                words.push(w);
            } else if base.is_some() || !words.is_empty() {
                if type_word(w) {
                    let mut all = spelled.clone();
                    all.push(w);
                    return Err(self.err_tok(self.pos, format!("cannot read the type '{}'", all.join(" "))));
                }
                break; // the declarator's name
            } else {
                base = Some(match w {
                    "void" => Base::Void,
                    "bool" | "_Bool" => Base::Bool,
                    "float" => Base::F32,
                    "double" => Base::F64,
                    "struct" | "union" | "enum" => {
                        self.pos += 1;
                        let Some(CT::Ident(tag)) = self.peek() else {
                            return Err(self.err_here("structs are not supported in extern — wrap the function in a @c block"));
                        };
                        if matches!(self.peek_at(1), Some(CT::Punct('{'))) {
                            return Err(self.err_tok(self.pos + 1, "structs are not supported in extern — wrap the function in a @c block"));
                        }
                        spelled.push(w);
                        spelled.push(tag);
                        span = Some((span.map_or(t.lo, |s| s.0), self.toks[self.pos].hi));
                        self.pos += 1;
                        base = Some(if w == "enum" { Base::Enum } else { Base::Struct });
                        continue;
                    }
                    w => match typedef(w) {
                        Some(b) => b,
                        None if matches!(self.kind_at(self.pos + 1), Some(CT::ColonColon)) => {
                            if self.cpp && w == "std" && matches!(self.kind_at(self.pos + 2), Some(CT::Ident("string"))) {
                                spelled.push("std::string");
                                span = Some((span.map_or(t.lo, |s| s.0), self.toks[self.pos + 2].hi));
                                self.pos += 3;
                                base = Some(Base::CppString);
                                continue;
                            }
                            // A qualified name (16.3).
                            return Err(self.err_tok(self.pos + 1, "cannot read this declaration"));
                        }
                        // An unknown name is a valid pointee only before `*`.
                        None if self.star_follows(self.pos + 1) => Base::Opaque(w.into()),
                        // A C++ reference to it: the references row (16.3).
                        None if self.cpp && self.amp_follows(self.pos + 1) => {
                            let mut k = self.pos + 1;
                            while !matches!(self.kind_at(k), Some(CT::Punct('&'))) {
                                k += 1;
                            }
                            return Err(self.err_tok(k, "cannot read this declaration"));
                        }
                        None => {
                            return Err(self.err_tok(self.pos, format!("unknown type '{w}' — remove macros from copied declarations")))
                        }
                    },
                });
            }
            spelled.push(w);
            span = Some((span.map_or(t.lo, |s| s.0), t.hi));
            self.pos += 1;
        }
        let (lo, hi) = span.unwrap_or_else(|| match self.toks.get(self.pos) {
            Some(t) => (t.lo, t.hi),
            None => (self.src.len(), self.src.len()),
        });
        let base = match (base, words.is_empty()) {
            (Some(b), true) => b,
            (None, false) => int_base(&words).ok_or_else(|| CdeclError::new(format!("cannot read the type '{}'", words.join(" ")), lo, hi))?,
            (Some(_), false) => return Err(CdeclError::new("cannot read this declaration", lo, hi)),
            (None, true) => {
                return Err(if matches!(self.peek(), Some(CT::Ellipsis)) {
                    self.err_here("variadic functions (like printf) are not supported — wrap the call in a @c block")
                } else {
                    self.err_here("expected a type")
                })
            }
        };
        let mut spelling = spelled.join(" ");
        if reference {
            spelling.push('&');
        }
        Ok(Specs { base, is_const, spelling, lo, hi })
    }

    /// `__declspec(…)` / `__attribute__((…))`: skip a balanced parenthesized group.
    fn skip_parens(&mut self) -> Result<(), CdeclError> {
        if !matches!(self.peek(), Some(CT::Punct('('))) {
            return Err(self.err_here("cannot read this declaration"));
        }
        let mut depth = 0;
        while let Some(k) = self.peek() {
            self.pos += 1;
            match k {
                CT::Punct('(') => depth += 1,
                CT::Punct(')') => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err(self.err_here("cannot read this declaration"))
    }

    /// `{ "*" { qual } }` → pointer depth.
    fn stars(&mut self) -> u32 {
        let mut n = 0;
        while matches!(self.peek(), Some(CT::Punct('*'))) {
            self.pos += 1;
            n += 1;
            while matches!(self.peek(), Some(CT::Ident(q)) if QUALS.contains(&q)) {
                self.pos += 1;
            }
        }
        n
    }

    /// `f(a, b) int a; …` or `f(a) { … }`: an old-style (K&R) definition. Its parameters are
    /// bare names; `f(int, double) {` is a definition with unnamed parameters instead.
    fn knr_list(&self) -> bool {
        let mut k = self.pos;
        loop {
            if !matches!(self.kind_at(k), Some(CT::Ident(w)) if !type_word(w)) {
                return false;
            }
            k += 1;
            match self.kind_at(k) {
                Some(CT::Punct(',')) => k += 1,
                Some(CT::Punct(')')) => {
                    return matches!(self.kind_at(k + 1), Some(CT::Punct('{')))
                        || matches!(self.kind_at(k + 1), Some(CT::Ident(w)) if type_word(w) || w == "register")
                }
                _ => return false,
            }
        }
    }

    /// After `(`: `void`, nothing, or a comma-separated list; consumes the `)`.
    fn params(&mut self) -> Result<Vec<Param>, CdeclError> {
        if matches!(self.peek(), Some(CT::Punct(')'))) {
            self.pos += 1;
            return Ok(Vec::new());
        }
        if matches!(self.peek(), Some(CT::Ident("void"))) && matches!(self.peek_at(1), Some(CT::Punct(')'))) {
            self.pos += 2;
            return Ok(Vec::new());
        }
        if self.knr_list() {
            return Err(self.err_here("cannot read this declaration"));
        }
        let mut params = Vec::new();
        loop {
            if matches!(self.peek(), Some(CT::Ellipsis)) {
                return Err(self.err_here("variadic functions (like printf) are not supported — wrap the call in a @c block"));
            }
            let specs = self.specs()?;
            let mut stars = self.stars();
            if matches!(self.peek(), Some(CT::Punct('('))) {
                return Err(self.paren_declarator());
            }
            let mut name = None;
            if let Some(CT::Ident(n)) = self.peek() {
                name = Some(Rc::<str>::from(n));
                self.pos += 1;
            }
            if matches!(self.peek(), Some(CT::Punct('['))) {
                // An array parameter decays to a pointer.
                self.pos += 1;
                if matches!(self.peek(), Some(CT::Int)) {
                    self.pos += 1;
                }
                if !matches!(self.peek(), Some(CT::Punct(']'))) {
                    return Err(self.err_here("cannot read this declaration"));
                }
                self.pos += 1;
                stars += 1;
            }
            match self.peek() {
                // `int...` (C++): the same as `int, ...`.
                Some(CT::Ellipsis) => {
                    return Err(self.err_here("variadic functions (like printf) are not supported — wrap the call in a @c block"))
                }
                // `int cb(int)`: a function parameter is a function pointer.
                Some(CT::Punct('(')) if name.is_some() => {
                    return Err(self.err_here("function pointers (callbacks) are not supported"))
                }
                _ => {}
            }
            let ty = resolve(&specs, stars, false).map_err(|m| CdeclError::new(m, specs.lo, specs.hi))?;
            // A C++ string parameter can't be null: the shim builds the string (15.3).
            let non_null = matches!(specs.base, Base::CppString) && stars == 0;
            params.push(Param { ty, spelling: spelling(&specs, stars).into(), name, non_null });
            match self.peek() {
                Some(CT::Punct(',')) => self.pos += 1,
                Some(CT::Punct(')')) => {
                    self.pos += 1;
                    return Ok(params);
                }
                Some(_) => {
                    self.check_unreadable(self.pos)?;
                    return Err(self.err_here("expected `,` or `)` in the parameter list"));
                }
                None => return Err(self.err_here("expected `)` to close the parameter list")),
            }
        }
    }
}

/// `char`, `unsigned short int`, `long long`, … (any C-valid mix).
fn int_base(words: &[&str]) -> Option<Base> {
    let count = |w: &str| words.iter().filter(|x| **x == w).count();
    let (signed, unsigned, char, short, int, long) =
        (count("signed"), count("unsigned"), count("char"), count("short"), count("int"), count("long"));
    if signed + unsigned > 1 || char > 1 || short > 1 || int > 1 || long > 2 {
        return None;
    }
    if char == 1 {
        if short + int + long > 0 {
            return None;
        }
        let plain = signed + unsigned == 0;
        let s = if plain { CHAR_SIGNED } else { signed == 1 };
        return Some(Base::Int { size: 1, signed: s, plain_char: plain });
    }
    if short == 1 && long > 0 {
        return None;
    }
    let size = if short == 1 {
        2
    } else if long == 2 {
        8
    } else if long == 1 {
        LONG_SIZE
    } else {
        4
    };
    Some(Base::Int { size, signed: unsigned == 0, plain_char: false })
}

/// The `CType` of a base type plus pointer depth (16.4).
fn resolve(specs: &Specs, stars: u32, is_return: bool) -> Result<CType, String> {
    if stars > 0 {
        let is_const_char = matches!(specs.base, Base::Int { plain_char: true, .. }) && specs.is_const && stars == 1;
        return Ok(match specs.base {
            Base::PtrTypedef(_) => CType::Ptr,
            _ if is_const_char => CType::Str,
            _ => CType::Ptr,
        });
    }
    Ok(match &specs.base {
        Base::Void if is_return => CType::Void,
        Base::Void => return Err("`void` is only valid as the whole parameter list: f(void)".into()),
        Base::Bool => CType::Bool,
        Base::Int { size, signed, .. } => CType::Int { size: *size, signed: *signed },
        Base::F32 => CType::F32,
        Base::F64 => CType::F64,
        Base::PtrTypedef(t) => *t,
        Base::Opaque(n) => return Err(format!("unknown type '{n}' — remove macros from copied declarations")),
        Base::Struct => return Err("structs are not supported in extern — wrap the function in a @c block".into()),
        Base::Enum => return Err("enum types are not supported by value — declare it as int".into()),
        Base::CppString if is_return => CType::OwnedStr,
        Base::CppString => CType::Str,
        Base::Unsupported(n) => return Err(format!("type '{n}' is not supported")),
    })
}

/// The type as written (`const char*`), without ignored words and calling conventions.
fn spelling(specs: &Specs, stars: u32) -> String {
    format!("{}{}", specs.spelling, "*".repeat(stars as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Sig {
        let v = parse_extern(text).unwrap();
        assert_eq!(v.len(), 1);
        v.into_iter().next().unwrap().0
    }

    fn err(text: &str) -> String {
        parse_extern(text).unwrap_err().message
    }

    #[test]
    fn accepts_common_prototypes() {
        let cos = one("double cos(double x);");
        assert_eq!((&*cos.name, cos.ret, cos.params.len()), ("cos", CType::F64, 1));
        assert_eq!(cos.params[0].ty, CType::F64);
        assert_eq!(cos.params[0].name.as_deref(), Some("x"));
        let v = one("const char* sqlite3_libversion(void);");
        assert_eq!((v.ret, v.params.len()), (CType::Str, 0));
        assert_eq!(&*v.ret_spelling, "const char*");
        let p = one("int pow2(int, unsigned long long);");
        assert_eq!(p.params[1].ty, CType::Int { size: 8, signed: false });
        assert!(p.params[0].name.is_none());
        assert_eq!(one("void* malloc(size_t n);").ret, CType::Ptr);
        assert_eq!(one("char const* getenv(const char* name);").ret, CType::Str);
        assert_eq!(one("char* strdup(const char* s);").ret, CType::Ptr, "non-const char* is opaque");
        assert_eq!(one("int f(FILE* f);").params[0].ty, CType::Ptr);
        assert_eq!(one("int f(struct sockaddr* a);").params[0].ty, CType::Ptr);
        assert_eq!(one("long f(void);").ret, CType::Int { size: LONG_SIZE, signed: true });
        assert_eq!(one("unsigned char f(void);").ret, CType::Int { size: 1, signed: false });
        assert_eq!(one("int f(int a[]);").params[0].ty, CType::Ptr);
        let m = one("int WINAPI MessageBoxA(HWND hWnd, LPCSTR text, LPCSTR caption, UINT type);");
        assert_eq!(m.params.iter().map(|p| p.ty).collect::<Vec<_>>(), vec![CType::Ptr, CType::Str, CType::Str, CType::Int { size: 4, signed: false }]);
        assert_eq!(one("__declspec(dllimport) int __cdecl f(void);").name.as_ref(), "f");
        assert_eq!(one("int __fastcall g(int a);").name.as_ref(), "g");
        assert_eq!(one("extern int f(); // comment\n/* block */").params.len(), 0);
        assert_eq!(parse_extern("int a(void); double b(double);").unwrap().len(), 2);
        assert_eq!(parse_extern("  // nothing\n").unwrap().len(), 0);
    }

    #[test]
    fn rejects_with_16_3_messages() {
        assert_eq!(err("struct point f(void);"), "structs are not supported in extern — wrap the function in a @c block");
        assert_eq!(err("struct s { int x; };"), "structs are not supported in extern — wrap the function in a @c block");
        assert_eq!(err("enum color f(void);"), "enum types are not supported by value — declare it as int");
        assert_eq!(err("int printf(const char* fmt, ...);"), "variadic functions (like printf) are not supported — wrap the call in a @c block");
        assert_eq!(err("void qsort(void* b, size_t n, size_t s, int (*cmp)(const void*, const void*));"), "function pointers (callbacks) are not supported");
        assert_eq!(err("typedef int myint;"), "typedef is not supported in extern — use the underlying type");
        assert_eq!(err("#include <math.h>"), "#include/#define are not supported in extern — copy the prototypes");
        assert_eq!(err("extern int errno;"), "only functions can be declared in extern");
        assert_eq!(err("SQLITE_API int sqlite3_open(const char* f, sqlite3** db);"), "unknown type 'SQLITE_API' — remove macros from copied declarations");
        assert_eq!(err("long double f(void);"), "type 'long double' is not supported");
        assert_eq!(err("wchar_t f(void);"), "type 'wchar_t' is not supported");
        assert_eq!(err("int a(void), b(void);"), "declare one function per declaration");
        assert_eq!(err("int& f(void);"), "cannot read this declaration");
        assert_eq!(err("int A::f(void);"), "cannot read this declaration");
        assert_eq!(err("int f(void); int f(void);"), "'f' is declared twice in this extern block");
        assert_eq!(err("double cos(double x)"), "expected `;` after the declaration");
    }

    #[test]
    fn single_heads() {
        assert_eq!(parse_definition_head("int mul(int a, int b)", Dialect::C).unwrap().params.len(), 2);
        assert!(parse_definition_head("int mul(int a, int b);", Dialect::C).is_ok());
        assert_eq!(parse_definition_head("int mul(int a) extra", Dialect::C).unwrap_err().message, "expected the end of the prototype");
    }

    #[test]
    fn error_offsets_point_at_the_token() {
        let e = parse_extern("int f(void);\nint printf(const char* fmt, ...);").unwrap_err();
        assert_eq!(&"int f(void);\nint printf(const char* fmt, ...);"[e.lo..e.hi], "...");
    }

    /// The text an error points at.
    fn at(text: &str) -> (String, String) {
        let e = parse_extern(text).unwrap_err();
        (e.message, text[e.lo..e.hi].to_string())
    }

    #[test]
    fn review_0_5_regressions() {
        // Unsupported types combined with other type words, in either order.
        assert_eq!(err("double _Complex f(void);"), "type '_Complex' is not supported");
        assert_eq!(err("_Complex double f(void);"), "type '_Complex' is not supported");
        assert_eq!(err("float _Complex f(void);"), "type '_Complex' is not supported");
        assert_eq!(err("unsigned __int128 f(void);"), "type '__int128' is not supported");
        assert_eq!(err("__int128 f(void);"), "type '__int128' is not supported");
        assert_eq!(err("double long f(void);"), "type 'long double' is not supported");
        assert_eq!(err("int f(long double x);"), "type 'long double' is not supported");
        assert_eq!(one("double _Complex* f(void);").ret, CType::Ptr, "a pointer to an unsupported type is opaque");
        // The 16.3 table rows.
        assert_eq!(at("int f(int a) { return a; }"), ("structs are not supported in extern — wrap the function in a @c block".into(), "{".into()));
        assert_eq!(at("int f(a, b) int a; int b;"), ("cannot read this declaration".into(), "a".into()));
        assert_eq!(err("int f(a) { return a; }"), "cannot read this declaration");
        assert_eq!(err("int f(size_t) size_t a;"), "expected `;` after the declaration", "size_t is a type, not a K&R name");
        assert_eq!(err("int (WINAPI *f)(void);"), "function pointers (callbacks) are not supported");
        assert_eq!(err("void f(int (__cdecl *cb)(int));"), "function pointers (callbacks) are not supported");
        assert_eq!(at("int SQLITE_STDCALL f(void);"), ("unknown type 'SQLITE_STDCALL' — remove macros from copied declarations".into(), "SQLITE_STDCALL".into()));
        assert_eq!(err("int SOME_MACRO x;"), "only functions can be declared in extern");
        // Type errors point at the type, not at ignored words before it.
        assert_eq!(at("extern struct point f(void);"), ("structs are not supported in extern — wrap the function in a @c block".into(), "struct point".into()));
        assert_eq!(at("static enum color f(void);"), ("enum types are not supported by value — declare it as int".into(), "enum color".into()));
        assert_eq!(at("int f(int a, void);").1, "void");
        // Duplicates point at the name, with a note at the first.
        let text = "double cos(double x);\ndouble cos(double y);";
        let e = parse_extern(text).unwrap_err();
        assert_eq!((&text[e.lo..e.hi], e.lo), ("cos", 29));
        assert_eq!(e.note, Some(("first declared here".into(), 7, 10)));
        assert_eq!(parse_extern("int a(void); double b(double);").unwrap().iter().map(|(_, lo, hi)| (*lo, *hi)).collect::<Vec<_>>(), vec![(4, 5), (20, 21)]);
        // Empty heads.
        for p in ["", "   ", "/* x */", "// c"] {
            assert_eq!(parse_definition_head(p, Dialect::C).unwrap_err().message, "expected a C prototype, like \"int add(int a, int b)\"", "{p:?}");
        }
        assert_eq!(parse_definition_head("int f(int a) { return a; }", Dialect::C).unwrap_err().message, "structs are not supported in extern — wrap the function in a @c block");
    }

    #[test]
    fn rereview_0_5_regressions() {
        // Unnamed parameters are types, never a K&R list: a body gets the 16.3 `{` row.
        for text in ["int add(int, int) { return 0; }", "double sq(double) { }", "int f(size_t, int) { }", "static int g(int) { }"] {
            assert_eq!(at(text), ("structs are not supported in extern — wrap the function in a @c block".into(), "{".into()), "{text}");
        }
        assert_eq!(err("int f(int) __attribute__((pure));"), "expected `;` after the declaration");
        assert_eq!(err("int h(a, b) { return 1; }"), "cannot read this declaration", "a real K&R list");
        // A declaration without a function.
        for (text, ty) in [("struct tm;", "struct tm"), ("union u;", "union u"), ("enum e;", "enum e"), ("int;", "int"), ("int *;", "int")] {
            assert_eq!(at(text), ("only functions can be declared in extern".into(), ty.into()), "{text}");
        }
        assert_eq!(parse_extern("struct tm;").unwrap_err().lo, 0);
        // An unknown pointee may be followed by ignored words before `*`.
        for text in [
            "FILE __cdecl *fopen(const char* path, const char* mode);",
            "FILE WINAPI *f(void);",
            "sqlite3 static *f(void);",
            "extern FILE __declspec(dllimport) *f(void);",
            "FILE __attribute__((x)) *f(void);",
            "void f(FILE __attribute__((unused)) *p);",
        ] {
            assert!(parse_extern(text).is_ok(), "{text}");
        }
        assert_eq!(&*one("FILE __cdecl *fopen(const char* path, const char* mode);").ret_spelling, "FILE*");
        // A qualified name is the 16.3 row, at the `::`.
        assert_eq!(at("std::string f(void);"), ("cannot read this declaration".into(), "::".into()));
    }

    #[test]
    fn cpp_dialect_heads() {
        let cpp = |t: &str| parse_definition_head(t, Dialect::Cpp);
        let g = cpp("std::string greet(const std::string& name)").unwrap();
        assert_eq!((g.ret, &*g.ret_spelling), (CType::OwnedStr, "std::string"));
        assert_eq!((g.params[0].ty, &*g.params[0].spelling, g.params[0].non_null), (CType::Str, "const std::string&", true));
        let r = cpp("std::string reversed(std::string s) noexcept").unwrap();
        assert_eq!((r.params[0].ty, r.params[0].non_null), (CType::Str, true));
        assert!(cpp("[[nodiscard]] int f(int a) noexcept(true) [[gnu::pure]]").is_ok());
        assert!(cpp("extern \"C\" int f(int a)").is_ok());
        assert_eq!(cpp("void f(std::string& s)").unwrap_err().message, "cannot read this declaration");
        assert_eq!(cpp("int f(int& x)").unwrap_err().message, "cannot read this declaration");
        assert_eq!(cpp("int A::f(void)").unwrap_err().message, "cannot read this declaration");
        // The C dialect has none of these.
        assert_eq!(parse_definition_head("std::string f(void)", Dialect::C).unwrap_err().message, "cannot read this declaration");
    }

    #[test]
    fn round2_0_5_regressions() {
        let cpp = |t: &str| parse_definition_head(t, Dialect::Cpp).unwrap_err();
        let at_cpp = |t: &str| {
            let e = cpp(t);
            (e.message, t[e.lo..e.hi].to_string())
        };
        // Definition heads use the 16.3 comma row too.
        assert_eq!(parse_definition_head("int a(void), b(void)", Dialect::C).unwrap_err().message, "declare one function per declaration");
        // A C++ trailer or an attribute after `)` is not a K&R declaration list.
        assert_eq!(cpp("int f(Foo) noexcept").message, "unknown type 'Foo' — remove macros from copied declarations");
        assert_eq!(err("int f(x) __attribute__((pure));"), "unknown type 'x' — remove macros from copied declarations");
        assert_eq!(err("int f(a) register int a;"), "cannot read this declaration");
        // References: only `const std::string&` reads.
        assert_eq!(at_cpp("void f(std::string&& s)"), ("cannot read this declaration".into(), "&".into()));
        assert_eq!(at_cpp("void f(const std::string&& s)"), ("cannot read this declaration".into(), "&".into()));
        assert_eq!(at_cpp("int f(const Foo& x)"), ("cannot read this declaration".into(), "&".into()));
        assert_eq!(at_cpp("Foo& f(void)"), ("cannot read this declaration".into(), "&".into()));
        assert!(parse_definition_head("int f(std::string const& s)", Dialect::Cpp).is_ok());
        // A type word after a complete type is never a name.
        assert_eq!(at("int double f(void);"), ("cannot read the type 'int double'".into(), "double".into()));
        assert_eq!(err("double float(void);"), "cannot read the type 'double float'");
        assert_eq!(err("int f(DWORD int);"), "cannot read the type 'DWORD int'");
        assert_eq!(err("unsigned size_t f(void);"), "cannot read the type 'unsigned size_t'");
        assert_eq!(err("int return(int x);"), "cannot read this declaration");
        assert_eq!(cpp("std::string int f(void)").message, "cannot read the type 'std::string int'");
        // `int...` and function-typed parameters get their own rows.
        assert_eq!(cpp("int f(int...)").message, "variadic functions (like printf) are not supported — wrap the call in a @c block");
        assert_eq!(cpp("int f(const char* fmt...)").message, "variadic functions (like printf) are not supported — wrap the call in a @c block");
        assert_eq!(err("void f(int cb(int));"), "function pointers (callbacks) are not supported");
        assert_eq!(
            err("void qsort(void* b, size_t n, size_t s, int compar(const void*, const void*));"),
            "function pointers (callbacks) are not supported"
        );
    }

    #[test]
    fn spellings_are_the_type_as_written() {
        assert_eq!(&*one("WINUSERAPI HWND WINAPI FindWindowA(LPCSTR a, LPCSTR b);").ret_spelling, "HWND");
        assert_eq!(&*one("extern size_t strlen(const char *s);").ret_spelling, "size_t");
        assert_eq!(&*one("__declspec(dllimport) int __cdecl g(void);").ret_spelling, "int");
        assert_eq!(&*one("static inline sqlite3* open_it(void);").ret_spelling, "sqlite3*");
        assert_eq!(&*one("struct tm *localtime(const time_t *t);").ret_spelling, "struct tm*");
        let c = one("int connect(int s, const struct sockaddr *addr, unsigned int n);");
        assert_eq!(c.params.iter().map(|p| &*p.spelling).collect::<Vec<_>>(), vec!["int", "const struct sockaddr*", "unsigned int"]);
        let h = one("enum e *h(union u **x);");
        assert_eq!((&*h.ret_spelling, &*h.params[0].spelling), ("enum e*", "union u**"));
        assert_eq!(&*one("char const* getenv(const char* name);").ret_spelling, "char const*");
        assert_eq!(&*one("unsigned long long f(void);").ret_spelling, "unsigned long long");
    }
}
