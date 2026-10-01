// Small backtracking regex engine with capture groups — no external crates.
//
// Supported syntax (per DOCS.md std.regex):
//   literals, `.`, `\d \D \w \W \s \S`, `\b \B`, `\n \t \r`, escapes
//   character classes `[a-z0-9_]`, negated `[^...]`, `\d`/`\w`/`\s` inside
//   anchors `^` `$`
//   groups `(...)`, non-capturing `(?:...)`, named `(?P<name>...)` / `(?<name>...)`
//   alternation `a|b`
//   quantifiers `* + ?` and `{n} {n,} {n,m}`, each with lazy `?` variant
//
// Matching is by chars (unicode scalar values); indices in MatchResult are
// char indices, not byte offsets.

use std::cell::Cell;

#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool), // items, negated
    Digit(bool),                 // negated flag
    Word(bool),
    Space(bool),
    Start,
    End,
    WordBoundary(bool),
    Group(usize, Box<Node>), // 1-based capture index
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Node>, usize, Option<usize>, bool), // min, max, greedy
}

#[derive(Debug, Clone)]
enum ClassItem {
    Ch(char),
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

pub struct Regex {
    root: Node,
    /// Names for capture groups 1..=n_groups (None = unnamed).
    pub group_names: Vec<Option<String>>,
    pub n_groups: usize,
}

pub struct MatchResult {
    pub start: usize,
    pub end: usize,
    /// Char ranges for groups 1..=n_groups.
    pub groups: Vec<Option<(usize, usize)>>,
}

pub fn compile(pattern: &str) -> Result<Regex, String> {
    let mut p = Parser {
        chars: pattern.chars().collect(),
        pos: 0,
        group_idx: 0,
        names: Vec::new(),
    };
    let root = p.parse_alt()?;
    if p.pos != p.chars.len() {
        return Err(format!("regex: unexpected '{}' at position {}", p.chars[p.pos], p.pos));
    }
    Ok(Regex {
        root,
        n_groups: p.group_idx,
        group_names: p.names,
    })
}

// ── Parser ───────────────────────────────────────────────

struct Parser {
    chars: Vec<char>,
    pos: usize,
    group_idx: usize,
    names: Vec<Option<String>>,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_alt(&mut self) -> Result<Node, String> {
        let mut branches = vec![self.parse_seq()?];
        while self.eat('|') {
            branches.push(self.parse_seq()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().unwrap()
        } else {
            Node::Alt(branches)
        })
    }

    fn parse_seq(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.parse_atom()?;
            items.push(self.parse_quant(atom)?);
        }
        Ok(if items.len() == 1 {
            items.pop().unwrap()
        } else {
            Node::Seq(items)
        })
    }

    fn parse_atom(&mut self) -> Result<Node, String> {
        let c = self.peek().ok_or("regex: unexpected end of pattern")?;
        match c {
            '(' => {
                self.pos += 1;
                // (?:...) non-capturing, (?P<name>...) / (?<name>...) named
                let mut name: Option<String> = None;
                let mut capturing = true;
                if self.eat('?') {
                    if self.eat(':') {
                        capturing = false;
                    } else {
                        self.eat('P'); // (?P<...> and (?<...> both accepted
                        if !self.eat('<') {
                            return Err("regex: expected '<' after '(?P'".into());
                        }
                        let mut n = String::new();
                        while let Some(nc) = self.peek() {
                            if nc == '>' {
                                break;
                            }
                            n.push(nc);
                            self.pos += 1;
                        }
                        if !self.eat('>') {
                            return Err("regex: unterminated group name".into());
                        }
                        name = Some(n);
                    }
                }
                let inner = self.parse_alt()?;
                if !self.eat(')') {
                    return Err("regex: missing ')'".into());
                }
                if capturing {
                    self.group_idx += 1;
                    self.names.push(name);
                    Ok(Node::Group(self.group_idx, Box::new(inner)))
                } else {
                    Ok(inner)
                }
            }
            '[' => {
                self.pos += 1;
                self.parse_class()
            }
            '\\' => {
                self.pos += 1;
                let e = self.peek().ok_or("regex: trailing '\\'")?;
                self.pos += 1;
                Ok(match e {
                    'd' => Node::Digit(false),
                    'D' => Node::Digit(true),
                    'w' => Node::Word(false),
                    'W' => Node::Word(true),
                    's' => Node::Space(false),
                    'S' => Node::Space(true),
                    'b' => Node::WordBoundary(false),
                    'B' => Node::WordBoundary(true),
                    'n' => Node::Char('\n'),
                    't' => Node::Char('\t'),
                    'r' => Node::Char('\r'),
                    other => Node::Char(other),
                })
            }
            '.' => {
                self.pos += 1;
                Ok(Node::Any)
            }
            '^' => {
                self.pos += 1;
                Ok(Node::Start)
            }
            '$' => {
                self.pos += 1;
                Ok(Node::End)
            }
            '*' | '+' | '?' => Err(format!("regex: dangling quantifier '{}'", c)),
            other => {
                self.pos += 1;
                Ok(Node::Char(other))
            }
        }
    }

    fn parse_class(&mut self) -> Result<Node, String> {
        let negated = self.eat('^');
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek().ok_or("regex: unterminated character class")?;
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            first = false;
            self.pos += 1;
            let item_char = if c == '\\' {
                let e = self.peek().ok_or("regex: trailing '\\' in class")?;
                self.pos += 1;
                match e {
                    'd' => {
                        items.push(ClassItem::Digit(false));
                        continue;
                    }
                    'D' => {
                        items.push(ClassItem::Digit(true));
                        continue;
                    }
                    'w' => {
                        items.push(ClassItem::Word(false));
                        continue;
                    }
                    'W' => {
                        items.push(ClassItem::Word(true));
                        continue;
                    }
                    's' => {
                        items.push(ClassItem::Space(false));
                        continue;
                    }
                    'S' => {
                        items.push(ClassItem::Space(true));
                        continue;
                    }
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                }
            } else {
                c
            };
            // Range a-z (a '-' just before ']' is a literal dash)
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).copied() != Some(']')
                && self.chars.get(self.pos + 1).is_some()
            {
                self.pos += 1; // '-'
                let hi = self.peek().unwrap();
                self.pos += 1;
                let hi = if hi == '\\' {
                    let e = self.peek().ok_or("regex: trailing '\\' in class")?;
                    self.pos += 1;
                    e
                } else {
                    hi
                };
                items.push(ClassItem::Range(item_char, hi));
            } else {
                items.push(ClassItem::Ch(item_char));
            }
        }
        Ok(Node::Class(items, negated))
    }

    fn parse_quant(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') => {
                // {n} {n,} {n,m} — if it doesn't parse as a counted quantifier,
                // treat '{' as a literal (common in practice).
                let save = self.pos;
                self.pos += 1;
                let mut n = String::new();
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    n.push(self.peek().unwrap());
                    self.pos += 1;
                }
                if n.is_empty() {
                    self.pos = save;
                    return Ok(atom);
                }
                let lo: usize = n.parse().map_err(|_| "regex: bad quantifier")?;
                let hi = if self.eat(',') {
                    let mut m = String::new();
                    while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                        m.push(self.peek().unwrap());
                        self.pos += 1;
                    }
                    if m.is_empty() {
                        None
                    } else {
                        Some(m.parse::<usize>().map_err(|_| "regex: bad quantifier")?)
                    }
                } else {
                    Some(lo)
                };
                if !self.eat('}') {
                    self.pos = save;
                    return Ok(atom);
                }
                if lo > 10_000 || hi.map(|h| h > 10_000).unwrap_or(false) {
                    return Err("regex: quantifier bound too large".into());
                }
                (lo, hi)
            }
            _ => return Ok(atom),
        };
        let greedy = !self.eat('?');
        Ok(Node::Repeat(Box::new(atom), min, max, greedy))
    }
}

// ── Matcher ──────────────────────────────────────────────

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn class_match(items: &[ClassItem], c: char) -> bool {
    items.iter().any(|item| match item {
        ClassItem::Ch(x) => *x == c,
        ClassItem::Range(lo, hi) => *lo <= c && c <= *hi,
        ClassItem::Digit(neg) => c.is_ascii_digit() != *neg,
        ClassItem::Word(neg) => is_word(c) != *neg,
        ClassItem::Space(neg) => c.is_whitespace() != *neg,
    })
}

type Caps = Vec<Option<(usize, usize)>>;

struct Matcher<'t> {
    text: &'t [char],
    steps: Cell<usize>,
}

const MAX_STEPS: usize = 1_000_000;

impl<'t> Matcher<'t> {
    fn m(
        &self,
        node: &Node,
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> bool,
    ) -> bool {
        let steps = self.steps.get() + 1;
        self.steps.set(steps);
        if steps > MAX_STEPS {
            return false; // catastrophic backtracking guard
        }
        match node {
            Node::Char(c) => pos < self.text.len() && self.text[pos] == *c && k(pos + 1, caps),
            Node::Any => pos < self.text.len() && k(pos + 1, caps),
            Node::Class(items, neg) => {
                pos < self.text.len()
                    && (class_match(items, self.text[pos]) != *neg)
                    && k(pos + 1, caps)
            }
            Node::Digit(neg) => {
                pos < self.text.len() && (self.text[pos].is_ascii_digit() != *neg) && k(pos + 1, caps)
            }
            Node::Word(neg) => {
                pos < self.text.len() && (is_word(self.text[pos]) != *neg) && k(pos + 1, caps)
            }
            Node::Space(neg) => {
                pos < self.text.len()
                    && (self.text[pos].is_whitespace() != *neg)
                    && k(pos + 1, caps)
            }
            Node::Start => pos == 0 && k(pos, caps),
            Node::End => pos == self.text.len() && k(pos, caps),
            Node::WordBoundary(neg) => {
                let before = pos > 0 && is_word(self.text[pos - 1]);
                let after = pos < self.text.len() && is_word(self.text[pos]);
                ((before != after) != *neg) && k(pos, caps)
            }
            Node::Seq(nodes) => self.m_seq(nodes, pos, caps, k),
            Node::Alt(branches) => {
                for b in branches {
                    let saved = caps.clone();
                    if self.m(b, pos, caps, k) {
                        return true;
                    }
                    *caps = saved;
                }
                false
            }
            Node::Group(idx, inner) => {
                let idx = *idx;
                self.m(inner, pos, caps, &mut |end, caps| {
                    let saved = caps[idx - 1];
                    caps[idx - 1] = Some((pos, end));
                    if k(end, caps) {
                        true
                    } else {
                        caps[idx - 1] = saved;
                        false
                    }
                })
            }
            Node::Repeat(inner, min, max, greedy) => {
                self.rep(inner, *min, *max, *greedy, pos, 0, caps, k)
            }
        }
    }

    fn m_seq(
        &self,
        nodes: &[Node],
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> bool,
    ) -> bool {
        match nodes.split_first() {
            None => k(pos, caps),
            Some((first, rest)) => {
                self.m(first, pos, caps, &mut |p2, caps| self.m_seq(rest, p2, caps, k))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(
        &self,
        inner: &Node,
        min: usize,
        max: Option<usize>,
        greedy: bool,
        pos: usize,
        count: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> bool,
    ) -> bool {
        let can_more = max.map(|m| count < m).unwrap_or(true);
        if greedy {
            if can_more {
                let saved = caps.clone();
                if self.m(inner, pos, caps, &mut |p2, caps| {
                    // zero-width repetition guard
                    p2 != pos && self.rep(inner, min, max, greedy, p2, count + 1, caps, k)
                }) {
                    return true;
                }
                *caps = saved;
            }
            count >= min && k(pos, caps)
        } else {
            if count >= min {
                let saved = caps.clone();
                if k(pos, caps) {
                    return true;
                }
                *caps = saved;
            }
            can_more
                && self.m(inner, pos, caps, &mut |p2, caps| {
                    p2 != pos && self.rep(inner, min, max, greedy, p2, count + 1, caps, k)
                })
        }
    }
}

impl Regex {
    /// Find the leftmost match at or after `from` (char index).
    pub fn search(&self, chars: &[char], from: usize) -> Option<MatchResult> {
        let matcher = Matcher {
            text: chars,
            steps: Cell::new(0),
        };
        for start in from..=chars.len() {
            let mut caps: Caps = vec![None; self.n_groups];
            let mut end_found: Option<usize> = None;
            let matched = matcher.m(&self.root, start, &mut caps, &mut |end, _caps| {
                end_found = Some(end);
                true
            });
            if matched {
                return Some(MatchResult {
                    start,
                    end: end_found.unwrap_or(start),
                    groups: caps,
                });
            }
            if matcher.steps.get() > MAX_STEPS {
                return None;
            }
        }
        None
    }

    /// True when the pattern matches anywhere in the text.
    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        self.search(&chars, 0).is_some()
    }

    /// All non-overlapping matches (full-match strings).
    pub fn find_iter(&self, chars: &[char]) -> Vec<MatchResult> {
        let mut out = Vec::new();
        let mut pos = 0;
        while pos <= chars.len() {
            match self.search(chars, pos) {
                Some(m) => {
                    let next = if m.end > m.start { m.end } else { m.end + 1 };
                    out.push(m);
                    pos = next;
                }
                None => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(pat: &str, text: &str) -> Option<String> {
        let re = compile(pat).unwrap();
        let chars: Vec<char> = text.chars().collect();
        re.search(&chars, 0)
            .map(|m| chars[m.start..m.end].iter().collect())
    }

    #[test]
    fn test_literal_and_classes() {
        assert_eq!(find(r"\w+", "hello world"), Some("hello".into()));
        assert_eq!(find(r"[a-z]\d", "a1 b2"), Some("a1".into()));
        assert_eq!(find(r"[^aeiou]+", "aeiox"), Some("x".into()));
    }

    #[test]
    fn test_groups_and_counted() {
        let re = compile(r"(\d{4})-(\d{2})-(\d{2})").unwrap();
        let chars: Vec<char> = "on 2025-04-11 ok".chars().collect();
        let m = re.search(&chars, 0).unwrap();
        let g = |i: usize| -> String {
            let (s, e) = m.groups[i].unwrap();
            chars[s..e].iter().collect()
        };
        assert_eq!(g(0), "2025");
        assert_eq!(g(1), "04");
        assert_eq!(g(2), "11");
    }

    #[test]
    fn test_alternation_and_anchors() {
        assert_eq!(find(r"^(cat|dog)$", "dog"), Some("dog".into()));
        assert_eq!(find(r"^(cat|dog)$", "dogs"), None);
        assert_eq!(find(r"colou?r", "my color!"), Some("color".into()));
    }

    #[test]
    fn test_named_groups() {
        let re = compile(r"(?P<y>\d{4})-(?P<m>\d{2})").unwrap();
        assert_eq!(re.group_names[0].as_deref(), Some("y"));
        assert_eq!(re.group_names[1].as_deref(), Some("m"));
        assert!(re.is_match("2025-04"));
    }

    #[test]
    fn test_word_boundary_and_lazy() {
        assert_eq!(find(r"\bworld\b", "hello world!"), Some("world".into()));
        assert_eq!(find(r"<.+?>", "<a><b>"), Some("<a>".into()));
        assert_eq!(find(r"<.+>", "<a><b>"), Some("<a><b>".into()));
    }

    #[test]
    fn test_find_iter_and_star() {
        let re = compile(r"\d+").unwrap();
        let chars: Vec<char> = "a1 b22 c333".chars().collect();
        let all: Vec<String> = re
            .find_iter(&chars)
            .into_iter()
            .map(|m| chars[m.start..m.end].iter().collect())
            .collect();
        assert_eq!(all, vec!["1", "22", "333"]);
        assert_eq!(find(r"ab*c", "ac"), Some("ac".into()));
    }
}
