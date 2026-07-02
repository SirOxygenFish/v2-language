/// Binary serialization/deserialization for V2 compiled bytecode (.v2c files).

use std::collections::HashMap;
use crate::bytecode::{Chunk, CompiledFunc};
use crate::compiler::{ClassDef, CompileOutput, EnumDef, ImplDef, StructDef, TraitDef};
use crate::value::Value;

// ── Magic & version ──────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"V2BC";
const FORMAT_VERSION: u32 = 1;

// ── Value type tags ──────────────────────────────────────────────

const TAG_NULL: u8 = 0;
const TAG_INT: u8 = 1;
const TAG_FLOAT: u8 = 2;
const TAG_STR: u8 = 3;
const TAG_BOOL: u8 = 4;
const TAG_LIST: u8 = 5;
const TAG_DICT: u8 = 6;
const TAG_TUPLE: u8 = 7;
const TAG_SET: u8 = 8;
const TAG_RANGE: u8 = 9;

// ── Writer ───────────────────────────────────────────────────────

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn write_bool(&mut self, v: bool) {
        self.buf.push(if v { 1 } else { 0 });
    }

    fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(&mut self, data: &[u8]) {
        self.write_u32(data.len() as u32);
        self.buf.extend_from_slice(data);
    }

    fn write_string(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    fn write_opt_string(&mut self, s: &Option<String>) {
        match s {
            Some(s) => {
                self.write_bool(true);
                self.write_string(s);
            }
            None => self.write_bool(false),
        }
    }

    fn write_value(&mut self, v: &Value) {
        match v {
            Value::Null => self.write_u8(TAG_NULL),
            Value::Int(n) => {
                self.write_u8(TAG_INT);
                self.write_i64(*n);
            }
            Value::Float(n) => {
                self.write_u8(TAG_FLOAT);
                self.write_f64(*n);
            }
            Value::Str(s) => {
                self.write_u8(TAG_STR);
                self.write_string(s);
            }
            Value::Bool(b) => {
                self.write_u8(TAG_BOOL);
                self.write_bool(*b);
            }
            Value::List(items) => {
                self.write_u8(TAG_LIST);
                self.write_u32(items.len() as u32);
                for item in items {
                    self.write_value(item);
                }
            }
            Value::Dict(pairs) => {
                self.write_u8(TAG_DICT);
                self.write_u32(pairs.len() as u32);
                for (k, v) in pairs {
                    self.write_value(k);
                    self.write_value(v);
                }
            }
            Value::Tuple(items) => {
                self.write_u8(TAG_TUPLE);
                self.write_u32(items.len() as u32);
                for item in items {
                    self.write_value(item);
                }
            }
            Value::Set(items) => {
                self.write_u8(TAG_SET);
                self.write_u32(items.len() as u32);
                for item in items {
                    self.write_value(item);
                }
            }
            Value::Range(start, end, inclusive) => {
                self.write_u8(TAG_RANGE);
                self.write_i64(*start);
                self.write_i64(*end);
                self.write_bool(*inclusive);
            }
            _ => {
                // Fallback: serialize as Null for non-constant-pool types
                self.write_u8(TAG_NULL);
            }
        }
    }

    fn write_chunk(&mut self, chunk: &Chunk) {
        // Code bytes
        self.write_bytes(&chunk.code);
        // Constants
        self.write_u32(chunk.constants.len() as u32);
        for c in &chunk.constants {
            self.write_value(c);
        }
        // Line numbers
        self.write_bytes(
            &chunk
                .lines
                .iter()
                .flat_map(|l| l.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        // Strings
        self.write_u32(chunk.strings.len() as u32);
        for s in &chunk.strings {
            self.write_string(s);
        }
    }

    fn write_compiled_func(&mut self, func: &CompiledFunc) {
        self.write_string(&func.name);
        self.write_u8(func.arity);
        self.write_bool(func.has_variadic);
        self.write_u8(func.default_count);
        self.write_u16(func.upvalue_count);
        self.write_bool(func.is_generator);
        self.write_chunk(&func.chunk);
    }

    fn write_class_def(&mut self, cd: &ClassDef) {
        self.write_string(&cd.name);
        self.write_opt_string(&cd.parent);
        // methods
        self.write_u32(cd.methods.len() as u32);
        for (name, func) in &cd.methods {
            self.write_string(name);
            self.write_compiled_func(func);
        }
        // fields
        self.write_u32(cd.fields.len() as u32);
        for (name, default) in &cd.fields {
            self.write_string(name);
            match default {
                Some(v) => {
                    self.write_bool(true);
                    self.write_value(v);
                }
                None => self.write_bool(false),
            }
        }
        self.write_bool(cd.is_fixed);
        self.write_bool(cd.is_data);
        self.write_bool(cd.is_sealed);
        // decorators
        self.write_u32(cd.decorators.len() as u32);
        for d in &cd.decorators {
            self.write_string(d);
        }
    }

    fn write_struct_def(&mut self, sd: &StructDef) {
        self.write_string(&sd.name);
        self.write_u32(sd.fields.len() as u32);
        for (name, ty) in &sd.fields {
            self.write_string(name);
            self.write_opt_string(ty);
        }
    }

    fn write_enum_def(&mut self, ed: &EnumDef) {
        self.write_string(&ed.name);
        self.write_u32(ed.variants.len() as u32);
        for (name, fields) in &ed.variants {
            self.write_string(name);
            self.write_u32(fields.len() as u32);
            for f in fields {
                self.write_string(f);
            }
        }
    }

    fn write_trait_def(&mut self, td: &TraitDef) {
        self.write_string(&td.name);
        // supertraits
        self.write_u32(td.supertraits.len() as u32);
        for s in &td.supertraits {
            self.write_string(s);
        }
        // method names
        self.write_u32(td.method_names.len() as u32);
        for n in &td.method_names {
            self.write_string(n);
        }
        // method funcs
        self.write_u32(td.method_funcs.len() as u32);
        for f in &td.method_funcs {
            self.write_compiled_func(f);
        }
    }

    fn write_impl_def(&mut self, ib: &ImplDef) {
        self.write_opt_string(&ib.trait_name);
        self.write_string(&ib.target);
        self.write_u32(ib.methods.len() as u32);
        for (name, func) in &ib.methods {
            self.write_string(name);
            self.write_compiled_func(func);
        }
    }
}

// ── Reader ───────────────────────────────────────────────────────

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        if self.pos >= self.data.len() {
            return Err("Unexpected end of bytecode file".to_string());
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_bool(&mut self) -> Result<bool, String> {
        Ok(self.read_u8()? != 0)
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        if self.remaining() < 2 {
            return Err("Unexpected end of bytecode file".to_string());
        }
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        if self.remaining() < 4 {
            return Err("Unexpected end of bytecode file".to_string());
        }
        let v = u32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    fn read_i64(&mut self) -> Result<i64, String> {
        if self.remaining() < 8 {
            return Err("Unexpected end of bytecode file".to_string());
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.data[self.pos..self.pos + 8]);
        self.pos += 8;
        Ok(i64::from_le_bytes(bytes))
    }

    fn read_f64(&mut self) -> Result<f64, String> {
        if self.remaining() < 8 {
            return Err("Unexpected end of bytecode file".to_string());
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.data[self.pos..self.pos + 8]);
        self.pos += 8;
        Ok(f64::from_le_bytes(bytes))
    }

    fn read_bytes(&mut self) -> Result<Vec<u8>, String> {
        let len = self.read_u32()? as usize;
        if self.remaining() < len {
            return Err("Unexpected end of bytecode file".to_string());
        }
        let v = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        Ok(v)
    }

    fn read_string(&mut self) -> Result<String, String> {
        let bytes = self.read_bytes()?;
        String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8 in bytecode: {}", e))
    }

    fn read_opt_string(&mut self) -> Result<Option<String>, String> {
        if self.read_bool()? {
            Ok(Some(self.read_string()?))
        } else {
            Ok(None)
        }
    }

    fn read_value(&mut self) -> Result<Value, String> {
        let tag = self.read_u8()?;
        match tag {
            TAG_NULL => Ok(Value::Null),
            TAG_INT => Ok(Value::Int(self.read_i64()?)),
            TAG_FLOAT => Ok(Value::Float(self.read_f64()?)),
            TAG_STR => Ok(Value::Str(self.read_string()?)),
            TAG_BOOL => Ok(Value::Bool(self.read_bool()?)),
            TAG_LIST => {
                let count = self.read_u32()? as usize;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(self.read_value()?);
                }
                Ok(Value::List(items))
            }
            TAG_DICT => {
                let count = self.read_u32()? as usize;
                let mut pairs = Vec::with_capacity(count);
                for _ in 0..count {
                    let k = self.read_value()?;
                    let v = self.read_value()?;
                    pairs.push((k, v));
                }
                Ok(Value::Dict(pairs))
            }
            TAG_TUPLE => {
                let count = self.read_u32()? as usize;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(self.read_value()?);
                }
                Ok(Value::Tuple(items))
            }
            TAG_SET => {
                let count = self.read_u32()? as usize;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(self.read_value()?);
                }
                Ok(Value::Set(items))
            }
            TAG_RANGE => {
                let start = self.read_i64()?;
                let end = self.read_i64()?;
                let inclusive = self.read_bool()?;
                Ok(Value::Range(start, end, inclusive))
            }
            _ => Err(format!("Unknown value tag in bytecode: {}", tag)),
        }
    }

    fn read_chunk(&mut self) -> Result<Chunk, String> {
        let code = self.read_bytes()?;
        let const_count = self.read_u32()? as usize;
        let mut constants = Vec::with_capacity(const_count);
        for _ in 0..const_count {
            constants.push(self.read_value()?);
        }
        // Line numbers: stored as raw u32 bytes
        let line_bytes = self.read_bytes()?;
        let lines: Vec<u32> = line_bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let string_count = self.read_u32()? as usize;
        let mut strings = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            strings.push(self.read_string()?);
        }
        Ok(Chunk {
            code,
            constants,
            lines,
            strings,
        })
    }

    fn read_compiled_func(&mut self) -> Result<CompiledFunc, String> {
        let name = self.read_string()?;
        let arity = self.read_u8()?;
        let has_variadic = self.read_bool()?;
        let default_count = self.read_u8()?;
        let upvalue_count = self.read_u16()?;
        let is_generator = self.read_bool()?;
        let chunk = self.read_chunk()?;
        Ok(CompiledFunc {
            name,
            arity,
            has_variadic,
            default_count,
            upvalue_count,
            is_generator,
            chunk,
        })
    }

    fn read_class_def(&mut self) -> Result<ClassDef, String> {
        let name = self.read_string()?;
        let parent = self.read_opt_string()?;
        let method_count = self.read_u32()? as usize;
        let mut methods = Vec::with_capacity(method_count);
        for _ in 0..method_count {
            let mname = self.read_string()?;
            let func = self.read_compiled_func()?;
            methods.push((mname, func));
        }
        let field_count = self.read_u32()? as usize;
        let mut fields = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            let fname = self.read_string()?;
            let has_default = self.read_bool()?;
            let default = if has_default {
                Some(self.read_value()?)
            } else {
                None
            };
            fields.push((fname, default));
        }
        let is_fixed = self.read_bool()?;
        let is_data = self.read_bool()?;
        let is_sealed = self.read_bool()?;
        let dec_count = self.read_u32()? as usize;
        let mut decorators = Vec::with_capacity(dec_count);
        for _ in 0..dec_count {
            decorators.push(self.read_string()?);
        }
        Ok(ClassDef {
            name,
            parent,
            methods,
            fields,
            is_fixed,
            is_data,
            is_sealed,
            decorators,
        })
    }

    fn read_struct_def(&mut self) -> Result<StructDef, String> {
        let name = self.read_string()?;
        let field_count = self.read_u32()? as usize;
        let mut fields = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            let fname = self.read_string()?;
            let ty = self.read_opt_string()?;
            fields.push((fname, ty));
        }
        Ok(StructDef { name, fields })
    }

    fn read_enum_def(&mut self) -> Result<EnumDef, String> {
        let name = self.read_string()?;
        let var_count = self.read_u32()? as usize;
        let mut variants = Vec::with_capacity(var_count);
        for _ in 0..var_count {
            let vname = self.read_string()?;
            let fc = self.read_u32()? as usize;
            let mut fnames = Vec::with_capacity(fc);
            for _ in 0..fc {
                fnames.push(self.read_string()?);
            }
            variants.push((vname, fnames));
        }
        Ok(EnumDef { name, variants })
    }

    fn read_trait_def(&mut self) -> Result<TraitDef, String> {
        let name = self.read_string()?;
        let st_count = self.read_u32()? as usize;
        let mut supertraits = Vec::with_capacity(st_count);
        for _ in 0..st_count {
            supertraits.push(self.read_string()?);
        }
        let mn_count = self.read_u32()? as usize;
        let mut method_names = Vec::with_capacity(mn_count);
        for _ in 0..mn_count {
            method_names.push(self.read_string()?);
        }
        let mf_count = self.read_u32()? as usize;
        let mut method_funcs = Vec::with_capacity(mf_count);
        for _ in 0..mf_count {
            method_funcs.push(self.read_compiled_func()?);
        }
        Ok(TraitDef {
            name,
            supertraits,
            method_names,
            method_funcs,
        })
    }

    fn read_impl_def(&mut self) -> Result<ImplDef, String> {
        let trait_name = self.read_opt_string()?;
        let target = self.read_string()?;
        let mc = self.read_u32()? as usize;
        let mut methods = Vec::with_capacity(mc);
        for _ in 0..mc {
            let mname = self.read_string()?;
            let func = self.read_compiled_func()?;
            methods.push((mname, func));
        }
        Ok(ImplDef {
            trait_name,
            target,
            methods,
        })
    }
}

// ── Public API ───────────────────────────────────────────────────

/// Serialize a CompileOutput to bytes.
pub fn serialize(output: &CompileOutput) -> Vec<u8> {
    let mut w = Writer::new();

    // Header
    w.buf.extend_from_slice(MAGIC);
    w.write_u32(FORMAT_VERSION);

    // Main function
    w.write_compiled_func(&output.main);

    // Compiled functions
    w.write_u32(output.compiled_funcs.len() as u32);
    for (name, func) in &output.compiled_funcs {
        w.write_string(name);
        w.write_compiled_func(func);
    }

    // Class defs
    w.write_u32(output.class_defs.len() as u32);
    for (_, cd) in &output.class_defs {
        w.write_class_def(cd);
    }

    // Struct defs
    w.write_u32(output.struct_defs.len() as u32);
    for (_, sd) in &output.struct_defs {
        w.write_struct_def(sd);
    }

    // Enum defs
    w.write_u32(output.enum_defs.len() as u32);
    for (_, ed) in &output.enum_defs {
        w.write_enum_def(ed);
    }

    // Trait defs
    w.write_u32(output.trait_defs.len() as u32);
    for (_, td) in &output.trait_defs {
        w.write_trait_def(td);
    }

    // Impl blocks
    w.write_u32(output.impl_blocks.len() as u32);
    for ib in &output.impl_blocks {
        w.write_impl_def(ib);
    }

    w.buf
}

/// Deserialize bytes into a CompileOutput.
pub fn deserialize(data: &[u8]) -> Result<CompileOutput, String> {
    let mut r = Reader::new(data);

    // Header
    if r.remaining() < 8 {
        return Err("Invalid bytecode file: too short".to_string());
    }
    let magic = &r.data[r.pos..r.pos + 4];
    if magic != MAGIC {
        return Err("Invalid bytecode file: bad magic".to_string());
    }
    r.pos += 4;
    let version = r.read_u32()?;
    if version != FORMAT_VERSION {
        return Err(format!(
            "Unsupported bytecode version {} (expected {})",
            version, FORMAT_VERSION
        ));
    }

    // Main function
    let main = r.read_compiled_func()?;

    // Compiled functions
    let fc = r.read_u32()? as usize;
    let mut compiled_funcs = HashMap::with_capacity(fc);
    for _ in 0..fc {
        let name = r.read_string()?;
        let func = r.read_compiled_func()?;
        compiled_funcs.insert(name, func);
    }

    // Class defs
    let cc = r.read_u32()? as usize;
    let mut class_defs = HashMap::with_capacity(cc);
    for _ in 0..cc {
        let cd = r.read_class_def()?;
        class_defs.insert(cd.name.clone(), cd);
    }

    // Struct defs
    let sc = r.read_u32()? as usize;
    let mut struct_defs = HashMap::with_capacity(sc);
    for _ in 0..sc {
        let sd = r.read_struct_def()?;
        struct_defs.insert(sd.name.clone(), sd);
    }

    // Enum defs
    let ec = r.read_u32()? as usize;
    let mut enum_defs = HashMap::with_capacity(ec);
    for _ in 0..ec {
        let ed = r.read_enum_def()?;
        enum_defs.insert(ed.name.clone(), ed);
    }

    // Trait defs
    let tc = r.read_u32()? as usize;
    let mut trait_defs = HashMap::with_capacity(tc);
    for _ in 0..tc {
        let td = r.read_trait_def()?;
        trait_defs.insert(td.name.clone(), td);
    }

    // Impl blocks
    let ic = r.read_u32()? as usize;
    let mut impl_blocks = Vec::with_capacity(ic);
    for _ in 0..ic {
        impl_blocks.push(r.read_impl_def()?);
    }

    Ok(CompileOutput {
        main,
        class_defs,
        struct_defs,
        enum_defs,
        trait_defs,
        impl_blocks,
        compiled_funcs,
    })
}
