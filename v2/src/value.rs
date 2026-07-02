use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::ast::{Expr, Param, Stmt};

#[derive(Debug, Clone)]
pub struct GeneratorState {
    pub items: Vec<Value>,
    pub index: usize,
    /// For lazy generators: stored function + args for re-execution
    pub lazy: Option<(FuncValue, Vec<(Option<String>, Value)>)>,
    pub started: bool,
    pub done: bool,
    pub resume_inputs: Vec<Value>,
}

/// Runtime value for V2.
#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    /// Arbitrary-precision integer (unsized `int` that has grown past i64).
    BigInt(crate::bigint::BigInt),
    /// Exact decimal (std.decimal) — no floating-point rounding error.
    Decimal(crate::decimal::Decimal),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Pointer(i64),
    List(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Tuple(Vec<Value>),
    Set(Vec<Value>),
    /// A user-defined function (closure).
    Func(FuncValue),
    /// A built-in function.
    BuiltinFunc(String),
    /// Class instance: (class_name, fields)
    Instance(String, HashMap<String, Value>),
    /// Copy-on-write class instance: shared fields with detach-on-mutation
    CowInstance(String, Rc<RefCell<HashMap<String, Value>>>),
    /// A class value (for `new`)
    Class(ClassValue),
    /// An enum variant: (enum_name, variant_name, data)
    EnumVariant(String, String, Vec<Value>),
    /// A struct instance: (struct_name, fields)
    StructInstance(String, HashMap<String, Value>),
    /// Range value
    Range(i64, i64, bool),
    /// Return signal (used internally)
    Return(Box<Value>),
    /// Break signal (used internally)
    Break,
    /// Continue signal (used internally)
    Continue,
    /// Error value (for throw/catch)
    Error(String),
    /// Ok(value) — Result type success
    Ok(Box<Value>),
    /// Err(value) — Result type error
    Err(Box<Value>),
    /// Some(value) — Option type present
    Some(Box<Value>),
    /// Byte string: b"..."
    Bytes(Vec<u8>),
    /// Break with label
    BreakLabel(String),
    /// Continue with label
    ContinueLabel(String),
    /// Generator (pre-collected with cursor)
    Generator(Rc<RefCell<GeneratorState>>),
    /// Lazy expression — re-evaluates on each read
    Lazy(Box<Expr>),
    /// Deque value
    Deque(Rc<RefCell<std::collections::VecDeque<Value>>>),
    /// Tail call optimization signal: (func_name, args)
    TailCall(String, Vec<(Option<String>, Value)>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::BigInt(n) => write!(f, "{}", n.to_string()),
            Value::Decimal(d) => write!(f, "{}", d.to_string()),
            Value::Float(n) => {
                if n.fract() == 0.0 {
                    write!(f, "{:.1}", n)
                } else {
                    write!(f, "{}", n)
                }
            }
            Value::Str(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Null => write!(f, "null"),
            Value::Pointer(id) => write!(f, "<pointer:{}>", id),
            Value::List(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match v {
                        Value::Str(s) => write!(f, "\"{}\"", s)?,
                        _ => write!(f, "{}", v)?,
                    }
                }
                write!(f, "]")
            }
            Value::Dict(pairs) => {
                write!(f, "{{")?;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
            Value::Tuple(items) => {
                write!(f, "(")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", v)?;
                }
                write!(f, ")")
            }
            Value::Set(items) => {
                write!(f, "#{{")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", v)?;
                }
                write!(f, "}}")
            }
            Value::Func(fv) => write!(f, "<func {}>", fv.name),
            Value::BuiltinFunc(name) => write!(f, "<builtin {}>", name),
            Value::Instance(cls, _) | Value::CowInstance(cls, _) => write!(f, "<{} instance>", cls),
            Value::Class(cv) => write!(f, "<class {}>", cv.name),
            Value::EnumVariant(e, v, data) => {
                if data.is_empty() {
                    write!(f, "{}.{}", e, v)
                } else {
                    write!(f, "{}.{}(", e, v)?;
                    for (i, d) in data.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", d)?;
                    }
                    write!(f, ")")
                }
            }
            Value::StructInstance(name, fields) => {
                write!(f, "{} {{", name)?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
            Value::Range(s, e, inclusive) => {
                if *inclusive {
                    write!(f, "{}..={}", s, e)
                } else {
                    write!(f, "{}..{}", s, e)
                }
            }
            Value::Return(v) => write!(f, "{}", v),
            Value::Break => write!(f, "<break>"),
            Value::Continue => write!(f, "<continue>"),
            Value::Error(msg) => write!(f, "Error: {}", msg),
            Value::Ok(v) => write!(f, "Ok({})", v),
            Value::Err(v) => write!(f, "Err({})", v),
            Value::Some(v) => write!(f, "Some({})", v),
            Value::Bytes(b) => write!(f, "b{:?}", b),
            Value::BreakLabel(lbl) => write!(f, "<break {}>", lbl),
            Value::ContinueLabel(lbl) => write!(f, "<continue {}>", lbl),
            Value::Generator(gs) => {
                let state = gs.borrow();
                if state.lazy.is_some() {
                    write!(f, "<generator lazy @{}>", state.index)
                } else {
                    write!(f, "<generator {}/{}>", state.index, state.items.len())
                }
            }
            Value::Lazy(_) => write!(f, "<lazy>"),
            Value::Deque(dq) => {
                let dq = dq.borrow();
                write!(f, "deque[")?;
                for (i, v) in dq.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", v)?;
                }
                write!(f, "]")
            }
            Value::TailCall(name, _) => write!(f, "<tailcall:{}>", name),
        }
    }
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Null => false,
            Value::Int(0) => false,
            Value::Float(f) if *f == 0.0 => false,
            Value::Str(s) if s.is_empty() => false,
            Value::Pointer(0) => false,
            Value::List(l) if l.is_empty() => false,
            _ => true,
        }
    }

    pub fn type_name(&self) -> &str {
        match self {
            Value::Int(_) => "int",
            Value::BigInt(_) => "int",
            Value::Decimal(_) => "decimal",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::Bool(_) => "bool",
            Value::Null => "null",
            Value::Pointer(_) => "pointer",
            Value::List(_) => "list",
            Value::Dict(_) => "dict",
            Value::Tuple(_) => "tuple",
            Value::Set(_) => "set",
            Value::Func(_) => "func",
            Value::BuiltinFunc(_) => "func",
            Value::Instance(cls, _) | Value::CowInstance(cls, _) => "instance",
            Value::Class(_) => "class",
            Value::EnumVariant(_, _, _) => "enum",
            Value::StructInstance(_, _) => "struct",
            Value::Range(_, _, _) => "range",
            Value::Return(_) | Value::Break | Value::Continue | Value::BreakLabel(_) | Value::ContinueLabel(_) => "signal",
            Value::Error(_) => "error",
            Value::Ok(_) => "ok",
            Value::Err(_) => "err",
            Value::Some(_) => "some",
            Value::Bytes(_) => "bytes",
            Value::Generator(_) => "generator",
            Value::Lazy(_) => "lazy",
            Value::Deque(_) => "deque",
            Value::TailCall(_, _) => "tailcall",
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Value::Str(s) = self { Some(s.as_str()) } else { None }
    }

    pub fn to_string_repr(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            other => format!("{}", other),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::BigInt(a), Value::BigInt(b)) => a.cmp(b) == std::cmp::Ordering::Equal,
            (Value::BigInt(a), Value::Int(b)) => a.cmp(&crate::bigint::BigInt::from_i64(*b)) == std::cmp::Ordering::Equal,
            (Value::Int(a), Value::BigInt(b)) => crate::bigint::BigInt::from_i64(*a).cmp(b) == std::cmp::Ordering::Equal,
            (Value::BigInt(a), Value::Float(b)) => a.to_f64() == *b,
            (Value::Float(a), Value::BigInt(b)) => *a == b.to_f64(),
            (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b) == std::cmp::Ordering::Equal,
            (Value::Decimal(a), Value::Int(b)) => a.cmp(&crate::decimal::Decimal::from_i64(*b)) == std::cmp::Ordering::Equal,
            (Value::Int(a), Value::Decimal(b)) => crate::decimal::Decimal::from_i64(*a).cmp(b) == std::cmp::Ordering::Equal,
            (Value::Decimal(a), Value::Float(b)) => a.to_f64() == *b,
            (Value::Float(a), Value::Decimal(b)) => *a == b.to_f64(),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Null, Value::Null) => true,
            (Value::Pointer(a), Value::Pointer(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Tuple(a), Value::Tuple(b)) => a == b,
            (Value::Dict(a), Value::Dict(b)) => {
                a.len() == b.len() && a.iter().all(|(k, v)| {
                    b.iter().any(|(bk, bv)| k == bk && v == bv)
                })
            }
            (Value::Set(a), Value::Set(b)) => a == b,
            (Value::Instance(c1, f1), Value::Instance(c2, f2)) => c1 == c2 && f1 == f2,
            (Value::StructInstance(c1, f1), Value::StructInstance(c2, f2)) => c1 == c2 && f1 == f2,
            (Value::CowInstance(c1, f1), Value::CowInstance(c2, f2)) => {
                c1 == c2 && *f1.borrow() == *f2.borrow()
            }
            (Value::Instance(c1, f1), Value::CowInstance(c2, f2)) => {
                c1 == c2 && *f1 == *f2.borrow()
            }
            (Value::CowInstance(c1, f1), Value::Instance(c2, f2)) => {
                c1 == c2 && *f1.borrow() == *f2
            }
            (Value::EnumVariant(e1, v1, d1), Value::EnumVariant(e2, v2, d2)) => {
                e1 == e2 && v1 == v2 && d1 == d2
            }
            (Value::Ok(a), Value::Ok(b)) => a == b,
            (Value::Err(a), Value::Err(b)) => a == b,
            (Value::Some(a), Value::Some(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Deque(a), Value::Deque(b)) => {
                let a = a.borrow();
                let b = b.borrow();
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
            }
            (Value::TailCall(n1, a1), Value::TailCall(n2, a2)) => n1 == n2 && a1 == a2,
            _ => false,
        }
    }
}

/// Stored function: captures name, params, body, and defining environment index.
#[derive(Clone, Debug)]
pub struct FuncValue {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    pub closure_env: usize,
    pub is_generator: bool,
}

/// Stored class value.
#[derive(Clone, Debug)]
pub struct ClassValue {
    pub name: String,
    pub parent: Option<String>,
    pub methods: HashMap<String, FuncValue>,
    pub fields: HashMap<String, Value>,
    pub field_order: Vec<String>,
    pub is_fixed: bool,
    pub is_data: bool,
    pub is_sealed: bool,
    pub is_cow: bool,
    pub sealed_children: Vec<String>,
    pub computed_properties: HashMap<String, ComputedProp>,
}

#[derive(Debug, Clone)]
pub struct ComputedProp {
    pub getter: Option<FuncValue>,
    pub setter: Option<FuncValue>,
}
