//! Argument binding, shared by V2 functions, builtins and methods (INTERNALS 5.7 steps 1-3).

use std::rc::Rc;

use super::error::{fail, with_suggestion, Exec};
use super::value::Value;

/// A builtin or method parameter. Parameters after the rest parameter are named-only.
pub struct ParamSpec {
    pub name: &'static str,
    pub default: Option<fn() -> Value>,
    pub rest: bool,
}

impl ParamSpec {
    pub const fn req(name: &'static str) -> ParamSpec {
        ParamSpec { name, default: None, rest: false }
    }

    pub const fn opt(name: &'static str, default: fn() -> Value) -> ParamSpec {
        ParamSpec { name, default: Some(default), rest: false }
    }

    pub const fn rest(name: &'static str) -> ParamSpec {
        ParamSpec { name, default: None, rest: true }
    }
}

/// Named arguments, in call order.
pub type Named = Vec<(Rc<str>, Value)>;

/// What the binder needs to know about one parameter.
pub struct Param<'a> {
    pub name: &'a str,
    pub has_default: bool,
    pub rest: bool,
}

fn arity<T>(message: String) -> Exec<T> {
    fail("ArityError", message)
}

fn quoted_list(names: &[&str]) -> String {
    let q: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    match q.len() {
        1 => q[0].clone(),
        _ => format!("{} and {}", q[..q.len() - 1].join(", "), q[q.len() - 1]),
    }
}

/// Bind arguments to parameters. Each parameter gets `Some(value)`, the rest parameter a list,
/// and an unfilled parameter with a default `None` (the caller evaluates it). `fname` names the
/// function in messages (`greet`, `list.get`).
pub fn bind(fname: &str, params: &[Param], pos: Vec<Value>, named: Vec<(Rc<str>, Value)>) -> Exec<Vec<Option<Value>>> {
    let rest = params.iter().position(|p| p.rest);
    let slots_before_rest = rest.unwrap_or(params.len());
    let mut bound: Vec<Option<Value>> = params.iter().map(|_| None).collect();
    let mut rest_items = Vec::new();
    let given = pos.len();
    for (i, v) in pos.into_iter().enumerate() {
        if i < slots_before_rest {
            bound[i] = Some(v);
        } else if rest.is_some() {
            rest_items.push(v);
        } else {
            let n = slots_before_rest;
            return arity(format!(
                "{fname}() takes {n} positional argument{} but {given} {} given",
                if n == 1 { "" } else { "s" },
                if given == 1 { "was" } else { "were" }
            ));
        }
    }
    for (name, v) in named {
        match params.iter().position(|p| p.name == &*name) {
            None => {
                let msg = format!("{fname}() got an unexpected argument '{name}'");
                let open = params.iter().zip(&bound).filter(|(p, b)| !p.rest && b.is_none()).map(|(p, _)| p.name);
                return arity(with_suggestion(msg, &name, open));
            }
            Some(i) if params[i].rest => {
                return arity(format!("{fname}() cannot take rest parameter '{name}' by name"));
            }
            Some(i) if bound[i].is_some() => {
                return arity(format!("{fname}() got multiple values for argument '{name}'"));
            }
            Some(i) => bound[i] = Some(v),
        }
    }
    let missing: Vec<&str> =
        params.iter().zip(&bound).filter(|(p, b)| !p.rest && !p.has_default && b.is_none()).map(|(p, _)| p.name).collect();
    if !missing.is_empty() {
        let s = if missing.len() == 1 { "" } else { "s" };
        return arity(format!("{fname}() missing required argument{s} {}", quoted_list(&missing)));
    }
    if let Some(r) = rest {
        bound[r] = Some(Value::list(rest_items));
    }
    Ok(bound)
}

/// The usual call: positional arguments that fit, the rest with defaults, or a rest parameter
/// last (`push(...items)`) with each parameter before it given. They are already the values in
/// order, so the binder isn't needed. `Err` hands them back for `bind_spec`, which checks them
/// and says what is wrong.
pub fn bind_plain(specs: &[ParamSpec], mut pos: Vec<Value>, named: &[(Rc<str>, Value)]) -> Result<Vec<Value>, Vec<Value>> {
    if !named.is_empty() {
        return Err(pos);
    }
    match specs.iter().position(|s| s.rest) {
        None if pos.len() <= specs.len() => {
            let left = &specs[pos.len()..];
            if left.iter().all(|s| s.default.is_some()) {
                pos.extend(left.iter().map(|s| s.default.map_or(Value::Null, |d| d())));
                return Ok(pos);
            }
            Err(pos)
        }
        Some(r) if r + 1 == specs.len() && pos.len() >= r => {
            let rest = pos.split_off(r);
            pos.push(Value::list(rest));
            Ok(pos)
        }
        _ => Err(pos),
    }
}

/// Bind for a builtin or method: defaults are filled in, so every slot holds a value.
pub fn bind_spec(fname: &str, specs: &[ParamSpec], pos: Vec<Value>, named: Vec<(Rc<str>, Value)>) -> Exec<Vec<Value>> {
    let pos = match bind_plain(specs, pos, &named) {
        Ok(args) => return Ok(args),
        Err(pos) => pos,
    };
    let params: Vec<Param> =
        specs.iter().map(|s| Param { name: s.name, has_default: s.default.is_some(), rest: s.rest }).collect();
    let bound = bind(fname, &params, pos, named)?;
    Ok(bound
        .into_iter()
        .zip(specs)
        .map(|(b, s)| b.unwrap_or_else(|| s.default.map_or(Value::Null, |d| d())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::error::Flow;

    fn p(name: &str, has_default: bool, rest: bool) -> Param<'_> {
        Param { name, has_default, rest }
    }

    fn msg(r: Exec<Vec<Option<Value>>>) -> String {
        match r {
            Err(Flow::Error(e)) => e.message.to_string(),
            _ => "ok".into(),
        }
    }

    #[test]
    fn arity_messages() {
        let greet = [p("name", false, false), p("greeting", true, false)];
        let n = |s: &str| -> Rc<str> { s.into() };
        assert_eq!(msg(bind("greet", &greet, vec![Value::Int(1); 3], vec![])), "greet() takes 2 positional arguments but 3 were given");
        assert_eq!(msg(bind("f", &greet[..1], vec![Value::Int(1); 2], vec![])), "f() takes 1 positional argument but 2 were given");
        assert_eq!(msg(bind("f", &[], vec![Value::Int(1)], vec![])), "f() takes 0 positional arguments but 1 was given");
        assert_eq!(
            msg(bind("greet", &greet, vec![Value::Int(1)], vec![(n("greting"), Value::Null)])),
            "greet() got an unexpected argument 'greting' — did you mean 'greeting'?"
        );
        assert_eq!(msg(bind("greet", &greet, vec![Value::Int(1)], vec![(n("name"), Value::Null)])), "greet() got multiple values for argument 'name'");
        assert_eq!(msg(bind("greet", &greet, vec![], vec![])), "greet() missing required argument 'name'");
        let three = [p("a", false, false), p("b", false, false), p("c", false, false)];
        assert_eq!(msg(bind("f", &three, vec![], vec![])), "f() missing required arguments 'a', 'b' and 'c'");
        let total = [p("nums", false, true)];
        assert_eq!(msg(bind("total", &total, vec![], vec![(n("nums"), Value::Null)])), "total() cannot take rest parameter 'nums' by name");
        let ok = bind("total", &total, vec![Value::Int(1), Value::Int(2)], vec![]).ok().unwrap();
        assert!(matches!(&ok[0], Some(Value::List(l)) if l.borrow().len() == 2));
    }
}
