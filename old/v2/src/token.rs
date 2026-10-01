/// Token types for the V2 language.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Int(i64),
    /// Integer literal too large for i64 — carried as its decimal digit string.
    BigIntLit(String),
    Float(f64),
    Str(String),
    FStr(String), // f"..." interpolation string (raw template)
    ByteStr(Vec<u8>), // b"..." byte string literal
    Bool(bool),
    Null,

    // Identifier
    Ident(String),
    DocComment(String),

    // Arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    DoubleStar, // **
    DoubleSlash, // //  (integer division)
    PlusPlus,   // ++
    MinusMinus, // --

    // Comparison
    Eq,         // ==
    NotEq,      // !=
    Lt,
    Gt,
    LtEq,
    GtEq,

    // Logical
    And,
    Or,
    Not,

    // Bitwise
    BitAnd,    // &
    BitOr,     // |
    BitXor,    // ^
    BitNot,    // ~
    Shl,       // <<
    Shr,       // >>

    // Assignment
    Assign,       // =
    PlusAssign,   // +=
    MinusAssign,  // -=
    StarAssign,   // *=
    SlashAssign,  // /=
    PercentAssign, // %=
    DoubleStarAssign, // **=
    ShlAssign,    // <<=
    ShrAssign,    // >>=
    BitAndAssign, // &=
    BitOrAssign,  // |=
    BitXorAssign, // ^=
    IntDivAssign, // //=

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,

    // Punctuation
    Comma,
    Dot,
    DotDot,      // ..
    DotDotEq,    // ..=
    Colon,
    ColonColon,  // ::
    Semicolon,
    Arrow,       // ->
    FatArrow,    // =>
    Question,    // ?
    QuestionDot, // ?. (optional chaining)
    QuestionQuestion, // ?? (null coalescing)
    At,          // @
    /// Raw embedded engine block captured at lex time: @py [name] { code }
    /// (tag, optional block name, raw foreign source)
    EmbeddedBlock(String, Option<String>, String),
    Hash,        // #
    Ellipsis,    // ...
    Pipe,        // |> (pipe operator)
    Underscore,  // _

    // Keywords
    Let,
    Const,
    Func,
    Return,
    If,
    Elif,
    Else,
    While,
    For,
    In,
    NotIn,      // "not in" compound
    Is,         // type check keyword
    Break,
    Continue,
    Match,
    Case,
    Default,
    Import,
    From,
    As,
    Pub,
    Private,
    Internal,
    Class,
    New,
    Extends,
    Extend,
    Super,
    Self_,
    Struct,
    Enum,
    Trait,
    Impl,
    Dyn,
    Mod,
    Async,
    Await,
    Try,
    Catch,
    Finally,
    Throw,
    Defer,
    Yield,
    Lambda,
    Move,
    Ref,
    Mut,
    Unsafe,
    Extern,
    Type,
    Where,
    Pure,
    Macro,
    Comptime,
    Actor,
    Agent,
    Sealed,
    Volatile,
    Label,
    Goto,
    Enable,
    Cimport,
    Do,
    TypeOf,
    TestBlock,
    BenchBlock,
    Lazy,
    Using,
    StaticAssert,
    Newtype,
    CStruct,
    Never,
    Bitfield,
    Isolate,

    // Special
    Eof,
    Newline,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub col: usize,
}

impl Token {
    pub fn new(kind: TokenKind, line: usize, col: usize) -> Self {
        Self { kind, line, col }
    }
}
