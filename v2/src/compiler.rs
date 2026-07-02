/// Compiler: walks the V2 AST and emits bytecode.

use std::collections::HashMap;
use crate::ast::*;
use crate::bytecode::{Chunk, CompiledFunc, Op};
use crate::value::Value;

/// Upvalue descriptor: captures a local from enclosing scope.
#[derive(Debug, Clone)]
struct Upvalue {
    /// Index in the enclosing function's locals (if is_local) or upvalues.
    index: u16,
    /// True if this captures a local from the immediately enclosing function.
    is_local: bool,
}

/// A local variable in the current compilation scope.
#[derive(Debug, Clone)]
struct Local {
    name: String,
    depth: i32,
    is_captured: bool,
}

/// One scope level being compiled (top-level, function, block).
struct CompilerFrame {
    function: CompiledFunc,
    locals: Vec<Local>,
    upvalues: Vec<Upvalue>,
    scope_depth: i32,
    /// Loop context stack: (loop_start, break_patch_list)
    loops: Vec<LoopCtx>,
    /// Label positions for goto
    labels: HashMap<String, usize>,
    /// Pending gotos to patch
    pending_gotos: Vec<(String, usize)>,
}

struct LoopCtx {
    start: usize,
    break_patches: Vec<usize>,
    continue_patches: Vec<usize>,
    label: Option<String>,
    scope_depth: i32,
}

pub struct Compiler {
    frames: Vec<CompilerFrame>,
    /// Interned class/struct/enum/trait definitions available to VM at runtime.
    pub class_defs: HashMap<String, ClassDef>,
    pub struct_defs: HashMap<String, StructDef>,
    pub enum_defs: HashMap<String, EnumDef>,
    pub trait_defs: HashMap<String, TraitDef>,
    pub impl_blocks: Vec<ImplDef>,
    /// Compiled functions by name â€” passed to VM for Closure lookups.
    pub func_store: HashMap<String, CompiledFunc>,
    /// Pending label to attach to next loop
    pending_loop_label: Option<String>,
    lambda_counter: usize,
    do_counter: usize,
}

#[derive(Debug, Clone)]
pub struct ClassDef {
    pub name: String,
    pub parent: Option<String>,
    pub methods: Vec<(String, CompiledFunc)>,
    pub fields: Vec<(String, Option<Value>)>,
    pub is_fixed: bool,
    pub is_data: bool,
    pub is_sealed: bool,
    pub decorators: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone)]
pub struct TraitDef {
    pub name: String,
    pub supertraits: Vec<String>,
    pub method_names: Vec<String>,
    pub method_funcs: Vec<CompiledFunc>,
}

#[derive(Debug, Clone)]
pub struct ImplDef {
    pub trait_name: Option<String>,
    pub target: String,
    pub methods: Vec<(String, CompiledFunc)>,
}

impl Compiler {
    pub fn new() -> Self {
        let top_frame = CompilerFrame {
            function: CompiledFunc {
                name: "<script>".to_string(),
                arity: 0,
                has_variadic: false,
                default_count: 0,
                upvalue_count: 0,
                is_generator: false,
                chunk: Chunk::new(),
            },
            locals: Vec::new(),
            upvalues: Vec::new(),
            scope_depth: 0,
            loops: Vec::new(),
            labels: HashMap::new(),
            pending_gotos: Vec::new(),
        };
        Compiler {
            frames: vec![top_frame],
            class_defs: HashMap::new(),
            struct_defs: HashMap::new(),
            enum_defs: HashMap::new(),
            trait_defs: HashMap::new(),
            impl_blocks: Vec::new(),
            func_store: HashMap::new(),
            pending_loop_label: None,
            lambda_counter: 0,
            do_counter: 0,
        }
    }

    // â”€â”€ Helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn current(&mut self) -> &mut CompilerFrame {
        self.frames.last_mut().unwrap()
    }

    fn chunk(&mut self) -> &mut Chunk {
        &mut self.frames.last_mut().unwrap().function.chunk
    }

    fn emit(&mut self, op: Op, line: u32) {
        self.chunk().write_op(op, line);
    }

    fn emit_u16(&mut self, val: u16, line: u32) {
        self.chunk().write_u16(val, line);
    }

    fn emit_byte(&mut self, byte: u8, line: u32) {
        self.chunk().write(byte, line);
    }

    fn emit_constant(&mut self, val: Value, line: u32) {
        let idx = self.chunk().add_constant(val);
        self.emit(Op::Constant, line);
        self.emit_u16(idx, line);
    }

    fn add_string(&mut self, s: &str) -> u16 {
        self.chunk().add_string(s)
    }

    fn emit_jump(&mut self, op: Op, line: u32) -> usize {
        self.emit(op, line);
        let offset = self.chunk().len();
        self.emit_u16(0xFFFF, line); // placeholder
        offset
    }

    fn patch_jump(&mut self, offset: usize) {
        let jump = self.chunk().len() - offset - 2;
        assert!(jump <= u16::MAX as usize, "Jump too large");
        self.chunk().patch_u16(offset, jump as u16);
    }

    fn emit_loop(&mut self, loop_start: usize, line: u32) {
        self.emit(Op::Loop, line);
        let offset = self.chunk().len() + 2 - loop_start;
        assert!(offset <= u16::MAX as usize, "Loop body too large");
        self.emit_u16(offset as u16, line);
    }

    fn begin_scope(&mut self) {
        self.current().scope_depth += 1;
    }

    fn end_scope(&mut self, line: u32) {
        let depth = self.current().scope_depth;
        self.current().scope_depth -= 1;
        // Pop locals that go out of scope
        while let Some(last) = self.current().locals.last() {
            if last.depth <= depth - 1 {
                break;
            }
            let captured = last.is_captured;
            self.current().locals.pop();
            if captured {
                self.emit(Op::CloseUpvalue, line);
            } else {
                self.emit(Op::Pop, line);
            }
        }
    }

    fn add_local(&mut self, name: &str) -> u16 {
        let depth = self.current().scope_depth;
        let slot = self.current().locals.len() as u16;
        self.current().locals.push(Local {
            name: name.to_string(),
            depth,
            is_captured: false,
        });
        slot
    }

    fn resolve_local(&self, name: &str) -> Option<u16> {
        let frame = self.frames.last().unwrap();
        for (i, local) in frame.locals.iter().enumerate().rev() {
            if local.name == name {
                return Some(i as u16);
            }
        }
        None
    }

    fn resolve_upvalue(&mut self, name: &str) -> Option<u16> {
        if self.frames.len() < 2 {
            return None;
        }
        let frame_count = self.frames.len();
        self._resolve_upvalue_recursive(frame_count - 1, name)
    }

    fn _resolve_upvalue_recursive(&mut self, frame_idx: usize, name: &str) -> Option<u16> {
        if frame_idx == 0 {
            return None;
        }
        // Check locals in enclosing frame
        let enclosing = &self.frames[frame_idx - 1];
        for (i, local) in enclosing.locals.iter().enumerate().rev() {
            if local.name == name {
                self.frames[frame_idx - 1].locals[i].is_captured = true;
                return Some(self._add_upvalue(frame_idx, i as u16, true));
            }
        }
        // Check upvalues in enclosing frame
        if let Some(upval_idx) = self._resolve_upvalue_recursive(frame_idx - 1, name) {
            return Some(self._add_upvalue(frame_idx, upval_idx, false));
        }
        None
    }

    fn _add_upvalue(&mut self, frame_idx: usize, index: u16, is_local: bool) -> u16 {
        let frame = &mut self.frames[frame_idx];
        // Check if we already have this upvalue
        for (i, uv) in frame.upvalues.iter().enumerate() {
            if uv.index == index && uv.is_local == is_local {
                return i as u16;
            }
        }
        let idx = frame.upvalues.len() as u16;
        frame.upvalues.push(Upvalue { index, is_local });
        frame.function.upvalue_count = idx + 1;
        idx
    }

    // â”€â”€ Public API â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    pub fn compile(mut self, program: &Program) -> Result<CompiledFunc, String> {
        for stmt in &program.stmts {
            self.compile_stmt(stmt, 1)?;
        }
        // Patch pending gotos
        let frame = self.frames.last().unwrap();
        let pending: Vec<(String, usize)> = frame.pending_gotos.clone();
        let labels = frame.labels.clone();
        for (label, patch_offset) in &pending {
            if let Some(&target) = labels.get(label) {
                let current = self.chunk().len();
                // Compute relative jumpâ€¦ this is a forward or backward jump
                // For simplicity: emit as forward jump, adjust
                let jump = if target > *patch_offset + 2 {
                    target - *patch_offset - 2
                } else {
                    0
                };
                self.chunk().patch_u16(*patch_offset, jump as u16);
            } else {
                return Err(format!("Undefined label '{}'", label));
            }
        }

        self.emit(Op::Halt, 0);
        Ok(self.frames.pop().unwrap().function)
    }

    // â”€â”€ Statement Compilation â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_stmt(&mut self, stmt: &Stmt, line: u32) -> Result<(), String> {
        match stmt {
            Stmt::Expr(expr) => {
                self.compile_expr(expr, line)?;
                self.emit(Op::Pop, line);
            }
            Stmt::Let { name, value, .. } => {
                let is_lazy = matches!(value, Some(Expr::Lazy(_)));
                if let Some(val) = value {
                    self.compile_expr(val, line)?;
                } else {
                    self.emit(Op::Null, line);
                }
                if self.current().scope_depth > 0 {
                    self.add_local(name);
                    if is_lazy {
                        let idx = self.add_string(name);
                        self.emit(Op::MarkLazy, line);
                        self.emit_u16(idx, line);
                    }
                } else {
                    let idx = self.add_string(name);
                    self.emit(Op::DefineGlobal, line);
                    self.emit_u16(idx, line);
                    if is_lazy {
                        let idx2 = self.add_string(name);
                        self.emit(Op::MarkLazy, line);
                        self.emit_u16(idx2, line);
                    }
                }
            }
            Stmt::Const { name, value, .. } => {
                self.compile_expr(value, line)?;
                if self.current().scope_depth > 0 {
                    self.add_local(name);
                } else {
                    let idx = self.add_string(name);
                    self.emit(Op::DefineGlobal, line);
                    self.emit_u16(idx, line);
                }
            }
            Stmt::Assign { target, op, value } => {
                self.compile_assignment(target, op, value, line)?;
                self.emit(Op::Pop, line);
            }
            Stmt::Return(expr) => {
                if let Some(e) = expr {
                    self.compile_expr(e, line)?;
                } else {
                    self.emit(Op::Null, line);
                }
                self.emit(Op::Return, line);
            }
            Stmt::If { condition, body, else_ifs, else_body } => {
                self.compile_if(condition, body, else_ifs, else_body, line)?;
            }
            Stmt::While { condition, body } => {
                let label = self.pending_loop_label.take();
                self.compile_while(condition, body, label, line)?;
            }
            Stmt::ForIn { var, iter, body } => {
                let label = self.pending_loop_label.take();
                self.compile_for_in_labeled(var, iter, body, label, line)?;
            }
            Stmt::ForInDestructure { vars, iter, body } => {
                let label = self.pending_loop_label.take();
                self.compile_for_in_destructure(vars, iter, body, line)?;
            }
            Stmt::ForClassic { init, condition, update, body } => {
                let label = self.pending_loop_label.take();
                self.compile_for_classic_labeled(init, condition, update, body, label, line)?;
            }
            Stmt::FuncDecl { name, params, body, is_generator, decorators, .. } => {
                self.compile_func_decl(name, params, body, *is_generator, line)?;
            }
            Stmt::Block(stmts) => {
                self.begin_scope();
                for s in stmts {
                    self.compile_stmt(s, line)?;
                }
                self.end_scope(line);
            }
            Stmt::Break => {
                self.compile_break(None, line)?;
            }
            Stmt::BreakLabel(lbl) => {
                self.compile_break(Some(lbl), line)?;
            }
            Stmt::Continue => {
                self.compile_continue(None, line)?;
            }
            Stmt::ContinueLabel(lbl) => {
                self.compile_continue(Some(lbl), line)?;
            }
            Stmt::Match { subject, arms } => {
                self.compile_match(subject, arms, line)?;
            }
            Stmt::Throw(expr) => {
                self.compile_expr(expr, line)?;
                self.emit(Op::Throw, line);
            }
            Stmt::TryCatch { body, catch_var, catch_body, finally_body, .. } => {
                self.compile_try_catch(body, catch_var, catch_body, finally_body, line)?;
            }
            Stmt::ClassDecl { name, parent, body, decorators, is_sealed, .. } => {
                self.compile_class(name, parent, body, decorators, *is_sealed, line)?;
            }
            Stmt::StructDecl { name, fields, .. } => {
                self.compile_struct_decl(name, fields, line)?;
            }
            Stmt::EnumDecl { name, variants, .. } => {
                self.compile_enum_decl(name, variants, line)?;
            }
            Stmt::TraitDecl { name, supertraits, methods, .. } => {
                self.compile_trait_decl(name, supertraits, methods, line)?;
            }
            Stmt::ImplBlock { trait_name, target, methods } => {
                self.compile_impl_block(trait_name, target, methods, line)?;
            }
            Stmt::Import { path, alias, names } => {
                self.compile_import(path, alias, names, line)?;
            }
            Stmt::Label(name) => {
                let pos = self.chunk().len();
                self.current().labels.insert(name.clone(), pos);
                // Also set as pending loop label for labeled break/continue
                self.pending_loop_label = Some(name.clone());
            }
            Stmt::Goto(name) => {
                let patch = self.emit_jump(Op::Jump, line);
                self.current().pending_gotos.push((name.clone(), patch));
            }
            Stmt::Yield(expr) => {
                if let Some(e) = expr {
                    self.compile_expr(e, line)?;
                } else {
                    self.emit(Op::Null, line);
                }
                self.emit(Op::Yield, line);
            }
            Stmt::Defer(stmts) => {
                // Defer: compile body now, it will be stored and executed at scope exit
                // For simplicity in the bytecode VM, defer is compiled inline at scope end
                // Store it as a deferred chunk and emit at end_scope
                // For now: just compile inline (TODO: proper defer semantics)
                for s in stmts {
                    self.compile_stmt(s, line)?;
                }
            }
            Stmt::Multi(stmts) => {
                for s in stmts {
                    self.compile_stmt(s, line)?;
                }
            }
            Stmt::TypeAlias { .. } => {
                // Type aliases are compile-time only, no bytecode needed
            }
            Stmt::NewtypeDecl { name, .. } => {
                let idx = self.chunk().add_constant(Value::Str(name.clone()));
                self.emit(Op::Constant, line);
                self.emit_u16(idx, line);
                let gi = self.add_string(&name);
                self.emit(Op::DefineGlobal, line);
                self.emit_u16(gi, line);
            }
            Stmt::Using { expr, body } => {
                self.compile_expr(expr, line)?;
                self.emit(Op::UsingExtract, line);
                if let Some(stmts) = body {
                    for s in stmts {
                        self.compile_stmt(s, line)?;
                    }
                }
            }
            Stmt::ComptimeBlock { body } => {
                // Comptime blocks: compile and execute like normal for now
                for s in body {
                    self.compile_stmt(s, line)?;
                }
            }
            Stmt::MacroDecl { name, params, body } => {
                // Macros: compile as a function, but return last expression
                let func_params: Vec<Param> = params.iter().map(|p| Param {
                    name: p.clone(),
                    type_ann: None,
                    default: None,
                    is_variadic: false,
                }).collect();
                // Transform last Expr stmt into Return
                let mut macro_body = body.clone();
                if let Some(last) = macro_body.last_mut() {
                    if let Stmt::Expr(e) = last {
                        *last = Stmt::Return(Some(e.clone()));
                    }
                }
                self.compile_func_decl(name, &func_params, &macro_body, false, line)?;
            }
            Stmt::UnsafeBlock { body } => {
                for s in body {
                    self.compile_stmt(s, line)?;
                }
            }
            Stmt::StaticAssert { condition, message } => {
                self.compile_expr(condition, line)?;
                let msg_idx = self.chunk().add_constant(Value::Str(message.clone()));
                let jump = self.emit_jump(Op::JumpIfTrue, line);
                self.emit(Op::Constant, line);
                self.emit_u16(msg_idx, line);
                self.emit(Op::Throw, line);
                self.patch_jump(jump);
                self.emit(Op::Pop, line);
            }
            Stmt::ActorDecl { name, is_agent, body, .. } => {
                // Actors/agents: compile body in a scope, then define global
                self.begin_scope();
                for s in body {
                    self.compile_stmt(s, line)?;
                }
                self.end_scope(line);
                // Define a global dict: {"__kind": kind, "name": name}
                let kind = if *is_agent { "agent" } else { "actor" };
                let kind_key = self.chunk().add_constant(Value::Str("__kind".into()));
                self.emit(Op::Constant, line); self.emit_u16(kind_key, line);
                let kind_val = self.chunk().add_constant(Value::Str(kind.into()));
                self.emit(Op::Constant, line); self.emit_u16(kind_val, line);
                let name_key = self.chunk().add_constant(Value::Str("name".into()));
                self.emit(Op::Constant, line); self.emit_u16(name_key, line);
                let name_val = self.chunk().add_constant(Value::Str(name.clone()));
                self.emit(Op::Constant, line); self.emit_u16(name_val, line);
                self.emit(Op::BuildDict, line); self.emit_u16(2, line);
                let gidx = self.add_string(name);
                self.emit(Op::DefineGlobal, line);
                self.emit_u16(gidx, line);
            }
            Stmt::IsolateBlock { body, .. } => {
                self.begin_scope();
                for s in body {
                    self.compile_stmt(s, line)?;
                }
                self.end_scope(line);
            }
            Stmt::CStructDecl { name, fields, .. }
            | Stmt::InlineStructDecl { name, fields, .. } => {
                let field_defs: Vec<(String, Option<String>)> = fields.iter()
                    .map(|f| (f.name.clone(), f.type_ann.clone()))
                    .collect();
                self.struct_defs.insert(name.clone(), StructDef {
                    name: name.clone(),
                    fields: field_defs,
                });
                let idx = self.add_string(name);
                self.emit(Op::DefineStruct, line);
                self.emit_u16(idx, line);
                self.emit_u16(fields.len() as u16, line);
                for f in fields {
                    let fidx = self.add_string(&f.name);
                    self.emit_u16(fidx, line);
                }
                if self.current().scope_depth > 0 {
                    self.add_local(name);
                } else {
                    let gidx = self.add_string(name);
                    self.emit(Op::DefineGlobal, line);
                    self.emit_u16(gidx, line);
                }
            }
            Stmt::BitfieldStructDecl { name, fields, .. } => {
                let field_defs: Vec<(String, Option<String>)> = fields.iter()
                    .map(|(n, _)| (n.clone(), None))
                    .collect();
                self.struct_defs.insert(name.clone(), StructDef {
                    name: name.clone(),
                    fields: field_defs,
                });
                let idx = self.add_string(name);
                self.emit(Op::DefineStruct, line);
                self.emit_u16(idx, line);
                self.emit_u16(fields.len() as u16, line);
                for (n, _) in fields {
                    let fidx = self.add_string(n);
                    self.emit_u16(fidx, line);
                }
                if self.current().scope_depth > 0 {
                    self.add_local(name);
                } else {
                    let gidx = self.add_string(name);
                    self.emit(Op::DefineGlobal, line);
                    self.emit_u16(gidx, line);
                }
            }
            Stmt::IfLet { pattern, var, expr, body, else_body } => {
                self.compile_if_let(pattern, var, expr, body, else_body, line)?;
            }
            Stmt::WhileLet { pattern, var, expr, body } => {
                self.compile_while_let(pattern, var, expr, body, line)?;
            }
            Stmt::LetElse { pattern, var, expr, else_body } => {
                self.compile_let_else(pattern, var, expr, else_body, line)?;
            }
            Stmt::TestBlock { .. } | Stmt::BenchBlock { .. } => {
                // Test/bench blocks: skip in normal compilation mode
            }
            // Stubs for features that don't need bytecode emission
            Stmt::EnableLangs { .. }
            | Stmt::EmbeddedLangBlock { .. }
            | Stmt::AsmBlock { .. }
            | Stmt::SourceDirective { .. } => {
                // No-ops in compiled mode
            }
        }
        Ok(())
    }

    // â”€â”€ Expression Compilation â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_expr(&mut self, expr: &Expr, line: u32) -> Result<(), String> {
        match expr {
            Expr::Int(n) => self.emit_constant(Value::Int(*n), line),
            Expr::BigIntLit(s) => {
                let v = crate::bigint::BigInt::from_str(s)
                    .map(|b| match b.to_i64() { Some(i) => Value::Int(i), None => Value::BigInt(b) })
                    .unwrap_or(Value::Int(0));
                self.emit_constant(v, line)
            }
            Expr::Float(f) => self.emit_constant(Value::Float(*f), line),
            Expr::Str(s) => self.emit_constant(Value::Str(s.clone()), line),
            Expr::Bool(true) => self.emit(Op::True, line),
            Expr::Bool(false) => self.emit(Op::False, line),
            Expr::Null => self.emit(Op::Null, line),
            Expr::ByteStr(b) => self.emit_constant(Value::Bytes(b.clone()), line),

            Expr::Ident(name) => {
                self.compile_variable_get(name, line)?;
            }
            Expr::Self_ => {
                self.compile_variable_get("self", line)?;
            }

            Expr::BinOp { left, op, right } => {
                // Short-circuit for And/Or
                match op {
                    BinOp::And => {
                        self.compile_expr(left, line)?;
                        let jump = self.emit_jump(Op::JumpIfFalse, line);
                        self.emit(Op::Pop, line);
                        self.compile_expr(right, line)?;
                        self.patch_jump(jump);
                        return Ok(());
                    }
                    BinOp::Or => {
                        self.compile_expr(left, line)?;
                        let jump = self.emit_jump(Op::JumpIfTrue, line);
                        self.emit(Op::Pop, line);
                        self.compile_expr(right, line)?;
                        self.patch_jump(jump);
                        return Ok(());
                    }
                    BinOp::NullCoalesce => {
                        self.compile_expr(left, line)?;
                        self.emit(Op::Dup, line);
                        let non_null_jump = self.emit_jump(Op::JumpIfTrue, line);
                        // Null path: pop dup and original, use right
                        self.emit(Op::Pop, line);
                        self.emit(Op::Pop, line);
                        self.compile_expr(right, line)?;
                        let end_jump = self.emit_jump(Op::Jump, line);
                        // Non-null path: pop dup, unwrap original
                        self.patch_jump(non_null_jump);
                        self.emit(Op::Pop, line);
                        self.emit(Op::UnwrapSome, line);
                        self.patch_jump(end_jump);
                        return Ok(());
                    }
                    _ => {}
                }
                self.compile_expr(left, line)?;
                self.compile_expr(right, line)?;
                match op {
                    BinOp::Add => self.emit(Op::Add, line),
                    BinOp::Sub => self.emit(Op::Sub, line),
                    BinOp::Mul => self.emit(Op::Mul, line),
                    BinOp::Div => self.emit(Op::Div, line),
                    BinOp::Mod => self.emit(Op::Mod, line),
                    BinOp::Pow => self.emit(Op::Pow, line),
                    BinOp::IntDiv => self.emit(Op::IntDiv, line),
                    BinOp::Eq => self.emit(Op::Eq, line),
                    BinOp::NotEq => self.emit(Op::NotEq, line),
                    BinOp::Lt => self.emit(Op::Lt, line),
                    BinOp::Gt => self.emit(Op::Gt, line),
                    BinOp::LtEq => self.emit(Op::LtEq, line),
                    BinOp::GtEq => self.emit(Op::GtEq, line),
                    BinOp::BitAnd => self.emit(Op::BitAnd, line),
                    BinOp::BitOr => self.emit(Op::BitOr, line),
                    BinOp::BitXor => self.emit(Op::BitXor, line),
                    BinOp::Shl => self.emit(Op::Shl, line),
                    BinOp::Shr => self.emit(Op::Shr, line),
                    BinOp::In => self.emit(Op::In, line),
                    BinOp::NotIn => self.emit(Op::NotIn, line),
                    BinOp::Is => self.emit(Op::Is, line),
                    BinOp::And | BinOp::Or | BinOp::NullCoalesce => unreachable!(),
                }
            }

            Expr::UnaryOp { op, expr: inner } => {
                self.compile_expr(inner, line)?;
                match op {
                    UnaryOp::Neg => self.emit(Op::Neg, line),
                    UnaryOp::Not => self.emit(Op::Not, line),
                    UnaryOp::BitNot => self.emit(Op::BitNot, line),
                }
            }

            Expr::Call { callee, args } => {
                self.compile_expr(callee, line)?;
                let argc = args.len() as u8;
                for arg in args {
                    self.compile_expr(&arg.value, line)?;
                }
                self.emit(Op::Call, line);
                self.emit_byte(argc, line);
            }

            Expr::MethodCall { object, method, args, .. } => {
                self.compile_expr(object, line)?;
                let name_idx = self.add_string(method);
                self.emit(Op::GetField, line);
                self.emit_u16(name_idx, line);
                // Push object as first arg (for self)
                // Actually: we need to handle method calls. Push object, get method, call.
                // The VM will handle bound methods.
                let argc = args.len() as u8;
                for arg in args {
                    self.compile_expr(&arg.value, line)?;
                }
                self.emit(Op::Call, line);
                self.emit_byte(argc, line);
            }

            Expr::FieldAccess { object, field, optional } => {
                self.compile_expr(object, line)?;
                if *optional {
                    // Stack: [obj]
                    self.emit(Op::Dup, line);        // [obj, obj]
                    self.emit(Op::Null, line);       // [obj, obj, null]
                    self.emit(Op::Eq, line);         // [obj, eq_result]
                    let skip_jump = self.emit_jump(Op::JumpIfTrue, line);
                    // Not null path: [obj, false]
                    self.emit(Op::Pop, line);        // [obj]
                    let idx = self.add_string(field);
                    self.emit(Op::GetField, line);
                    self.emit_u16(idx, line);        // [field_value]
                    let end_jump = self.emit_jump(Op::Jump, line);
                    // Null path: [obj, true]
                    self.patch_jump(skip_jump);
                    self.emit(Op::Pop, line);        // [obj]
                    self.emit(Op::Pop, line);        // []
                    self.emit(Op::Null, line);       // [null]
                    self.patch_jump(end_jump);
                } else {
                    let idx = self.add_string(field);
                    self.emit(Op::GetField, line);
                    self.emit_u16(idx, line);
                }
            }

            Expr::Index { object, index } => {
                self.compile_expr(object, line)?;
                self.compile_expr(index, line)?;
                self.emit(Op::GetIndex, line);
            }

            Expr::Slice { object, start, end, .. } => {
                self.compile_expr(object, line)?;
                if let Some(s) = start {
                    self.compile_expr(s, line)?;
                } else {
                    self.emit(Op::Null, line);
                }
                if let Some(e) = end {
                    self.compile_expr(e, line)?;
                } else {
                    self.emit(Op::Null, line);
                }
                self.emit(Op::Slice, line);
            }

            Expr::List(items) => {
                // Check if any items are spreads
                let has_spread = items.iter().any(|e| matches!(e, Expr::Spread(_)));
                if has_spread {
                    // Build each segment: spread items push their inner list, 
                    // non-spread items wrap in single-element list
                    // Then concatenate all
                    let mut segment_count = 0u16;
                    for item in items {
                        if let Expr::Spread(inner) = item {
                            self.compile_expr(inner, line)?;
                        } else {
                            self.compile_expr(item, line)?;
                            self.emit(Op::BuildList, line);
                            self.emit_u16(1, line);
                        }
                        segment_count += 1;
                    }
                    // Now we have segment_count lists on stack, concatenate them
                    // Use repeated Add (list + list = concatenated list)
                    for _ in 1..segment_count {
                        self.emit(Op::Add, line);
                    }
                } else {
                    for item in items {
                        self.compile_expr(item, line)?;
                    }
                    let count = items.len() as u16;
                    self.emit(Op::BuildList, line);
                    self.emit_u16(count, line);
                }
            }

            Expr::Dict(pairs) => {
                if pairs.iter().any(|(k, _)| matches!(k, Expr::Spread(_))) {
                    return Err("Dict spread is currently supported only in interpreter mode".to_string());
                }
                for (k, v) in pairs {
                    self.compile_expr(k, line)?;
                    self.compile_expr(v, line)?;
                }
                let count = pairs.len() as u16;
                self.emit(Op::BuildDict, line);
                self.emit_u16(count, line);
            }

            Expr::Tuple(items) => {
                for item in items {
                    self.compile_expr(item, line)?;
                }
                self.emit(Op::BuildTuple, line);
                self.emit_u16(items.len() as u16, line);
            }

            Expr::Set(items) => {
                for item in items {
                    self.compile_expr(item, line)?;
                }
                self.emit(Op::BuildSet, line);
                self.emit_u16(items.len() as u16, line);
            }

            Expr::Lambda { params, body, .. } => {
                self.compile_lambda(params, &[Stmt::Return(Some(*body.clone()))], line)?;
            }

            Expr::LambdaBlock { params, body, .. } => {
                self.compile_lambda(params, body, line)?;
            }

            Expr::Ternary { condition, then_expr, else_expr } => {
                self.compile_expr(condition, line)?;
                let else_jump = self.emit_jump(Op::JumpIfFalse, line);
                self.emit(Op::Pop, line);
                self.compile_expr(then_expr, line)?;
                let end_jump = self.emit_jump(Op::Jump, line);
                self.patch_jump(else_jump);
                self.emit(Op::Pop, line);
                self.compile_expr(else_expr, line)?;
                self.patch_jump(end_jump);
            }

            Expr::Range { start, end, inclusive } => {
                self.compile_expr(start, line)?;
                self.compile_expr(end, line)?;
                self.emit(Op::BuildRange, line);
                self.emit_byte(if *inclusive { 1 } else { 0 }, line);
            }

            Expr::FStr(template) => {
                self.compile_fstring(template, line)?;
            }
            Expr::TaggedTemplate { .. } => {
                return Err("tagged template literals are not supported in the bytecode backend; run with the interpreter".into());
            }

            Expr::New { class, args } => {
                self.compile_variable_get(class, line)?;
                let argc = args.len() as u8;
                for arg in args {
                    self.compile_expr(&arg.value, line)?;
                }
                self.emit(Op::NewInstance, line);
                self.emit_byte(argc, line);
            }

            Expr::Await(inner) => {
                self.compile_expr(inner, line)?;
                self.emit(Op::Await, line);
            }

            Expr::StructLit { name, fields, spread } => {
                // Push field values with names
                for (fname, fval) in fields {
                    let field_idx = self.add_string(fname);
                    self.compile_expr(fval, line)?;
                }
                if let Some(sp) = spread {
                    self.compile_expr(sp, line)?;
                    self.emit(Op::Spread, line);
                }
                let name_idx = self.add_string(name);
                self.emit(Op::BuildStruct, line);
                self.emit_u16(name_idx, line);
                self.emit_u16(fields.len() as u16, line);
            }

            Expr::TypeOf(inner) => {
                self.compile_expr(inner, line)?;
                self.emit(Op::TypeOf, line);
            }

            Expr::DoBlock(stmts) => {
                self.begin_scope();
                for (i, s) in stmts.iter().enumerate() {
                    if i == stmts.len() - 1 {
                        // Last statement: if it's an Expr, leave value on stack
                        if let Stmt::Expr(e) = s {
                            self.compile_expr(e, line)?;
                        } else {
                            self.compile_stmt(s, line)?;
                            self.emit(Op::Null, line);
                        }
                    } else {
                        self.compile_stmt(s, line)?;
                    }
                }
                // Pop locals but preserve the top-of-stack result
                let depth = self.current().scope_depth;
                self.current().scope_depth -= 1;
                let mut pop_count = 0u16;
                while let Some(last) = self.current().locals.last() {
                    if last.depth <= depth - 1 {
                        break;
                    }
                    let captured = last.is_captured;
                    self.current().locals.pop();
                    if captured {
                        // Need to close upvalue individually
                        // Swap result past this local, then close
                        // For simplicity, just close and count
                        pop_count += 1;
                    } else {
                        pop_count += 1;
                    }
                }
                if pop_count > 0 {
                    // Store result in a global temp, pop locals, restore
                    let do_id = self.do_counter;
                    self.do_counter += 1;
                    let tmp_name = format!("__do_tmp_{}", do_id);
                    let idx = self.add_string(&tmp_name);
                    self.emit(Op::DefineGlobal, line);
                    self.emit_u16(idx, line);
                    for _ in 0..pop_count {
                        self.emit(Op::Pop, line);
                    }
                    let gi = self.add_string(&tmp_name);
                    self.emit(Op::GetGlobal, line);
                    self.emit_u16(gi, line);
                }
            }

            Expr::MatchExpr { subject, arms } => {
                self.compile_match_expr(subject, arms, line)?;
            }

            Expr::Cast { expr: inner, target } => {
                self.compile_expr(inner, line)?;
                let idx = self.add_string(target);
                self.emit(Op::Cast, line);
                self.emit_u16(idx, line);
            }

            Expr::TryUnwrap(inner) => {
                // expr? â€” unwrap or early return
                self.compile_expr(inner, line)?;
                // TODO: proper try unwrap in VM
            }

            Expr::Grouped(inner) => {
                self.compile_expr(inner, line)?;
            }

            Expr::Lazy(inner) => {
                // Compile as a lambda that returns the inner expr
                let params = Vec::new();
                let body = vec![Stmt::Return(Some(*inner.clone()))];
                self.compile_lambda(&params, &body, line)?;
            }

            Expr::Spread(inner) => {
                self.compile_expr(inner, line)?;
                self.emit(Op::Spread, line);
            }

            Expr::Pipe { left, right } => {
                // Desugar: left |> right becomes right(left)
                self.compile_expr(right, line)?;
                self.compile_expr(left, line)?;
                self.emit(Op::Call, line);
                self.emit_byte(1, line);
            }

            Expr::MacroCall { name, args } => {
                // Compile like a function call
                self.compile_variable_get(name, line)?;
                let argc = args.len() as u8;
                for arg in args {
                    self.compile_expr(&arg.value, line)?;
                }
                self.emit(Op::Call, line);
                self.emit_byte(argc, line);
            }

            Expr::ListComp { expr, clauses } => {
                if let Some(first) = clauses.first() {
                    self.compile_list_comp(expr, &first.var, &first.iter, &first.cond, line)?;
                }
            }

            Expr::DictComp { key_expr, val_expr, clauses } => {
                if let Some(first) = clauses.first() {
                    self.compile_dict_comp(key_expr, val_expr, &first.var, &first.iter, &first.cond, line)?;
                }
            }

            Expr::SetComp { expr, clauses } => {
                if let Some(first) = clauses.first() {
                    self.compile_set_comp(expr, &first.var, &first.iter, &first.cond, line)?;
                }
            }

            Expr::GenComp { expr, clauses } => {
                // Generator comp: compile as list comp for now (lazy semantics in interpreter)
                if let Some(first) = clauses.first() {
                    self.compile_list_comp(expr, &first.var, &first.iter, &first.cond, line)?;
                }
            }
        }
        Ok(())
    }

    // â”€â”€ Variable access â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_variable_get(&mut self, name: &str, line: u32) -> Result<(), String> {
        if let Some(slot) = self.resolve_local(name) {
            self.emit(Op::GetLocal, line);
            self.emit_u16(slot, line);
        } else if let Some(idx) = self.resolve_upvalue(name) {
            self.emit(Op::GetUpvalue, line);
            self.emit_u16(idx, line);
        } else {
            let idx = self.add_string(name);
            self.emit(Op::GetGlobal, line);
            self.emit_u16(idx, line);
        }
        Ok(())
    }

    fn compile_variable_set(&mut self, name: &str, line: u32) -> Result<(), String> {
        if let Some(slot) = self.resolve_local(name) {
            self.emit(Op::SetLocal, line);
            self.emit_u16(slot, line);
        } else if let Some(idx) = self.resolve_upvalue(name) {
            self.emit(Op::SetUpvalue, line);
            self.emit_u16(idx, line);
        } else {
            let idx = self.add_string(name);
            self.emit(Op::SetGlobal, line);
            self.emit_u16(idx, line);
        }
        Ok(())
    }

    // â”€â”€ Assignment â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_assignment(&mut self, target: &Expr, op: &AssignOp, value: &Expr, line: u32) -> Result<(), String> {
        match target {
            Expr::Ident(name) => {
                match op {
                    AssignOp::Assign => {
                        self.compile_expr(value, line)?;
                    }
                    _ => {
                        self.compile_variable_get(name, line)?;
                        self.compile_expr(value, line)?;
                        match op {
                            AssignOp::PlusAssign => self.emit(Op::Add, line),
                            AssignOp::MinusAssign => self.emit(Op::Sub, line),
                            AssignOp::StarAssign => self.emit(Op::Mul, line),
                            AssignOp::SlashAssign => self.emit(Op::Div, line),
                            AssignOp::PercentAssign => self.emit(Op::Mod, line),
                            AssignOp::DoubleStarAssign => self.emit(Op::Pow, line),
                            AssignOp::ShlAssign => self.emit(Op::Shl, line),
                            AssignOp::ShrAssign => self.emit(Op::Shr, line),
                            AssignOp::BitAndAssign => self.emit(Op::BitAnd, line),
                            AssignOp::BitOrAssign => self.emit(Op::BitOr, line),
                            AssignOp::BitXorAssign => self.emit(Op::BitXor, line),
                            AssignOp::IntDivAssign => self.emit(Op::IntDiv, line),
                            AssignOp::Assign => unreachable!(),
                        }
                    }
                }
                self.compile_variable_set(name, line)?;
            }
            Expr::FieldAccess { object, field, .. } => {
                self.compile_expr(object, line)?;
                match op {
                    AssignOp::Assign => {
                        self.compile_expr(value, line)?;
                    }
                    _ => {
                        self.emit(Op::Dup, line); // dup object
                        let fidx = self.add_string(field);
                        self.emit(Op::GetField, line);
                        self.emit_u16(fidx, line);
                        self.compile_expr(value, line)?;
                        match op {
                            AssignOp::PlusAssign => self.emit(Op::Add, line),
                            AssignOp::MinusAssign => self.emit(Op::Sub, line),
                            AssignOp::StarAssign => self.emit(Op::Mul, line),
                            AssignOp::SlashAssign => self.emit(Op::Div, line),
                            _ => self.emit(Op::Add, line), // fallback
                        }
                    }
                }
                let fidx = self.add_string(field);
                self.emit(Op::SetField, line);
                self.emit_u16(fidx, line);
                // Writeback: if the object was a simple variable, update it
                if let Expr::Ident(var_name) = object.as_ref() {
                    if var_name != "self" {
                        if let Some(local_idx) = self.resolve_local(var_name) {
                            self.emit(Op::SetLocal, line);
                            self.emit_u16(local_idx, line);
                        } else {
                            let gidx = self.add_string(var_name);
                            self.emit(Op::SetGlobal, line);
                            self.emit_u16(gidx, line);
                        }
                    }
                }
            }
            Expr::Index { object, index } => {
                self.compile_expr(object, line)?;
                self.compile_expr(index, line)?;
                self.compile_expr(value, line)?;
                self.emit(Op::SetIndex, line);
            }
            _ => {
                return Err(format!("Invalid assignment target"));
            }
        }
        Ok(())
    }

    // â”€â”€ Control Flow â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_if(&mut self, cond: &Expr, body: &[Stmt], else_ifs: &[(Expr, Vec<Stmt>)], else_body: &Option<Vec<Stmt>>, line: u32) -> Result<(), String> {
        self.compile_expr(cond, line)?;
        let else_jump = self.emit_jump(Op::JumpIfFalse, line);
        self.emit(Op::Pop, line);

        self.begin_scope();
        for s in body {
            self.compile_stmt(s, line)?;
        }
        self.end_scope(line);

        let mut end_jumps = Vec::new();

        // Always jump over the else-branch Pop, even when there's no else
        end_jumps.push(self.emit_jump(Op::Jump, line));
        self.patch_jump(else_jump);
        self.emit(Op::Pop, line);

        for (elif_cond, elif_body) in else_ifs {
            self.compile_expr(elif_cond, line)?;
            let next_jump = self.emit_jump(Op::JumpIfFalse, line);
            self.emit(Op::Pop, line);
            self.begin_scope();
            for s in elif_body {
                self.compile_stmt(s, line)?;
            }
            self.end_scope(line);
            end_jumps.push(self.emit_jump(Op::Jump, line));
            self.patch_jump(next_jump);
            self.emit(Op::Pop, line);
        }

        if let Some(els) = else_body {
            self.begin_scope();
            for s in els {
                self.compile_stmt(s, line)?;
            }
            self.end_scope(line);
        }

        for j in end_jumps {
            self.patch_jump(j);
        }
        Ok(())
    }

    fn compile_while(&mut self, cond: &Expr, body: &[Stmt], label: Option<String>, line: u32) -> Result<(), String> {
        let loop_start = self.chunk().len();
        let __sd = self.current().scope_depth;
        self.current().loops.push(LoopCtx {
            start: loop_start,
            break_patches: Vec::new(),
            continue_patches: Vec::new(),
            label,
            scope_depth: __sd,
        });

        self.compile_expr(cond, line)?;
        let exit_jump = self.emit_jump(Op::JumpIfFalse, line);
        self.emit(Op::Pop, line);

        self.begin_scope();
        for s in body {
            self.compile_stmt(s, line)?;
        }
        self.end_scope(line);

        // Patch continue points to here (before the loop-back jump)
        let loop_ctx = self.current().loops.last().unwrap();
        let continues: Vec<usize> = loop_ctx.continue_patches.clone();
        for patch in &continues {
            self.patch_jump(*patch);
        }

        self.emit_loop(loop_start, line);

        self.patch_jump(exit_jump);
        self.emit(Op::Pop, line);

        let loop_ctx = self.current().loops.pop().unwrap();
        for patch in &loop_ctx.break_patches {
            self.patch_jump(*patch);
        }

        Ok(())
    }

    fn compile_for_in(&mut self, var: &str, iter: &Expr, body: &[Stmt], line: u32) -> Result<(), String> {
        self.compile_for_in_labeled(var, iter, body, None, line)
    }

    fn compile_for_in_labeled(&mut self, var: &str, iter: &Expr, body: &[Stmt], label: Option<String>, line: u32) -> Result<(), String> {
        // Compile iterator expression
        self.compile_expr(iter, line)?;
        self.emit(Op::GetIter, line);

        let loop_start = self.chunk().len();
        let __sd = self.current().scope_depth;
        self.current().loops.push(LoopCtx {
            start: loop_start,
            break_patches: Vec::new(),
            continue_patches: Vec::new(),
            label,
            scope_depth: __sd,
        });

        // ForIter pushes next value or jumps to end
        let exit_jump = self.emit_jump(Op::ForIter, line);

        // Define loop variable â€” store into a global so iteration values
        // are accessible regardless of scope depth
        let var_idx = self.add_string(var);
        self.emit(Op::DefineGlobal, line);
        self.emit_u16(var_idx, line);

        for s in body {
            self.compile_stmt(s, line)?;
        }

        let continues: Vec<usize> = self.current().loops.last().unwrap().continue_patches.clone();
        for patch in &continues {
            self.patch_jump(*patch);
        }

        self.emit_loop(loop_start, line);

        self.patch_jump(exit_jump);
        self.emit(Op::Pop, line); // pop iterator

        let loop_ctx = self.current().loops.pop().unwrap();
        for patch in &loop_ctx.break_patches {
            self.patch_jump(*patch);
        }

        Ok(())
    }

    fn compile_for_in_destructure(&mut self, vars: &[String], iter: &Expr, body: &[Stmt], line: u32) -> Result<(), String> {
        // Same as for-in but destructure each element into globals
        self.compile_expr(iter, line)?;
        self.emit(Op::GetIter, line);

        let loop_start = self.chunk().len();
        let __sd = self.current().scope_depth;
        self.current().loops.push(LoopCtx {
            start: loop_start,
            break_patches: Vec::new(),
            continue_patches: Vec::new(),
            label: None,
            scope_depth: __sd,
        });

        let exit_jump = self.emit_jump(Op::ForIter, line);

        // Stack now has: [..., iter_id, element]
        // Destructure element into global variables
        for (i, v) in vars.iter().enumerate() {
            self.emit(Op::Dup, line); // dup element (always at top before first var, or we re-dup from original)
            self.emit_constant(Value::Int(i as i64), line);
            self.emit(Op::GetIndex, line);
            let var_idx = self.add_string(v);
            self.emit(Op::DefineGlobal, line);
            self.emit_u16(var_idx, line);
        }
        self.emit(Op::Pop, line); // pop original element

        for s in body {
            self.compile_stmt(s, line)?;
        }

        let continues: Vec<usize> = self.current().loops.last().unwrap().continue_patches.clone();
        for patch in &continues {
            self.patch_jump(*patch);
        }

        self.emit_loop(loop_start, line);

        self.patch_jump(exit_jump);
        self.emit(Op::Pop, line); // pop iterator

        let loop_ctx = self.current().loops.pop().unwrap();
        for patch in &loop_ctx.break_patches {
            self.patch_jump(*patch);
        }
        Ok(())
    }

    fn compile_for_classic(&mut self, init: &Option<Box<Stmt>>, cond: &Option<Expr>, update: &Option<Box<Stmt>>, body: &[Stmt], line: u32) -> Result<(), String> {
        self.compile_for_classic_labeled(init, cond, update, body, None, line)
    }

    fn compile_for_classic_labeled(&mut self, init: &Option<Box<Stmt>>, cond: &Option<Expr>, update: &Option<Box<Stmt>>, body: &[Stmt], label: Option<String>, line: u32) -> Result<(), String> {
        self.begin_scope();
        if let Some(init) = init {
            self.compile_stmt(init, line)?;
        }

        let loop_start = self.chunk().len();
        let __sd = self.current().scope_depth;
        self.current().loops.push(LoopCtx {
            start: loop_start,
            break_patches: Vec::new(),
            continue_patches: Vec::new(),
            label,
            scope_depth: __sd,
        });

        let exit_jump = if let Some(c) = cond {
            self.compile_expr(c, line)?;
            let j = self.emit_jump(Op::JumpIfFalse, line);
            self.emit(Op::Pop, line);
            Some(j)
        } else {
            None
        };

        for s in body {
            self.compile_stmt(s, line)?;
        }

        let continues: Vec<usize> = self.current().loops.last().unwrap().continue_patches.clone();
        for patch in &continues {
            self.patch_jump(*patch);
        }

        if let Some(upd) = update {
            self.compile_stmt(upd, line)?;
        }

        self.emit_loop(loop_start, line);

        if let Some(j) = exit_jump {
            self.patch_jump(j);
            self.emit(Op::Pop, line);
        }

        let loop_ctx = self.current().loops.pop().unwrap();
        for patch in &loop_ctx.break_patches {
            self.patch_jump(*patch);
        }

        self.end_scope(line);
        Ok(())
    }

    fn compile_break(&mut self, label: Option<&str>, line: u32) -> Result<(), String> {
        // Pop locals for scopes between current and target loop
        if let Some(lbl) = label {
            let target_depth = {
                let loops = &self.current().loops;
                let mut found = None;
                for l in loops.iter().rev() {
                    if l.label.as_deref() == Some(lbl) {
                        found = Some(l.scope_depth);
                        break;
                    }
                }
                found.ok_or_else(|| format!("Unknown loop label '{}'", lbl))?
            };
            // Pop all locals deeper than target loop scope
            let current_depth = self.current().scope_depth;
            let pop_count = self.current().locals.iter()
                .filter(|l| l.depth > target_depth)
                .count();
            for _ in 0..pop_count {
                self.emit(Op::Pop, line);
            }
            let patch = self.emit_jump(Op::Jump, line);
            self.current().loops.iter_mut().rev()
                .find(|l| l.label.as_deref() == Some(lbl))
                .unwrap()
                .break_patches.push(patch);
        } else {
            let patch = self.emit_jump(Op::Jump, line);
            if let Some(l) = self.current().loops.last_mut() {
                l.break_patches.push(patch);
            } else {
                return Err("break outside of loop".to_string());
            }
        }
        Ok(())
    }

    fn compile_continue(&mut self, label: Option<&str>, line: u32) -> Result<(), String> {
        if let Some(lbl) = label {
            let target_depth = {
                let loops = &self.current().loops;
                let mut found = None;
                for l in loops.iter().rev() {
                    if l.label.as_deref() == Some(lbl) {
                        found = Some(l.scope_depth);
                        break;
                    }
                }
                found.ok_or_else(|| format!("Unknown loop label '{}'", lbl))?
            };
            // Pop all locals deeper than target loop scope
            let pop_count = self.current().locals.iter()
                .filter(|l| l.depth > target_depth)
                .count();
            for _ in 0..pop_count {
                self.emit(Op::Pop, line);
            }
            let patch = self.emit_jump(Op::Jump, line);
            self.current().loops.iter_mut().rev()
                .find(|l| l.label.as_deref() == Some(lbl))
                .unwrap()
                .continue_patches.push(patch);
        } else {
            let patch = self.emit_jump(Op::Jump, line);
            if let Some(l) = self.current().loops.last_mut() {
                l.continue_patches.push(patch);
            } else {
                return Err("continue outside of loop".to_string());
            }
        }
        Ok(())
    }

    // â”€â”€ Functions â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_func_decl(&mut self, name: &str, params: &[Param], body: &[Stmt], is_generator: bool, line: u32) -> Result<(), String> {
        let arity = params.len() as u8;
        let has_variadic = params.last().map_or(false, |p| p.is_variadic);
        let default_count = params.iter().filter(|p| p.default.is_some()).count() as u8;

        let frame = CompilerFrame {
            function: CompiledFunc {
                name: name.to_string(),
                arity,
                has_variadic,
                default_count,
                upvalue_count: 0,
                is_generator,
                chunk: Chunk::new(),
            },
            locals: Vec::new(),
            upvalues: Vec::new(),
            scope_depth: 0,
            loops: Vec::new(),
            labels: HashMap::new(),
            pending_gotos: Vec::new(),
        };
        self.frames.push(frame);

        self.begin_scope();
        // Define parameters as locals
        for param in params {
            self.add_local(&param.name);
        }

        // Compile default parameter check code
        // For each param with a default, emit: if local == Null, set to default
        let required_count = (arity as usize) - (default_count as usize);
        for (i, param) in params.iter().enumerate() {
            if let Some(default_expr) = &param.default {
                // GetLocal(i), if Null, replace with default
                let local_idx = i as u16;
                self.emit(Op::GetLocal, line);
                self.emit_u16(local_idx, line);
                self.emit(Op::Null, line);
                self.emit(Op::Eq, line);
                let skip_jump = self.emit_jump(Op::JumpIfFalse, line);
                self.emit(Op::Pop, line); // pop the false
                self.compile_expr(default_expr, line)?;
                self.emit(Op::SetLocal, line);
                self.emit_u16(local_idx, line);
                self.emit(Op::Pop, line); // pop the value returned by SetLocal
                let end_jump = self.emit_jump(Op::Jump, line);
                self.patch_jump(skip_jump);
                self.emit(Op::Pop, line); // pop the true (condition result)
                self.patch_jump(end_jump);
            }
        }

        // Compile body
        for s in body {
            self.compile_stmt(s, line)?;
        }

        // Implicit null return
        self.emit(Op::Null, line);
        self.emit(Op::Return, line);

        self.end_scope(line);

        let completed_frame = self.frames.pop().unwrap();
        let func = completed_frame.function;
        let upvalues = completed_frame.upvalues;

        // Store function as a constant
        let func_const = Value::Func(crate::value::FuncValue {
            name: func.name.clone(),
            params: params.to_vec(),
            body: Vec::new(), // no AST body needed â€” bytecoded
            closure_env: 0,
            is_generator,
        });

        // Store the CompiledFunc in the constant pool
        let cf_const = Value::Dict(vec![
            (Value::Str("__compiled_func__".to_string()), Value::Str(func.name.clone())),
        ]);
        let idx = self.chunk().add_constant(cf_const);

        // Actually, we need a way to store CompiledFunc. Use a side channel.
        // Store the compiled function in global data that the VM will pick up.
        // For now, encode it as a special constant.
        // We'll refactor: store CompiledFunc in a separate vec, index by constant pool idx.

        // Emit Closure instruction
        self.emit(Op::Closure, line);
        self.emit_u16(idx, line);

        // Emit upvalue descriptors after the Closure op
        for uv in &upvalues {
            self.emit_byte(if uv.is_local { 1 } else { 0 }, line);
            self.emit_u16(uv.index, line);
        }

        // Define the function name
        if self.current().scope_depth > 0 {
            self.add_local(name);
        } else {
            let name_idx = self.add_string(name);
            self.emit(Op::DefineGlobal, line);
            self.emit_u16(name_idx, line);
        }

        // Store the actual compiled func in our side map
        // We'll need the VM to look this up
        self.store_compiled_func(idx, func);

        Ok(())
    }

    fn compile_lambda(&mut self, params: &[Param], body: &[Stmt], line: u32) -> Result<(), String> {
        let id = self.lambda_counter;
        self.lambda_counter += 1;
        let name = format!("<lambda_{}>", id);
        let arity = params.len() as u8;
        let has_variadic = params.last().map_or(false, |p| p.is_variadic);
        let default_count = params.iter().filter(|p| p.default.is_some()).count() as u8;

        let frame = CompilerFrame {
            function: CompiledFunc {
                name: name.to_string(),
                arity,
                has_variadic,
                default_count,
                upvalue_count: 0,
                is_generator: false,
                chunk: Chunk::new(),
            },
            locals: Vec::new(),
            upvalues: Vec::new(),
            scope_depth: 0,
            loops: Vec::new(),
            labels: HashMap::new(),
            pending_gotos: Vec::new(),
        };
        self.frames.push(frame);

        self.begin_scope();
        for param in params {
            self.add_local(&param.name);
        }

        for s in body {
            self.compile_stmt(s, line)?;
        }

        self.emit(Op::Null, line);
        self.emit(Op::Return, line);
        self.end_scope(line);

        let completed_frame = self.frames.pop().unwrap();
        let func = completed_frame.function;
        let upvalues = completed_frame.upvalues;

        let cf_const = Value::Dict(vec![
            (Value::Str("__compiled_func__".to_string()), Value::Str(func.name.clone())),
        ]);
        let idx = self.chunk().add_constant(cf_const);

        self.emit(Op::Closure, line);
        self.emit_u16(idx, line);

        for uv in &upvalues {
            self.emit_byte(if uv.is_local { 1 } else { 0 }, line);
            self.emit_u16(uv.index, line);
        }

        self.store_compiled_func(idx, func);

        Ok(())
    }

    // â”€â”€ Try/Catch â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_try_catch(&mut self, body: &[Stmt], catch_var: &Option<String>, catch_body: &Option<Vec<Stmt>>, finally_body: &Option<Vec<Stmt>>, line: u32) -> Result<(), String> {
        let catch_jump = self.emit_jump(Op::TryBegin, line);

        self.begin_scope();
        for s in body {
            self.compile_stmt(s, line)?;
        }
        self.end_scope(line);
        self.emit(Op::TryEnd, line);

        let finally_jump = self.emit_jump(Op::Jump, line);
        self.patch_jump(catch_jump);

        // Catch block: error value is on stack
        if let Some(catch_stmts) = catch_body {
            if let Some(var) = catch_var {
                if self.frames.len() <= 1 {
                    // Top-level: use global for catch variable
                    let gidx = self.add_string(var);
                    self.emit(Op::DefineGlobal, line);
                    self.emit_u16(gidx, line);
                } else {
                    self.begin_scope();
                    self.add_local(var);
                }
            } else {
                self.emit(Op::Pop, line);
            }
            for s in catch_stmts {
                self.compile_stmt(s, line)?;
            }
            if catch_var.is_some() && self.frames.len() > 1 {
                self.end_scope(line);
            }
        } else {
            self.emit(Op::Pop, line);
        }

        self.patch_jump(finally_jump);

        if let Some(finally_stmts) = finally_body {
            for s in finally_stmts {
                self.compile_stmt(s, line)?;
            }
        }
        Ok(())
    }

    // â”€â”€ Classes â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_class(&mut self, name: &str, parent: &Option<String>, body: &[Stmt], decorators: &[String], is_sealed: bool, line: u32) -> Result<(), String> {
        let name_idx = self.add_string(name);
        self.emit(Op::Class, line);
        self.emit_u16(name_idx, line);

        if let Some(p) = parent {
            self.compile_variable_get(p, line)?;
            self.emit(Op::Inherit, line);
        }

        // Compile methods
        let mut compiled_methods = Vec::new();
        let mut field_defs = Vec::new();
        for s in body {
            match s {
                Stmt::FuncDecl { name: mname, params, body: mbody, is_generator, .. } => {
                    // Compile method as a standalone function
                    let arity = params.len() as u8;
                    let has_variadic = params.last().map_or(false, |p| p.is_variadic);
                    let default_count = params.iter().filter(|p| p.default.is_some()).count() as u8;

                    let frame = CompilerFrame {
                        function: CompiledFunc {
                            name: mname.clone(),
                            arity,
                            has_variadic,
                            default_count,
                            upvalue_count: 0,
                            is_generator: false,
                            chunk: Chunk::new(),
                        },
                        locals: Vec::new(),
                        upvalues: Vec::new(),
                        scope_depth: 0,
                        loops: Vec::new(),
                        labels: HashMap::new(),
                        pending_gotos: Vec::new(),
                    };
                    self.frames.push(frame);
                    self.begin_scope();
                    for param in params {
                        self.add_local(&param.name);
                    }
                    for ms in mbody {
                        self.compile_stmt(ms, line)?;
                    }
                    self.emit(Op::Null, line);
                    self.emit(Op::Return, line);
                    self.end_scope(line);

                    let cf = self.frames.pop().unwrap();
                    compiled_methods.push((mname.clone(), cf.function));

                    let method_name_idx = self.add_string(mname);
                    // Store as constant and emit Method op
                    let cf_const = Value::Dict(vec![
                        (Value::Str("__compiled_func__".to_string()), Value::Str(mname.clone())),
                    ]);
                    let cidx = self.chunk().add_constant(cf_const);
                    self.emit(Op::Closure, line);
                    self.emit_u16(cidx, line);
                    self.emit(Op::Method, line);
                    self.emit_u16(method_name_idx, line);

                    // store the compiled func
                    self.store_compiled_func(cidx, compiled_methods.last().unwrap().1.clone());
                }
                Stmt::Let { name: fname, value, .. } => {
                    field_defs.push((fname.clone(), None));
                }
                _ => {}
            }
        }

        // Define as global
        if self.current().scope_depth > 0 {
            self.add_local(name);
        } else {
            let gidx = self.add_string(name);
            self.emit(Op::DefineGlobal, line);
            self.emit_u16(gidx, line);
        }

        // Store class definition for the VM
        self.class_defs.insert(name.to_string(), ClassDef {
            name: name.to_string(),
            parent: parent.clone(),
            methods: compiled_methods,
            fields: field_defs,
            is_fixed: decorators.contains(&"fixed".to_string()),
            is_data: decorators.contains(&"data".to_string()),
            is_sealed,
            decorators: decorators.to_vec(),
        });

        Ok(())
    }

    // â”€â”€ Structs â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_struct_decl(&mut self, name: &str, fields: &[StructField], line: u32) -> Result<(), String> {
        self.struct_defs.insert(name.to_string(), StructDef {
            name: name.to_string(),
            fields: fields.iter().map(|f| (f.name.clone(), f.type_ann.clone())).collect(),
        });

        let idx = self.add_string(name);
        self.emit(Op::DefineStruct, line);
        self.emit_u16(idx, line);
        self.emit_u16(fields.len() as u16, line);
        // Emit field name indices
        for f in fields {
            let fidx = self.add_string(&f.name);
            self.emit_u16(fidx, line);
        }

        if self.current().scope_depth > 0 {
            self.add_local(name);
        } else {
            let gidx = self.add_string(name);
            self.emit(Op::DefineGlobal, line);
            self.emit_u16(gidx, line);
        }
        Ok(())
    }

    // â”€â”€ Enums â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_enum_decl(&mut self, name: &str, variants: &[EnumVariant], line: u32) -> Result<(), String> {
        self.enum_defs.insert(name.to_string(), EnumDef {
            name: name.to_string(),
            variants: variants.iter().map(|v| (v.name.clone(), v.fields.clone())).collect(),
        });

        let idx = self.add_string(name);
        self.emit(Op::DefineEnum, line);
        self.emit_u16(idx, line);

        if self.current().scope_depth > 0 {
            self.add_local(name);
        } else {
            let gidx = self.add_string(name);
            self.emit(Op::DefineGlobal, line);
            self.emit_u16(gidx, line);
        }
        Ok(())
    }

    // â”€â”€ Traits â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_trait_decl(&mut self, name: &str, supertraits: &[String], methods: &[Stmt], line: u32) -> Result<(), String> {
        let mut method_names = Vec::new();
        let mut method_funcs = Vec::new();

        for m in methods {
            if let Stmt::FuncDecl { name: mname, params, body, .. } = m {
                method_names.push(mname.clone());
                // Compile default impl
                let arity = params.len() as u8;
                let frame = CompilerFrame {
                    function: CompiledFunc {
                        name: mname.clone(),
                        arity,
                        has_variadic: false,
                        default_count: 0,
                        upvalue_count: 0,
                        is_generator: false,
                        chunk: Chunk::new(),
                    },
                    locals: Vec::new(),
                    upvalues: Vec::new(),
                    scope_depth: 0,
                    loops: Vec::new(),
                    labels: HashMap::new(),
                    pending_gotos: Vec::new(),
                };
                self.frames.push(frame);
                self.begin_scope();
                for p in params {
                    self.add_local(&p.name);
                }
                for s in body {
                    self.compile_stmt(s, line)?;
                }
                self.emit(Op::Null, line);
                self.emit(Op::Return, line);
                self.end_scope(line);
                method_funcs.push(self.frames.pop().unwrap().function);
            }
        }

        self.trait_defs.insert(name.to_string(), TraitDef {
            name: name.to_string(),
            supertraits: supertraits.to_vec(),
            method_names,
            method_funcs,
        });

        // Emit as a no-op/definition for the runtime
        let idx = self.add_string(name);
        self.emit(Op::DefineTrait, line);
        self.emit_u16(idx, line);

        if self.current().scope_depth > 0 {
            self.add_local(name);
        } else {
            let gidx = self.add_string(name);
            self.emit(Op::DefineGlobal, line);
            self.emit_u16(gidx, line);
        }
        Ok(())
    }

    // â”€â”€ Impl Blocks â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_impl_block(&mut self, trait_name: &Option<String>, target: &str, methods: &[Stmt], line: u32) -> Result<(), String> {
        let mut compiled_methods = Vec::new();

        for m in methods {
            match m {
                Stmt::FuncDecl { name: mname, params, body, .. } => {
                    let arity = params.len() as u8;
                    let frame = CompilerFrame {
                        function: CompiledFunc {
                            name: mname.clone(),
                            arity,
                            has_variadic: false,
                            default_count: 0,
                            upvalue_count: 0,
                            is_generator: false,
                            chunk: Chunk::new(),
                        },
                        locals: Vec::new(),
                        upvalues: Vec::new(),
                        scope_depth: 0,
                        loops: Vec::new(),
                        labels: HashMap::new(),
                        pending_gotos: Vec::new(),
                    };
                    self.frames.push(frame);
                    self.begin_scope();
                    for p in params {
                        self.add_local(&p.name);
                    }
                    for s in body {
                        self.compile_stmt(s, line)?;
                    }
                    self.emit(Op::Null, line);
                    self.emit(Op::Return, line);
                    self.end_scope(line);
                    compiled_methods.push((mname.clone(), self.frames.pop().unwrap().function));
                }
                Stmt::TypeAlias { .. } => {} // type Item = int in impl â€” skip
                _ => {}
            }
        }

        let target_idx = self.add_string(target);
        let trait_idx = if let Some(tn) = trait_name {
            self.add_string(tn)
        } else {
            0xFFFF
        };

        self.emit(Op::BeginImpl, line);
        self.emit_u16(target_idx, line);
        self.emit_u16(trait_idx, line);

        // Emit each method
        for (mname, mfunc) in &compiled_methods {
            let cf_const = Value::Dict(vec![
                (Value::Str("__compiled_func__".to_string()), Value::Str(mname.clone())),
            ]);
            let cidx = self.chunk().add_constant(cf_const);
            self.emit(Op::Closure, line);
            self.emit_u16(cidx, line);
            let mn_idx = self.add_string(mname);
            self.emit(Op::Method, line);
            self.emit_u16(mn_idx, line);
            self.store_compiled_func(cidx, mfunc.clone());
        }

        self.impl_blocks.push(ImplDef {
            trait_name: trait_name.clone(),
            target: target.to_string(),
            methods: compiled_methods,
        });

        Ok(())
    }

    // â”€â”€ Imports â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_import(&mut self, path: &str, alias: &Option<String>, _names: &Option<Vec<String>>, line: u32) -> Result<(), String> {
        let path_idx = self.chunk().add_constant(Value::Str(path.to_string()));
        self.emit(Op::Import, line);
        self.emit_u16(path_idx, line);

        let bind_name = alias.as_deref().unwrap_or_else(|| {
            path.split('.').last().unwrap_or(path)
        });
        if self.current().scope_depth > 0 {
            self.add_local(bind_name);
        } else {
            let idx = self.add_string(bind_name);
            self.emit(Op::DefineGlobal, line);
            self.emit_u16(idx, line);
        }
        Ok(())
    }

    // â”€â”€ Match â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_match(&mut self, subject: &Expr, arms: &[MatchArm], line: u32) -> Result<(), String> {
        self.compile_expr(subject, line)?;
        let mut end_jumps = Vec::new();

        for arm in arms {
            self.emit(Op::Dup, line); // keep a copy of subject for next arm

            match &arm.pattern {
                Pattern::Literal(lit) => {
                    self.compile_expr(lit, line)?;
                    self.emit(Op::Eq, line);
                }
                Pattern::Wildcard | Pattern::Default => {
                    self.emit(Op::Pop, line); // pop the dup'd subject
                    self.emit(Op::True, line);
                }
                Pattern::Ident(name) => {
                    // Bind the value
                    self.emit(Op::Pop, line);
                    self.emit(Op::True, line);
                }
                _ => {
                    self.emit(Op::Pop, line);
                    self.emit(Op::True, line);
                }
            }

            let next_arm = self.emit_jump(Op::JumpIfFalse, line);
            self.emit(Op::Pop, line); // pop true
            self.emit(Op::Pop, line); // pop subject copy

            self.begin_scope();

            // Bind pattern variable if any
            if let Pattern::Ident(name) = &arm.pattern {
                self.compile_expr(subject, line)?;
                self.add_local(name);
            }

            // Compile arm body
            for s in &arm.body {
                self.compile_stmt(s, line)?;
            }
            self.end_scope(line);

            end_jumps.push(self.emit_jump(Op::Jump, line));
            self.patch_jump(next_arm);
            self.emit(Op::Pop, line); // pop false
        }

        self.emit(Op::Pop, line); // pop subject

        for j in end_jumps {
            self.patch_jump(j);
        }
        Ok(())
    }

    fn compile_match_expr(&mut self, subject: &Expr, arms: &[MatchArm], line: u32) -> Result<(), String> {
        self.compile_expr(subject, line)?;
        let mut end_jumps = Vec::new();

        for arm in arms {
            self.emit(Op::Dup, line);

            let binding_name = if let Pattern::Ident(name) = &arm.pattern { Some(name.clone()) } else { None };

            match &arm.pattern {
                Pattern::Literal(lit) => {
                    self.compile_expr(lit, line)?;
                    self.emit(Op::Eq, line);
                }
                Pattern::Wildcard | Pattern::Default | Pattern::Ident(_) => {
                    self.emit(Op::Pop, line);
                    self.emit(Op::True, line);
                }
                _ => {
                    self.emit(Op::Pop, line);
                    self.emit(Op::True, line);
                }
            }

            let next_arm = self.emit_jump(Op::JumpIfFalse, line);
            self.emit(Op::Pop, line); // pop true/false

            // Bind ident pattern variable (as a global for simplicity)
            if let Some(ref bname) = binding_name {
                // Subject is on top of stack. Dup it, then store as global.
                self.emit(Op::Dup, line);
                let idx = self.add_string(bname);
                self.emit(Op::DefineGlobal, line);
                self.emit_u16(idx, line);
            }

            // Check guard if present
            let mut guard_jump = None;
            if let Some(ref guard) = arm.guard {
                self.compile_expr(guard, line)?;
                guard_jump = Some(self.emit_jump(Op::JumpIfFalse, line));
                self.emit(Op::Pop, line); // pop guard true
            }

            self.emit(Op::Pop, line); // pop subject copy

            // Compile body
            if arm.body.len() == 1 {
                match &arm.body[0] {
                    Stmt::Expr(e) => { self.compile_expr(e, line)?; }
                    _ => { self.compile_stmt(&arm.body[0], line)?; self.emit(Op::Null, line); }
                }
            } else if arm.body.is_empty() {
                self.emit(Op::Null, line);
            } else {
                for s in &arm.body[..arm.body.len() - 1] {
                    self.compile_stmt(s, line)?;
                }
                match arm.body.last().unwrap() {
                    Stmt::Expr(e) => { self.compile_expr(e, line)?; }
                    s => { self.compile_stmt(s, line)?; self.emit(Op::Null, line); }
                }
            }

            end_jumps.push(self.emit_jump(Op::Jump, line));

            if let Some(gj) = guard_jump {
                self.patch_jump(gj);
                self.emit(Op::Pop, line); // pop guard false
                // Subject is still on stack, continue to next arm
            } else {
                self.patch_jump(next_arm);
                self.emit(Op::Pop, line); // pop false
            }
        }

        // No arm matched â€” pop subject, push null
        self.emit(Op::Pop, line);
        self.emit(Op::Null, line);

        for j in end_jumps {
            self.patch_jump(j);
        }
        Ok(())
    }

    // â”€â”€ F-strings â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_fstring(&mut self, template: &str, line: u32) -> Result<(), String> {
        // Parse f-string: split on ${ and }
        // Each expression part may have a format spec after ':'
        let mut parts: Vec<(bool, String, Option<String>)> = Vec::new(); // (is_literal, content, format_spec)
        let chars: Vec<char> = template.chars().collect();
        let mut i = 0;
        let mut current = String::new();

        while i < chars.len() {
            if i + 1 < chars.len() && chars[i] == '$' && chars[i + 1] == '{' {
                if !current.is_empty() {
                    parts.push((true, current.clone(), None));
                    current.clear();
                }
                i += 2;
                let mut depth = 1;
                let mut expr_str = String::new();
                while i < chars.len() && depth > 0 {
                    if chars[i] == '{' { depth += 1; }
                    if chars[i] == '}' {
                        depth -= 1;
                        if depth == 0 { break; }
                    }
                    expr_str.push(chars[i]);
                    i += 1;
                }
                i += 1; // skip closing }
                // Split expression from format spec at last ':'
                // But be careful: ':' can appear in ternary etc.
                // Simple approach: split at last ':' only if format spec looks valid
                let mut expr_part = expr_str.clone();
                let mut fmt_spec = None;
                if let Some(colon_pos) = expr_str.rfind(':') {
                    let after = &expr_str[colon_pos + 1..];
                    // Check if after looks like a format spec (e.g., .2f, x, X, b, o, >5d, <5d, 05d, .1%)
                    let is_fmt = after.len() > 0 && after.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '<' || c == '>' || c == '%');
                    if is_fmt {
                        expr_part = expr_str[..colon_pos].to_string();
                        fmt_spec = Some(after.to_string());
                    }
                }
                parts.push((false, expr_part, fmt_spec));
            } else {
                current.push(chars[i]);
                i += 1;
            }
        }
        if !current.is_empty() {
            parts.push((true, current, None));
        }

        if parts.is_empty() {
            self.emit_constant(Value::Str(String::new()), line);
            return Ok(());
        }

        let mut first = true;
        for (is_lit, content, fmt_spec) in &parts {
            if *is_lit {
                if first {
                    self.emit_constant(Value::Str(content.clone()), line);
                    first = false;
                } else {
                    self.emit_constant(Value::Str(content.clone()), line);
                    self.emit(Op::Add, line);
                }
            } else {
                let mut lexer = crate::lexer::Lexer::new(content);
                let tokens = lexer.tokenize().map_err(|e| format!("f-string error: {}", e))?;
                let mut parser = crate::parser::Parser::new(tokens);
                let expr = parser.parse_expr().map_err(|e| format!("f-string error: {}", e))?;
                self.compile_expr(&expr, line)?;

                if let Some(spec) = fmt_spec {
                    // Call __format(value, spec)
                    let fmt_idx = self.add_string("__format");
                    self.emit(Op::GetGlobal, line);
                    self.emit_u16(fmt_idx, line);
                    self.emit(Op::Swap, line);
                    self.emit_constant(Value::Str(spec.clone()), line);
                    self.emit(Op::Call, line);
                    self.emit_byte(2, line);
                } else {
                    // Convert to string via str()
                    let str_idx = self.add_string("str");
                    self.emit(Op::GetGlobal, line);
                    self.emit_u16(str_idx, line);
                    self.emit(Op::Swap, line);
                    self.emit(Op::Call, line);
                    self.emit_byte(1, line);
                }
                if !first {
                    self.emit(Op::Add, line);
                }
                first = false;
            }
        }

        Ok(())
    }

    // â”€â”€ Comprehensions â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn compile_list_comp(&mut self, expr: &Expr, var: &str, iter: &Expr, cond: &Option<Box<Expr>>, line: u32) -> Result<(), String> {
        let __comp_id = self.do_counter; self.do_counter += 1; let tmp = format!("__lcomp_{}", __comp_id);
        let ti = self.add_string(&tmp);

        self.emit(Op::BuildList, line);
        self.emit_u16(0, line);
        self.emit(Op::DefineGlobal, line);
        self.emit_u16(ti, line);

        self.compile_expr(iter, line)?;
        self.emit(Op::GetIter, line);

        let loop_start = self.chunk().len();
        let exit_jump = self.emit_jump(Op::ForIter, line);

        // ForIter pushes next value; store as global (like for-in)
        let var_idx = self.add_string(var);
        self.emit(Op::DefineGlobal, line);
        self.emit_u16(var_idx, line);

        if let Some(c) = cond {
            self.compile_expr(c, line)?;
            let skip = self.emit_jump(Op::JumpIfFalse, line);
            self.emit(Op::Pop, line);

            let gi = self.add_string(&tmp);
            self.emit(Op::GetGlobal, line);
            self.emit_u16(gi, line);
            self.compile_expr(expr, line)?;
            self.emit(Op::ListAppend, line);
            let si = self.add_string(&tmp);
            self.emit(Op::SetGlobal, line);
            self.emit_u16(si, line);
            self.emit(Op::Pop, line);

            let end_cond = self.emit_jump(Op::Jump, line);
            self.patch_jump(skip);
            self.emit(Op::Pop, line);
            self.patch_jump(end_cond);
        } else {
            let gi = self.add_string(&tmp);
            self.emit(Op::GetGlobal, line);
            self.emit_u16(gi, line);
            self.compile_expr(expr, line)?;
            self.emit(Op::ListAppend, line);
            let si = self.add_string(&tmp);
            self.emit(Op::SetGlobal, line);
            self.emit_u16(si, line);
            self.emit(Op::Pop, line);
        }

        self.emit_loop(loop_start, line);
        self.patch_jump(exit_jump);
        self.emit(Op::Pop, line);

        let ri = self.add_string(&tmp);
        self.emit(Op::GetGlobal, line);
        self.emit_u16(ri, line);
        Ok(())
    }

    fn compile_dict_comp(&mut self, key_expr: &Expr, val_expr: &Expr, var: &str, iter: &Expr, cond: &Option<Box<Expr>>, line: u32) -> Result<(), String> {
        let __comp_id = self.do_counter; self.do_counter += 1; let tmp = format!("__dcomp_{}", __comp_id);
        let ti = self.add_string(&tmp);

        self.emit(Op::BuildDict, line);
        self.emit_u16(0, line);
        self.emit(Op::DefineGlobal, line);
        self.emit_u16(ti, line);

        self.compile_expr(iter, line)?;
        self.emit(Op::GetIter, line);

        let loop_start = self.chunk().len();
        let exit_jump = self.emit_jump(Op::ForIter, line);

        let var_idx = self.add_string(var);
        self.emit(Op::DefineGlobal, line);
        self.emit_u16(var_idx, line);

        if let Some(c) = cond {
            self.compile_expr(c, line)?;
            let skip = self.emit_jump(Op::JumpIfFalse, line);
            self.emit(Op::Pop, line);

            let gi = self.add_string(&tmp);
            self.emit(Op::GetGlobal, line);
            self.emit_u16(gi, line);
            self.compile_expr(key_expr, line)?;
            self.compile_expr(val_expr, line)?;
            self.emit(Op::DictInsert, line);
            let si = self.add_string(&tmp);
            self.emit(Op::SetGlobal, line);
            self.emit_u16(si, line);
            self.emit(Op::Pop, line);

            let end_cond = self.emit_jump(Op::Jump, line);
            self.patch_jump(skip);
            self.emit(Op::Pop, line);
            self.patch_jump(end_cond);
        } else {
            let gi = self.add_string(&tmp);
            self.emit(Op::GetGlobal, line);
            self.emit_u16(gi, line);
            self.compile_expr(key_expr, line)?;
            self.compile_expr(val_expr, line)?;
            self.emit(Op::DictInsert, line);
            let si = self.add_string(&tmp);
            self.emit(Op::SetGlobal, line);
            self.emit_u16(si, line);
            self.emit(Op::Pop, line);
        }

        self.emit_loop(loop_start, line);
        self.patch_jump(exit_jump);
        self.emit(Op::Pop, line);

        let ri = self.add_string(&tmp);
        self.emit(Op::GetGlobal, line);
        self.emit_u16(ri, line);
        Ok(())
    }

    fn compile_set_comp(&mut self, expr: &Expr, var: &str, iter: &Expr, cond: &Option<Box<Expr>>, line: u32) -> Result<(), String> {
        let __comp_id = self.do_counter; self.do_counter += 1; let tmp = format!("__scomp_{}", __comp_id);
        let ti = self.add_string(&tmp);

        self.emit(Op::BuildSet, line);
        self.emit_u16(0, line);
        self.emit(Op::DefineGlobal, line);
        self.emit_u16(ti, line);

        self.compile_expr(iter, line)?;
        self.emit(Op::GetIter, line);

        let loop_start = self.chunk().len();
        let exit_jump = self.emit_jump(Op::ForIter, line);

        let var_idx = self.add_string(var);
        self.emit(Op::DefineGlobal, line);
        self.emit_u16(var_idx, line);

        if let Some(c) = cond {
            self.compile_expr(c, line)?;
            let skip = self.emit_jump(Op::JumpIfFalse, line);
            self.emit(Op::Pop, line);

            let gi = self.add_string(&tmp);
            self.emit(Op::GetGlobal, line);
            self.emit_u16(gi, line);
            self.compile_expr(expr, line)?;
            self.emit(Op::SetAdd, line);
            let si = self.add_string(&tmp);
            self.emit(Op::SetGlobal, line);
            self.emit_u16(si, line);
            self.emit(Op::Pop, line);

            let end_cond = self.emit_jump(Op::Jump, line);
            self.patch_jump(skip);
            self.emit(Op::Pop, line);
            self.patch_jump(end_cond);
        } else {
            let gi = self.add_string(&tmp);
            self.emit(Op::GetGlobal, line);
            self.emit_u16(gi, line);
            self.compile_expr(expr, line)?;
            self.emit(Op::SetAdd, line);
            let si = self.add_string(&tmp);
            self.emit(Op::SetGlobal, line);
            self.emit_u16(si, line);
            self.emit(Op::Pop, line);
        }

        self.emit_loop(loop_start, line);
        self.patch_jump(exit_jump);
        self.emit(Op::Pop, line);

        let ri = self.add_string(&tmp);
        self.emit(Op::GetGlobal, line);
        self.emit_u16(ri, line);
        Ok(())
    }


    fn compile_if_let(&mut self, pattern: &str, var: &str, expr: &Expr, body: &[Stmt], else_body: &Option<Vec<Stmt>>, line: u32) -> Result<(), String> {
        self.compile_expr(expr, line)?;
        self.emit(Op::Dup, line);
        // Type-specific check for Ok/Err, null check for Some
        if pattern == "Ok" || pattern == "Err" {
            self.emit(Op::TypeOf, line);
            let type_str = pattern.to_lowercase();
            self.emit_constant(Value::Str(type_str), line);
            self.emit(Op::Eq, line);
        } else {
            self.emit(Op::Null, line);
            self.emit(Op::NotEq, line);
        }
        let else_jump = self.emit_jump(Op::JumpIfFalse, line);
        self.emit(Op::Pop, line);

        // Unwrap Some/Ok/Err to get inner value
        if pattern == "Some" || pattern == "Ok" || pattern == "Err" {
            self.emit(Op::UnwrapSome, line);
        }

        self.begin_scope();
        self.add_local(var); // bind the value
        for s in body {
            self.compile_stmt(s, line)?;
        }
        self.end_scope(line);

        let end_jump = self.emit_jump(Op::Jump, line);
        self.patch_jump(else_jump);
        self.emit(Op::Pop, line);
        self.emit(Op::Pop, line);

        if let Some(els) = else_body {
            self.begin_scope();
            for s in els {
                self.compile_stmt(s, line)?;
            }
            self.end_scope(line);
        }
        self.patch_jump(end_jump);
        Ok(())
    }

    fn compile_while_let(&mut self, pattern: &str, var: &str, expr: &Expr, body: &[Stmt], line: u32) -> Result<(), String> {
        let loop_start = self.chunk().len();
        self.compile_expr(expr, line)?;
        self.emit(Op::Dup, line);
        if pattern == "Ok" || pattern == "Err" {
            self.emit(Op::TypeOf, line);
            let type_str = pattern.to_lowercase();
            self.emit_constant(Value::Str(type_str), line);
            self.emit(Op::Eq, line);
        } else {
            self.emit(Op::Null, line);
            self.emit(Op::NotEq, line);
        }
        let exit_jump = self.emit_jump(Op::JumpIfFalse, line);
        self.emit(Op::Pop, line);

        if pattern == "Some" || pattern == "Ok" || pattern == "Err" {
            self.emit(Op::UnwrapSome, line);
        }

        self.begin_scope();
        self.add_local(var);
        for s in body {
            self.compile_stmt(s, line)?;
        }
        self.end_scope(line);

        self.emit_loop(loop_start, line);
        self.patch_jump(exit_jump);
        self.emit(Op::Pop, line);
        self.emit(Op::Pop, line);
        Ok(())
    }

    fn compile_let_else(&mut self, pattern: &str, var: &str, expr: &Expr, else_body: &[Stmt], line: u32) -> Result<(), String> {
        self.compile_expr(expr, line)?;
        self.emit(Op::Dup, line);
        if pattern == "Ok" || pattern == "Err" {
            self.emit(Op::TypeOf, line);
            let type_str = pattern.to_lowercase();
            self.emit_constant(Value::Str(type_str), line);
            self.emit(Op::Eq, line);
        } else {
            self.emit(Op::Null, line);
            self.emit(Op::NotEq, line);
        }
        let else_jump = self.emit_jump(Op::JumpIfFalse, line);
        self.emit(Op::Pop, line);
        if pattern == "Some" || pattern == "Ok" || pattern == "Err" {
            self.emit(Op::UnwrapSome, line);
        }
        if self.current().scope_depth > 0 {
            self.add_local(var);
        } else {
            let idx = self.add_string(var);
            self.emit(Op::DefineGlobal, line);
            self.emit_u16(idx, line);
        }
        let end_jump = self.emit_jump(Op::Jump, line);
        self.patch_jump(else_jump);
        self.emit(Op::Pop, line);
        self.emit(Op::Pop, line);
        for s in else_body {
            self.compile_stmt(s, line)?;
        }
        self.patch_jump(end_jump);
        Ok(())
    }

    // â”€â”€ Compiled function storage â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// Side storage for compiled functions (indexed by constant pool index)  
    fn store_compiled_func(&mut self, _idx: u16, func: CompiledFunc) {
        self.func_store.insert(func.name.clone(), func);
    }
}

/// Public struct to hold full compilation output.
pub struct CompileOutput {
    pub main: CompiledFunc,
    pub class_defs: HashMap<String, ClassDef>,
    pub struct_defs: HashMap<String, StructDef>,
    pub enum_defs: HashMap<String, EnumDef>,
    pub trait_defs: HashMap<String, TraitDef>,
    pub impl_blocks: Vec<ImplDef>,
    pub compiled_funcs: HashMap<String, CompiledFunc>,
}

/// Compile a program into bytecode.
pub fn compile_program(program: &Program) -> Result<CompileOutput, String> {
    let mut compiler = Compiler::new();

    // First pass: walk all statements
    for stmt in &program.stmts {
        compiler.compile_stmt(stmt, 1)?;
    }

    // Patch gotos
    let frame = compiler.frames.last().unwrap();
    let pending: Vec<(String, usize)> = frame.pending_gotos.clone();
    let labels = frame.labels.clone();
    for (label, patch_offset) in &pending {
        if let Some(&target) = labels.get(label) {
            let jump = if target > *patch_offset + 2 {
                target - *patch_offset - 2
            } else {
                0
            };
            compiler.chunk().patch_u16(*patch_offset, jump as u16);
        } else {
            return Err(format!("Undefined label '{}'", label));
        }
    }

    compiler.emit(Op::Halt, 0);

    let main = compiler.frames.pop().unwrap().function;

    // Collect all compiled functions
    let mut compiled_funcs = compiler.func_store;
    // Also include class/trait/impl method funcs
    for cd in compiler.class_defs.values() {
        for (name, func) in &cd.methods {
            compiled_funcs.insert(format!("{}::{}", cd.name, name), func.clone());
        }
    }
    for ib in &compiler.impl_blocks {
        for (name, func) in &ib.methods {
            compiled_funcs.insert(format!("{}::{}", ib.target, name), func.clone());
        }
    }
    for td in compiler.trait_defs.values() {
        for (i, func) in td.method_funcs.iter().enumerate() {
            compiled_funcs.insert(format!("{}::{}", td.name, td.method_names[i]), func.clone());
        }
    }

    Ok(CompileOutput {
        main,
        class_defs: compiler.class_defs,
        struct_defs: compiler.struct_defs,
        enum_defs: compiler.enum_defs,
        trait_defs: compiler.trait_defs,
        impl_blocks: compiler.impl_blocks,
        compiled_funcs,
    })
}

