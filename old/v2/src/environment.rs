use std::collections::HashMap;
use crate::value::Value;

/// A single scope frame.
#[derive(Debug, Clone)]
pub struct Scope {
    pub vars: HashMap<String, Value>,
    pub consts: HashMap<String, bool>, // tracks which names are const
    pub parent: Option<usize>,         // index of parent scope
}

/// Environment: a chain of scopes.
pub struct Environment {
    scopes: Vec<Scope>,
    pub current: usize,
}

impl Environment {
    pub fn new() -> Self {
        let global = Scope {
            vars: HashMap::new(),
            consts: HashMap::new(),
            parent: None,
        };
        Self {
            scopes: vec![global],
            current: 0,
        }
    }

    /// Push a new child scope and make it current.
    pub fn push_scope(&mut self) -> usize {
        let idx = self.scopes.len();
        self.scopes.push(Scope {
            vars: HashMap::new(),
            consts: HashMap::new(),
            parent: Some(self.current),
        });
        self.current = idx;
        idx
    }

    /// Push a new scope with a specific parent (for closures).
    pub fn push_scope_with_parent(&mut self, parent: usize) -> usize {
        let idx = self.scopes.len();
        self.scopes.push(Scope {
            vars: HashMap::new(),
            consts: HashMap::new(),
            parent: Some(parent),
        });
        self.current = idx;
        idx
    }

    /// Pop back to the parent scope.
    pub fn pop_scope(&mut self) {
        if let Some(parent) = self.scopes[self.current].parent {
            self.current = parent;
        }
    }

    /// Restore to a specific scope index.
    pub fn set_scope(&mut self, idx: usize) {
        self.current = idx;
    }

    /// Define a variable in the current scope.
    pub fn define(&mut self, name: &str, value: Value) {
        self.scopes[self.current]
            .vars
            .insert(name.to_string(), value);
    }

    /// Define a constant in the current scope.
    pub fn define_const(&mut self, name: &str, value: Value) {
        self.scopes[self.current]
            .vars
            .insert(name.to_string(), value);
        self.scopes[self.current]
            .consts
            .insert(name.to_string(), true);
    }

    /// Get a variable, searching up the scope chain.
    pub fn get(&self, name: &str) -> Option<Value> {
        let mut scope_idx = self.current;
        loop {
            if let Some(val) = self.scopes[scope_idx].vars.get(name) {
                return Some(val.clone());
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                return None;
            }
        }
    }

    /// Set (assign) a variable, searching up the scope chain.
    /// Returns Err if the variable is const or not found.
    pub fn set(&mut self, name: &str, value: Value) -> Result<(), String> {
        let mut scope_idx = self.current;
        loop {
            if self.scopes[scope_idx].vars.contains_key(name) {
                if self.scopes[scope_idx].consts.contains_key(name) {
                    return Err(format!("Cannot reassign constant '{}'", name));
                }
                self.scopes[scope_idx]
                    .vars
                    .insert(name.to_string(), value);
                return Ok(());
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                return Err(format!("Undefined variable '{}'", name));
            }
        }
    }

    /// Get a mutable reference to a list/dict in the environment (for index assignment).
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        let mut scope_idx = self.current;
        loop {
            if self.scopes[scope_idx].vars.contains_key(name) {
                return self.scopes[scope_idx].vars.get_mut(name);
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                return None;
            }
        }
    }

    /// Get all variables in the current scope only (not parent scopes).
    pub fn current_scope_vars(&self) -> Vec<(String, Value)> {
        self.scopes[self.current]
            .vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Get all visible variables (current scope + parents) as a flat HashMap.
    pub fn get_current_scope_dict(&self) -> Vec<(String, Value)> {
        let mut result: HashMap<String, Value> = HashMap::new();
        let mut scope_idx = self.current;
        // Walk up the scope chain; inner scopes shadow outer
        let mut chain = Vec::new();
        loop {
            chain.push(scope_idx);
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        // Process from outermost to innermost so inner shadows outer
        for &idx in chain.iter().rev() {
            for (k, v) in &self.scopes[idx].vars {
                result.insert(k.clone(), v.clone());
            }
        }
        result.into_iter().collect()
    }

    /// Find the closest match for an undefined variable name using Levenshtein distance
    pub fn did_you_mean(&self, name: &str) -> Option<String> {
        let mut best: Option<(String, usize)> = None;
        let max_dist = 3.min(name.len());  // only suggest if within 3 edits
        let mut scope_idx = self.current;
        loop {
            for key in self.scopes[scope_idx].vars.keys() {
                let dist = levenshtein(name, key);
                if dist > 0 && dist <= max_dist {
                    if best.as_ref().map_or(true, |(_, d)| dist < *d) {
                        best = Some((key.clone(), dist));
                    }
                }
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        best.map(|(s, _)| s)
    }
}

/// Classic Levenshtein distance
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    let mut prev = (0..=n).collect::<Vec<usize>>();
    let mut curr = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i-1] == b[j-1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j-1] + 1).min(prev[j-1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}
