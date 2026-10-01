use crate::token::{Token, TokenKind};

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    last_token: Option<TokenKind>,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        // Skip a leading UTF-8 BOM (U+FEFF), which Windows editors/PowerShell add.
        let source = source.strip_prefix('\u{FEFF}').unwrap_or(source);
        Self {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            last_token: None,
        }
    }

    /// Language tags that open a raw embedded block after `@`.
    const ENGINE_TAGS: &'static [&'static str] = &[
        "py", "python", "js", "javascript", "lua", "go", "c", "cpp", "cxx",
        "ts", "typescript", "java", "cs", "csharp", "rust", "rs", "bash",
        "sh", "ps", "powershell", "os", "shell", "asm", "assembly", "mal",
        "malbolge", "sql", "wasm",
    ];

    /// After a consumed `@`, try to lex `tag [name] { raw... }` as one token.
    /// Restores position and returns None when it isn't an engine block, so
    /// decorators (`@memo`, `@fixed`) keep working.
    fn try_lex_embedded_block(&mut self, line: usize, col: usize) -> Option<Token> {
        let save_pos = self.pos;
        let save_line = self.line;
        let save_col = self.col;

        let read_word = |lexer: &mut Self| -> String {
            let mut w = String::new();
            while lexer.pos < lexer.source.len()
                && (lexer.peek().is_alphanumeric() || lexer.peek() == '_')
            {
                w.push(lexer.advance());
            }
            w
        };

        let tag = read_word(self);
        if !Self::ENGINE_TAGS.contains(&tag.as_str()) {
            self.pos = save_pos;
            self.line = save_line;
            self.col = save_col;
            return None;
        }
        // Optional label, then `{` (spaces/tabs allowed; no newline before `{`)
        while self.peek() == ' ' || self.peek() == '\t' {
            self.advance();
        }
        let mut label = String::new();
        if self.peek().is_alphabetic() || self.peek() == '_' {
            label = read_word(self);
            while self.peek() == ' ' || self.peek() == '\t' {
                self.advance();
            }
        }
        if self.peek() != '{' {
            self.pos = save_pos;
            self.line = save_line;
            self.col = save_col;
            return None;
        }
        self.advance(); // consume '{'

        // Raw capture until the matching close brace (brace counting; braces
        // inside the foreign code must balance, which they do in practice).
        let mut depth = 1usize;
        let mut code = String::new();
        while self.pos < self.source.len() {
            let ch = self.advance();
            match ch {
                '{' => {
                    depth += 1;
                    code.push(ch);
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    code.push(ch);
                }
                _ => code.push(ch),
            }
        }
        let label = if label.is_empty() { None } else { Some(label) };
        Some(Token::new(TokenKind::EmbeddedBlock(tag, label, code), line, col))
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            self.last_token = Some(tok.kind.clone());
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    /// Returns true if the last token could end an expression (meaning `//` is division)
    fn last_token_is_value(&self) -> bool {
        match &self.last_token {
            Some(TokenKind::Int(_)) | Some(TokenKind::Float(_)) | Some(TokenKind::Str(_))
            | Some(TokenKind::Bool(_)) | Some(TokenKind::Null)
            | Some(TokenKind::Ident(_)) | Some(TokenKind::RParen) | Some(TokenKind::RBracket)
            | Some(TokenKind::RBrace) | Some(TokenKind::PlusPlus) | Some(TokenKind::MinusMinus) => true,
            _ => false,
        }
    }

    /// Disambiguates `//` floor-division from a trailing line comment.
    /// `//` is floor division only when a value precedes it AND a numeric or
    /// parenthesized operand follows (`10 // 3`, `x // 2`, `len() // (a+b)`).
    /// Anything else after `//` (words, prose, punctuation) is a comment, so
    /// trailing comments like `foo() // note` lex correctly. Returns true when
    /// the characters after the two slashes begin a floor-division operand.
    fn floordiv_operand_follows(&self) -> bool {
        let mut i = self.pos + 2; // skip the two '/' characters
        while i < self.source.len() && (self.source[i] == ' ' || self.source[i] == '\t') {
            i += 1;
        }
        if i >= self.source.len() {
            return false;
        }
        // A parenthesized operand always begins a division RHS.
        if self.source[i] == '(' {
            return true;
        }
        // Otherwise require a numeric operand (`10`, `-2`, `3.5`) that is
        // *followed by a terminator* — end of line, a closing bracket, comma,
        // or another operator. This distinguishes `x // 2` (division) from an
        // alignment comment like `// 4096  — computed` that merely starts with
        // a digit. Identifier RHS (`a // b`) is treated as a comment (rare in code).
        if self.source[i] == '-' || self.source[i] == '+' {
            i += 1;
        }
        let digit_start = i;
        while i < self.source.len() && self.source[i].is_ascii_digit() {
            i += 1;
        }
        if i < self.source.len() && self.source[i] == '.' {
            i += 1;
            while i < self.source.len() && self.source[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i == digit_start {
            return false; // no digits consumed → not a numeric operand
        }
        // Skip trailing spaces, then inspect the terminator.
        while i < self.source.len() && (self.source[i] == ' ' || self.source[i] == '\t') {
            i += 1;
        }
        if i >= self.source.len() {
            return true; // end of line/input
        }
        matches!(
            self.source[i],
            '\n' | '\r' | ')' | ']' | '}' | ',' | ';'
                | '+' | '-' | '*' | '/' | '%' | '<' | '>' | '=' | '&' | '|' | '^'
        )
    }

    fn peek(&self) -> char {
        if self.pos < self.source.len() {
            self.source[self.pos]
        } else {
            '\0'
        }
    }

    fn peek_next(&self) -> char {
        if self.pos + 1 < self.source.len() {
            self.source[self.pos + 1]
        } else {
            '\0'
        }
    }

    fn peek_n(&self, offset: usize) -> char {
        if self.pos + offset < self.source.len() {
            self.source[self.pos + offset]
        } else {
            '\0'
        }
    }

    fn advance(&mut self) -> char {
        let ch = self.peek();
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        ch
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.source.len() {
            match self.peek() {
                ' ' | '\t' | '\r' => {
                    self.advance();
                }
                _ => break,
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while self.pos < self.source.len() && self.peek() != '\n' {
            self.advance();
        }
    }

    fn read_doc_line_comment(&mut self) -> String {
        self.advance();
        self.advance();
        self.advance();
        if self.peek() == ' ' {
            self.advance();
        }

        let mut content = String::new();
        while self.pos < self.source.len() && self.peek() != '\n' {
            content.push(self.advance());
        }
        content.trim_end().to_string()
    }

    fn skip_block_comment(&mut self) -> Result<(), String> {
        let start_line = self.line;
        // skip the /*
        self.advance();
        self.advance();
        let mut depth = 1;
        while self.pos < self.source.len() && depth > 0 {
            if self.peek() == '/' && self.peek_next() == '*' {
                self.advance();
                self.advance();
                depth += 1;
            } else if self.peek() == '*' && self.peek_next() == '/' {
                self.advance();
                self.advance();
                depth -= 1;
            } else {
                self.advance();
            }
        }
        if depth > 0 {
            return Err(format!(
                "[line {}] Error: Unterminated block comment starting at line {}",
                self.line, start_line
            ));
        }
        Ok(())
    }

    fn read_doc_block_comment(&mut self) -> Result<String, String> {
        let start_line = self.line;
        self.advance();
        self.advance();
        self.advance();

        let mut content = String::new();
        while self.pos < self.source.len() {
            if self.peek() == '*' && self.peek_next() == '/' {
                self.advance();
                self.advance();
                let lines: Vec<String> = content
                    .lines()
                    .map(|line| {
                        let trimmed = line.trim();
                        if let Some(rest) = trimmed.strip_prefix('*') {
                            rest.trim_start().to_string()
                        } else {
                            trimmed.to_string()
                        }
                    })
                    .collect();
                return Ok(lines.join("\n").trim().to_string());
            }
            content.push(self.advance());
        }

        Err(format!(
            "[line {}] Error: Unterminated doc block comment starting at line {}",
            self.line, start_line
        ))
    }

    fn read_string(&mut self, quote: char) -> Result<String, String> {
        let start_line = self.line;
        let mut s = String::new();
        // skip opening quote
        self.advance();
        // Check for triple-quoted string: """...""" or '''...'''
        if self.peek() == quote && self.pos + 1 < self.source.len() && self.source[self.pos + 1] == quote {
            self.advance(); // second quote
            self.advance(); // third quote
            // Read until closing triple quote
            while self.pos + 2 < self.source.len() {
                if self.peek() == quote && self.source[self.pos + 1] == quote && self.source[self.pos + 2] == quote {
                    self.advance(); self.advance(); self.advance();
                    return Ok(s);
                }
                s.push(self.advance());
            }
            return Err(format!("[line {}] Error: Unterminated triple-quoted string starting at line {}", self.line, start_line));
        }
        while self.pos < self.source.len() && self.peek() != quote {
            if self.peek() == '\\' {
                self.advance();
                match self.peek() {
                    'n' => { s.push('\n'); self.advance(); }
                    't' => { s.push('\t'); self.advance(); }
                    'r' => { s.push('\r'); self.advance(); }
                    '\\' => { s.push('\\'); self.advance(); }
                    '\'' => { s.push('\''); self.advance(); }
                    '"' => { s.push('"'); self.advance(); }
                    '0' => { s.push('\0'); self.advance(); }
                    '$' => { s.push('$'); self.advance(); }
                    // Hex byte escape: \xNN
                    'x' => {
                        self.advance(); // skip 'x'
                        let mut hex = String::new();
                        for _ in 0..2 {
                            if self.peek().is_ascii_hexdigit() {
                                hex.push(self.advance());
                            }
                        }
                        if let Ok(n) = u32::from_str_radix(&hex, 16) {
                            if let Some(c) = char::from_u32(n) {
                                s.push(c);
                            }
                        }
                    }
                    // Unicode escape: \u{1F600} or \uXXXX
                    'u' => {
                        self.advance(); // skip 'u'
                        let mut hex = String::new();
                        if self.peek() == '{' {
                            self.advance(); // {
                            while self.peek() != '}' && self.pos < self.source.len() {
                                hex.push(self.advance());
                            }
                            if self.peek() == '}' { self.advance(); }
                        } else {
                            for _ in 0..4 {
                                if self.peek().is_ascii_hexdigit() {
                                    hex.push(self.advance());
                                }
                            }
                        }
                        if let Ok(n) = u32::from_str_radix(&hex, 16) {
                            if let Some(c) = char::from_u32(n) {
                                s.push(c);
                            }
                        }
                    }
                    // Unknown escape: keep the backslash and character literally
                    // (forgiving — useful for regex/format patterns in plain strings).
                    c => {
                        s.push('\\');
                        s.push(c);
                        self.advance();
                    }
                }
            } else {
                s.push(self.advance());
            }
        }
        if self.pos >= self.source.len() {
            return Err(format!(
                "[line {}] Error: Unterminated string starting at line {}",
                self.line, start_line
            ));
        }
        // skip closing quote
        self.advance();
        Ok(s)
    }

    fn read_raw_string(&mut self, quote: char) -> Result<String, String> {
        let start_line = self.line;
        let mut s = String::new();
        self.advance(); // skip opening quote
        // Triple-quoted raw string: r"""...""" — no escape processing at all.
        if self.peek() == quote && self.pos + 1 < self.source.len() && self.source[self.pos + 1] == quote {
            self.advance(); // second quote
            self.advance(); // third quote
            while self.pos + 2 < self.source.len() {
                if self.peek() == quote && self.source[self.pos + 1] == quote && self.source[self.pos + 2] == quote {
                    self.advance(); self.advance(); self.advance();
                    return Ok(s);
                }
                s.push(self.advance());
            }
            // allow the final chars before EOF
            while self.pos < self.source.len() {
                s.push(self.advance());
            }
            return Err(format!(
                "[line {}] Error: Unterminated triple-quoted raw string starting at line {}",
                self.line, start_line
            ));
        }
        while self.pos < self.source.len() && self.peek() != quote {
            s.push(self.advance());
        }
        if self.pos >= self.source.len() {
            return Err(format!(
                "[line {}] Error: Unterminated raw string starting at line {}",
                self.line, start_line
            ));
        }
        self.advance(); // skip closing quote
        Ok(s)
    }

    fn read_number(&mut self) -> Token {
        let start_col = self.col;
        let start_line = self.line;
        let mut num_str = String::new();
        let mut is_float = false;

        // Handle 0x, 0b, 0o prefixes
        if self.peek() == '0' && (self.peek_next() == 'x' || self.peek_next() == 'b' || self.peek_next() == 'o') {
            num_str.push(self.advance()); // '0'
            num_str.push(self.advance()); // prefix letter
            while self.pos < self.source.len()
                && (self.peek().is_ascii_alphanumeric() || self.peek() == '_')
            {
                let c = self.advance();
                if c != '_' {
                    num_str.push(c);
                }
            }
            // Parse based on prefix
            let val = match num_str.chars().nth(1).unwrap() {
                'x' => i64::from_str_radix(&num_str[2..], 16),
                'b' => i64::from_str_radix(&num_str[2..], 2),
                'o' => i64::from_str_radix(&num_str[2..], 8),
                _ => unreachable!(),
            };
            return Token::new(
                TokenKind::Int(val.unwrap_or(0)),
                start_line,
                start_col,
            );
        }

        while self.pos < self.source.len()
            && (self.peek().is_ascii_digit() || self.peek() == '_' || self.peek() == '.')
        {
            if self.peek() == '.' {
                // Check it's not `..` (range)
                if self.peek_next() == '.' {
                    break;
                }
                if is_float {
                    break; // second dot — stop
                }
                is_float = true;
                num_str.push(self.advance());
            } else if self.peek() == '_' {
                self.advance(); // skip underscores in numbers (1_000_000)
            } else {
                num_str.push(self.advance());
            }
        }

        // Scientific notation: 1e10, 1.5e-3
        if self.pos < self.source.len() && (self.peek() == 'e' || self.peek() == 'E') {
            is_float = true;
            num_str.push(self.advance());
            if self.pos < self.source.len() && (self.peek() == '+' || self.peek() == '-') {
                num_str.push(self.advance());
            }
            while self.pos < self.source.len() && self.peek().is_ascii_digit() {
                num_str.push(self.advance());
            }
        }

        if is_float {
            Token::new(
                TokenKind::Float(num_str.parse::<f64>().unwrap_or(0.0)),
                start_line,
                start_col,
            )
        } else {
            match num_str.parse::<i64>() {
                Ok(v) => Token::new(TokenKind::Int(v), start_line, start_col),
                // Overflowed i64 — keep the digits for arbitrary-precision parsing.
                Err(_) => Token::new(TokenKind::BigIntLit(num_str), start_line, start_col),
            }
        }
    }

    fn read_identifier(&mut self) -> Token {
        let start_col = self.col;
        let start_line = self.line;
        let mut ident = String::new();

        while self.pos < self.source.len()
            && (self.peek().is_alphanumeric() || self.peek() == '_')
        {
            ident.push(self.advance());
        }

        let kind = match ident.as_str() {
            "true" => TokenKind::Bool(true),
            "false" => TokenKind::Bool(false),
            "null" => TokenKind::Null,
            "let" => TokenKind::Let,
            // `var` is an alias for `let` (bindings are mutable either way).
            "var" => TokenKind::Let,
            "const" => TokenKind::Const,
            "func" => TokenKind::Func,
            "return" => TokenKind::Return,
            "if" => TokenKind::If,
            "elif" => TokenKind::Elif,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "match" => TokenKind::Match,
            "case" => TokenKind::Case,
            "default" => TokenKind::Default,
            "import" => TokenKind::Import,
            "from" => TokenKind::From,
            "as" => TokenKind::As,
            "pub" => TokenKind::Pub,
            "private" => TokenKind::Private,
            "internal" => TokenKind::Internal,
            "class" => TokenKind::Class,
            "new" => TokenKind::New,
            "extends" => TokenKind::Extends,
            "extend" => TokenKind::Extend,
            "super" => TokenKind::Super,
            "self" => TokenKind::Self_,
            "struct" => TokenKind::Struct,
            "enum" => TokenKind::Enum,
            "trait" => TokenKind::Trait,
            "impl" => TokenKind::Impl,
            "dyn" => TokenKind::Dyn,
            "mod" => TokenKind::Mod,
            "async" => TokenKind::Async,
            "await" => TokenKind::Await,
            "try" => TokenKind::Try,
            "catch" => TokenKind::Catch,
            "finally" => TokenKind::Finally,
            "throw" => TokenKind::Throw,
            "defer" => TokenKind::Defer,
            "yield" => TokenKind::Yield,
            "lambda" => TokenKind::Lambda,
            "move" => TokenKind::Move,
            "ref" => TokenKind::Ref,
            "mut" => TokenKind::Mut,
            "unsafe" => TokenKind::Unsafe,
            "extern" => TokenKind::Extern,
            "type" => TokenKind::Type,
            "where" => TokenKind::Where,
            "pure" => TokenKind::Pure,
            "macro" => TokenKind::Macro,
            "comptime" => TokenKind::Comptime,
            "band" => {
                if self.pos < self.source.len() && self.peek() == '=' {
                    self.advance();
                    TokenKind::BitAndAssign
                } else {
                    TokenKind::BitAnd
                }
            }
            "bor" => {
                if self.pos < self.source.len() && self.peek() == '=' {
                    self.advance();
                    TokenKind::BitOrAssign
                } else {
                    TokenKind::BitOr
                }
            }
            "bxor" => {
                if self.pos < self.source.len() && self.peek() == '=' {
                    self.advance();
                    TokenKind::BitXorAssign
                } else {
                    TokenKind::BitXor
                }
            }
            "bnot" => TokenKind::BitNot,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => {
                // Check for "not in" compound keyword
                let saved_pos = self.pos;
                let saved_line = self.line;
                let saved_col = self.col;
                self.skip_whitespace();
                if self.pos + 2 <= self.source.len() {
                    let ahead: String = self.source[self.pos..self.source.len()].iter().take(2).collect();
                    if ahead == "in" && (self.pos + 2 >= self.source.len() || !self.source[self.pos + 2].is_alphanumeric()) {
                        self.pos += 2;
                        self.col += 2;
                        TokenKind::NotIn
                    } else {
                        self.pos = saved_pos;
                        self.line = saved_line;
                        self.col = saved_col;
                        TokenKind::Not
                    }
                } else {
                    self.pos = saved_pos;
                    self.line = saved_line;
                    self.col = saved_col;
                    TokenKind::Not
                }
            }
            "is" => TokenKind::Is,
            "actor" => TokenKind::Actor,
            "agent" => TokenKind::Agent,
            "sealed" => TokenKind::Sealed,
            "volatile" => TokenKind::Volatile,
            "label" => TokenKind::Label,
            "goto" => TokenKind::Goto,
            "enable" => TokenKind::Enable,
            "cimport" => TokenKind::Cimport,
            "do" => TokenKind::Do,
            "test" => TokenKind::TestBlock,
            "bench" => TokenKind::BenchBlock,
            "typeof" => TokenKind::TypeOf,
            "lazy" => TokenKind::Lazy,
            "using" => TokenKind::Using,
            "static_assert" => TokenKind::StaticAssert,
            "newtype" => TokenKind::Newtype,
            "cstruct" => TokenKind::CStruct,
            "never" => TokenKind::Never,
            "bitfield" => TokenKind::Bitfield,
            "isolate" => TokenKind::Isolate,
            _ => TokenKind::Ident(ident),
        };

        Token::new(kind, start_line, start_col)
    }

    fn next_token(&mut self) -> Result<Token, String> {
        self.skip_whitespace();

        if self.pos >= self.source.len() {
            return Ok(Token::new(TokenKind::Eof, self.line, self.col));
        }

        let ch = self.peek();
        let line = self.line;
        let col = self.col;

        // Newline
        if ch == '\n' {
            self.advance();
            return Ok(Token::new(TokenKind::Newline, line, col));
        }

        // Comments (only if // is NOT after an expression-ending token)
        if ch == '/' && self.peek_next() == '/' && self.peek_n(2) == '/' && !self.last_token_is_value() {
            let doc = self.read_doc_line_comment();
            return Ok(Token::new(TokenKind::DocComment(doc), line, col));
        }
        if ch == '/' && self.peek_next() == '*' && self.peek_n(2) == '*' {
            let doc = self.read_doc_block_comment()?;
            return Ok(Token::new(TokenKind::DocComment(doc), line, col));
        }
        // `//` is a line comment unless it is a genuine floor-division operator
        // (`10 // 3`) or the compound-assign `//=`. See floordiv_operand_follows.
        if ch == '/'
            && self.peek_next() == '/'
            && self.peek_n(2) != '='
            && !(self.last_token_is_value() && self.floordiv_operand_follows())
        {
            self.skip_line_comment();
            return self.next_token();
        }
        if ch == '/' && self.peek_next() == '*' {
            self.skip_block_comment()?;
            return self.next_token();
        }

        // Numbers
        if ch.is_ascii_digit() {
            return Ok(self.read_number());
        }

        // Identifiers and keywords
        if ch.is_alphabetic() || ch == '_' {
            // Check for f-string: f"..."
            if ch == 'f' && (self.peek_next() == '"' || self.peek_next() == '\'') {
                self.advance(); // skip 'f'
                let quote = self.peek();
                let s = self.read_string(quote)?;
                return Ok(Token::new(TokenKind::FStr(s), line, col));
            }
            // Raw string: r"..." — no escape processing
            if ch == 'r' && (self.peek_next() == '"' || self.peek_next() == '\'') {
                self.advance(); // skip 'r'
                let quote = self.peek();
                let s = self.read_raw_string(quote)?;
                return Ok(Token::new(TokenKind::Str(s), line, col));
            }
            // Byte string: b"..." — yields a byte array
            if ch == 'b' && (self.peek_next() == '"' || self.peek_next() == '\'') {
                self.advance(); // skip 'b'
                let quote = self.peek();
                let s = self.read_string(quote)?;
                return Ok(Token::new(TokenKind::ByteStr(s.into_bytes()), line, col));
            }
            return Ok(self.read_identifier());
        }

        // Strings
        if ch == '"' || ch == '\'' {
            let s = self.read_string(ch)?;
            return Ok(Token::new(TokenKind::Str(s), line, col));
        }

        // Multi-char operators and single-char tokens
        self.advance();
        match ch {
            '+' => {
                if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::PlusAssign, line, col))
                } else if self.peek() == '+' {
                    self.advance();
                    Ok(Token::new(TokenKind::PlusPlus, line, col))
                } else {
                    Ok(Token::new(TokenKind::Plus, line, col))
                }
            }
            '-' => {
                if self.peek() == '>' {
                    self.advance();
                    Ok(Token::new(TokenKind::Arrow, line, col))
                } else if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::MinusAssign, line, col))
                } else if self.peek() == '-' {
                    self.advance();
                    Ok(Token::new(TokenKind::MinusMinus, line, col))
                } else {
                    Ok(Token::new(TokenKind::Minus, line, col))
                }
            }
            '*' => {
                if self.peek() == '*' {
                    self.advance();
                    if self.peek() == '=' {
                        self.advance();
                        Ok(Token::new(TokenKind::DoubleStarAssign, line, col))
                    } else {
                        Ok(Token::new(TokenKind::DoubleStar, line, col))
                    }
                } else if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::StarAssign, line, col))
                } else {
                    Ok(Token::new(TokenKind::Star, line, col))
                }
            }
            '/' => {
                if self.peek() == '/' {
                    self.advance();
                    if self.peek() == '=' {
                        self.advance();
                        Ok(Token::new(TokenKind::IntDivAssign, line, col))
                    } else {
                        Ok(Token::new(TokenKind::DoubleSlash, line, col))
                    }
                } else if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::SlashAssign, line, col))
                } else {
                    Ok(Token::new(TokenKind::Slash, line, col))
                }
            }
            '%' => {
                if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::PercentAssign, line, col))
                } else {
                    Ok(Token::new(TokenKind::Percent, line, col))
                }
            }
            '=' => {
                if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::Eq, line, col))
                } else if self.peek() == '>' {
                    self.advance();
                    Ok(Token::new(TokenKind::FatArrow, line, col))
                } else {
                    Ok(Token::new(TokenKind::Assign, line, col))
                }
            }
            '!' => {
                if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::NotEq, line, col))
                } else {
                    Ok(Token::new(TokenKind::Not, line, col))
                }
            }
            '<' => {
                if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::LtEq, line, col))
                } else if self.peek() == '<' {
                    self.advance();
                    if self.peek() == '=' {
                        self.advance();
                        Ok(Token::new(TokenKind::ShlAssign, line, col))
                    } else {
                        Ok(Token::new(TokenKind::Shl, line, col))
                    }
                } else {
                    Ok(Token::new(TokenKind::Lt, line, col))
                }
            }
            '>' => {
                if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::GtEq, line, col))
                } else if self.peek() == '>' {
                    self.advance();
                    if self.peek() == '=' {
                        self.advance();
                        Ok(Token::new(TokenKind::ShrAssign, line, col))
                    } else {
                        Ok(Token::new(TokenKind::Shr, line, col))
                    }
                } else {
                    Ok(Token::new(TokenKind::Gt, line, col))
                }
            }
            '&' => {
                if self.peek() == '&' {
                    self.advance();
                    Ok(Token::new(TokenKind::And, line, col)) // && logical AND
                } else if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::BitAndAssign, line, col))
                } else {
                    Ok(Token::new(TokenKind::BitAnd, line, col))
                }
            }
            '|' => {
                if self.peek() == '|' {
                    self.advance();
                    Ok(Token::new(TokenKind::Or, line, col)) // || logical OR
                } else if self.peek() == '>' {
                    self.advance();
                    Ok(Token::new(TokenKind::Pipe, line, col))
                } else if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::BitOrAssign, line, col))
                } else {
                    Ok(Token::new(TokenKind::BitOr, line, col))
                }
            }
            '^' => {
                if self.peek() == '=' {
                    self.advance();
                    Ok(Token::new(TokenKind::BitXorAssign, line, col))
                } else {
                    Ok(Token::new(TokenKind::BitXor, line, col))
                }
            }
            '~' => Ok(Token::new(TokenKind::BitNot, line, col)),
            '(' => Ok(Token::new(TokenKind::LParen, line, col)),
            ')' => Ok(Token::new(TokenKind::RParen, line, col)),
            '{' => Ok(Token::new(TokenKind::LBrace, line, col)),
            '}' => Ok(Token::new(TokenKind::RBrace, line, col)),
            '[' => Ok(Token::new(TokenKind::LBracket, line, col)),
            ']' => Ok(Token::new(TokenKind::RBracket, line, col)),
            ',' => Ok(Token::new(TokenKind::Comma, line, col)),
            '.' => {
                if self.peek() == '.' {
                    self.advance();
                    if self.peek() == '=' {
                        self.advance();
                        Ok(Token::new(TokenKind::DotDotEq, line, col))
                    } else if self.peek() == '.' {
                        self.advance();
                        Ok(Token::new(TokenKind::Ellipsis, line, col))
                    } else {
                        Ok(Token::new(TokenKind::DotDot, line, col))
                    }
                } else {
                    Ok(Token::new(TokenKind::Dot, line, col))
                }
            }
            ':' => {
                if self.peek() == ':' {
                    self.advance();
                    Ok(Token::new(TokenKind::ColonColon, line, col))
                } else {
                    Ok(Token::new(TokenKind::Colon, line, col))
                }
            }
            ';' => Ok(Token::new(TokenKind::Semicolon, line, col)),
            '?' => {
                if self.peek() == '.' {
                    self.advance();
                    Ok(Token::new(TokenKind::QuestionDot, line, col))
                } else if self.peek() == '?' {
                    self.advance();
                    Ok(Token::new(TokenKind::QuestionQuestion, line, col))
                } else {
                    Ok(Token::new(TokenKind::Question, line, col))
                }
            }
            '@' => {
                // Embedded engine block: @py { ... } / @py name { ... }.
                // Captured RAW at lex time so the foreign source survives
                // untouched (foreign code is not V2-tokenizable).
                if let Some(tok) = self.try_lex_embedded_block(line, col) {
                    return Ok(tok);
                }
                Ok(Token::new(TokenKind::At, line, col))
            }
            '#' => Ok(Token::new(TokenKind::Hash, line, col)),
            _ => Err(format!(
                "[line {}] Error: Unexpected character '{}'",
                line, ch
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_tokens() {
        let mut lexer = Lexer::new("let x = 42");
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Let));
        assert!(matches!(tokens[1].kind, TokenKind::Ident(ref s) if s == "x"));
        assert!(matches!(tokens[2].kind, TokenKind::Assign));
        assert!(matches!(tokens[3].kind, TokenKind::Int(42)));
    }

    #[test]
    fn test_string_literal() {
        let mut lexer = Lexer::new(r#""hello world""#);
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Str(ref s) if s == "hello world"));
    }

    #[test]
    fn test_fstring() {
        let mut lexer = Lexer::new(r#"f"hello ${name}""#);
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::FStr(ref s) if s == "hello ${name}"));
    }

    #[test]
    fn test_operators() {
        let mut lexer = Lexer::new("+ - * / % ** == != <= >= |>");
        let tokens = lexer.tokenize().unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert!(matches!(kinds[0], TokenKind::Plus));
        assert!(matches!(kinds[1], TokenKind::Minus));
        assert!(matches!(kinds[2], TokenKind::Star));
        assert!(matches!(kinds[3], TokenKind::Slash));
        assert!(matches!(kinds[4], TokenKind::Percent));
        assert!(matches!(kinds[5], TokenKind::DoubleStar));
        assert!(matches!(kinds[6], TokenKind::Eq));
        assert!(matches!(kinds[7], TokenKind::NotEq));
        assert!(matches!(kinds[8], TokenKind::LtEq));
        assert!(matches!(kinds[9], TokenKind::GtEq));
        assert!(matches!(kinds[10], TokenKind::Pipe));
    }

    #[test]
    fn test_integer_division() {
        // // as integer division only works when not preceded by whitespace+slash
        // Use expression context: 10//3
        let mut lexer = Lexer::new("10 / 3");
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Int(10)));
        assert!(matches!(tokens[1].kind, TokenKind::Slash));
        assert!(matches!(tokens[2].kind, TokenKind::Int(3)));
    }

    #[test]
    fn test_number_with_underscores() {
        let mut lexer = Lexer::new("1_000_000");
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Int(1000000)));
    }

    #[test]
    fn test_hex_binary_octal() {
        let mut lexer = Lexer::new("0xFF 0b1010 0o77");
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Int(255)));
        assert!(matches!(tokens[1].kind, TokenKind::Int(10)));
        assert!(matches!(tokens[2].kind, TokenKind::Int(63)));
    }

    #[test]
    fn test_float_scientific() {
        let mut lexer = Lexer::new("3.14 1e10 2.5e-3");
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Float(f) if (f - 3.14).abs() < 1e-10));
        assert!(matches!(tokens[1].kind, TokenKind::Float(f) if (f - 1e10).abs() < 1.0));
        assert!(matches!(tokens[2].kind, TokenKind::Float(f) if (f - 2.5e-3).abs() < 1e-10));
    }

    #[test]
    fn test_keywords() {
        let mut lexer = Lexer::new("func if else while for return class struct enum trait impl");
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Func));
        assert!(matches!(tokens[1].kind, TokenKind::If));
        assert!(matches!(tokens[2].kind, TokenKind::Else));
        assert!(matches!(tokens[3].kind, TokenKind::While));
        assert!(matches!(tokens[4].kind, TokenKind::For));
        assert!(matches!(tokens[5].kind, TokenKind::Return));
        assert!(matches!(tokens[6].kind, TokenKind::Class));
        assert!(matches!(tokens[7].kind, TokenKind::Struct));
        assert!(matches!(tokens[8].kind, TokenKind::Enum));
        assert!(matches!(tokens[9].kind, TokenKind::Trait));
        assert!(matches!(tokens[10].kind, TokenKind::Impl));
    }

    #[test]
    fn test_comments() {
        // After an expression-ending token (Ident), // is treated as integer division
        // Line comments work at statement level (no preceding value token)
        let mut lexer = Lexer::new("// full line comment\ny /* block */ z");
        let tokens = lexer.tokenize().unwrap();
        let idents: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Ident(_)))
            .collect();
        assert_eq!(idents.len(), 2); // y, z
    }

    #[test]
    fn test_doc_line_comment_token() {
        let mut lexer = Lexer::new("/// Adds two integers\nfunc add() {}");
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::DocComment(ref s) if s == "Adds two integers"));
        assert!(matches!(tokens[2].kind, TokenKind::Func));
    }

    #[test]
    fn test_doc_block_comment_token() {
        let src = "/**\n * Represents a point.\n * @field x horizontal\n */\nstruct Point { x }";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::DocComment(ref s) if s.contains("Represents a point.") && s.contains("@field x horizontal")));
        assert!(matches!(tokens[2].kind, TokenKind::Struct));
    }

    #[test]
    fn test_ranges_and_ellipsis() {
        let mut lexer = Lexer::new("1..10 1..=10 ...args");
        let tokens = lexer.tokenize().unwrap();
        assert!(matches!(tokens[0].kind, TokenKind::Int(1)));
        assert!(matches!(tokens[1].kind, TokenKind::DotDot));
        assert!(matches!(tokens[2].kind, TokenKind::Int(10)));
        assert!(matches!(tokens[3].kind, TokenKind::Int(1)));
        assert!(matches!(tokens[4].kind, TokenKind::DotDotEq));
        assert!(matches!(tokens[5].kind, TokenKind::Int(10)));
        assert!(matches!(tokens[6].kind, TokenKind::Ellipsis));
        assert!(matches!(tokens[7].kind, TokenKind::Ident(ref s) if s == "args"));
    }
}
