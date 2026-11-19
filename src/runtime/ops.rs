//! Equality, ordering and the arithmetic operators (INTERNALS 5.2, 5.3).

use std::cmp::Ordering;
use std::rc::Rc;

use super::error::{fail, type_error, value_error, Exec};
use super::value::{Dict, Func, List, Value};
use crate::syntax::ast::BinOp;

/// Past this many nested containers `==` and `str()` stop (5.2).
pub const MAX_VALUE_DEPTH: usize = 1_000;

pub fn too_deep<T>() -> Exec<T> {
    fail("RecursionError", "value nested too deeply (or cyclic)")
}

const TWO_63: f64 = 9_223_372_036_854_775_808.0;

/// `==` (5.2): deep for lists and dicts, exact between int and float, `false` across types.
pub fn eq(a: &Value, b: &Value) -> Exec<bool> {
    eq_at(a, b, 0)
}

fn eq_at(a: &Value, b: &Value, depth: usize) -> Exec<bool> {
    Ok(match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Int(i), Value::Float(f)) | (Value::Float(f), Value::Int(i)) => int_float_eq(*i, *f),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bytes(x), Value::Bytes(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            if List::ptr_eq(x, y) {
                return Ok(true);
            }
            if depth >= MAX_VALUE_DEPTH {
                return too_deep();
            }
            // `==` runs no user code, so the borrows can stay open (5.11 borrow rule).
            let (xs, ys) = (x.borrow(), y.borrow());
            if xs.len() != ys.len() {
                return Ok(false);
            }
            for (p, q) in xs.iter().zip(ys.iter()) {
                if !eq_at(p, q, depth + 1)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Dict(x), Value::Dict(y)) => {
            if Dict::ptr_eq(x, y) {
                return Ok(true);
            }
            if depth >= MAX_VALUE_DEPTH {
                return too_deep();
            }
            let (xs, ys) = (x.borrow(), y.borrow());
            if xs.len() != ys.len() {
                return Ok(false);
            }
            for (k, p) in xs.iter() {
                match ys.get(k) {
                    Some(q) if eq_at(p, q, depth + 1)? => {}
                    _ => return Ok(false),
                }
            }
            true
        }
        (Value::Func(x), Value::Func(y)) => {
            use super::value::Callable;
            match (&**x, &**y) {
                (Callable::Builtin(p), Callable::Builtin(q)) => std::ptr::eq(*p, *q),
                (Callable::Proxy { session: s1, unit: u1, name: n1 }, Callable::Proxy { session: s2, unit: u2, name: n2 }) => {
                    s1 == s2 && u1 == u2 && n1 == n2
                }
                (Callable::Route { route: r1, .. }, Callable::Route { route: r2, .. }) => r1 == r2,
                _ => Func::ptr_eq(x, y),
            }
        }
        (Value::Error(x), Value::Error(y)) => Rc::ptr_eq(x, y),
        // The same foreign object: same session and id (11.6).
        (Value::Handle(x), Value::Handle(y)) => x.same(y),
        _ => false,
    })
}

/// Exact: never `i as f64 == f`, which is wrong beyond 2^53.
fn int_float_eq(i: i64, f: f64) -> bool {
    f.fract() == 0.0 && (-TWO_63..TWO_63).contains(&f) && f as i64 == i
}

/// The exact order of an int and a float; `None` when `f` is NaN (5.2).
fn cmp_int_float(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f >= TWO_63 {
        return Some(Ordering::Less);
    }
    if f < -TWO_63 {
        return Some(Ordering::Greater);
    }
    let t = f.trunc();
    Some(match i.cmp(&(t as i64)) {
        Ordering::Equal => {
            let frac = f - t;
            if frac > 0.0 {
                Ordering::Less
            } else if frac < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        other => other,
    })
}

/// Number–number or str–str; `None` when a NaN is involved (5.2).
pub fn order(a: &Value, b: &Value, op: &str) -> Exec<Option<Ordering>> {
    Ok(match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Int(i), Value::Float(f)) => cmp_int_float(*i, *f),
        (Value::Float(f), Value::Int(i)) => cmp_int_float(*i, *f).map(Ordering::reverse),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        _ => {
            let mut hint = operand_hint(a, b, &format!("h {op} x"));
            if text_and_number(a, b) {
                hint = " — to compare text such as input() gives with a number, convert it: int(x) or float(x)".into();
            }
            return type_error(format!("cannot compare {} and {} with '{op}'{hint}", a.type_name(), b.type_name()));
        }
    })
}

/// The total order `sort()` uses: numbers exactly with NaN last, or strings (5.11).
pub fn sort_key_cmp(a: &Value, b: &Value) -> Ordering {
    let nan = |v: &Value| matches!(v, Value::Float(x) if x.is_nan());
    match (nan(a), nan(b)) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => order(a, b, "<").ok().flatten().unwrap_or(Ordering::Equal),
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(x) => Some(*x),
        _ => None,
    }
}

fn overflow<T>(op: &str) -> Exec<T> {
    value_error(format!("integer overflow in '{op}'"))
}

fn unsupported<T>(op: &str, a: &Value, b: &Value) -> Exec<T> {
    let mut msg = format!("unsupported operand types for {op}: '{}' and '{}'", a.type_name(), b.type_name());
    let seq = |v: &Value| matches!(v, Value::Str(_) | Value::List(_) | Value::Bytes(_));
    if is_handle(a) || is_handle(b) {
        msg.push_str(&operand_hint(a, b, &format!("h {op} 1")));
    } else if matches!(a, Value::Null) || matches!(b, Value::Null) {
        // Where the null came from is the interpreter's to say (5.3).
    } else if op == "*" && ((seq(a) && matches!(b, Value::Float(_))) || (matches!(a, Value::Float(_)) && seq(b))) {
        msg.push_str(" — repeat with a whole number, as in \"ab\" * 2");
    } else if op == "+" && matches!(a, Value::List(_)) && !matches!(b, Value::List(_)) {
        msg.push_str(" — add an item with xs.push(x), or join lists with xs + [x]");
    } else if op == "+" && matches!((a, b), (Value::Dict(_), Value::Dict(_))) {
        msg.push_str(" — to merge two dicts, ");
        msg.push_str(crate::syntax::lexer::MERGE_HINT);
    } else if op == "+" && (matches!(a, Value::Str(_)) || matches!(b, Value::Str(_))) {
        msg.push_str(" — to join text, use f\"...${x}\" or str(x); to count with text such as input() gives, convert it: int(x) or float(x)");
    } else if text_and_number(a, b) {
        msg.push_str(" — to count with text such as input() gives, convert it: int(x) or float(x)");
    }
    type_error(msg)
}

/// What a TypeError about a handle adds (11.7): the operation done in the handle's own
/// language, as `py` or `js` writes it; nothing for another value.
pub fn handle_hint(v: &Value, py: &str, js: &str) -> String {
    let Value::Handle(h) = v else { return String::new() };
    let (tag, example) = if &*h.tag == "js" { ("js", js) } else { (&*h.tag, py) };
    format!(" — do it inside a block of its language, as in @{tag}(h) {{ {example} }}, or call a method on it; to_v2(h) converts one that holds plain data (lists, dicts, numbers, text)")
}

/// The hint for an operator with a handle operand: for an integer too big for V2's 64-bit int (a
/// Python `int`, a JavaScript `bigint`), `to_v2` can't help, so the work stays in the block (9.1).
fn operand_hint(a: &Value, b: &Value, example: &str) -> String {
    let big = |v: &Value| matches!(v, Value::Handle(h) if h.big_int());
    if big(a) || big(b) {
        " — this integer needs more than 64 bits, which V2's int doesn't have: keep the arithmetic in the block".into()
    } else {
        handle_hint(if is_handle(a) { a } else { b }, example, example)
    }
}

/// A str with an int or a float, either way round: text read as a number (5.3).
fn text_and_number(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| matches!(v, Value::Int(_) | Value::Float(_));
    (matches!(a, Value::Str(_)) && num(b)) || (num(a) && matches!(b, Value::Str(_)))
}

pub fn is_handle(v: &Value) -> bool {
    matches!(v, Value::Handle(_))
}

/// `count` copies of a sequence of `len` items: the size is checked and the buffer reserved
/// before anything is copied (5.3).
fn repeat_capacity(len: usize, count: i64, limit: u64, unit: &str, most: &str) -> Exec<usize> {
    if count <= 0 {
        return Ok(0);
    }
    match len.checked_mul(count as usize) {
        Some(n) if n as u64 > limit => value_error(format!("repetition result too large: {n} {unit} ({most} at most)")),
        Some(n) => Ok(n),
        None => value_error("repetition result too large"),
    }
}

fn repeat_str(s: &str, count: i64) -> Exec<Value> {
    let n = repeat_capacity(s.len(), count, 1 << 30, "bytes of text", "1 GiB")?;
    if n == 0 {
        return Ok(Value::str(""));
    }
    let mut out = String::new();
    if out.try_reserve_exact(n).is_err() {
        return value_error("repetition result too large");
    }
    for _ in 0..count.max(0) {
        out.push_str(s);
    }
    Ok(Value::Str(out.into()))
}

fn repeat_list(items: &[Value], count: i64) -> Exec<Value> {
    let n = repeat_capacity(items.len(), count, crate::runtime::builtins::MAX_ITEMS, "items", "100000000")?;
    if n == 0 {
        return Ok(Value::list(Vec::new()));
    }
    let mut out = Vec::new();
    if out.try_reserve_exact(n).is_err() {
        return value_error("repetition result too large");
    }
    for _ in 0..count.max(0) {
        out.extend_from_slice(items);
    }
    Ok(Value::list(out))
}

/// Floor remainder: the result takes the divisor's sign (5.3).
pub(crate) fn floor_rem_int(a: i64, b: i64) -> i64 {
    let r = a.wrapping_rem(b);
    if r != 0 && (r < 0) != (b < 0) {
        r + b
    } else {
        r
    }
}

/// The same on floats, where a zero result takes the divisor's sign too (`-6.0 % 3` is `0.0`).
fn floor_rem_float(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r == 0.0 {
        0.0f64.copysign(b)
    } else if (r < 0.0) != (b < 0.0) {
        r + b
    } else {
        r
    }
}

fn int_pow(base: i64, exp: i64) -> Exec<Value> {
    if exp < 0 {
        if base == 0 {
            return value_error("division by zero");
        }
        return Ok(Value::Float((base as f64).powf(exp as f64)));
    }
    if exp > u32::MAX as i64 {
        return match base {
            0 | 1 => Ok(Value::Int(base)),
            -1 => Ok(Value::Int(if exp % 2 == 0 { 1 } else { -1 })),
            _ => overflow("**"),
        };
    }
    match base.checked_pow(exp as u32) {
        Some(n) => Ok(Value::Int(n)),
        None => overflow("**"),
    }
}

/// `a op b` for every binary operator except `&&`, `||` and `??` (5.3).
pub fn binop(op: BinOp, a: &Value, b: &Value) -> Exec<Value> {
    use Value::{Bytes, Float, Int, List, Str};
    Ok(match op {
        BinOp::Add => match (a, b) {
            (Int(x), Int(y)) => Int(x.checked_add(*y).map_or_else(|| overflow("+"), Ok)?),
            (Str(x), Str(y)) => Str(format!("{x}{y}").into()),
            (Bytes(x), Bytes(y)) => Bytes([&x[..], &y[..]].concat().into()),
            (List(x), List(y)) => {
                let mut items = x.borrow().clone();
                items.extend(y.borrow().iter().cloned());
                Value::list(items)
            }
            _ => match (num(a), num(b)) {
                (Some(x), Some(y)) => Float(x + y),
                _ => return unsupported("+", a, b),
            },
        },
        BinOp::Sub => match (a, b) {
            (Int(x), Int(y)) => Int(x.checked_sub(*y).map_or_else(|| overflow("-"), Ok)?),
            _ => match (num(a), num(b)) {
                (Some(x), Some(y)) => Float(x - y),
                _ => return unsupported("-", a, b),
            },
        },
        BinOp::Mul => match (a, b) {
            (Int(x), Int(y)) => Int(x.checked_mul(*y).map_or_else(|| overflow("*"), Ok)?),
            (Str(s), Int(n)) | (Int(n), Str(s)) => return repeat_str(s, *n),
            (List(l), Int(n)) | (Int(n), List(l)) => {
                let items = l.borrow().clone();
                return repeat_list(&items, *n);
            }
            _ => match (num(a), num(b)) {
                (Some(x), Some(y)) => Float(x * y),
                _ => return unsupported("*", a, b),
            },
        },
        BinOp::Div => match (num(a), num(b)) {
            (Some(x), Some(y)) => {
                if y == 0.0 {
                    return value_error("division by zero");
                }
                Float(x / y)
            }
            _ => return unsupported("/", a, b),
        },
        BinOp::Rem => match (a, b) {
            (Int(_), Int(0)) => return value_error("division by zero"),
            (Int(x), Int(y)) => Int(floor_rem_int(*x, *y)),
            _ => match (num(a), num(b)) {
                (Some(x), Some(y)) => {
                    if y == 0.0 {
                        return value_error("division by zero");
                    }
                    Float(floor_rem_float(x, y))
                }
                _ => return unsupported("%", a, b),
            },
        },
        BinOp::Pow => match (a, b) {
            (Int(x), Int(y)) => return int_pow(*x, *y),
            _ => match (num(a), num(b)) {
                (Some(x), Some(y)) if x == 0.0 && y < 0.0 => return value_error("division by zero"),
                (Some(x), Some(y)) => Float(x.powf(y)),
                _ => return unsupported("**", a, b),
            },
        },
        BinOp::Eq => Value::Bool(eq(a, b)?),
        BinOp::Ne => Value::Bool(!eq(a, b)?),
        BinOp::Lt => Value::Bool(order(a, b, "<")? == Some(Ordering::Less)),
        BinOp::Gt => Value::Bool(order(a, b, ">")? == Some(Ordering::Greater)),
        BinOp::Le => Value::Bool(matches!(order(a, b, "<=")?, Some(Ordering::Less | Ordering::Equal))),
        BinOp::Ge => Value::Bool(matches!(order(a, b, ">=")?, Some(Ordering::Greater | Ordering::Equal))),
        BinOp::In => Value::Bool(contains(b, a)?),
        BinOp::NotIn => Value::Bool(!contains(b, a)?),
    })
}

/// `-x` (5.3).
pub fn neg(v: &Value) -> Exec<Value> {
    match v {
        Value::Int(n) => n.checked_neg().map(Value::Int).map_or_else(|| overflow("-"), Ok),
        Value::Float(x) => Ok(Value::Float(-x)),
        other => {
            let hint = handle_hint(other, "-h", "-h");
            type_error(format!("bad operand type for unary -: '{}'{hint}", other.type_name()))
        }
    }
}

/// `x in y` (5.3).
pub fn contains(y: &Value, x: &Value) -> Exec<bool> {
    match y {
        Value::List(l) => {
            for item in l.borrow().iter() {
                if eq(item, x)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        // Keys are strings, so anything else is simply not a key (5.3).
        Value::Dict(d) => Ok(matches!(x, Value::Str(k) if d.borrow().contains_key(k))),
        Value::Str(s) => match x {
            Value::Str(sub) => Ok(s.contains(&**sub)),
            other => type_error(format!("'in' on a str needs a str, got {}{}", other.type_name(), handle_hint(other, "h in text", "text.includes(h)"))),
        },
        Value::Bytes(b) => match x {
            Value::Bytes(sub) => Ok(sub.is_empty() || b.windows(sub.len()).any(|w| w == &sub[..])),
            Value::Int(n) => Ok((0..=255).contains(n) && b.contains(&(*n as u8))),
            other => type_error(format!("'in' on bytes needs bytes or an int, got {}{}", other.type_name(), handle_hint(other, "h in data", "data.includes(h)"))),
        },
        h @ Value::Handle(_) => type_error(format!("'in' needs a list, dict, str or bytes on the right, got handle{}", handle_hint(h, "x in h", "h.includes(x)"))),
        other => type_error(format!("'in' needs a list, dict, str or bytes on the right, got {}", other.type_name())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(op: BinOp, a: Value, b: Value) -> String {
        match binop(op, &a, &b) {
            Ok(v) => format!("{v:?}"),
            Err(_) => "error".into(),
        }
    }

    fn err(op: BinOp, a: Value, b: Value) -> String {
        match binop(op, &a, &b) {
            Err(super::super::error::Flow::Error(e)) => format!("{}: {}", e.kind, e.message),
            _ => "no error".into(),
        }
    }

    #[test]
    fn arithmetic_rules() {
        use Value::{Float, Int};
        assert_eq!(ok(BinOp::Div, Int(7), Int(2)), "3.5");
        assert_eq!(ok(BinOp::Rem, Int(-7), Int(3)), "2");
        assert_eq!(ok(BinOp::Rem, Int(7), Int(-3)), "-2");
        assert_eq!(ok(BinOp::Rem, Int(i64::MIN), Int(-1)), "0");
        assert_eq!(ok(BinOp::Rem, Float(-7.5), Float(2.0)), "0.5");
        assert_eq!(ok(BinOp::Pow, Int(2), Int(10)), "1024");
        assert_eq!(ok(BinOp::Pow, Int(2), Int(-1)), "0.5");
        assert_eq!(ok(BinOp::Pow, Int(-1), Int(5_000_000_001)), "-1");
        assert_eq!(err(BinOp::Pow, Int(2), Int(64)), "ValueError: integer overflow in '**'");
        assert_eq!(err(BinOp::Pow, Int(0), Int(-1)), "ValueError: division by zero");
        assert_eq!(err(BinOp::Pow, Float(0.0), Int(-2)), "ValueError: division by zero");
        assert_eq!(err(BinOp::Add, Int(i64::MAX), Int(1)), "ValueError: integer overflow in '+'");
        assert_eq!(err(BinOp::Div, Int(1), Float(0.0)), "ValueError: division by zero");
        assert_eq!(err(BinOp::Rem, Int(1), Int(0)), "ValueError: division by zero");
        let count = "to count with text such as input() gives, convert it: int(x) or float(x)";
        assert_eq!(err(BinOp::Add, Value::str("a"), Int(1)), format!("TypeError: unsupported operand types for +: 'str' and 'int' — to join text, use f\"...${{x}}\" or str(x); {count}"));
        assert_eq!(err(BinOp::Sub, Value::str("a"), Int(1)), format!("TypeError: unsupported operand types for -: 'str' and 'int' — {count}"));
        assert_eq!(ok(BinOp::Mul, Value::str("ab"), Int(3)), "\"ababab\"");
        assert_eq!(ok(BinOp::Mul, Int(-1), Value::str("ab")), "\"\"");
        // Past what memory holds: refused before anything is reserved (5.3).
        assert_eq!(err(BinOp::Mul, Value::str("ab"), Int(i64::MAX)), "ValueError: repetition result too large: 18446744073709551614 bytes of text (1 GiB at most)");
        assert_eq!(err(BinOp::Mul, Value::list(vec![Int(0)]), Int(200_000_000)), "ValueError: repetition result too large: 200000000 items (100000000 at most)");
        assert_eq!(err(BinOp::Mul, Value::str("abc"), Int(i64::MAX)), "ValueError: repetition result too large");
    }

    #[test]
    fn equality_and_order_are_exact() {
        use Value::{Float, Int};
        assert_eq!(ok(BinOp::Eq, Int(1), Float(1.0)), "true");
        assert_eq!(ok(BinOp::Eq, Int(9_007_199_254_740_993), Float(9_007_199_254_740_992.0)), "false");
        assert_eq!(ok(BinOp::Lt, Int(9_007_199_254_740_993), Float(9_007_199_254_740_992.0)), "false");
        assert_eq!(ok(BinOp::Gt, Int(9_007_199_254_740_993), Float(9_007_199_254_740_992.0)), "true");
        assert_eq!(ok(BinOp::Lt, Int(i64::MAX), Float(TWO_63)), "true");
        assert_eq!(ok(BinOp::Lt, Int(1), Float(1.5)), "true");
        assert_eq!(ok(BinOp::Gt, Int(-1), Float(-1.5)), "true");
        assert_eq!(ok(BinOp::Eq, Float(f64::NAN), Float(f64::NAN)), "false");
        assert_eq!(ok(BinOp::Lt, Int(1), Float(f64::NAN)), "false");
        assert_eq!(ok(BinOp::Eq, Int(1), Value::Bool(true)), "false");
        assert_eq!(err(BinOp::Lt, Value::list(vec![]), Int(1)), "TypeError: cannot compare list and int with '<'");
        assert_eq!(ok(BinOp::Lt, Value::str("B"), Value::str("a")), "true");
        let a = Value::list(vec![Int(1), Value::list(vec![Int(2)])]);
        let b = Value::list(vec![Float(1.0), Value::list(vec![Int(2)])]);
        assert_eq!(ok(BinOp::Eq, a, b), "true");
    }

    #[test]
    fn deep_values_raise_recursion_error() {
        let mut a = Value::list(vec![]);
        let mut b = Value::list(vec![]);
        for _ in 0..1_100 {
            a = Value::list(vec![a]);
            b = Value::list(vec![b]);
        }
        assert_eq!(err(BinOp::Eq, a, b), "RecursionError: value nested too deeply (or cyclic)");
    }
}
