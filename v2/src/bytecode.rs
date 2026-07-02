/// Bytecode instruction set and chunk container for V2.

use crate::value::Value;

/// Every instruction the VM understands.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum Op {
    // ── Constants & Literals ─────────────────────────
    /// Push constant from pool: operand = constant index (u16)
    Constant,
    /// Push null
    Null,
    /// Push true
    True,
    /// Push false
    False,

    // ── Arithmetic ───────────────────────────────────
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    IntDiv,
    Neg,

    // ── Bitwise ──────────────────────────────────────
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Shl,
    Shr,

    // ── Comparison & Logic ───────────────────────────
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    Not,
    And,
    Or,
    In,
    NotIn,
    Is,
    NullCoalesce,

    // ── Variables ────────────────────────────────────
    /// Define a global variable: operand = name index (u16)
    DefineGlobal,
    /// Get a global variable: operand = name index (u16)
    GetGlobal,
    /// Set a global variable: operand = name index (u16)
    SetGlobal,
    /// Get a local variable: operand = stack slot (u16)
    GetLocal,
    /// Set a local variable: operand = stack slot (u16)
    SetLocal,
    /// Get an upvalue (captured variable): operand = upvalue index (u16)
    GetUpvalue,
    /// Set an upvalue: operand = upvalue index (u16)
    SetUpvalue,

    // ── Control Flow ─────────────────────────────────
    /// Unconditional forward jump: operand = offset (u16)
    Jump,
    /// Jump if top of stack is falsy: operand = offset (u16)
    JumpIfFalse,
    /// Jump if top of stack is truthy: operand = offset (u16)
    JumpIfTrue,
    /// Backward jump (loops): operand = offset (u16)
    Loop,

    // ── Functions & Calls ────────────────────────────
    /// Call a function: operand = arg count (u8)
    Call,
    /// Return from function
    Return,
    /// Create a closure: operand = function constant index (u16)
    Closure,

    // ── Collections ──────────────────────────────────
    /// Build a list from N items on stack: operand = count (u16)
    BuildList,
    /// Build a dict from 2*N items on stack: operand = pair count (u16)
    BuildDict,
    /// Build a tuple from N items on stack: operand = count (u16)
    BuildTuple,
    /// Build a set from N items on stack: operand = count (u16)
    BuildSet,

    // ── Field & Index Access ─────────────────────────
    /// Get field by name: operand = name index (u16)
    GetField,
    /// Set field by name: operand = name index (u16)
    SetField,
    /// Index access: stack has [obj, index]
    GetIndex,
    /// Index set: stack has [obj, index, value]
    SetIndex,
    /// Slice: stack has [obj, start_or_null, end_or_null]
    Slice,

    // ── Class & Object ───────────────────────────────
    /// Declare a class: operand = name index (u16)
    Class,
    /// Inherit from a superclass
    Inherit,
    /// Define a method on current class: operand = name index (u16)
    Method,
    /// Create a new instance: operand = arg count (u8)
    NewInstance,

    // ── Struct ───────────────────────────────────────
    /// Define a struct type: operand = name index (u16), followed by field count (u16)
    DefineStruct,
    /// Create struct literal: operand = name index (u16), field count (u16)
    BuildStruct,

    // ── Enum ─────────────────────────────────────────
    /// Define enum type: operand = name index (u16)
    DefineEnum,
    /// Create an enum variant: operand = name index (u16), variant index (u16), data count (u8)
    BuildEnumVariant,

    // ── Trait & Impl ─────────────────────────────────
    /// Define a trait: operand = name index (u16)
    DefineTrait,
    /// Begin impl block: operand = target name index (u16), optional trait name index (u16)
    BeginImpl,

    // ── Error Handling ───────────────────────────────
    /// Throw: pops value, raises error
    Throw,
    /// Set up try-catch: operand = catch jump offset (u16)
    TryBegin,
    /// End try block (pop error handler)
    TryEnd,

    // ── Closures & Upvalues ──────────────────────────
    /// Close upvalue at stack slot
    CloseUpvalue,

    // ── Import ───────────────────────────────────────
    /// Import module: operand = path constant index (u16)
    Import,

    // ── Iterators ────────────────────────────────────
    /// Get an iterator from the top of stack
    GetIter,
    /// Advance iterator; push next value or jump if done: operand = exit offset (u16)
    ForIter,

    // ── Match ────────────────────────────────────────
    /// Duplicate top of stack (for match subject)
    Dup,

    // ── Result/Option ────────────────────────────────
    /// Wrap top of stack in Ok()
    WrapOk,
    /// Wrap top of stack in Err()
    WrapErr,
    /// Wrap top of stack in Some()
    WrapSome,

    // ── Misc ─────────────────────────────────────────
    /// Pop top of stack
    Pop,
    /// Print top of stack (for `print()` builtin)
    Print,
    /// Swap top two stack items
    Swap,
    /// Duplicate N-th item from top
    DupN,
    /// Range: start..end (inclusive flag from operand u8)
    BuildRange,
    /// Spread operator
    Spread,
    /// Typeof
    TypeOf,
    /// Cast: operand = target type name index (u16)
    Cast,
    /// Do block: marker
    DoBlock,
    /// Yield value
    Yield,
    /// Await
    Await,
    /// Halt execution
    Halt,
    UnwrapSome, // If TOS is Some(x), replace with x
    /// Append TOS to list below it on stack (for comprehensions)
    ListAppend,
    /// Insert key-value pair into dict below on stack (for dict comprehensions)
    DictInsert,
    /// Add TOS to set below it on stack (for set comprehensions)
    SetAdd,
    MarkLazy, // operand: name_idx (u16) — marks a global as lazy
    UsingExtract, // pops dict/instance, defines globals for each key
}

/// A compiled chunk of bytecode.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Raw bytecode stream.
    pub code: Vec<u8>,
    /// Constant pool.
    pub constants: Vec<Value>,
    /// Line number for each byte (for error reporting).
    pub lines: Vec<u32>,
    /// String intern table (for global names, field names).
    pub strings: Vec<String>,
}

impl Chunk {
    pub fn new() -> Self {
        Chunk {
            code: Vec::new(),
            constants: Vec::new(),
            lines: Vec::new(),
            strings: Vec::new(),
        }
    }

    /// Write a single byte.
    pub fn write(&mut self, byte: u8, line: u32) {
        self.code.push(byte);
        self.lines.push(line);
    }

    /// Write an opcode.
    pub fn write_op(&mut self, op: Op, line: u32) {
        self.write(op as u8, line);
    }

    /// Write a u16 operand (big-endian).
    pub fn write_u16(&mut self, val: u16, line: u32) {
        self.write((val >> 8) as u8, line);
        self.write((val & 0xff) as u8, line);
    }

    /// Read a u16 at the given offset.
    pub fn read_u16(&self, offset: usize) -> u16 {
        ((self.code[offset] as u16) << 8) | (self.code[offset + 1] as u16)
    }

    /// Add a constant and return its index.
    pub fn add_constant(&mut self, value: Value) -> u16 {
        // Dedup ints, floats, strings and bools
        for (i, c) in self.constants.iter().enumerate() {
            if *c == value {
                return i as u16;
            }
        }
        let idx = self.constants.len();
        assert!(idx <= u16::MAX as usize, "Too many constants in chunk");
        self.constants.push(value);
        idx as u16
    }

    /// Add a string to the intern table and return its index.
    pub fn add_string(&mut self, s: &str) -> u16 {
        for (i, existing) in self.strings.iter().enumerate() {
            if existing == s {
                return i as u16;
            }
        }
        let idx = self.strings.len();
        assert!(idx <= u16::MAX as usize, "Too many strings in chunk");
        self.strings.push(s.to_string());
        idx as u16
    }

    /// Current code length (for jump patching).
    pub fn len(&self) -> usize {
        self.code.len()
    }

    /// Patch a u16 at the given offset.
    pub fn patch_u16(&mut self, offset: usize, val: u16) {
        self.code[offset] = (val >> 8) as u8;
        self.code[offset + 1] = (val & 0xff) as u8;
    }

    /// Disassemble for debugging.
    pub fn disassemble(&self, name: &str) {
        println!("== {} ==", name);
        let mut offset = 0;
        while offset < self.code.len() {
            offset = self.disassemble_instruction(offset);
        }
    }

    pub fn disassemble_instruction(&self, offset: usize) -> usize {
        print!("{:04} ", offset);
        if offset > 0 && self.lines[offset] == self.lines[offset - 1] {
            print!("   | ");
        } else {
            print!("{:4} ", self.lines[offset]);
        }

        let op = self.code[offset];
        match op {
            x if x == Op::Constant as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_CONSTANT", idx, self.constants[idx as usize]);
                offset + 3
            }
            x if x == Op::Null as u8 => { println!("OP_NULL"); offset + 1 }
            x if x == Op::True as u8 => { println!("OP_TRUE"); offset + 1 }
            x if x == Op::False as u8 => { println!("OP_FALSE"); offset + 1 }
            x if x == Op::Add as u8 => { println!("OP_ADD"); offset + 1 }
            x if x == Op::Sub as u8 => { println!("OP_SUB"); offset + 1 }
            x if x == Op::Mul as u8 => { println!("OP_MUL"); offset + 1 }
            x if x == Op::Div as u8 => { println!("OP_DIV"); offset + 1 }
            x if x == Op::Mod as u8 => { println!("OP_MOD"); offset + 1 }
            x if x == Op::Pow as u8 => { println!("OP_POW"); offset + 1 }
            x if x == Op::IntDiv as u8 => { println!("OP_INTDIV"); offset + 1 }
            x if x == Op::Neg as u8 => { println!("OP_NEG"); offset + 1 }
            x if x == Op::Eq as u8 => { println!("OP_EQ"); offset + 1 }
            x if x == Op::NotEq as u8 => { println!("OP_NOTEQ"); offset + 1 }
            x if x == Op::Lt as u8 => { println!("OP_LT"); offset + 1 }
            x if x == Op::Gt as u8 => { println!("OP_GT"); offset + 1 }
            x if x == Op::LtEq as u8 => { println!("OP_LTEQ"); offset + 1 }
            x if x == Op::GtEq as u8 => { println!("OP_GTEQ"); offset + 1 }
            x if x == Op::Not as u8 => { println!("OP_NOT"); offset + 1 }
            x if x == Op::And as u8 => { println!("OP_AND"); offset + 1 }
            x if x == Op::Or as u8 => { println!("OP_OR"); offset + 1 }
            x if x == Op::BitAnd as u8 => { println!("OP_BITAND"); offset + 1 }
            x if x == Op::BitOr as u8 => { println!("OP_BITOR"); offset + 1 }
            x if x == Op::BitXor as u8 => { println!("OP_BITXOR"); offset + 1 }
            x if x == Op::BitNot as u8 => { println!("OP_BITNOT"); offset + 1 }
            x if x == Op::Shl as u8 => { println!("OP_SHL"); offset + 1 }
            x if x == Op::Shr as u8 => { println!("OP_SHR"); offset + 1 }
            x if x == Op::Pop as u8 => { println!("OP_POP"); offset + 1 }
            x if x == Op::Print as u8 => { println!("OP_PRINT"); offset + 1 }
            x if x == Op::Return as u8 => { println!("OP_RETURN"); offset + 1 }
            x if x == Op::Halt as u8 => { println!("OP_HALT"); offset + 1 }
            x if x == Op::Dup as u8 => { println!("OP_DUP"); offset + 1 }
            x if x == Op::Swap as u8 => { println!("OP_SWAP"); offset + 1 }
            x if x == Op::Throw as u8 => { println!("OP_THROW"); offset + 1 }
            x if x == Op::TryEnd as u8 => { println!("OP_TRYEND"); offset + 1 }
            x if x == Op::GetIter as u8 => { println!("OP_GETITER"); offset + 1 }
            x if x == Op::WrapOk as u8 => { println!("OP_WRAP_OK"); offset + 1 }
            x if x == Op::WrapErr as u8 => { println!("OP_WRAP_ERR"); offset + 1 }
            x if x == Op::WrapSome as u8 => { println!("OP_WRAP_SOME"); offset + 1 }
            x if x == Op::GetIndex as u8 => { println!("OP_GETINDEX"); offset + 1 }
            x if x == Op::SetIndex as u8 => { println!("OP_SETINDEX"); offset + 1 }
            x if x == Op::TypeOf as u8 => { println!("OP_TYPEOF"); offset + 1 }
            x if x == Op::Yield as u8 => { println!("OP_YIELD"); offset + 1 }
            x if x == Op::Await as u8 => { println!("OP_AWAIT"); offset + 1 }
            x if x == Op::NullCoalesce as u8 => { println!("OP_NULLCOALESCE"); offset + 1 }
            x if x == Op::In as u8 => { println!("OP_IN"); offset + 1 }
            x if x == Op::NotIn as u8 => { println!("OP_NOTIN"); offset + 1 }
            x if x == Op::Is as u8 => { println!("OP_IS"); offset + 1 }
            x if x == Op::Spread as u8 => { println!("OP_SPREAD"); offset + 1 }
            x if x == Op::CloseUpvalue as u8 => { println!("OP_CLOSE_UPVALUE"); offset + 1 }
            x if x == Op::Slice as u8 => { println!("OP_SLICE"); offset + 1 }
            x if x == Op::Inherit as u8 => { println!("OP_INHERIT"); offset + 1 }
            // u16 operand instructions
            x if x == Op::DefineGlobal as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_DEFINE_GLOBAL", idx, self.strings[idx as usize]);
                offset + 3
            }
            x if x == Op::GetGlobal as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_GET_GLOBAL", idx, self.strings[idx as usize]);
                offset + 3
            }
            x if x == Op::SetGlobal as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_SET_GLOBAL", idx, self.strings[idx as usize]);
                offset + 3
            }
            x if x == Op::GetLocal as u8 => {
                let slot = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_GET_LOCAL", slot);
                offset + 3
            }
            x if x == Op::SetLocal as u8 => {
                let slot = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_SET_LOCAL", slot);
                offset + 3
            }
            x if x == Op::GetUpvalue as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_GET_UPVALUE", idx);
                offset + 3
            }
            x if x == Op::SetUpvalue as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_SET_UPVALUE", idx);
                offset + 3
            }
            x if x == Op::Jump as u8 => {
                let off = self.read_u16(offset + 1);
                println!("{:<20} {:4} -> {}", "OP_JUMP", off, offset + 3 + off as usize);
                offset + 3
            }
            x if x == Op::JumpIfFalse as u8 => {
                let off = self.read_u16(offset + 1);
                println!("{:<20} {:4} -> {}", "OP_JUMP_IF_FALSE", off, offset + 3 + off as usize);
                offset + 3
            }
            x if x == Op::JumpIfTrue as u8 => {
                let off = self.read_u16(offset + 1);
                println!("{:<20} {:4} -> {}", "OP_JUMP_IF_TRUE", off, offset + 3 + off as usize);
                offset + 3
            }
            x if x == Op::Loop as u8 => {
                let off = self.read_u16(offset + 1);
                println!("{:<20} {:4} -> {}", "OP_LOOP", off, offset + 3 - off as usize);
                offset + 3
            }
            x if x == Op::Call as u8 => {
                let argc = self.code[offset + 1];
                println!("{:<20} {:4}", "OP_CALL", argc);
                offset + 2
            }
            x if x == Op::Closure as u8 => {
                let idx = self.read_u16(offset + 1);
                print!("{:<20} {:4} '{}'", "OP_CLOSURE", idx, self.constants[idx as usize]);
                let mut o = offset + 3;
                // Read upvalue descriptors
                if let Value::Func(ref fv) = self.constants[idx as usize] {
                    let upvalue_count = fv.params.len(); // store upvalue_count in a field…
                    // For now just print the constant
                }
                println!();
                o
            }
            x if x == Op::BuildList as u8 => {
                let count = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_BUILD_LIST", count);
                offset + 3
            }
            x if x == Op::BuildDict as u8 => {
                let count = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_BUILD_DICT", count);
                offset + 3
            }
            x if x == Op::BuildTuple as u8 => {
                let count = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_BUILD_TUPLE", count);
                offset + 3
            }
            x if x == Op::BuildSet as u8 => {
                let count = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_BUILD_SET", count);
                offset + 3
            }
            x if x == Op::GetField as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_GET_FIELD", idx, self.strings[idx as usize]);
                offset + 3
            }
            x if x == Op::SetField as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_SET_FIELD", idx, self.strings[idx as usize]);
                offset + 3
            }
            x if x == Op::Class as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_CLASS", idx, self.strings[idx as usize]);
                offset + 3
            }
            x if x == Op::Method as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_METHOD", idx, self.strings[idx as usize]);
                offset + 3
            }
            x if x == Op::NewInstance as u8 => {
                let argc = self.code[offset + 1];
                println!("{:<20} {:4}", "OP_NEW_INSTANCE", argc);
                offset + 2
            }
            x if x == Op::Import as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_IMPORT", idx);
                offset + 3
            }
            x if x == Op::ForIter as u8 => {
                let off = self.read_u16(offset + 1);
                println!("{:<20} {:4} -> {}", "OP_FORITER", off, offset + 3 + off as usize);
                offset + 3
            }
            x if x == Op::TryBegin as u8 => {
                let off = self.read_u16(offset + 1);
                println!("{:<20} {:4} -> {}", "OP_TRY_BEGIN", off, offset + 3 + off as usize);
                offset + 3
            }
            x if x == Op::BuildRange as u8 => {
                let inclusive = self.code[offset + 1];
                println!("{:<20} inclusive={}", "OP_BUILD_RANGE", inclusive);
                offset + 2
            }
            x if x == Op::DupN as u8 => {
                let n = self.read_u16(offset + 1);
                println!("{:<20} {:4}", "OP_DUP_N", n);
                offset + 3
            }
            x if x == Op::DefineStruct as u8 => {
                let name = self.read_u16(offset + 1);
                let fc = self.read_u16(offset + 3);
                println!("{:<20} {:4} '{}' fields={}", "OP_DEFINE_STRUCT", name, self.strings[name as usize], fc);
                offset + 5
            }
            x if x == Op::BuildStruct as u8 => {
                let name = self.read_u16(offset + 1);
                let fc = self.read_u16(offset + 3);
                println!("{:<20} {:4} '{}' fields={}", "OP_BUILD_STRUCT", name, self.strings[name as usize], fc);
                offset + 5
            }
            x if x == Op::DefineEnum as u8 => {
                let name = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_DEFINE_ENUM", name, self.strings[name as usize]);
                offset + 3
            }
            x if x == Op::BuildEnumVariant as u8 => {
                let name = self.read_u16(offset + 1);
                let variant = self.read_u16(offset + 3);
                let data = self.code[offset + 5];
                println!("{:<20} {}::{} data={}", "OP_BUILD_ENUM_VAR", self.strings[name as usize], self.strings[variant as usize], data);
                offset + 6
            }
            x if x == Op::DefineTrait as u8 => {
                let name = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_DEFINE_TRAIT", name, self.strings[name as usize]);
                offset + 3
            }
            x if x == Op::BeginImpl as u8 => {
                let target = self.read_u16(offset + 1);
                let trait_idx = self.read_u16(offset + 3);
                println!("{:<20} target='{}' trait='{}'", "OP_BEGIN_IMPL", self.strings[target as usize],
                    if trait_idx == 0xFFFF { "none".to_string() } else { self.strings[trait_idx as usize].clone() });
                offset + 5
            }
            x if x == Op::Cast as u8 => {
                let idx = self.read_u16(offset + 1);
                println!("{:<20} {:4} '{}'", "OP_CAST", idx, self.strings[idx as usize]);
                offset + 3
            }
            _ => {
                println!("Unknown opcode {}", op);
                offset + 1
            }
        }
    }
}

/// A compiled function object stored in the constant pool.
#[derive(Debug, Clone)]
pub struct CompiledFunc {
    pub name: String,
    pub arity: u8,
    pub has_variadic: bool,
    pub default_count: u8,
    pub upvalue_count: u16,
    pub is_generator: bool,
    pub chunk: Chunk,
}
