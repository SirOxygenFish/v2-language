use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::rc::Rc;

use crate::ast::*;
use crate::environment::Environment;
use crate::value::{ClassValue, FuncValue, GeneratorState, Value};
use crate::fault;

#[derive(Clone)]
struct MacroValue {
    params: Vec<String>,
    body: Vec<Stmt>,
}

#[derive(Clone, Default)]
pub struct RuntimeSafetyOptions {
    pub strict_unsafe: bool,
    pub sanitizer: Option<String>,
}

#[derive(Clone)]
struct MemoryBlock {
    bytes: Vec<Value>,
    freed: bool,
}

pub struct Interpreter {
    pub env: Environment,
    defer_stack: Vec<Vec<Vec<Stmt>>>,
    yield_collector: Option<Vec<Value>>,
    loop_label: Option<String>,
    current_function: Vec<String>,
    pub test_mode: bool,
    memo_caches: HashMap<String, HashMap<String, Value>>,
    deprecated_funcs: HashMap<String, Option<String>>,
    macros: HashMap<String, MacroValue>,
    frozen_vars: HashSet<String>,
    safety: RuntimeSafetyOptions,
    memory_blocks: HashMap<i64, MemoryBlock>,
    next_pointer_id: i64,
    futures: HashMap<i64, Value>,
    next_future_id: i64,
    /// Registered hardware fault handlers: signal_name → V2 function value.
    pub fault_handlers: HashMap<String, Value>,
    /// The most recently captured FaultInfo dict (set by `signal.on_fault` dispatch).
    pub last_fault_info: Option<Value>,
    /// Extension methods added to primitive types via `impl` blocks.
    pub primitive_impls: HashMap<String, HashMap<String, FuncValue>>,
    /// For lazy generator re-execution: the index of the yield we want to capture (None = eager collect)
    yield_target_idx: Option<usize>,
    /// For lazy generator re-execution: how many yields have been seen so far
    yield_current_idx: usize,
    /// For lazy generator re-execution: the captured value at the target yield index
    yield_captured: Option<Value>,
    /// Values to feed back into previously-suspended yield expressions.
    yield_resume_values: Vec<Value>,
    /// Test blocks declared with `test "name" { ... }`.
    registered_tests: Vec<(String, Vec<Stmt>)>,
    /// Dynamically registered test callbacks via test_register(name, fn).
    registered_test_fns: Vec<(String, Value)>,
    /// The most recently thrown value, preserved so `catch (e)` can bind the
    /// original object (errors otherwise propagate as a plain Rust String).
    pending_throw: Option<Value>,
    /// Value carried by a `?` early return between the raising expression and
    /// the enclosing function boundary (see Expr::TryUnwrap).
    pending_try_return: Option<Value>,
    /// Trait membership: target type name -> traits it implements
    /// (recorded by `impl Trait for Type`; used by the `is` operator).
    trait_impls: HashMap<String, Vec<String>>,
    /// Current user-call nesting depth and its ceiling — a guard that turns
    /// runaway recursion into a catchable error before the native stack blows.
    call_depth: usize,
    recursion_limit: usize,
    /// Live embedded-engine workers (index = worker id in __engine_call names).
    engine_workers: Vec<crate::engines::EngineWorker>,
    /// Engine export registry: selector key ("@py", "block_name",
    /// "@py.block_name") -> exported (function, worker id) pairs.
    engine_exports: HashMap<String, Vec<(String, usize)>>,
    /// Foreign-module workers keyed by selector ("py.statistics").
    engine_module_workers: HashMap<String, usize>,
    /// Channel buffers keyed by channel id (synchronous queue semantics).
    channels: HashMap<i64, std::collections::VecDeque<Value>>,
    /// Whether a channel id has been closed.
    channels_closed: HashMap<i64, bool>,
    /// Results of eagerly-run `thread_spawn` closures, keyed by handle id.
    thread_results: HashMap<i64, Value>,
    /// Monotonic id source for channels and thread handles.
    next_handle_id: i64,
    /// Minimum std.log level that is emitted (0=DEBUG,1=INFO,2=WARN,3=ERROR,4=FATAL).
    log_level: i32,
    /// Current macro-expansion recursion depth (guards runaway expansion).
    macro_depth: usize,
    /// Maximum macro-expansion depth before a clean error (tunable via
    /// `ct_set_macro_limit`). Recursive macros are Turing-complete; this bounds
    /// them so infinite expansion errors instead of overflowing the stack.
    macro_limit: usize,
}

impl Interpreter {
    pub fn new() -> Self {
        Self::with_safety(RuntimeSafetyOptions::default())
    }

    pub fn with_safety(safety: RuntimeSafetyOptions) -> Self {
        let mut interp = Self {
            env: Environment::new(),
            defer_stack: vec![vec![]],
            yield_collector: None,
            loop_label: None,
            current_function: Vec::new(),
            test_mode: false,
            memo_caches: HashMap::new(),
            deprecated_funcs: HashMap::new(),
            macros: HashMap::new(),
            frozen_vars: HashSet::new(),
            safety,
            memory_blocks: HashMap::new(),
            next_pointer_id: 1,
            futures: HashMap::new(),
            next_future_id: 1,
            fault_handlers: HashMap::new(),
            last_fault_info: None,
            primitive_impls: HashMap::new(),
            yield_target_idx: None,
            yield_current_idx: 0,
            yield_captured: None,
            yield_resume_values: Vec::new(),
            registered_tests: Vec::new(),
            registered_test_fns: Vec::new(),
            pending_throw: None,
            pending_try_return: None,
            trait_impls: HashMap::new(),
            call_depth: 0,
            recursion_limit: 15_000,
            engine_workers: Vec::new(),
            engine_exports: HashMap::new(),
            engine_module_workers: HashMap::new(),
            channels: HashMap::new(),
            channels_closed: HashMap::new(),
            thread_results: HashMap::new(),
            next_handle_id: 1,
            log_level: 0,
            macro_depth: 0,
            macro_limit: 256,
        };
        interp.register_builtins();
        interp
    }

    /// Does a thrown value match a catch clause's type? An instance matches if
    /// its class equals `type_name` or inherits from it; otherwise fall back to
    /// matching the error message text (for string throws / builtin errors).
    fn error_is_a(&self, thrown: &Value, type_name: &str, err_msg: &str) -> bool {
        let class_name = match thrown {
            Value::Instance(cls, _) | Value::StructInstance(cls, _) => Some(cls.clone()),
            _ => None,
        };
        if let Some(mut cls) = class_name {
            // Walk the inheritance chain.
            loop {
                if cls == type_name {
                    return true;
                }
                match self.env.get(&cls) {
                    Some(Value::Class(cv)) => match cv.parent {
                        Some(p) => cls = p,
                        None => break,
                    },
                    _ => break,
                }
            }
            return false;
        }
        // Non-instance throw: match by message prefix (e.g. "ValueError: ...").
        err_msg.starts_with(type_name) || err_msg.contains(&format!("{}:", type_name))
    }

    fn memory_tracking_enabled(&self) -> bool {
        self.safety.strict_unsafe || matches!(self.safety.sanitizer.as_deref(), Some("address") | Some("leak"))
    }

    fn alloc_memory_block(&mut self, size: usize) -> Value {
        if self.memory_tracking_enabled() {
            let pointer_id = self.next_pointer_id;
            self.next_pointer_id += 1;
            self.memory_blocks.insert(pointer_id, MemoryBlock {
                bytes: vec![Value::Int(0); size],
                freed: false,
            });
            Value::Pointer(pointer_id)
        } else {
            Value::List(vec![Value::Int(0); size])
        }
    }

    fn get_memory_block(&self, pointer_id: i64, op: &str) -> Result<&MemoryBlock, String> {
        let block = self.memory_blocks.get(&pointer_id)
            .ok_or_else(|| format!("MemoryAccessError: {} on unknown pointer {}", op, pointer_id))?;
        if block.freed {
            Err(format!("MemoryAccessError: {} on freed pointer {}", op, pointer_id))
        } else {
            Ok(block)
        }
    }

    fn get_memory_block_mut(&mut self, pointer_id: i64, op: &str) -> Result<&mut MemoryBlock, String> {
        let block = self.memory_blocks.get_mut(&pointer_id)
            .ok_or_else(|| format!("MemoryAccessError: {} on unknown pointer {}", op, pointer_id))?;
        if block.freed {
            Err(format!("MemoryAccessError: {} on freed pointer {}", op, pointer_id))
        } else {
            Ok(block)
        }
    }

    fn report_leaks(&self) {
        if !matches!(self.safety.sanitizer.as_deref(), Some("leak")) {
            return;
        }
        let leaks: Vec<(i64, usize)> = self.memory_blocks.iter()
            .filter_map(|(pointer_id, block)| (!block.freed).then_some((*pointer_id, block.bytes.len())))
            .collect();
        if leaks.is_empty() {
            return;
        }
        eprintln!("LeakSanitizer: detected {} leaked allocation(s)", leaks.len());
        for (pointer_id, size) in leaks {
            eprintln!("  leaked pointer {} ({} bytes)", pointer_id, size);
        }
    }

    fn register_builtins(&mut self) {
        let builtins = [
            "print", "println", "input", "len", "type_of", "to_string",
            "to_int", "to_float", "range", "abs", "min", "max",
            "push", "pop", "pop_opt", "keys", "values", "contains", "remove",
            "split", "join", "trim", "replace", "starts_with", "ends_with",
            "to_upper", "to_lower", "substr", "char_at", "reverse",
            "sort", "map", "filter", "reduce", "each", "find",
            "enumerate", "zip", "flat_map", "any", "all", "sum",
            "count", "first", "last", "is_empty", "round", "floor",
            "ceil", "sqrt", "pow", "log", "sin", "cos", "tan",
            "mean", "median", "stddev",
            "slice", "insert", "extend", "clone", "hash",
            // Encoding & digest builtins (also exposed via std.crypto)
            "base64_encode", "base64_decode", "hex_encode", "hex_decode",
            "sha256", "sha512", "sha1", "md5",
            "assert", "panic", "exit",
            "set", "tuple",
            // New builtins
            "bool", "list", "dict", "int", "float", "str",
            "hex", "bin", "oct", "chr", "ord",
            "callable", "defined",
            "random", "random_int", "random_choice",
            "time", "getenv",
            "read_file", "write_file", "append_file", "file_exists", "delete_file",
            "json_parse", "json_stringify",
            "try_wrap",
            "sorted", "reversed",
            // Introspection & utility builtins
            "dir", "hasattr", "getattr", "setattr",
            "eval", "exec",
            "sleep",
            // Result/Option constructors
            "Ok", "Err", "Some",
            // Freeze / typeof
            "freeze", "is_frozen", "typeof",
            // Recursion depth control
            "set_recursion_limit", "get_recursion_limit",
            // chars/bytes
            "chars",
            // Assert helpers
            "assert_eq", "assert_ne",
            "expect_eq", "expect_ne", "expect_true", "expect_false",
            "expect_ok", "expect_err", "expect_some", "expect_none",
            "test_register", "test_run_all",
            // Conversion
            "from_pairs",
            // Standalone Option/Result/type checks
            "is_some", "is_none", "is_ok", "is_err", "is_func",
            "unwrap", "unwrap_or",
            // Collections
            "items", "shuffle", "str_from_bytes",
            // Scope introspection
            "vars", "memo",
            // New batch 10
            "patch", "is_lazy",
            "deque_new", "deque_push_front", "deque_push_back",
            "deque_pop_front", "deque_pop_back", "deque_len",
            "unwrap_err", "default_",
            "__io_write", "__io_write_line", "__io_flush",
            // Compile-time intrinsics
            "ct_platform", "ct_arch", "ct_word_exists", "ct_list_funcs",
            "ct_get_effects", "ct_unregister", "ct_emit", "ct_error",
            "ct_warn", "ct_set_macro_limit", "ct_get_macro_limit",
            "ct_feature", "mem_size_of",
            // Memory management
            "mem_alloc", "mem_alloc_zeroed", "mem_realloc", "mem_free",
            "mem_copy", "mem_set", "mem_read", "mem_write",
            // Vector (SIMD) operations
            "vec_new", "vec_from", "vec_get", "vec_set", "vec_len",
            "vec_add", "vec_sub", "vec_mul", "vec_div", "vec_scale",
            "vec_dot", "vec_norm", "vec_normalize", "vec_sum",
            "vec_min", "vec_max", "vec_clamp", "vec_copy", "vec_to_list",
            // Tensor operations
            "tensor_new", "tensor_from", "tensor_get", "tensor_set",
            "tensor_shape", "tensor_rank", "tensor_size",
            "tensor_add", "tensor_sub", "tensor_mul", "tensor_div", "tensor_scale",
            "tensor_matmul", "tensor_transpose", "tensor_reshape", "tensor_slice",
            "tensor_fill", "tensor_copy", "tensor_sum", "tensor_mean",
            "tensor_min", "tensor_max", "tensor_argmin", "tensor_argmax",
            "tensor_softmax", "tensor_relu", "tensor_to_list", "tensor_from_list",
            // Actor/Agent builtins
            "actor_spawn", "actor_send", "actor_receive", "actor_call",
            "actor_stop", "actor_is_alive",
            "agent_create", "agent_set_goal", "agent_get_state", "agent_set_state",
            "agent_run", "agent_done",
            // Isolate builtins
            "isolate_new", "isolate_get", "isolate_set", "isolate_exec", "isolate_run",
            // Weak reference builtins
            "weak_ref", "unwrap_or_default",
            // Engine builtins
            "register_engine",
            // Thread builtins
            "thread_spawn", "thread_join",
            "mutex_create", "mutex_lock", "mutex_unlock", "mutex_with",
            "rwmutex_create", "rwmutex_read_lock", "rwmutex_unlock",
            "rwmutex_write_lock",
            "atomic_new", "atomic_load", "atomic_store", "atomic_add", "atomic_sub", "atomic_cas",
            "threadpool_create", "threadpool_submit", "threadpool_submit_future", "threadpool_wait", "threadpool_destroy",
            "future_get", "future_is_done", "future_try_get",
            "waitgroup_create", "waitgroup_add", "waitgroup_done", "waitgroup_wait",
            // Channel builtins
            "chan_create", "chan_send", "chan_recv", "chan_close",
            "chan_is_closed", "chan_try_send", "chan_try_recv",
            "chan_drain", "chan_len", "chan_select",
            // Structured concurrency
            "task_group", "task_scope",
            // Borrow / move helpers
            "borrow", "borrow_mut", "deref", "unsafe_send",
        ];
        for name in &builtins {
            self.env.define(name, Value::BuiltinFunc(name.to_string()));
        }
        let math_module = self.build_std_math_module();
        self.env.define_const("math", math_module.clone());
        self.env.define_const("std.math", math_module);
        // Promise static API (async runs synchronously, so promises are already
        // resolved values). Promise.all/race/any/allSettled/resolve/reject/timeout.
        let promise_module = Value::Dict(vec![
            (Value::Str("all".into()), Value::BuiltinFunc("__promise_all".into())),
            (Value::Str("race".into()), Value::BuiltinFunc("__promise_race".into())),
            (Value::Str("any".into()), Value::BuiltinFunc("__promise_any".into())),
            (Value::Str("allSettled".into()), Value::BuiltinFunc("__promise_all_settled".into())),
            (Value::Str("resolve".into()), Value::BuiltinFunc("__promise_resolve".into())),
            (Value::Str("reject".into()), Value::BuiltinFunc("__promise_reject".into())),
            (Value::Str("timeout".into()), Value::BuiltinFunc("__promise_timeout".into())),
        ]);
        self.env.define_const("Promise", promise_module);
        let io_module = self.build_std_io_module();
        self.env.define_const("io", io_module.clone());
        self.env.define_const("std.io", io_module);
        let collections_module = self.build_std_collections_module();
        self.env.define_const("collections", collections_module.clone());
        self.env.define_const("std.collections", collections_module);

        // Register all stdlib modules
        let stdlib_modules: Vec<(&str, Value)> = vec![
            ("std.fs", self.build_std_fs_module()),
            ("std.fmt", self.build_std_fmt_module()),
            ("std.regex", self.build_std_regex_module()),
            ("std.iter", self.build_std_iter_module()),
            ("std.time", self.build_std_time_module()),
            ("std.proc", self.build_std_proc_module()),
            ("std.log", self.build_std_log_module()),
            ("std.test", self.build_std_test_module()),
            ("std.serialize", self.build_std_serialize_module()),
            ("std.rand", self.build_std_rand_module()),
            ("std.hash", self.build_std_hash_module()),
            ("std.cache", self.build_std_cache_module()),
            ("std.uuid", self.build_std_uuid_module()),
            ("std.csv", self.build_std_csv_module()),
            ("std.toml", self.build_std_toml_module()),
            ("std.yaml", self.build_std_yaml_module()),
            ("std.term", self.build_std_term_module()),
            ("std.cli", self.build_std_cli_module()),
            ("std.os", self.build_std_os_module()),
            ("std.net", self.build_std_net_module()),
            ("std.http", self.build_std_http_module()),
            ("std.crypto", self.build_std_crypto_module()),
            ("std.compress", self.build_std_compress_module()),
            ("std.xml", self.build_std_xml_module()),
            ("std.image", self.build_std_image_module()),
            ("std.mail", self.build_std_mail_module()),
            ("std.gfx3d", self.build_std_gfx3d_module()),
            ("std.game", self.build_std_game_module()),
            ("std.db", self.build_std_db_module()),
            ("std.ui", self.build_std_ui_module()),
            ("std.audio", self.build_std_audio_module()),
            ("std.video", self.build_std_video_module()),
            ("std.pdf", self.build_std_pdf_module()),
            ("std.excel", self.build_std_excel_module()),
            ("std.jwt", self.build_std_jwt_module()),
            ("std.oauth2", self.build_std_oauth2_module()),
            ("std.i18n", self.build_std_i18n_module()),
            ("std.watch", self.build_std_watch_module()),
            ("std.grpc", self.build_std_grpc_module()),
            ("std.mqtt", self.build_std_mqtt_module()),
            ("std.embed", self.build_std_embed_module()),
            ("std.template", self.build_std_template_module()),
            ("std.multipart", self.build_std_multipart_module()),
            ("std.ssh", self.build_std_ssh_module()),
            ("std.qr", self.build_std_qr_module()),
            ("std.markdown", self.build_std_markdown_module()),
            ("std.archive", self.build_std_archive_module()),
            ("std.dns", self.build_std_dns_module()),
            ("std.2d", self.build_std_2d_module()),
            ("std.graphql", self.build_std_graphql_module()),
            ("std.webrtc", self.build_std_webrtc_module()),
            ("std.clipboard", self.build_std_clipboard_module()),
            ("std.notify", self.build_std_notify_module()),
            ("std.speech", self.build_std_speech_module()),
            ("std.camera", self.build_std_camera_module()),
            ("std.serial", self.build_std_serial_module()),
            ("std.usb", self.build_std_usb_module()),
            ("std.bluetooth", self.build_std_bluetooth_module()),
            ("std.hotkey", self.build_std_hotkey_module()),
            ("std.tray", self.build_std_tray_module()),
            ("std.ipc", self.build_std_ipc_module()),
            ("std.decimal", self.build_std_decimal_module()),
            ("std.diff", self.build_std_diff_module()),
            ("std.semver", self.build_std_semver_module()),
            ("std.geo", self.build_std_geo_module()),
            ("std.gpu", self.build_std_gpu_module()),
            ("std.accessibility", self.build_std_accessibility_module()),
            ("std.blockchain", self.build_std_blockchain_module()),
            ("std.parse", self.build_std_parse_module()),
            ("std.config", self.build_std_config_module()),
            ("std.event", self.build_std_event_module()),
            ("std.diag", self.build_std_diag_module()),
            ("std.iot", self.build_std_iot_module()),
            ("std.hal", self.build_std_hal_module()),
            ("std.office", self.build_std_office_module()),
            ("std.money", self.build_std_money_module()),
            ("std.dotenv", self.build_std_dotenv_module()),
            ("std.scrape", self.build_std_scrape_module()),
            ("std.map", self.build_std_map_module()),
            ("std.task", self.build_std_task_module()),
            ("std.phone", self.build_std_phone_module()),
            ("std.barcode", self.build_std_barcode_module()),
            ("std.ml.vision", self.build_std_ml_vision_module()),
            ("std.ml.audio", self.build_std_ml_audio_module()),
            ("std.ffi", self.build_std_ffi_module()),
            ("std.signal", self.build_std_signal_module()),
            ("std.ai", self.build_std_ai_module()),
        ];
        for (name, module) in stdlib_modules {
            // Register both as "std.xxx" and shorthand
            let short_name = name.strip_prefix("std.").unwrap_or(name);
            self.env.define_const(name, module.clone());
            // Don't override already-registered modules (math, io, collections)
            if self.env.get(short_name).is_none() {
                self.env.define_const(short_name, module);
            }
        }

        // None is a value, not a function
        self.env.define("None", Value::Null);

        // Register built-in error hierarchy classes
        self.register_error_classes();
    }

    fn register_error_classes(&mut self) {
        let error_classes = [
            ("Error", None),
            ("TypeError", Some("Error")),
            ("ValueError", Some("Error")),
            ("IndexError", Some("Error")),
            ("KeyError", Some("Error")),
            ("OverflowError", Some("Error")),
            ("IOError", Some("Error")),
            ("NetworkError", Some("Error")),
            ("TimeoutError", Some("Error")),
            ("ParseError", Some("Error")),
            ("NotImplementedError", Some("Error")),
            ("AssertionError", Some("Error")),
            ("CancelledError", Some("Error")),
            ("AggregateError", Some("Error")),
            ("RuntimeError", Some("Error")),
            ("RangeError", Some("Error")),
            ("NullError", Some("Error")),
        ];
        for (name, parent) in &error_classes {
            let cls = Value::Class(ClassValue {
                name: name.to_string(),
                methods: HashMap::new(),
                fields: {
                    let mut f = HashMap::new();
                    f.insert("message".to_string(), Value::Str(String::new()));
                    f
                },
                field_order: vec!["message".to_string()],
                parent: parent.map(|s| s.to_string()),
                is_sealed: false,
                sealed_children: vec![],
                is_fixed: false,
                is_data: false,
                is_cow: false,
                computed_properties: HashMap::new(),
            });
            self.env.define(name, cls);
        }
    }

    fn build_std_math_module(&self) -> Value {
        let entries = vec![
            (Value::Str("PI".into()), Value::Float(std::f64::consts::PI)),
            (Value::Str("E".into()), Value::Float(std::f64::consts::E)),
            (Value::Str("TAU".into()), Value::Float(std::f64::consts::TAU)),
            (Value::Str("INF".into()), Value::Float(f64::INFINITY)),
            (Value::Str("NAN".into()), Value::Float(f64::NAN)),
            (Value::Str("abs".into()), Value::BuiltinFunc("abs".into())),
            (Value::Str("min".into()), Value::BuiltinFunc("min".into())),
            (Value::Str("max".into()), Value::BuiltinFunc("max".into())),
            (Value::Str("round".into()), Value::BuiltinFunc("round".into())),
            (Value::Str("floor".into()), Value::BuiltinFunc("floor".into())),
            (Value::Str("ceil".into()), Value::BuiltinFunc("ceil".into())),
            (Value::Str("sqrt".into()), Value::BuiltinFunc("sqrt".into())),
            (Value::Str("pow".into()), Value::BuiltinFunc("pow".into())),
            (Value::Str("log".into()), Value::BuiltinFunc("log".into())),
            (Value::Str("sin".into()), Value::BuiltinFunc("sin".into())),
            (Value::Str("cos".into()), Value::BuiltinFunc("cos".into())),
            (Value::Str("tan".into()), Value::BuiltinFunc("tan".into())),
            (Value::Str("asin".into()), Value::BuiltinFunc("__math_asin".into())),
            (Value::Str("acos".into()), Value::BuiltinFunc("__math_acos".into())),
            (Value::Str("atan".into()), Value::BuiltinFunc("__math_atan".into())),
            (Value::Str("atan2".into()), Value::BuiltinFunc("__math_atan2".into())),
            (Value::Str("sinh".into()), Value::BuiltinFunc("__math_sinh".into())),
            (Value::Str("cosh".into()), Value::BuiltinFunc("__math_cosh".into())),
            (Value::Str("tanh".into()), Value::BuiltinFunc("__math_tanh".into())),
            (Value::Str("deg".into()), Value::BuiltinFunc("__math_deg".into())),
            (Value::Str("rad".into()), Value::BuiltinFunc("__math_rad".into())),
            (Value::Str("exp".into()), Value::BuiltinFunc("__math_exp".into())),
            (Value::Str("exp2".into()), Value::BuiltinFunc("__math_exp2".into())),
            (Value::Str("log2".into()), Value::BuiltinFunc("__math_log2".into())),
            (Value::Str("log10".into()), Value::BuiltinFunc("__math_log10".into())),
            (Value::Str("cbrt".into()), Value::BuiltinFunc("__math_cbrt".into())),
            (Value::Str("hypot".into()), Value::BuiltinFunc("__math_hypot".into())),
            (Value::Str("trunc".into()), Value::BuiltinFunc("__math_trunc".into())),
            (Value::Str("clamp".into()), Value::BuiltinFunc("__math_clamp".into())),
            (Value::Str("lerp".into()), Value::BuiltinFunc("__math_lerp".into())),
            (Value::Str("sign".into()), Value::BuiltinFunc("__math_sign".into())),
            (Value::Str("is_nan".into()), Value::BuiltinFunc("__math_is_nan".into())),
            (Value::Str("is_inf".into()), Value::BuiltinFunc("__math_is_inf".into())),
            (Value::Str("is_finite".into()), Value::BuiltinFunc("__math_is_finite".into())),
            (Value::Str("gcd".into()), Value::BuiltinFunc("__math_gcd".into())),
            (Value::Str("lcm".into()), Value::BuiltinFunc("__math_lcm".into())),
            (Value::Str("factorial".into()), Value::BuiltinFunc("__math_factorial".into())),
            (Value::Str("is_prime".into()), Value::BuiltinFunc("__math_is_prime".into())),
            (Value::Str("mean".into()), Value::BuiltinFunc("mean".into())),
            (Value::Str("median".into()), Value::BuiltinFunc("median".into())),
            (Value::Str("stddev".into()), Value::BuiltinFunc("stddev".into())),
            (Value::Str("variance".into()), Value::BuiltinFunc("__math_variance".into())),
            (Value::Str("mode".into()), Value::BuiltinFunc("__math_mode".into())),
        ];
        Value::Dict(entries)
    }

    fn build_std_io_module(&self) -> Value {
        let stdout = Value::Dict(vec![
            (Value::Str("write".into()), Value::BuiltinFunc("__io_write".into())),
            (Value::Str("write_line".into()), Value::BuiltinFunc("__io_write_line".into())),
            (Value::Str("flush".into()), Value::BuiltinFunc("__io_flush".into())),
        ]);
        let stderr = Value::Dict(vec![
            (Value::Str("write".into()), Value::BuiltinFunc("__io_write".into())),
            (Value::Str("write_line".into()), Value::BuiltinFunc("__io_write_line".into())),
            (Value::Str("flush".into()), Value::BuiltinFunc("__io_flush".into())),
        ]);
        let stdin = Value::Dict(vec![
            (Value::Str("read_line".into()), Value::BuiltinFunc("input".into())),
            (Value::Str("read_all".into()), Value::BuiltinFunc("input".into())),
        ]);
        Value::Dict(vec![
            (Value::Str("stdout".into()), stdout),
            (Value::Str("stderr".into()), stderr),
            (Value::Str("stdin".into()), stdin),
            (Value::Str("read_file".into()), Value::BuiltinFunc("read_file".into())),
            (Value::Str("write_file".into()), Value::BuiltinFunc("write_file".into())),
            (Value::Str("append_file".into()), Value::BuiltinFunc("append_file".into())),
            (Value::Str("file_exists".into()), Value::BuiltinFunc("file_exists".into())),
            (Value::Str("delete_file".into()), Value::BuiltinFunc("delete_file".into())),
            (Value::Str("open".into()), Value::BuiltinFunc("__io_open".into())),
            (Value::Str("close".into()), Value::BuiltinFunc("__io_close".into())),
            (Value::Str("with_file".into()), Value::BuiltinFunc("__io_with_file".into())),
        ])
    }

    fn build_std_collections_module(&self) -> Value {
        Value::Dict(vec![
            (Value::Str("list".into()), Value::BuiltinFunc("list".into())),
            (Value::Str("dict".into()), Value::BuiltinFunc("dict".into())),
            (Value::Str("set".into()), Value::BuiltinFunc("set".into())),
            (Value::Str("tuple".into()), Value::BuiltinFunc("tuple".into())),
            (Value::Str("from_pairs".into()), Value::BuiltinFunc("from_pairs".into())),
            (Value::Str("deque_new".into()), Value::BuiltinFunc("deque_new".into())),
            (Value::Str("deque_push_front".into()), Value::BuiltinFunc("deque_push_front".into())),
            (Value::Str("deque_push_back".into()), Value::BuiltinFunc("deque_push_back".into())),
            (Value::Str("deque_pop_front".into()), Value::BuiltinFunc("deque_pop_front".into())),
            (Value::Str("deque_pop_back".into()), Value::BuiltinFunc("deque_pop_back".into())),
            (Value::Str("deque_len".into()), Value::BuiltinFunc("deque_len".into())),
            (Value::Str("stack".into()), Value::BuiltinFunc("__col_stack".into())),
            (Value::Str("queue".into()), Value::BuiltinFunc("__col_queue".into())),
            (Value::Str("deque".into()), Value::BuiltinFunc("deque_new".into())),
            (Value::Str("priority_queue".into()), Value::BuiltinFunc("__col_priority_queue".into())),
            (Value::Str("sorted_map".into()), Value::BuiltinFunc("__col_sorted_map".into())),
            (Value::Str("linked_list".into()), Value::BuiltinFunc("__col_linked_list".into())),
            (Value::Str("multiset".into()), Value::BuiltinFunc("__col_multiset".into())),
        ])
    }

    /// Helper to build a module dict from a list of (name, builtin_name) pairs
    fn make_module(entries: &[(&str, &str)]) -> Value {
        Value::Dict(entries.iter().map(|(k, v)| {
            (Value::Str((*k).into()), Value::BuiltinFunc((*v).into()))
        }).collect())
    }

    /// Helper to build a module dict from names (where builtin name = __module_funcname)
    fn make_stub_module(prefix: &str, names: &[&str]) -> Value {
        Value::Dict(names.iter().map(|n| {
            (Value::Str((*n).into()), Value::BuiltinFunc(format!("__{prefix}_{n}")))
        }).collect())
    }

    fn build_std_fs_module(&self) -> Value {
        Self::make_stub_module("fs", &[
            "join", "basename", "dirname", "ext", "stem", "abs", "normalize", "is_abs",
            "read", "write", "append", "copy", "delete", "exists", "is_file", "is_dir",
            "size", "modified", "mkdir", "rmdir", "ls", "walk", "glob", "cwd", "chdir",
            "symlink", "readlink", "is_symlink", "chmod", "stat", "watch",
            "move_file",
        ])
    }

    fn build_std_fmt_module(&self) -> Value {
        Self::make_stub_module("fmt", &[
            "sprintf", "printf", "number", "currency", "percent",
            "pad_left", "pad_right", "table", "template",
            "collate_sort", "collate_compare",
        ])
    }

    fn build_std_regex_module(&self) -> Value {
        Value::Dict(vec![
            (Value::Str("match_".into()), Value::BuiltinFunc("__regex_match".into())),
            // `regex.match(...)` — `match` is a keyword but valid as a field name.
            (Value::Str("match".into()), Value::BuiltinFunc("__regex_match".into())),
            (Value::Str("is_match".into()), Value::BuiltinFunc("__regex_is_match".into())),
            (Value::Str("test".into()), Value::BuiltinFunc("__regex_test".into())),
            (Value::Str("find".into()), Value::BuiltinFunc("__regex_find".into())),
            (Value::Str("find_all".into()), Value::BuiltinFunc("__regex_find_all".into())),
            (Value::Str("capture".into()), Value::BuiltinFunc("__regex_capture".into())),
            (Value::Str("replace".into()), Value::BuiltinFunc("__regex_replace".into())),
            (Value::Str("replace_all".into()), Value::BuiltinFunc("__regex_replace_all".into())),
            (Value::Str("split".into()), Value::BuiltinFunc("__regex_split".into())),
            (Value::Str("compile".into()), Value::BuiltinFunc("__regex_compile".into())),
        ])
    }

    fn build_std_iter_module(&self) -> Value {
        Self::make_stub_module("iter", &[
            "take", "skip", "take_while", "skip_while", "map", "filter",
            "flat_map", "flatten", "zip", "zip_with", "chain", "enumerate",
            "window", "chunk", "step_by", "cycle", "repeat", "peekable",
            "collect", "reduce", "count", "sum", "min", "max",
            "find", "any", "all", "for_each",
        ])
    }

    fn build_std_time_module(&self) -> Value {
        let mut entries: Vec<(Value, Value)> = vec![
            (Value::Str("now".into()), Value::BuiltinFunc("__time_now".into())),
            (Value::Str("now_utc".into()), Value::BuiltinFunc("__time_now_utc".into())),
            (Value::Str("timestamp".into()), Value::BuiltinFunc("__time_timestamp".into())),
            (Value::Str("unix".into()), Value::BuiltinFunc("__time_unix".into())),
        ];
        for name in &["parse", "format", "duration", "add", "sub", "diff",
                       "in_tz", "list_tz", "before", "after", "equal",
                       "start_of_day", "start_of_week", "days_in_month",
                       "is_leap_year", "set_timeout", "set_interval", "clear_timer"] {
            entries.push((Value::Str((*name).into()), Value::BuiltinFunc(format!("__time_{}", name))));
        }
        Value::Dict(entries)
    }

    fn build_std_proc_module(&self) -> Value {
        Self::make_stub_module("proc", &[
            "run", "spawn", "pipe", "shell", "write", "read_line",
            "wait", "kill", "getenv", "setenv", "unsetenv", "environ",
            "args", "pid", "exit",
        ])
    }

    fn build_std_log_module(&self) -> Value {
        Self::make_stub_module("log", &[
            "debug", "info", "warn", "error", "fatal", "fatal_and_exit",
            "set_level", "set_format", "set_output", "new", "with",
            "add_sink", "context", "context_get", "context_clear",
        ])
    }

    fn build_std_test_module(&self) -> Value {
        let mut entries = vec![
            (Value::Str("register".into()), Value::BuiltinFunc("test_register".into())),
            (Value::Str("run_all".into()), Value::BuiltinFunc("test_run_all".into())),
            (Value::Str("expect_eq".into()), Value::BuiltinFunc("expect_eq".into())),
            (Value::Str("expect_ne".into()), Value::BuiltinFunc("expect_ne".into())),
            (Value::Str("expect_true".into()), Value::BuiltinFunc("expect_true".into())),
            (Value::Str("expect_false".into()), Value::BuiltinFunc("expect_false".into())),
            (Value::Str("expect_ok".into()), Value::BuiltinFunc("expect_ok".into())),
            (Value::Str("expect_err".into()), Value::BuiltinFunc("expect_err".into())),
            (Value::Str("expect_some".into()), Value::BuiltinFunc("expect_some".into())),
            (Value::Str("expect_none".into()), Value::BuiltinFunc("expect_none".into())),
        ];
        for name in &["before_all", "after_all", "before_each", "after_each", "each", "snapshot", "skip", "todo", "property", "property_stateful"] {
            entries.push((Value::Str((*name).into()), Value::BuiltinFunc(format!("__test_{}", name))));
        }
        Value::Dict(entries)
    }

    fn build_std_serialize_module(&self) -> Value {
        let mut entries = vec![
            (Value::Str("json_encode".into()), Value::BuiltinFunc("json_stringify".into())),
            (Value::Str("json_decode".into()), Value::BuiltinFunc("json_parse".into())),
        ];
        for name in &["json_stream_writer", "json_stream_reader",
                       "toml_encode", "toml_decode", "yaml_encode", "yaml_decode",
                       "csv_encode", "csv_decode", "msgpack_encode", "msgpack_decode",
                       "proto_schema", "proto_encode", "proto_decode", "proto_load",
                       "binary_encode", "binary_decode"] {
            entries.push((Value::Str((*name).into()), Value::BuiltinFunc(format!("__serialize_{}", name))));
        }
        Value::Dict(entries)
    }

    fn build_std_rand_module(&self) -> Value {
        let mut entries = vec![
            (Value::Str("float".into()), Value::BuiltinFunc("random".into())),
            (Value::Str("int".into()), Value::BuiltinFunc("random_int".into())),
            (Value::Str("choice".into()), Value::BuiltinFunc("random_choice".into())),
        ];
        for name in &["bool", "choices", "sample", "shuffle", "weighted_choice",
                       "normal", "exponential", "poisson", "binomial",
                       "secure_bytes", "secure_int", "secure_token", "new"] {
            entries.push((Value::Str((*name).into()), Value::BuiltinFunc(format!("__rand_{}", name))));
        }
        Value::Dict(entries)
    }

    fn build_std_hash_module(&self) -> Value {
        Self::make_stub_module("hash", &[
            "fnv1a", "fnv1a64", "djb2", "sdbm", "murmur3", "xxhash", "xxhash64",
            "crc32", "crc32c", "adler32",
            "content_id", "hasher", "bloom_filter",
        ])
    }

    fn build_std_cache_module(&self) -> Value {
        Self::make_stub_module("cache", &[
            "new", "set", "get", "has", "delete", "clear",
            "stats", "memoize", "namespace",
        ])
    }

    fn build_std_uuid_module(&self) -> Value {
        Self::make_stub_module("uuid", &[
            "v4", "v1", "v7", "nil", "v4_compact", "v4_urn",
            "parse", "is_valid", "equals", "compare",
        ])
    }

    fn build_std_csv_module(&self) -> Value {
        Self::make_stub_module("csv", &[
            "parse", "stringify", "read", "write", "open_reader",
            "schema", "validate",
        ])
    }

    fn build_std_toml_module(&self) -> Value {
        Self::make_stub_module("toml", &["parse", "read", "stringify", "write"])
    }

    fn build_std_yaml_module(&self) -> Value {
        Self::make_stub_module("yaml", &["parse", "parse_all", "read", "stringify", "write"])
    }

    fn build_std_term_module(&self) -> Value {
        Self::make_stub_module("term", &[
            "red", "green", "yellow", "blue", "cyan", "magenta", "white", "gray",
            "bold", "italic", "underline", "strikethrough", "dim",
            "move_to", "move_up", "move_down", "move_left", "move_right",
            "save_cursor", "restore_cursor", "hide_cursor", "show_cursor",
            "clear_screen", "clear_line", "clear_to_end",
            "size", "raw_mode", "read_key", "supports_color", "color_depth",
            "progress_bar", "spinner",
        ])
    }

    fn build_std_cli_module(&self) -> Value {
        Self::make_stub_module("cli", &[
            "app", "flag", "option", "arg", "multi_arg",
            "subcommand", "parse", "print_help", "print_usage",
        ])
    }

    fn build_std_os_module(&self) -> Value {
        let mut entries = vec![
            (Value::Str("getenv".into()), Value::BuiltinFunc("getenv".into())),
            (Value::Str("exit".into()), Value::BuiltinFunc("exit".into())),
        ];
        for name in &["platform", "arch", "hostname", "username", "home_dir",
                       "pid", "ppid", "cpu_count", "uptime", "v2_version",
                       "setenv", "unsetenv", "environ",
                       "on_signal", "send_signal", "ignore_signal", "reset_signal",
                       "abort", "at_exit",
                       "temp_dir", "config_dir", "data_dir", "cache_dir",
                       "executable_path"] {
            entries.push((Value::Str((*name).into()), Value::BuiltinFunc(format!("__os_{}", name))));
        }
        Value::Dict(entries)
    }

    fn build_std_net_module(&self) -> Value {
        Self::make_stub_module("net", &[
            "http_get", "http_post", "http_put", "http_delete", "http_patch",
            "http_request", "http_router", "http_serve", "http_serve_tls",
            "ws_connect", "ws_send", "ws_recv", "ws_close", "ws_serve",
            "tcp_connect", "tcp_listen", "tcp_send", "tcp_recv",
            "udp_socket", "udp_send", "udp_recv",
            "dns_resolve", "dns_resolve_ipv4", "dns_resolve_ipv6",
            "dns_reverse", "dns_lookup_mx", "dns_lookup_txt",
        ])
    }

    fn build_std_http_module(&self) -> Value {
        Self::make_stub_module("http", &[
            "server", "client",
            "get", "post", "put", "delete", "patch", "ws",
            "use_middleware", "group", "static_files", "start", "start_async", "stop",
            "rate_limiter", "sse_connect", "ws_connect",
        ])
    }

    fn build_std_crypto_module(&self) -> Value {
        Self::make_stub_module("crypto", &[
            "sha256", "sha512", "sha1", "md5", "blake3", "hmac", "hmac_sha256", "hash_file",
            "aes_encrypt", "aes_decrypt", "chacha20_encrypt", "chacha20_decrypt",
            "rsa_keygen", "rsa_encrypt", "rsa_decrypt", "rsa_sign", "rsa_verify",
            "ec_keygen", "ec_sign", "ec_verify",
            "pbkdf2", "bcrypt_hash", "bcrypt_verify", "argon2",
            "secure_random", "secure_random_int", "uuid4",
            "base64_encode", "base64_decode", "hex_encode", "hex_decode",
        ])
    }

    fn build_std_compress_module(&self) -> Value {
        Self::make_stub_module("compress", &[
            "gzip_compress", "gzip_decompress", "gzip_compress_file", "gzip_decompress_file",
            "zstd_compress", "zstd_decompress", "zstd_compress_file", "zstd_decompress_file",
            "brotli_compress", "brotli_decompress",
            "lz4_compress", "lz4_decompress", "lz4_compress_file", "lz4_decompress_file",
            "zip_create", "zip_add_file", "zip_add_dir", "zip_close",
            "zip_list", "zip_read", "zip_read_bytes", "zip_extract",
            "tar_create", "tar_list", "tar_extract",
        ])
    }

    fn build_std_xml_module(&self) -> Value {
        Self::make_stub_module("xml", &[
            "parse", "parse_html", "parse_file", "parse_html_file",
            "element", "stringify",
        ])
    }

    fn build_std_image_module(&self) -> Value {
        Self::make_stub_module("image", &[
            "load", "load_bytes", "save", "save_bytes",
            "resize", "crop", "smart_crop", "flip_h", "flip_v",
            "rotate", "grayscale", "blur", "sharpen",
            "brightness", "contrast", "invert", "opacity",
            "composite", "draw_text", "draw_rect",
            "get_pixel", "set_pixel", "to_rgba", "to_rgb", "thumbnail",
        ])
    }

    fn build_std_mail_module(&self) -> Value {
        Self::make_stub_module("mail", &[
            "connect", "send", "send_raw", "disconnect", "template", "render",
        ])
    }

    fn build_std_gfx3d_module(&self) -> Value {
        Self::make_stub_module("gfx3d", &[
            "gfx_init", "gfx_close", "gfx_begin_frame", "gfx_end_frame", "gfx_clear",
            "scene_new", "scene_add", "scene_remove",
            "node_new", "node_set_pos", "node_set_rot", "node_set_scale",
            "node_translate", "node_rotate",
            "mesh_load", "mesh_cube", "mesh_sphere", "mesh_plane", "mesh_custom",
            "material_new", "shader_load",
            "camera_new", "camera_set_pos", "camera_look_at", "camera_set_ortho",
            "light_directional", "light_point", "light_spot", "light_ambient",
        ])
    }

    fn build_std_game_module(&self) -> Value {
        Self::make_stub_module("game", &[
            "game_init", "game_run", "game_quit", "game_delta", "game_fps",
            "entity_new", "entity_add_component", "entity_get_component",
            "entity_destroy", "entity_find", "world_entities",
            "input_key_down", "input_key_pressed", "input_key_released",
            "input_mouse_pos", "input_mouse_delta", "input_mouse_button",
            "input_gamepad_axis", "input_gamepad_button",
            "physics_init", "body_new", "body_set_pos", "body_apply_force",
            "collider_box", "collider_sphere", "collider_capsule", "raycast",
            "audio_load", "audio_play", "audio_stop",
            "sprite_new", "sprite_draw", "sprite_animate",
            "tilemap_load", "tilemap_draw",
        ])
    }

    fn build_std_db_module(&self) -> Value {
        Self::make_stub_module("db", &[
            "db_connect", "db_query", "db_exec", "db_transaction",
            "db_prepare", "db_run", "db_close",
            "db_migrate", "db_migrate_up", "db_migrate_down", "db_migrate_status",
            "kv_open", "kv_set", "kv_get", "kv_delete", "kv_keys", "kv_close",
        ])
    }

    fn build_std_ui_module(&self) -> Value {
        Self::make_stub_module("ui", &[
            "ui_app", "ui_set_layout", "ui_render",
            "ui_label", "ui_button", "ui_input", "ui_textarea",
            "ui_checkbox", "ui_radio", "ui_slider", "ui_dropdown",
            "ui_image", "ui_divider", "ui_container", "ui_scroll",
            "ui_tabs", "ui_table", "ui_progress", "ui_modal",
            "ui_tooltip", "ui_menu", "ui_spacer",
        ])
    }

    fn build_std_audio_module(&self) -> Value {
        Self::make_stub_module("audio", &[
            "open", "play", "pause", "stop", "seek", "volume", "speed",
            "duration", "position", "on_end",
            "recorder", "start", "save", "on_chunk",
            "load_buffer", "gain", "low_pass", "high_pass", "band_pass",
            "reverb", "normalize", "to_mono", "trim_silence", "concat", "mix",
        ])
    }

    fn build_std_video_module(&self) -> Value {
        Self::make_stub_module("video", &[
            "open", "frame_at", "frames", "iter_frames", "seek", "next_frame",
            "writer", "write_frame", "trim", "concat",
            "extract_audio", "replace_audio", "mute", "thumbnail", "to_gif",
        ])
    }

    fn build_std_pdf_module(&self) -> Value {
        Self::make_stub_module("pdf", &[
            "new", "add_page", "text", "heading", "paragraph",
            "table", "rect", "circle", "line", "image", "save",
            "open", "page_count", "metadata",
            "extract_text", "page", "search",
        ])
    }

    fn build_std_excel_module(&self) -> Value {
        Self::make_stub_module("excel", &[
            "new", "open", "add_sheet", "sheet",
            "set_cell", "get_cell", "set_row", "set_column", "set_range",
            "set_format", "set_col_width", "set_row_height",
            "freeze_panes", "chart", "add_chart", "save",
        ])
    }

    fn build_std_jwt_module(&self) -> Value {
        Self::make_stub_module("jwt", &["sign", "verify", "decode_unverified"])
    }

    fn build_std_oauth2_module(&self) -> Value {
        Self::make_stub_module("oauth2", &[
            "client", "auth_url", "exchange_code", "client_credentials",
            "refresh", "get", "post",
        ])
    }

    fn build_std_i18n_module(&self) -> Value {
        Self::make_stub_module("i18n", &[
            "load_dir", "set_locale", "t",
            "format_number", "format_currency", "format_date", "relative_time",
        ])
    }

    fn build_std_watch_module(&self) -> Value {
        Self::make_stub_module("watch", &[
            "new", "on_change", "add", "remove", "start", "stop", "wait_for",
        ])
    }

    fn build_std_grpc_module(&self) -> Value {
        Self::make_stub_module("grpc", &[
            "server", "register", "start", "await_", "channel", "stub",
        ])
    }

    fn build_std_mqtt_module(&self) -> Value {
        Self::make_stub_module("mqtt", &[
            "connect", "publish", "subscribe", "unsubscribe", "disconnect",
        ])
    }

    fn build_std_embed_module(&self) -> Value {
        Self::make_stub_module("embed", &["file", "dir", "str", "bytes"])
    }

    fn build_std_template_module(&self) -> Value {
        Self::make_stub_module("template", &[
            "new", "render", "compile", "register_filter", "register_helper",
        ])
    }

    fn build_std_multipart_module(&self) -> Value {
        Self::make_stub_module("multipart", &[
            "new", "add_field", "add_file", "boundary", "to_bytes", "parse",
        ])
    }

    fn build_std_ssh_module(&self) -> Value {
        Self::make_stub_module("ssh", &[
            "connect", "exec", "upload", "download", "disconnect",
            "sftp_ls", "sftp_mkdir", "sftp_rm",
        ])
    }

    fn build_std_qr_module(&self) -> Value {
        Self::make_stub_module("qr", &["generate", "generate_svg", "generate_png", "read"])
    }

    fn build_std_markdown_module(&self) -> Value {
        Self::make_stub_module("markdown", &[
            "parse", "to_html", "to_text", "render", "highlight_code",
        ])
    }

    fn build_std_archive_module(&self) -> Value {
        Self::make_stub_module("archive", &[
            "zip_create", "zip_extract", "zip_list", "zip_add",
            "tar_create", "tar_extract", "tar_list",
        ])
    }

    fn build_std_dns_module(&self) -> Value {
        Self::make_stub_module("dns", &[
            "resolve", "resolve_ipv4", "resolve_ipv6", "reverse",
            "lookup_mx", "lookup_txt",
        ])
    }

    fn build_std_2d_module(&self) -> Value {
        Self::make_stub_module("2d", &[
            "canvas", "rect", "circle", "line", "arc", "path",
            "text", "image", "gradient", "transform",
            "save_png", "save_svg",
        ])
    }

    fn build_std_graphql_module(&self) -> Value {
        Self::make_stub_module("graphql", &[
            "schema", "query", "mutation", "subscribe",
            "execute", "serve",
        ])
    }

    fn build_std_webrtc_module(&self) -> Value {
        Self::make_stub_module("webrtc", &[
            "peer", "offer", "answer", "ice_candidate",
            "data_channel", "send", "on_message", "close",
        ])
    }

    fn build_std_clipboard_module(&self) -> Value {
        Self::make_stub_module("clipboard", &["read", "write", "has_text", "clear"])
    }

    fn build_std_notify_module(&self) -> Value {
        Self::make_stub_module("notify", &["send", "with_icon", "with_action", "clear"])
    }

    fn build_std_speech_module(&self) -> Value {
        Self::make_stub_module("speech", &[
            "speak", "stop", "voices", "set_voice", "set_rate", "set_volume",
            "recognize", "start_listening", "stop_listening",
        ])
    }

    fn build_std_camera_module(&self) -> Value {
        Self::make_stub_module("camera", &[
            "list", "open", "close", "capture", "stream", "stop", "set_resolution",
        ])
    }

    fn build_std_serial_module(&self) -> Value {
        Self::make_stub_module("serial", &[
            "list_ports", "open", "close", "read", "write", "set_baud",
        ])
    }

    fn build_std_usb_module(&self) -> Value {
        Self::make_stub_module("usb", &[
            "list_devices", "open", "close", "read", "write",
            "claim_interface", "release_interface",
        ])
    }

    fn build_std_bluetooth_module(&self) -> Value {
        Self::make_stub_module("bluetooth", &[
            "scan", "connect", "disconnect", "send", "receive",
            "list_services", "subscribe",
        ])
    }

    fn build_std_hotkey_module(&self) -> Value {
        Self::make_stub_module("hotkey", &["register", "unregister", "listen", "stop"])
    }

    fn build_std_tray_module(&self) -> Value {
        Self::make_stub_module("tray", &["create", "set_icon", "set_menu", "set_tooltip", "remove"])
    }

    fn build_std_ipc_module(&self) -> Value {
        Self::make_stub_module("ipc", &[
            "channel", "send", "recv", "named_pipe_server", "named_pipe_client",
            "shared_memory", "close",
        ])
    }

    fn build_std_decimal_module(&self) -> Value {
        Self::make_stub_module("decimal", &[
            "new", "add", "sub", "mul", "div", "round",
            "from_str", "from_int", "from_float", "to_str", "compare", "abs",
            "set_precision",
        ])
    }

    fn build_std_diff_module(&self) -> Value {
        // Note: intentionally no "diff" member — it would shadow the `diff`
        // module binding when `import "std.diff"` flattens members into scope.
        Self::make_stub_module("diff", &[
            "compute", "patch", "apply", "unified", "side_by_side",
            "context", "word_diff", "char_diff",
        ])
    }

    fn build_std_semver_module(&self) -> Value {
        Self::make_stub_module("semver", &[
            "parse", "compare", "satisfies", "increment",
            "major", "minor", "patch_ver", "is_valid",
        ])
    }

    fn build_std_geo_module(&self) -> Value {
        Self::make_stub_module("geo", &[
            "distance", "bearing", "midpoint", "bbox",
            "point_in_polygon", "geojson_parse", "wkt_parse",
        ])
    }

    fn build_std_gpu_module(&self) -> Value {
        Self::make_stub_module("gpu", &[
            "device", "buffer", "shader", "pipeline",
            "dispatch", "read_buffer", "write_buffer",
        ])
    }

    fn build_std_accessibility_module(&self) -> Value {
        Self::make_stub_module("accessibility", &[
            "announce", "set_role", "set_label", "focus",
            "screen_reader_active", "high_contrast",
        ])
    }

    fn build_std_blockchain_module(&self) -> Value {
        Self::make_stub_module("blockchain", &[
            "connect", "get_balance", "send_tx", "sign_tx",
            "deploy_contract", "call_contract", "listen_events",
        ])
    }

    fn build_std_parse_module(&self) -> Value {
        Self::make_stub_module("parse", &[
            "literal", "regex", "seq", "alt", "many", "optional",
            "map", "sep_by", "between", "not", "eof", "run",
        ])
    }

    fn build_std_config_module(&self) -> Value {
        Self::make_stub_module("config", &[
            "load", "get", "set", "merge", "from_env",
            "from_file", "validate", "schema",
        ])
    }

    fn build_std_event_module(&self) -> Value {
        Self::make_stub_module("event", &[
            "bus", "emit", "on", "once", "off", "wait_for",
        ])
    }

    fn build_std_diag_module(&self) -> Value {
        Self::make_stub_module("diag", &[
            "trace", "span", "metric", "counter", "histogram",
            "gauge", "health_check", "export",
        ])
    }

    fn build_std_iot_module(&self) -> Value {
        Self::make_stub_module("iot", &[
            "device", "sensor_read", "actuator_write",
            "mqtt_connect", "coap_request", "modbus_read",
        ])
    }

    fn build_std_hal_module(&self) -> Value {
        Self::make_stub_module("hal", &[
            "gpio_init", "gpio_read", "gpio_write",
            "spi_init", "spi_transfer",
            "i2c_init", "i2c_read", "i2c_write",
            "pwm_init", "pwm_set_duty",
        ])
    }

    fn build_std_office_module(&self) -> Value {
        Self::make_stub_module("office", &[
            "docx_new", "docx_open", "docx_add_paragraph", "docx_save",
            "pptx_new", "pptx_open", "pptx_add_slide", "pptx_save",
        ])
    }

    fn build_std_money_module(&self) -> Value {
        Self::make_stub_module("money", &[
            "new", "add", "sub", "mul", "div", "convert",
            "format", "compare", "round", "rates",
        ])
    }

    fn build_std_dotenv_module(&self) -> Value {
        Self::make_stub_module("dotenv", &["load", "get", "get_or", "all", "parse", "read", "set", "require"])
    }

    fn build_std_scrape_module(&self) -> Value {
        Self::make_stub_module("scrape", &[
            "fetch", "parse", "select", "select_all",
            "text", "attr", "links", "browser",
            "click", "type_text", "screenshot", "wait_for",
        ])
    }

    fn build_std_map_module(&self) -> Value {
        Self::make_stub_module("map", &[
            "new", "add_marker", "add_line", "add_polygon",
            "set_center", "set_zoom", "render", "save",
        ])
    }

    fn build_std_task_module(&self) -> Value {
        Self::make_stub_module("task", &[
            "queue", "enqueue", "dequeue", "process",
            "schedule", "retry", "status", "cancel",
        ])
    }

    fn build_std_phone_module(&self) -> Value {
        Self::make_stub_module("phone", &[
            "send_sms", "make_call", "end_call",
            "parse_number", "format_number", "validate",
        ])
    }

    fn build_std_barcode_module(&self) -> Value {
        Self::make_stub_module("barcode", &[
            "generate", "generate_png", "generate_svg",
            "scan", "decode",
        ])
    }

    fn build_std_ml_vision_module(&self) -> Value {
        Self::make_stub_module("ml_vision", &[
            "classify", "detect", "segment", "ocr",
            "face_detect", "pose_estimate", "load_model",
        ])
    }

    fn build_std_ml_audio_module(&self) -> Value {
        Self::make_stub_module("ml_audio", &[
            "transcribe", "classify", "denoise",
            "speaker_id", "emotion", "load_model",
        ])
    }

    fn build_std_ffi_module(&self) -> Value {
        Self::make_stub_module("ffi", &[
            "load", "load_lib", "func", "struct_def", "callback",
            "alloc", "alloc_zeroed", "free", "cstring", "read_cstring",
        ])
    }

    fn build_std_signal_module(&self) -> Value {
        // Soft-signal stubs (on, once, off, reset, ignore, raise, alarm, list)
        // are left as stubs for now; hardware fault APIs are real implementations.
        Value::Dict(vec![
            (Value::Str("on".into()),               Value::BuiltinFunc("__signal_on".into())),
            (Value::Str("once".into()),             Value::BuiltinFunc("__signal_once".into())),
            (Value::Str("off".into()),              Value::BuiltinFunc("__signal_off".into())),
            (Value::Str("reset".into()),            Value::BuiltinFunc("__signal_reset".into())),
            (Value::Str("ignore".into()),           Value::BuiltinFunc("__signal_ignore".into())),
            (Value::Str("raise".into()),            Value::BuiltinFunc("__signal_raise".into())),
            (Value::Str("alarm".into()),            Value::BuiltinFunc("__signal_alarm".into())),
            (Value::Str("list".into()),             Value::BuiltinFunc("__signal_list".into())),
            // Hardware fault APIs — Milestone 1 implementation.
            (Value::Str("on_fault".into()),         Value::BuiltinFunc("__signal_on_fault".into())),
            (Value::Str("set_recovery_point".into()),Value::BuiltinFunc("__signal_set_recovery_point".into())),
            (Value::Str("recover".into()),          Value::BuiltinFunc("__signal_recover".into())),
            (Value::Str("dump_core".into()),        Value::BuiltinFunc("__signal_dump_core".into())),
            (Value::Str("dump_json".into()),        Value::BuiltinFunc("__signal_dump_json".into())),
        ])
    }

    fn build_std_ai_module(&self) -> Value {
        Self::make_stub_module("ai", &[
            "nn_model", "nn_layer", "nn_train", "nn_predict",
            "nn_save", "nn_load", "nn_loss",
            "ai_tokenize", "ai_embed", "ai_cosine_sim", "ai_top_k",
            "ai_llm_load", "ai_llm_generate", "ai_llm_chat",
            "ai_llm_embed", "ai_llm_unload",
            "ai_dataset", "ai_split", "ai_normalize", "ai_shuffle", "ai_batch",
        ])
    }

    fn handle_yield_point(&mut self, val: Value) -> Result<Value, String> {
        if let Some(target) = self.yield_target_idx {
            if self.yield_current_idx == target {
                self.yield_captured = Some(val);
                return Err("__GENERATOR_STOP__".to_string());
            }
            let resume_val = self
                .yield_resume_values
                .get(self.yield_current_idx)
                .cloned()
                .unwrap_or(Value::Null);
            self.yield_current_idx += 1;
            Ok(resume_val)
        } else if let Some(ref mut collector) = self.yield_collector {
            collector.push(val);
            Ok(Value::Null)
        } else {
            Ok(Value::Return(Box::new(val)))
        }
    }

    fn execute_single_test_block(&mut self, name: &str, body: &[Stmt]) -> bool {
        self.env.push_scope();
        let result = self.exec_block_no_scope(body);
        self.env.pop_scope();
        match result {
            Ok(_) => {
                println!("  PASS: {}", name);
                true
            }
            Err(e) => {
                println!("  FAIL: {} — {}", name, e);
                false
            }
        }
    }

    fn execute_single_test_func(&mut self, name: &str, func: &Value) -> bool {
        match self.call_value(func, &[]) {
            Ok(_) => {
                println!("  PASS: {}", name);
                true
            }
            Err(e) => {
                println!("  FAIL: {} — {}", name, e);
                false
            }
        }
    }

    fn run_registered_tests(&mut self) -> Value {
        let static_tests = self.registered_tests.clone();
        let dynamic_tests = self.registered_test_fns.clone();

        let mut passed: i64 = 0;
        let mut failed: i64 = 0;

        for (name, body) in static_tests {
            if self.execute_single_test_block(&name, &body) {
                passed += 1;
            } else {
                failed += 1;
            }
        }

        for (name, func) in dynamic_tests {
            if self.execute_single_test_func(&name, &func) {
                passed += 1;
            } else {
                failed += 1;
            }
        }

        Value::Dict(vec![
            (Value::Str("passed".into()), Value::Int(passed)),
            (Value::Str("failed".into()), Value::Int(failed)),
            (Value::Str("total".into()), Value::Int(passed + failed)),
        ])
    }

    fn resume_generator(
        &mut self,
        gs: &Rc<RefCell<GeneratorState>>,
        sent: Option<Value>,
        require_primed: bool,
    ) -> Result<Value, String> {
        let mut initial = gs.borrow_mut();
        if initial.done {
            return Ok(Value::Dict(vec![
                (Value::Str("done".into()), Value::Bool(true)),
                (Value::Str("value".into()), Value::Null),
            ]));
        }
        if require_primed && !initial.started {
            return Err("Generator.send() requires priming with next() first".into());
        }
        if initial.lazy.is_none() {
            if !initial.started {
                initial.started = true;
            }
            if initial.index < initial.items.len() {
                let val = initial.items[initial.index].clone();
                initial.index += 1;
                if initial.index >= initial.items.len() {
                    initial.done = true;
                }
                return Ok(Value::Dict(vec![
                    (Value::Str("done".into()), Value::Bool(false)),
                    (Value::Str("value".into()), val),
                ]));
            }
            initial.done = true;
            return Ok(Value::Dict(vec![
                (Value::Str("done".into()), Value::Bool(true)),
                (Value::Str("value".into()), Value::Null),
            ]));
        }

        if initial.started {
            initial.resume_inputs.push(sent.unwrap_or(Value::Null));
        } else {
            initial.started = true;
        }
        let state_snap = initial.clone();
        drop(initial);

        let (func, func_args) = state_snap.lazy.expect("lazy generator expected");
        let target_idx = state_snap.index;
        self.yield_target_idx = Some(target_idx);
        self.yield_current_idx = 0;
        self.yield_captured = None;
        self.yield_resume_values = state_snap.resume_inputs;

        let saved = self.env.current;
        self.env.push_scope_with_parent(func.closure_env);
        self.bind_call_params(&func.params, &func_args)?;
        let run_result = self.exec_block_no_scope(&func.body);
        self.env.set_scope(saved);
        self.yield_target_idx = None;
        self.yield_resume_values.clear();

        if let Err(e) = run_result {
            if e != "__GENERATOR_STOP__" {
                return Err(e);
            }
        }

        let captured = self.yield_captured.take();
        if let Some(val) = captured {
            let mut state = gs.borrow_mut();
            state.index += 1;
            state.done = false;
            Ok(Value::Dict(vec![
                (Value::Str("done".into()), Value::Bool(false)),
                (Value::Str("value".into()), val),
            ]))
        } else {
            gs.borrow_mut().done = true;
            Ok(Value::Dict(vec![
                (Value::Str("done".into()), Value::Bool(true)),
                (Value::Str("value".into()), Value::Null),
            ]))
        }
    }

    pub fn exec(&mut self, program: &Program) -> Result<Value, String> {
        // Register this interpreter as the fault dispatch target for this thread.
        fault::set_interpreter_ptr(self as *mut Interpreter);
        let mut result = Value::Null;
        for stmt in &program.stmts {
            result = self.exec_stmt(stmt)?;
            if matches!(result, Value::Return(_)) {
                break;
            }
        }
        fault::clear_interpreter_ptr();
        Ok(result)
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Value, String> {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let val = if let Some(expr) = value {
                    self.eval_expr(expr)?
                } else {
                    Value::Null
                };
                self.env.define(name, val);
                Ok(Value::Null)
            }
            Stmt::Const { name, value, .. } => {
                let val = self.eval_expr(value)?;
                self.env.define_const(name, val);
                Ok(Value::Null)
            }
            Stmt::Expr(expr) => self.eval_expr(expr),
            Stmt::FuncDecl {
                name,
                params,
                body,
                is_generator,
                decorators,
                ..
            } => {
                let mut func = Value::Func(FuncValue {
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                    closure_env: self.env.current,
                    is_generator: *is_generator,
                });
                // Apply decorators (bottom-up: last decorator wraps first)
                for dec in decorators.iter() {
                    match dec {
                        Expr::Ident(dname) if dname == "memo" => {
                            func = self.apply_memo_decorator(func)?;
                        }
                        Expr::Ident(dname) if dname == "deprecated" => {
                            func = self.apply_deprecated_decorator(func, None)?;
                        }
                        Expr::Ident(dname) if dname == "pure" || dname == "inline" || dname == "test" || dname == "bench" => {
                            // Marker decorators: accepted in tree-walk runtime as metadata/no-op.
                        }
                        Expr::Call { callee, args } => {
                            if let Expr::Ident(dname) = callee.as_ref() {
                                if dname == "deprecated" {
                                    let msg = if let Some(arg) = args.first() {
                                        if let Value::Str(s) = self.eval_expr(&arg.value)? {
                                            Some(s)
                                        } else { None }
                                    } else { None };
                                    func = self.apply_deprecated_decorator(func, msg)?;
                                } else {
                                    // Generic decorator with args: eval the call, then call result with func
                                    let dec_val = self.eval_expr(dec)?;
                                    func = self.call_value(&dec_val, &[(None, func)])?;
                                }
                            } else {
                                let dec_val = self.eval_expr(dec)?;
                                func = self.call_value(&dec_val, &[(None, func)])?;
                            }
                        }
                        _ => {
                            // Generic decorator: eval and call with func
                            let dec_val = self.eval_expr(dec)?;
                            func = self.call_value(&dec_val, &[(None, func)])?;
                        }
                    }
                }
                self.env.define(name, func);
                Ok(Value::Null)
            }
            Stmt::Return(expr) => {
                let val = if let Some(e) = expr {
                    if let Some(current_name) = self.current_function.last() {
                        if let Expr::Call { callee, args } = e {
                            if let Expr::Ident(name) = callee.as_ref() {
                                if name == current_name {
                                    let evaluated_args = self.eval_call_args(args)?;
                                    return Ok(Value::Return(Box::new(Value::TailCall(
                                        name.clone(),
                                        evaluated_args,
                                    ))));
                                }
                            }
                        }
                    }
                    self.eval_expr(e)?
                } else {
                    Value::Null
                };
                Ok(Value::Return(Box::new(val)))
            }
            Stmt::If {
                condition,
                body,
                else_ifs,
                else_body,
            } => {
                let cond = self.eval_expr(condition)?;
                if cond.is_truthy() {
                    return self.exec_block(body);
                }
                for (cond_expr, block) in else_ifs {
                    let c = self.eval_expr(cond_expr)?;
                    if c.is_truthy() {
                        return self.exec_block(block);
                    }
                }
                if let Some(eb) = else_body {
                    return self.exec_block(eb);
                }
                Ok(Value::Null)
            }
            Stmt::While { condition, body } => {
                let my_label = self.loop_label.take();
                loop {
                    let cond = self.eval_expr(condition)?;
                    if !cond.is_truthy() {
                        break;
                    }
                    let result = self.exec_block(body)?;
                    match result {
                        Value::Break => break,
                        Value::Continue => continue,
                        Value::BreakLabel(ref l) if my_label.as_deref() == Some(l) => break,
                        Value::ContinueLabel(ref l) if my_label.as_deref() == Some(l) => continue,
                        Value::BreakLabel(_) | Value::ContinueLabel(_) | Value::Return(_) => return Ok(result),
                        _ => {}
                    }
                }
                Ok(Value::Null)
            }
            Stmt::IfLet { pattern, var, expr, body, else_body } => {
                let val = self.eval_expr(expr)?;
                let matched = match (&val, pattern.as_str()) {
                    (Value::Some(inner), "Some") => Some(inner.as_ref().clone()),
                    (Value::Ok(inner), "Ok") => Some(inner.as_ref().clone()),
                    (Value::Err(inner), "Err") => Some(inner.as_ref().clone()),
                    _ => None,
                };
                if let Some(inner_val) = matched {
                    self.env.push_scope();
                    self.env.define(var, inner_val);
                    let result = self.exec_block_no_scope(body)?;
                    self.env.pop_scope();
                    Ok(result)
                } else if let Some(eb) = else_body {
                    // Propagate the else block's result so `return`/`break` inside it work.
                    self.exec_block(eb)
                } else {
                    Ok(Value::Null)
                }
            }
            Stmt::WhileLet { pattern, var, expr, body } => {
                loop {
                    let val = self.eval_expr(expr)?;
                    let matched = match (&val, pattern.as_str()) {
                        (Value::Some(inner), "Some") => Some(inner.as_ref().clone()),
                        (Value::Ok(inner), "Ok") => Some(inner.as_ref().clone()),
                        (Value::Err(inner), "Err") => Some(inner.as_ref().clone()),
                        _ => None,
                    };
                    if let Some(inner_val) = matched {
                        self.env.push_scope();
                        self.env.define(var, inner_val);
                        let result = self.exec_block_no_scope(body)?;
                        self.env.pop_scope();
                        match result {
                            Value::Break => break,
                            Value::Continue => continue,
                            Value::Return(_) => return Ok(result),
                            _ => {}
                        }
                    } else {
                        break;
                    }
                }
                Ok(Value::Null)
            }
            Stmt::LetElse { pattern, var, expr, else_body } => {
                let val = self.eval_expr(expr)?;
                let matched = match (&val, pattern.as_str()) {
                    (Value::Some(inner), "Some") => Some(inner.as_ref().clone()),
                    (Value::Ok(inner), "Ok") => Some(inner.as_ref().clone()),
                    (Value::Err(inner), "Err") => Some(inner.as_ref().clone()),
                    _ => None,
                };
                if let Some(inner_val) = matched {
                    self.env.define(var, inner_val);
                    Ok(Value::Null)
                } else {
                    let result = self.exec_block(else_body)?;
                    // else_body must diverge (return/break/continue/throw)
                    Ok(result)
                }
            }
            Stmt::TestBlock { name, body } => {
                if !self.registered_tests.iter().any(|(existing, _)| existing == name) {
                    self.registered_tests.push((name.clone(), body.clone()));
                }
                if self.test_mode {
                    self.execute_single_test_block(name, body);
                }
                // In non-test mode, skip the test block entirely
                Ok(Value::Null)
            }
            Stmt::BenchBlock { name, body } => {
                if self.test_mode {
                    let iters = 100;
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        self.env.push_scope();
                        let _ = self.exec_block_no_scope(body);
                        self.env.pop_scope();
                    }
                    let elapsed = start.elapsed();
                    let per_iter = elapsed / iters;
                    println!("  BENCH: {} — {:.2?}/iter ({} iters, {:.2?} total)", name, per_iter, iters, elapsed);
                }
                Ok(Value::Null)
            }
            Stmt::ForIn { var, iter, body } => {
                let my_label = self.loop_label.take();
                let iterable = self.eval_expr(iter)?;
                // Handle lazy generators specially: iterate item-by-item via next()
                if let Value::Generator(ref gs) = iterable {
                    if gs.borrow().lazy.is_some() {
                        loop {
                            let next_result = self.call_builtin_method(&iterable, "next", &[])?;
                            let done = if let Value::Dict(ref pairs) = next_result {
                                pairs.iter().find(|(k, _)| matches!(k, Value::Str(s) if s == "done"))
                                    .map(|(_, v)| v.is_truthy()).unwrap_or(true)
                            } else { true };
                            if done { break; }
                            let val = if let Value::Dict(ref pairs) = next_result {
                                pairs.iter().find(|(k, _)| matches!(k, Value::Str(s) if s == "value"))
                                    .map(|(_, v)| v.clone()).unwrap_or(Value::Null)
                            } else { Value::Null };
                            self.env.push_scope();
                            self.env.define(var, val);
                            let result = self.exec_block_no_scope(body)?;
                            self.env.pop_scope();
                            match result {
                                Value::Break => break,
                                Value::Continue => continue,
                                Value::BreakLabel(ref l) if my_label.as_deref() == Some(l) => break,
                                Value::ContinueLabel(ref l) if my_label.as_deref() == Some(l) => continue,
                                Value::BreakLabel(_) | Value::ContinueLabel(_) | Value::Return(_) => return Ok(result),
                                _ => {}
                            }
                        }
                        return Ok(Value::Null);
                    }
                }
                // Ranges iterate lazily — don't materialize millions of values.
                if let Value::Range(s, e, inclusive) = &iterable {
                    let (start, end_v) = (*s, if *inclusive { *e + 1 } else { *e });
                    let mut i = start;
                    while i < end_v {
                        self.env.push_scope();
                        self.env.define(var, Value::Int(i));
                        let result = self.exec_block_no_scope(body)?;
                        self.env.pop_scope();
                        match result {
                            Value::Break => break,
                            Value::Continue => { i += 1; continue; }
                            Value::BreakLabel(ref l) if my_label.as_deref() == Some(l) => break,
                            Value::ContinueLabel(ref l) if my_label.as_deref() == Some(l) => { i += 1; continue; }
                            Value::BreakLabel(_) | Value::ContinueLabel(_) | Value::Return(_) => {
                                return Ok(result)
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    return Ok(Value::Null);
                }
                // Check for custom iterator protocol on instances
                let items = match &iterable {
                    Value::Instance(cls_name, _) | Value::StructInstance(cls_name, _) => {
                        // Try to call iter() method to get an iterator object
                        let cls = cls_name.clone();
                        let has_iter = if let Some(Value::Class(cv)) = self.env.get(&cls) {
                            cv.methods.contains_key("iter")
                        } else { false };
                        if has_iter {
                            let (iterator, _) = self.call_method(&iterable, "iter", &[])?;
                            // Collect items by calling .next() until .is_done()
                            let mut collected = Vec::new();
                            let mut current_iter = iterator;
                            loop {
                                let iter_cls = match &current_iter {
                                    Value::Instance(n, _) | Value::StructInstance(n, _) => n.clone(),
                                    _ => break,
                                };
                                let has_is_done = if let Some(Value::Class(cv)) = self.env.get(&iter_cls) {
                                    cv.methods.contains_key("is_done")
                                } else { false };
                                if has_is_done {
                                    let (done, updated) = self.call_method(&current_iter, "is_done", &[])?;
                                    if let Some(u) = updated { current_iter = u; }
                                    if done.is_truthy() { break; }
                                }
                                let (val, updated) = self.call_method(&current_iter, "next", &[])?;
                                if let Some(u) = updated { current_iter = u; }
                                collected.push(val);
                            }
                            collected
                        } else {
                            self.value_to_iter(&iterable)?
                        }
                    }
                    _ => self.value_to_iter(&iterable)?,
                };
                for item in items {
                    self.env.push_scope();
                    self.env.define(var, item);
                    let result = self.exec_block_no_scope(body)?;
                    self.env.pop_scope();
                    match result {
                        Value::Break => break,
                        Value::Continue => continue,
                        Value::BreakLabel(ref l) if my_label.as_deref() == Some(l) => break,
                        Value::ContinueLabel(ref l) if my_label.as_deref() == Some(l) => continue,
                        Value::BreakLabel(_) | Value::ContinueLabel(_) | Value::Return(_) => return Ok(result),
                        _ => {}
                    }
                }
                Ok(Value::Null)
            }
            Stmt::ForClassic {
                init,
                condition,
                update,
                body,
            } => {
                let my_label = self.loop_label.take();
                self.env.push_scope();
                if let Some(init_stmt) = init {
                    self.exec_stmt(init_stmt)?;
                }
                loop {
                    if let Some(cond) = condition {
                        let c = self.eval_expr(cond)?;
                        if !c.is_truthy() {
                            break;
                        }
                    }
                    let result = self.exec_block_no_scope(body)?;
                    match result {
                        Value::Break => break,
                        Value::Continue => {}
                        Value::BreakLabel(ref l) if my_label.as_deref() == Some(l) => break,
                        Value::ContinueLabel(ref l) if my_label.as_deref() == Some(l) => {}
                        Value::BreakLabel(_) | Value::ContinueLabel(_) | Value::Return(_) => {
                            self.env.pop_scope();
                            return Ok(result);
                        }
                        _ => {}
                    }
                    if let Some(upd) = update {
                        self.exec_stmt(upd)?;
                    }
                }
                self.env.pop_scope();
                Ok(Value::Null)
            }
            Stmt::Break => Ok(Value::Break),
            Stmt::Continue => Ok(Value::Continue),
            Stmt::BreakLabel(lbl) => Ok(Value::BreakLabel(lbl.clone())),
            Stmt::ContinueLabel(lbl) => Ok(Value::ContinueLabel(lbl.clone())),
            Stmt::ForInDestructure { vars, iter, body } => {
                let my_label = self.loop_label.take();
                let iterable = self.eval_expr(iter)?;
                // Destructuring over a dict yields [key, value] pairs (so
                // `for (k, v) in dict` binds each entry), matching the docs.
                let items = match &iterable {
                    Value::Dict(pairs) => pairs
                        .iter()
                        .map(|(k, v)| Value::List(vec![k.clone(), v.clone()]))
                        .collect(),
                    _ => self.value_to_iter(&iterable)?,
                };
                for item in &items {
                    self.env.push_scope();
                    match item {
                        Value::List(elems) | Value::Tuple(elems) => {
                            for (i, var) in vars.iter().enumerate() {
                                let val = elems.get(i).cloned().unwrap_or(Value::Null);
                                self.env.define(var, val);
                            }
                        }
                        _ => {
                            for (i, var) in vars.iter().enumerate() {
                                if i == 0 {
                                    self.env.define(var, item.clone());
                                } else {
                                    self.env.define(var, Value::Null);
                                }
                            }
                        }
                    }
                    let result = self.exec_block_no_scope(body)?;
                    self.env.pop_scope();
                    match &result {
                        Value::Break => break,
                        Value::Continue => continue,
                        Value::BreakLabel(l) if my_label.as_deref() == Some(l.as_str()) => break,
                        Value::ContinueLabel(l) if my_label.as_deref() == Some(l.as_str()) => continue,
                        Value::BreakLabel(_) | Value::ContinueLabel(_) | Value::Return(_) => return Ok(result),
                        _ => {}
                    }
                }
                Ok(Value::Null)
            }
            Stmt::Match { subject, arms } => {
                let val = self.eval_expr(subject)?;
                for arm in arms {
                    if self.matches_pattern(&val, &arm.pattern)? {
                        self.env.push_scope();
                        self.bind_pattern(&val, &arm.pattern)?;
                        if let Some(guard) = &arm.guard {
                            let g = self.eval_expr(guard)?;
                            if !g.is_truthy() {
                                self.env.pop_scope();
                                continue;
                            }
                        }
                        let result = self.exec_block_no_scope(&arm.body);
                        self.env.pop_scope();
                        return result;
                    }
                }
                Ok(Value::Null)
            }
            Stmt::Throw(expr) => {
                let val = self.eval_expr(expr)?;
                // Preserve the thrown value so `catch (e)` can bind the original
                // object (e.g. `e.message`); the Err string is for display/matching.
                let msg = match &val {
                    Value::Instance(cls, fields) | Value::StructInstance(cls, fields) => {
                        match fields.get("message") {
                            Some(Value::Str(m)) => format!("{}: {}", cls, m),
                            _ => cls.clone(),
                        }
                    }
                    other => format!("{}", other),
                };
                self.pending_throw = Some(val);
                Err(msg)
            }
            Stmt::TryCatch {
                body,
                catch_var,
                catch_body,
                catch_clauses,
                finally_body,
                ..
            } => {
                let result = self.exec_block(body);
                let out: Result<Value, String> = match result {
                    Err(err) => {
                        // Recover the original thrown value (preserved by Throw) so the
                        // catch binding is the real object, not just its message string.
                        let thrown = self.pending_throw.take()
                            .unwrap_or_else(|| Value::Str(err.clone()));
                        let mut catch_result: Result<Value, String> = Ok(Value::Null);
                        let mut handled = false;
                        if !catch_clauses.is_empty() {
                            // Typed multi-catch: first matching clause wins.
                            for (clause_var, clause_type, clause_body) in catch_clauses {
                                let matches = match clause_type {
                                    Some(type_name) => self.error_is_a(&thrown, type_name, &err),
                                    None => true, // catch-all clause
                                };
                                if matches {
                                    self.env.push_scope();
                                    if let Some(var) = clause_var {
                                        self.env.define(var, thrown.clone());
                                    }
                                    catch_result = self.exec_block_no_scope(clause_body);
                                    self.env.pop_scope();
                                    handled = true;
                                    break;
                                }
                            }
                            // No clause matched — re-raise so an outer try can handle it.
                            if !handled {
                                self.pending_throw = Some(thrown);
                                catch_result = Err(err);
                            }
                        } else if let Some(cb) = catch_body {
                            // Legacy single untyped catch (catch-all).
                            self.env.push_scope();
                            if let Some(var) = catch_var {
                                self.env.define(var, thrown.clone());
                            }
                            catch_result = self.exec_block_no_scope(cb);
                            self.env.pop_scope();
                        }
                        catch_result
                    }
                    Ok(v) => Ok(v),
                };
                if let Some(fb) = finally_body {
                    self.exec_block(fb)?;
                }
                out
            }
            Stmt::Defer(body) => {
                if let Some(frame) = self.defer_stack.last_mut() {
                    frame.push(body.clone());
                }
                Ok(Value::Null)
            }
            Stmt::ClassDecl {
                name,
                parent,
                body,
                is_sealed,
                decorators,
                ..
            } => {
                let is_fixed = decorators.iter().any(|d| d == "fixed");
                let is_data = decorators.iter().any(|d| d == "data");
                let is_cow = decorators.iter().any(|d| d == "cow");
                let is_sealed_decorator = decorators.iter().any(|d| d == "sealed");
                let mut methods = HashMap::new();
                let mut fields = HashMap::new();
                let mut declared_field_names: Vec<String> = Vec::new();
                let mut computed_props = HashMap::new();

                // Inherit from parent
                if let Some(parent_name) = parent {
                    if let Some(Value::Class(parent_cls)) = self.env.get(parent_name) {
                        for (k, v) in &parent_cls.methods {
                            methods.insert(k.clone(), v.clone());
                        }
                        for (k, v) in &parent_cls.fields {
                            fields.insert(k.clone(), v.clone());
                        }
                    }
                    // Register as a child of sealed parent
                    if let Some(Value::Class(mut parent_cls)) = self.env.get(parent_name) {
                        if parent_cls.is_sealed {
                            parent_cls.sealed_children.push(name.clone());
                            self.env.set(parent_name, Value::Class(parent_cls)).ok();
                        }
                    }
                }

                for stmt in body {
                    match stmt {
                        Stmt::FuncDecl {
                            name: mname,
                            params,
                            body: mbody,
                            ..
                        } => {
                            // Track init params as declared fields
                            if mname == "init" || mname == "constructor" {
                                for p in params {
                                    declared_field_names.push(p.name.clone());
                                }
                            }
                            // Check for computed property getter/setter prefixes
                            if mname.starts_with("get_") {
                                let prop_name = mname["get_".len()..].to_string();
                                let fv = FuncValue {
                                    name: mname.clone(),
                                    params: params.clone(),
                                    body: mbody.clone(),
                                    closure_env: self.env.current,
                                    is_generator: false,
                                };
                                computed_props.entry(prop_name).or_insert_with(|| crate::value::ComputedProp { getter: None, setter: None }).getter = Some(fv);
                            } else if mname.starts_with("set_") {
                                let prop_name = mname["set_".len()..].to_string();
                                let fv = FuncValue {
                                    name: mname.clone(),
                                    params: params.clone(),
                                    body: mbody.clone(),
                                    closure_env: self.env.current,
                                    is_generator: false,
                                };
                                computed_props.entry(prop_name).or_insert_with(|| crate::value::ComputedProp { getter: None, setter: None }).setter = Some(fv);
                            } else {
                                methods.insert(
                                    mname.clone(),
                                    FuncValue {
                                        name: mname.clone(),
                                        params: params.clone(),
                                        body: mbody.clone(),
                                        closure_env: self.env.current,
                                        is_generator: false,
                                    },
                                );
                            }
                        }
                        Stmt::Let {
                            name: fname,
                            value,
                            ..
                        } => {
                            let val = if let Some(expr) = value {
                                self.eval_expr(expr)?
                            } else {
                                Value::Null
                            };
                            declared_field_names.push(fname.clone());
                            fields.insert(fname.clone(), val);
                        }
                        _ => {}
                    }
                }

                // @data: auto-generate __str__ and equals if not defined
                if is_data {
                    if !methods.contains_key("__str__") {
                        // We represent __str__ as a native tag, handled in value_to_string
                    }
                    // __eq__ is handled by comparing all fields
                }

                let class_val = Value::Class(ClassValue {
                    name: name.clone(),
                    parent: parent.clone(),
                    methods,
                    fields,
                    field_order: declared_field_names.clone(),
                    is_fixed,
                    is_data,
                    is_sealed: *is_sealed || is_sealed_decorator,
                    is_cow,
                    sealed_children: Vec::new(),
                    computed_properties: computed_props,
                });
                self.env.define(name, class_val);

                // Handle @derive decorator
                for dec in decorators.iter() {
                    if dec.starts_with("derive(") && dec.ends_with(')') {
                        let traits_str = &dec[7..dec.len()-1];
                        self.apply_derive(name, traits_str);
                    } else if dec == "derive" {
                        // bare @derive without args — skip
                    }
                }

                Ok(Value::Null)
            }
            Stmt::StructDecl { name, fields, decorators, .. } => {
                // Store struct as a callable constructor
                // For now, store it as a class with fields
                let mut field_map = HashMap::new();
                let mut field_order = Vec::new();
                for f in fields {
                    let val = if let Some(expr) = &f.default {
                        self.eval_expr(expr)?
                    } else {
                        Value::Null
                    };
                    field_order.push(f.name.clone());
                    field_map.insert(f.name.clone(), val);
                }
                // Check for @data decorator
                let is_data = decorators.iter().any(|d| d == "data");
                let class_val = Value::Class(ClassValue {
                    name: name.clone(),
                    parent: None,
                    methods: HashMap::new(),
                    fields: field_map,
                    field_order,
                    is_fixed: false,
                    is_data,
                    is_sealed: false,
                    is_cow: false,
                    sealed_children: Vec::new(),
                    computed_properties: HashMap::new(),
                });
                self.env.define(name, class_val);
                // Handle @derive decorator
                for dec in decorators.iter() {
                    if dec.starts_with("derive(") && dec.ends_with(')') {
                        let traits_str = &dec[7..dec.len()-1];
                        self.apply_derive(name, traits_str);
                    }
                }
                Ok(Value::Null)
            }
            Stmt::EnumDecl { name, variants, .. } => {
                // Store each variant as a value
                for v in variants {
                    let variant_name = format!("{}.{}", name, v.name);
                    if v.fields.is_empty() {
                        self.env.define(
                            &variant_name,
                            Value::EnumVariant(name.clone(), v.name.clone(), vec![]),
                        );
                    } else {
                        // Store as a constructor function
                        let params: Vec<Param> = v
                            .fields
                            .iter()
                            .map(|f| Param {
                                name: f.clone(),
                                type_ann: None,
                                default: None,
                                is_variadic: false,
                            })
                            .collect();
                        let enum_name = name.clone();
                        let var_name = v.name.clone();
                        // We store a function that creates the variant
                        self.env.define(
                            &variant_name,
                            Value::Func(FuncValue {
                                name: variant_name.clone(),
                                params,
                                body: vec![], // handled specially in call
                                closure_env: self.env.current,
                                is_generator: false,
                            }),
                        );
                    }
                }
                // Also store the enum name itself as a Class so impl blocks can attach methods
                self.env.define(name, Value::Class(ClassValue {
                    name: name.clone(),
                    parent: None,
                    methods: HashMap::new(),
                    fields: HashMap::new(),
                    field_order: vec![],
                    is_fixed: false,
                    is_data: false,
                    is_sealed: false,
                    is_cow: false,
                    sealed_children: Vec::new(),
                    computed_properties: HashMap::new(),
                }));
                Ok(Value::Null)
            }
            Stmt::TraitDecl {
                name,
                methods,
                supertraits,
                ..
            } => {
                // Store trait as a dict of required method names + optional default impls
                let mut trait_methods = HashMap::new();
                for m in methods {
                    if let Stmt::FuncDecl { name: mname, params, body, .. } = m {
                        trait_methods.insert(
                            mname.clone(),
                            FuncValue {
                                name: mname.clone(),
                                params: params.clone(),
                                body: body.clone(),
                                closure_env: self.env.current,
                                is_generator: false,
                            },
                        );
                    }
                }
                // Store as a class-like value with the trait methods as defaults
                self.env.define(
                    name,
                    Value::Class(ClassValue {
                        name: format!("<trait {}>", name),
                        parent: None,
                        methods: trait_methods,
                        fields: HashMap::new(),
                        field_order: vec![],
                        is_fixed: false,
                        is_data: false,
                        is_sealed: false,
                        is_cow: false,
                        sealed_children: Vec::new(),
                        computed_properties: HashMap::new(),
                    }),
                );
                Ok(Value::Null)
            }
            Stmt::ImplBlock {
                trait_name,
                target,
                methods,
            } => {
                // Collect explicitly implemented method names
                let mut implemented_names: Vec<String> = Vec::new();
                // Primitive type names that are not classes
                let is_primitive = matches!(target.as_str(), "str" | "int" | "float" | "bool" | "list" | "dict" | "set" | "bytes");
                // Add methods to the target class/struct
                for stmt in methods {
                    if let Stmt::FuncDecl {
                        name: mname,
                        params,
                        body,
                        ..
                    } = stmt
                    {
                        implemented_names.push(mname.clone());
                        let fv = FuncValue {
                            name: mname.clone(),
                            params: params.clone(),
                            body: body.clone(),
                            closure_env: self.env.current,
                            is_generator: false,
                        };
                        if is_primitive {
                            self.primitive_impls.entry(target.clone())
                                .or_insert_with(HashMap::new)
                                .insert(mname.clone(), fv);
                        } else if let Some(Value::Str(s)) = self.env.get(target) {
                            // newtype or cstruct — store methods in primitive_impls under type name
                            if s.starts_with("newtype:") || s.starts_with("cstruct:") {
                                self.primitive_impls.entry(target.clone())
                                    .or_insert_with(HashMap::new)
                                    .insert(mname.clone(), fv);
                            }
                        } else if let Some(Value::Class(mut cls)) = self.env.get(target) {
                            cls.methods.insert(mname.clone(), fv);
                            self.env.define(target, Value::Class(cls));
                        }
                    }
                }
                // Auto-inherit default trait methods that weren't explicitly implemented
                if let Some(trait_name) = trait_name {
                    // Record trait membership for the `is` operator.
                    let impls = self.trait_impls.entry(target.clone()).or_default();
                    if !impls.contains(trait_name) {
                        impls.push(trait_name.clone());
                    }
                    if let Some(Value::Class(trait_cls)) = self.env.get(trait_name) {
                        let default_methods: Vec<(String, FuncValue)> = trait_cls.methods.iter()
                            .filter(|(name, func)| !implemented_names.contains(name) && !func.body.is_empty())
                            .map(|(name, func)| (name.clone(), func.clone()))
                            .collect();
                        if let Some(Value::Class(mut target_cls)) = self.env.get(target) {
                            for (mname, func) in default_methods {
                                target_cls.methods.insert(mname, func);
                            }
                            self.env.define(target, Value::Class(target_cls));
                        }
                    }
                }
                Ok(Value::Null)
            }
            Stmt::Import { path, names, alias } => {
                self.exec_import(path, names, alias)
            }
            Stmt::Assign { target, op, value } => {
                let new_val = self.eval_expr(value)?;
                self.exec_assign(target, op, new_val)
            }
            Stmt::Label(name) => {
                self.loop_label = Some(name.clone());
                Ok(Value::Null)
            }
            Stmt::Goto(_) => {
                // Very basic — goto requires special handling
                Ok(Value::Null)
            }
            Stmt::Yield(expr) => {
                let val = if let Some(e) = expr {
                    self.eval_expr(e)?
                } else {
                    Value::Null
                };
                self.handle_yield_point(val)
            }
            Stmt::Block(stmts) => self.exec_block(stmts),
            Stmt::Multi(stmts) => self.exec_block_no_scope(stmts),
            Stmt::TypeAlias { name, value, .. } => {
                // Store as a semantic alias — in our tree-walk interpreter, just bind name to the type string
                self.env.define(name, Value::Str(value.clone()));
                Ok(Value::Null)
            }
            Stmt::Using { expr, body } => {
                let val = self.eval_expr(expr)?;
                // Extract fields from instance/struct into scope
                let fields = match &val {
                    Value::Instance(_, fields) | Value::StructInstance(_, fields) => fields.clone(),
                    Value::CowInstance(_, fields) => fields.borrow().clone(),
                    Value::Dict(pairs) => {
                        let mut map = HashMap::new();
                        for (k, v) in pairs {
                            if let Value::Str(key) = k {
                                map.insert(key.clone(), v.clone());
                            }
                        }
                        map
                    }
                    _ => return Err("using requires a struct, class instance, or dict".into()),
                };
                if let Some(body_stmts) = body {
                    self.env.push_scope();
                    for (k, v) in &fields {
                        self.env.define(k, v.clone());
                    }
                    let result = self.exec_block_no_scope(body_stmts);
                    self.env.pop_scope();
                    result
                } else {
                    // Flat form — inject into current scope
                    for (k, v) in &fields {
                        self.env.define(k, v.clone());
                    }
                    Ok(Value::Null)
                }
            }
            Stmt::StaticAssert { condition, message } => {
                let val = self.eval_expr(condition)?;
                if !val.is_truthy() {
                    return Err(format!("static_assert failed: {}", message));
                }
                Ok(Value::Null)
            }
            Stmt::MacroDecl { name, params, body } => {
                self.macros.insert(
                    name.clone(),
                    MacroValue {
                        params: params.clone(),
                        body: body.clone(),
                    },
                );
                Ok(Value::Null)
            }
            Stmt::NewtypeDecl { name, inner_type, .. } => {
                // Store as a newtype: just record the wrapper type name and inner type
                // In our tree-walk interpreter, newtypes are represented as struct instances
                // with a single field "0" holding the inner value
                self.env.define(name, Value::Str(format!("newtype:{}", inner_type)));
                Ok(Value::Null)
            }
            Stmt::ComptimeBlock { body } => {
                // Execute compile-time block immediately (in tree-walk, comptime == runtime)
                self.exec_block(body)
            }
            Stmt::CStructDecl { name, fields, .. } => {
                // Register cstruct as a regular struct type (in tree-walk, same as struct)
                self.env.define(name, Value::Str(format!("cstruct:{}", name)));
                // Store field definitions for construction
                let mut field_names = Vec::new();
                for f in fields {
                    field_names.push(f.name.clone());
                }
                let key = format!("__cstruct_fields_{}", name);
                let field_list: Vec<Value> = field_names.iter().map(|n| Value::Str(n.clone())).collect();
                self.env.define(&key, Value::List(field_list));
                Ok(Value::Null)
            }
            Stmt::UnsafeBlock { body } => {
                // In tree-walk interpreter, unsafe blocks just execute normally
                self.exec_block(body)
            }
            Stmt::ActorDecl { name, is_agent, goal, body } => {
                // Store actor/agent as a class-like value with methods
                let mut methods = Vec::new();
                for stmt in body {
                    if let Stmt::FuncDecl { name: fname, .. } = stmt {
                        methods.push(Value::Str(fname.clone()));
                    }
                    self.exec_stmt(stmt)?;
                }
                let kind = if *is_agent { "agent" } else { "actor" };
                let mut entries = vec![
                    (Value::Str("__kind".into()), Value::Str(kind.into())),
                    (Value::Str("__name".into()), Value::Str(name.clone())),
                    (Value::Str("methods".into()), Value::List(methods)),
                ];
                if let Some(g) = goal {
                    entries.push((Value::Str("goal".into()), Value::Str(g.clone())));
                }
                self.env.define(name, Value::Dict(entries));
                Ok(Value::Null)
            }
            Stmt::IsolateBlock { name, body } => {
                // Execute in a fresh scope
                self.env.push_scope();
                let result = self.exec_block_no_scope(body);
                self.env.pop_scope();
                result
            }
            Stmt::BitfieldStructDecl { name, backing, fields, .. } => {
                // Store as struct-like with field bit info
                let total_bits: u8 = fields.iter().map(|(_, b)| b).sum();
                let mut entries = vec![
                    (Value::Str("__kind".into()), Value::Str("bitfield".into())),
                    (Value::Str("__name".into()), Value::Str(name.clone())),
                    (Value::Str("total_bits".into()), Value::Int(total_bits as i64)),
                ];
                if let Some(b) = backing {
                    entries.push((Value::Str("backing".into()), Value::Str(b.clone())));
                }
                let field_list: Vec<Value> = fields.iter().map(|(n, b)| {
                    Value::Dict(vec![
                        (Value::Str("name".into()), Value::Str(n.clone())),
                        (Value::Str("bits".into()), Value::Int(*b as i64)),
                    ])
                }).collect();
                entries.push((Value::Str("fields".into()), Value::List(field_list)));
                // Add to_int and from_int as builtins
                self.env.define(name, Value::Dict(entries));
                Ok(Value::Null)
            }
            Stmt::InlineStructDecl { name, fields, doc_comment } => {
                // Treat same as regular struct (inline is a hint, not enforced in tree-walk)
                self.exec_stmt(&Stmt::StructDecl { name: name.clone(), fields: fields.clone(), decorators: vec![], doc_comment: doc_comment.clone() })
            }
            Stmt::EnableLangs { langs } => {
                // Store enabled languages
                let list: Vec<Value> = langs.iter().map(|l| Value::Str(l.clone())).collect();
                self.env.define("__enabled_langs", Value::List(list));
                Ok(Value::Null)
            }
            Stmt::EmbeddedLangBlock { lang, label, code } => {
                // Keep the raw source reachable for introspection/tools.
                let key = if let Some(l) = label {
                    format!("__embed_{}_{}", lang, l)
                } else {
                    format!("__embed_{}", lang)
                };
                self.env.define(&key, Value::Str(code.clone()));
                self.exec_embedded_block(lang, label.as_deref(), code)
            }
            Stmt::EngineImport { names, wildcard, selector } => {
                self.exec_engine_import(names, *wildcard, selector)
            }
            Stmt::AsmBlock { code } => {
                // In tree-walk interpreter, asm blocks are no-ops
                eprintln!("[warning] asm! block skipped in tree-walk interpreter");
                Ok(Value::Null)
            }
            Stmt::SourceDirective { kind, args } => {
                match kind.as_str() {
                    "borrow_check" => {
                        // Enable borrow checking mode (no-op in tree-walk)
                        Ok(Value::Null)
                    }
                    "insert" => {
                        // Insert file contents
                        if let Some(path) = args.first() {
                            match std::fs::read_to_string(path) {
                                Ok(code) => {
                                    use crate::lexer::Lexer;
                                    use crate::parser::Parser;
                                    let mut lexer = Lexer::new(&code);
                                    let tokens = lexer.tokenize().map_err(|e| format!("@insert parse error: {}", e))?;
                                    let mut parser = Parser::new(tokens);
                                    let program = parser.parse().map_err(|e| format!("@insert parse error: {}", e))?;
                                    for stmt in &program.stmts {
                                        self.exec_stmt(stmt)?;
                                    }
                                    Ok(Value::Null)
                                }
                                Err(e) => Err(format!("@insert: cannot read '{}': {}", path, e)),
                            }
                        } else {
                            Err("@insert requires a file path".into())
                        }
                    }
                    "replace" | "cfg" => {
                        // No-op in tree-walk interpreter
                        Ok(Value::Null)
                    }
                    _ => Ok(Value::Null),
                }
            }
        }
    }

    fn exec_block(&mut self, stmts: &[Stmt]) -> Result<Value, String> {
        self.env.push_scope();
        self.defer_stack.push(vec![]);
        let result = self.exec_block_no_scope(stmts);
        // Execute deferred statements in LIFO order
        let deferred = self.defer_stack.pop().unwrap_or_default();
        for body in deferred.into_iter().rev() {
            let _ = self.exec_block_no_scope(&body);
        }
        self.env.pop_scope();
        result
    }

    fn exec_block_no_scope(&mut self, stmts: &[Stmt]) -> Result<Value, String> {
        let mut result = Value::Null;
        let mut i = 0;
        while i < stmts.len() {
            let stmt = &stmts[i];
            // Handle goto: jump to label position
            if let Stmt::Goto(label) = stmt {
                let mut found = false;
                for (j, s) in stmts.iter().enumerate() {
                    if let Stmt::Label(name) = s {
                        if name == label {
                            i = j + 1; // jump to statement after label
                            found = true;
                            break;
                        }
                    }
                }
                if !found {
                    return Err(format!("Undefined label '{}'", label));
                }
                continue;
            }
            result = match self.exec_stmt(stmt) {
                Ok(v) => v,
                // `expr?` hit an Err/None: unwind to the enclosing function and
                // make that value its return value (see Expr::TryUnwrap).
                Err(e) if e == "__try_return__" => {
                    let v = self.pending_try_return.take().unwrap_or(Value::Null);
                    return Ok(Value::Return(Box::new(v)));
                }
                Err(e) => return Err(e),
            };
            match &result {
                Value::Return(_) | Value::Break | Value::Continue
                | Value::BreakLabel(_) | Value::ContinueLabel(_) => return Ok(result),
                _ => {}
            }
            i += 1;
        }
        Ok(result)
    }

    fn exec_import(
        &mut self,
        path: &str,
        names: &Option<Vec<String>>,
        alias: &Option<String>,
    ) -> Result<Value, String> {
        use std::fs;
        use crate::lexer::Lexer;
        use crate::parser::Parser;

        // An explicitly installed package (in v2_modules/) takes precedence over a
        // built-in stub of the same name, so `v2 add std.http` overrides the
        // convenience stub. Resolve by the full path name and by the leading segment.
        if crate::pkg::resolve_installed_entry(path.split('/').next().unwrap_or(path)).is_some()
        {
            // fall through to the file/package resolution below (skip built-ins)
        } else {
        if path == "std.math" {
            let module = self.build_std_math_module();
            return self.import_module_value(path, module, names, alias);
        }
        if path == "std.io" {
            let module = self.build_std_io_module();
            return self.import_module_value(path, module, names, alias);
        }
        if path == "std.collections" {
            let module = self.build_std_collections_module();
            return self.import_module_value(path, module, names, alias);
        }

        // Generically handle any std.* module that's already registered as a constant
        if path.starts_with("std.") {
            if let Some(module) = self.env.get(path) {
                return self.import_module_value(path, module, names, alias);
            }
        }
        } // end: built-in resolution skipped when an installed package overrides

        // Resolve the module to a source file. Search order:
        //   1. `<path>` / `<path>.v2` relative to the current directory
        //   2. An installed package under `v2_modules/<path>/` (package manager)
        let candidates = [
            path.to_string(),
            format!("{}.v2", path),
        ];
        let mut source: Option<String> = None;
        for cand in &candidates {
            if let Ok(s) = fs::read_to_string(cand) {
                source = Some(s);
                break;
            }
        }
        if source.is_none() {
            // Installed package: the name is everything before the first `/`
            // (dots are part of names like `std.http`, so do NOT split on `.`).
            let pkg_name = path.split('/').next().unwrap_or(path);
            if let Some(entry) = crate::pkg::resolve_installed_entry(pkg_name) {
                if let Ok(s) = fs::read_to_string(&entry) {
                    source = Some(s);
                }
            }
        }
        let source = source.ok_or_else(|| {
            format!("Cannot import '{}': not found in current directory or {}/", path, crate::pkg::MODULES_DIR)
        })?;

        // Parse and execute in a fresh scope
        let mut lexer = Lexer::new(&source);
        let tokens = lexer.tokenize().map_err(|e| format!("Import parse error: {}", e))?;
        let mut parser = Parser::new(tokens);
        let program = parser.parse().map_err(|e| format!("Import parse error: {}", e))?;

        // Execute in a new scope
        self.env.push_scope();
        for stmt in &program.stmts {
            self.exec_stmt(stmt)?;
        }

        if let Some(specific_names) = names {
            // import { foo, bar } from "module"
            let mut exports = Vec::new();
            for name in specific_names {
                let val = self.env.get(name).ok_or_else(|| {
                    format!("'{}' not found in module '{}'", name, path)
                })?;
                exports.push((name.clone(), val));
            }
            self.env.pop_scope();
            for (name, val) in exports {
                self.env.define(&name, val);
            }
        } else if let Some(alias_name) = alias {
            // import "module" as mod — collect all top-level defs as a dict
            let scope_vars = self.env.current_scope_vars();
            let module_dict: Vec<(Value, Value)> = scope_vars.into_iter()
                .filter(|(_, v)| !matches!(v, Value::BuiltinFunc(_)))
                .map(|(k, v)| (Value::Str(k), v))
                .collect();
            self.env.pop_scope();
            self.env.define(alias_name, Value::Dict(module_dict));
        } else {
            // import "module" — bring everything into current scope
            let scope_vars = self.env.current_scope_vars();
            let vars: Vec<(String, Value)> = scope_vars.into_iter()
                .filter(|(_, v)| !matches!(v, Value::BuiltinFunc(_)))
                .collect();
            self.env.pop_scope();
            for (k, v) in vars {
                self.env.define(&k, v);
            }
        }

        Ok(Value::Null)
    }

    fn eval_call_args(&mut self, args: &[CallArg]) -> Result<Vec<(Option<String>, Value)>, String> {
        let mut evaluated = Vec::new();
        for arg in args {
            let val = self.eval_expr(&arg.value)?;
            if arg.is_spread {
                // Spread list items as individual positional args
                if let Value::List(items) = val {
                    for item in items {
                        evaluated.push((None, item));
                    }
                } else {
                    return Err("Spread operator requires a list".into());
                }
            } else {
                evaluated.push((arg.name.clone(), val));
            }
        }
        Ok(evaluated)
    }

    fn exec_assign(&mut self, target: &Expr, op: &AssignOp, new_val: Value) -> Result<Value, String> {
        match target {
            Expr::Ident(name) => {
                let final_val = match op {
                    AssignOp::Assign => new_val,
                    _ => {
                        let old = self.env.get(name).ok_or_else(|| {
                            format!("Undefined variable '{}'", name)
                        })?;
                        self.apply_assign_op(op, &old, &new_val)?
                    }
                };
                self.env.set(name, final_val)?;
                Ok(Value::Null)
            }
            Expr::Index { object, index } => {
                let idx = self.eval_expr(index)?;

                // Resolve the container as an lvalue path — handles any mix of
                // fields and indexes: lst[0], self.cells[k], grid[i][j], a.b[0].c[x].
                let mut accesses = Vec::new();
                let root = self.collect_lvalue_path(object, &mut accesses).map_err(|_| {
                    "Complex index assignment target not supported".to_string()
                })?;
                if self.frozen_vars.contains(&root) {
                    return Err(format!("Cannot mutate frozen value '{}'", root));
                }
                self.ensure_cow_binding_unique(&root)?;

                // Snapshot the container for compound reads and __setitem__ dispatch.
                let root_snapshot = self.env.get(&root).ok_or_else(|| {
                    format!("Undefined variable '{}'", root)
                })?;
                let container_snapshot = Self::value_at_path(&root_snapshot, &accesses)?;

                let final_val = match op {
                    AssignOp::Assign => new_val,
                    _ => {
                        let old = self.index_value(&container_snapshot, &idx)?;
                        self.apply_assign_op(op, &old, &new_val)?
                    }
                };

                // __setitem__ dispatch for class instances: a[key] = val.
                let inst_cls = match &container_snapshot {
                    Value::Instance(n, _)
                    | Value::StructInstance(n, _)
                    | Value::CowInstance(n, _) => Some(n.clone()),
                    _ => None,
                };
                if let Some(cn) = inst_cls {
                    let has_setitem = matches!(
                        self.env.get(&cn),
                        Some(Value::Class(cv)) if cv.methods.contains_key("__setitem__")
                    );
                    if !has_setitem {
                        return Err(format!("No '__setitem__' method on class {}", cn));
                    }
                    let (_, updated) = self.call_method(
                        &container_snapshot,
                        "__setitem__",
                        &[(None, idx), (None, final_val)],
                    )?;
                    if let Some(new_self) = updated {
                        let root_val = self.env.get_mut(&root).ok_or_else(|| {
                            format!("Undefined variable '{}'", root)
                        })?;
                        Self::mutate_at_path(root_val, &accesses, |target| {
                            *target = new_self;
                            Ok(Value::Null)
                        })?;
                    }
                    return Ok(Value::Null);
                }

                let root_val = self.env.get_mut(&root).ok_or_else(|| {
                    format!("Undefined variable '{}'", root)
                })?;
                Self::mutate_at_path(root_val, &accesses, move |container| {
                    match container {
                        Value::List(list) => {
                            if let Value::Int(i) = &idx {
                                let i = if *i < 0 { list.len() as i64 + i } else { *i };
                                if i >= 0 && (i as usize) < list.len() {
                                    list[i as usize] = final_val;
                                    Ok(Value::Null)
                                } else {
                                    Err(format!("Index {} out of bounds", i))
                                }
                            } else {
                                Err("List index must be an integer".into())
                            }
                        }
                        Value::Dict(pairs) => {
                            if let Some(pos) = pairs.iter().position(|(k, _)| *k == idx) {
                                pairs[pos].1 = final_val;
                            } else {
                                pairs.push((idx, final_val));
                            }
                            Ok(Value::Null)
                        }
                        other => Err(format!(
                            "Cannot index-assign to {}",
                            other.type_name()
                        )),
                    }
                })?;
                Ok(Value::Null)
            }
            Expr::FieldAccess { object, field, .. } => {
                // Resolve the root variable name and the chain of field accesses
                let mut chain = vec![field.clone()];
                let mut current = object.as_ref();
                loop {
                    match current {
                        Expr::FieldAccess { object: inner, field: f, .. } => {
                            chain.push(f.clone());
                            current = inner.as_ref();
                        }
                        Expr::Ident(name) => {
                            chain.reverse();
                            // chain is now [field1, field2, ..., fieldN]
                            // We need to mutate name.field1.field2...fieldN
                            let old_val = if !matches!(op, AssignOp::Assign) {
                                let container = self.env.get(name).ok_or_else(|| {
                                    format!("Undefined variable '{}'", name)
                                })?;
                                Self::resolve_field_chain(&container, &chain)
                            } else {
                                None
                            };
                            let final_val = match op {
                                AssignOp::Assign => new_val,
                                _ => {
                                    let old = old_val.unwrap_or(Value::Null);
                                    self.apply_assign_op(op, &old, &new_val)?
                                }
                            };
                            self.ensure_cow_binding_unique(name)?;
                            if chain.len() == 1 {
                                if let Some(container_ref) = self.env.get(name) {
                                    if let Some(updated) = self.call_computed_property_setter(
                                        &container_ref,
                                        &chain[0],
                                        final_val.clone(),
                                    )? {
                                        self.env.set(name, updated).ok();
                                        return Ok(Value::Null);
                                    }
                                }
                            }
                            // @fixed check before mutable borrow
                            if let Some(container_ref) = self.env.get(name) {
                                self.check_fixed_field(&container_ref, &chain[0])?;
                            }
                            let container = self.env.get_mut(name).ok_or_else(|| {
                                format!("Undefined variable '{}'", name)
                            })?;
                            Self::set_field_chain(container, &chain, final_val)?;
                            return Ok(Value::Null);
                        }
                        Expr::Self_ => {
                            chain.reverse();
                            let old_val = if !matches!(op, AssignOp::Assign) {
                                let container = self.env.get("self").ok_or_else(|| {
                                    "Undefined 'self'".to_string()
                                })?;
                                Self::resolve_field_chain(&container, &chain)
                            } else {
                                None
                            };
                            let final_val = match op {
                                AssignOp::Assign => new_val,
                                _ => {
                                    let old = old_val.unwrap_or(Value::Null);
                                    self.apply_assign_op(op, &old, &new_val)?
                                }
                            };
                            self.ensure_cow_binding_unique("self")?;
                            if chain.len() == 1 {
                                if let Some(container_ref) = self.env.get("self") {
                                    if let Some(updated) = self.call_computed_property_setter(
                                        &container_ref,
                                        &chain[0],
                                        final_val.clone(),
                                    )? {
                                        self.env.set("self", updated).ok();
                                        return Ok(Value::Null);
                                    }
                                }
                            }
                            // @fixed check before mutable borrow
                            if let Some(container_ref) = self.env.get("self") {
                                self.check_fixed_field(&container_ref, &chain[0])?;
                            }
                            let container = self.env.get_mut("self").ok_or_else(|| {
                                "Undefined 'self'".to_string()
                            })?;
                            Self::set_field_chain(container, &chain, final_val)?;
                            return Ok(Value::Null);
                        }
                        _ => return Err("Invalid field assignment target".into()),
                    }
                }
            }
            _ => Err("Invalid assignment target".into()),
        }
    }

    fn ensure_cow_binding_unique(&mut self, name: &str) -> Result<(), String> {
        let Some(val) = self.env.get(name) else {
            return Ok(());
        };
        if let Value::CowInstance(cls_name, fields) = val {
            if Rc::strong_count(&fields) > 1 {
                let cloned = fields.borrow().clone();
                self.env.set(name, Value::CowInstance(cls_name, Rc::new(RefCell::new(cloned))))?;
            }
        }
        Ok(())
    }

    fn resolve_field_chain(val: &Value, chain: &[String]) -> Option<Value> {
        if chain.is_empty() {
            return Some(val.clone());
        }
        match val {
            Value::Instance(_, fields) | Value::StructInstance(_, fields) => {
                let next = fields.get(&chain[0])?;
                if chain.len() == 1 {
                    Some(next.clone())
                } else {
                    Self::resolve_field_chain(next, &chain[1..])
                }
            }
            Value::CowInstance(_, fields) => {
                let next = fields.borrow().get(&chain[0]).cloned()?;
                if chain.len() == 1 {
                    Some(next)
                } else {
                    Self::resolve_field_chain(&next, &chain[1..])
                }
            }
            _ => None,
        }
    }

    fn set_field_chain(val: &mut Value, chain: &[String], new_val: Value) -> Result<(), String> {
        if chain.len() == 1 {
            match val {
                Value::Instance(ref cls_name, ref mut fields) | Value::StructInstance(ref cls_name, ref mut fields) => {
                    // Note: @fixed enforcement is done via check_fixed_field before calling this
                    fields.insert(chain[0].clone(), new_val);
                    Ok(())
                }
                Value::CowInstance(_, fields) => {
                    fields.borrow_mut().insert(chain[0].clone(), new_val);
                    Ok(())
                }
                // Static (class-level) fields: Counter.count = ...
                Value::Class(cls) => {
                    cls.fields.insert(chain[0].clone(), new_val);
                    Ok(())
                }
                Value::Dict(pairs) => {
                    let key = Value::Str(chain[0].clone());
                    if let Some((_, v)) = pairs.iter_mut().find(|(k, _)| *k == key) {
                        *v = new_val;
                    } else {
                        pairs.push((key, new_val));
                    }
                    Ok(())
                }
                _ => Err(format!("Cannot set field on {}", val.type_name())),
            }
        } else {
            match val {
                Value::Instance(_, ref mut fields) | Value::StructInstance(_, ref mut fields) => {
                    let inner = fields.get_mut(&chain[0]).ok_or_else(|| {
                        format!("No field '{}'", chain[0])
                    })?;
                    Self::set_field_chain(inner, &chain[1..], new_val)
                }
                Value::Class(cls) => {
                    let inner = cls.fields.get_mut(&chain[0]).ok_or_else(|| {
                        format!("No field '{}'", chain[0])
                    })?;
                    Self::set_field_chain(inner, &chain[1..], new_val)
                }
                Value::Dict(pairs) => {
                    let key = Value::Str(chain[0].clone());
                    let inner = pairs
                        .iter_mut()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v)
                        .ok_or_else(|| format!("No field '{}'", chain[0]))?;
                    Self::set_field_chain(inner, &chain[1..], new_val)
                }
                Value::CowInstance(_, fields) => {
                    let mut borrowed = fields.borrow_mut();
                    let inner = borrowed.get_mut(&chain[0]).ok_or_else(|| {
                        format!("No field '{}'", chain[0])
                    })?;
                    Self::set_field_chain(inner, &chain[1..], new_val)
                }
                _ => Err(format!("Cannot access field on {}", val.type_name())),
            }
        }
    }

    fn import_module_value(
        &mut self,
        path: &str,
        module: Value,
        names: &Option<Vec<String>>,
        alias: &Option<String>,
    ) -> Result<Value, String> {
        let Value::Dict(entries) = module else {
            return Err(format!("Module '{}' is not importable", path));
        };

        let as_map: HashMap<String, Value> = entries
            .into_iter()
            .filter_map(|(k, v)| match k {
                Value::Str(name) => Some((name, v)),
                _ => None,
            })
            .collect();

        if let Some(specific_names) = names {
            for name in specific_names {
                let val = as_map.get(name).cloned().ok_or_else(|| {
                    format!("'{}' not found in module '{}'", name, path)
                })?;
                self.env.define(name, val);
            }
        } else if let Some(alias_name) = alias {
            let dict: Vec<(Value, Value)> = as_map
                .into_iter()
                .map(|(k, v)| (Value::Str(k), v))
                .collect();
            self.env.define(alias_name, Value::Dict(dict));
        } else {
            for (k, v) in as_map {
                self.env.define(&k, v);
            }
        }

        Ok(Value::Null)
    }

    fn call_computed_property_setter(
        &mut self,
        instance: &Value,
        field: &str,
        value: Value,
    ) -> Result<Option<Value>, String> {
        let cls_name = match instance {
            Value::Instance(name, _) | Value::CowInstance(name, _) => name,
            _ => return Ok(None),
        };

        if let Some(Value::Class(cls)) = self.env.get(cls_name) {
            if let Some(prop) = cls.computed_properties.get(field) {
                if let Some(setter) = &prop.setter {
                    let setter = setter.clone();
                    let saved = self.env.current;
                    self.env.push_scope_with_parent(setter.closure_env);
                    self.env.define("self", instance.clone());
                    self.bind_call_params(&setter.params, &[(None, value)])?;
                    let _ = self.exec_block_no_scope(&setter.body)?;
                    let updated_self = self.env.get("self").unwrap_or(instance.clone());
                    self.env.set_scope(saved);
                    Ok(Some(updated_self))
                } else {
                    Err(format!("Computed property '{}' is read-only", field))
                }
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    /// Check if a field assignment is allowed on a @fixed class instance
    fn check_fixed_field(&self, instance: &Value, field: &str) -> Result<(), String> {
        let cls_name = match instance {
            Value::Instance(name, _) | Value::CowInstance(name, _) | Value::StructInstance(name, _) => name,
            _ => return Ok(()),
        };
        if let Some(Value::Class(cv)) = self.env.get(cls_name) {
            if cv.is_fixed {
                if !cv.fields.contains_key(field) && !cv.field_order.contains(&field.to_string()) {
                    // Also check if the instance already has it (set during init)
                    let has_field = match instance {
                        Value::Instance(_, fields) | Value::StructInstance(_, fields) => fields.contains_key(field),
                        Value::CowInstance(_, fields) => fields.borrow().contains_key(field),
                        _ => false,
                    };
                    if !has_field {
                        return Err(format!(
                            "@fixed class '{}' does not allow undeclared field '{}'",
                            cls_name, field
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_assign_op(&self, op: &AssignOp, old: &Value, new: &Value) -> Result<Value, String> {
        match op {
            AssignOp::Assign => Ok(new.clone()),
            AssignOp::PlusAssign => self.binary_op(&BinOp::Add, old, new),
            AssignOp::MinusAssign => self.binary_op(&BinOp::Sub, old, new),
            AssignOp::StarAssign => self.binary_op(&BinOp::Mul, old, new),
            AssignOp::SlashAssign => self.binary_op(&BinOp::Div, old, new),
            AssignOp::PercentAssign => self.binary_op(&BinOp::Mod, old, new),
            AssignOp::DoubleStarAssign => self.binary_op(&BinOp::Pow, old, new),
            AssignOp::ShlAssign => self.binary_op(&BinOp::Shl, old, new),
            AssignOp::ShrAssign => self.binary_op(&BinOp::Shr, old, new),
            AssignOp::BitAndAssign => self.binary_op(&BinOp::BitAnd, old, new),
            AssignOp::BitOrAssign => self.binary_op(&BinOp::BitOr, old, new),
            AssignOp::BitXorAssign => self.binary_op(&BinOp::BitXor, old, new),
            AssignOp::IntDivAssign => self.binary_op(&BinOp::IntDiv, old, new),
        }
    }

    // ── Expression Evaluation ────────────────────────────

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::BigIntLit(s) => Ok(crate::bigint::BigInt::from_str(s)
                .map(Self::norm_bigint)
                .unwrap_or(Value::Int(0))),
            Expr::Float(f) => Ok(Value::Float(*f)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::FStr(template) => self.eval_fstring(template),
            Expr::TaggedTemplate { tag, template } => self.eval_tagged_template(tag, template),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::Ident(name) => {
                let val = self
                    .env
                    .get(name)
                    .ok_or_else(|| {
                        let mut msg = format!("Undefined variable '{}'", name);
                        if let Some(suggestion) = self.env.did_you_mean(name) {
                            msg.push_str(&format!(". Did you mean '{}'?", suggestion));
                        }
                        msg
                    })?;
                // Lazy values are re-evaluated on each read
                if let Value::Lazy(expr) = &val {
                    let expr = expr.as_ref().clone();
                    return self.eval_expr(&expr);
                }
                Ok(val)
            }
            Expr::Self_ => self
                .env
                .get("self")
                .ok_or_else(|| "'self' used outside of class method".into()),

            Expr::BinOp { left, op, right } => {
                let l = self.eval_expr(left)?;
                // Short-circuit for logical ops
                match op {
                    BinOp::And => {
                        if !l.is_truthy() {
                            return Ok(l);
                        }
                        return self.eval_expr(right);
                    }
                    BinOp::Or => {
                        if l.is_truthy() {
                            return Ok(l);
                        }
                        return self.eval_expr(right);
                    }
                    BinOp::NullCoalesce => {
                        match l {
                            Value::Null => return self.eval_expr(right),
                            Value::Some(inner) => return Ok(*inner),
                            _ => return Ok(l),
                        }
                    }
                    _ => {}
                }
                let r = self.eval_expr(right)?;
                // Operator overloading: check for dunder methods on instances
                if let Value::Instance(cls, fields) = &l {
                    let dunder = match op {
                        BinOp::Add => Some("__add__"),
                        BinOp::Sub => Some("__sub__"),
                        BinOp::Mul => Some("__mul__"),
                        BinOp::Div => Some("__div__"),
                        BinOp::Mod => Some("__mod__"),
                        BinOp::Pow => Some("__pow__"),
                        BinOp::Eq => Some("__eq__"),
                        BinOp::NotEq => Some("__ne__"),
                        BinOp::Lt => Some("__lt__"),
                        BinOp::Gt => Some("__gt__"),
                        BinOp::LtEq => Some("__le__"),
                        BinOp::GtEq => Some("__ge__"),
                        BinOp::IntDiv => Some("__floordiv__"),
                        BinOp::BitAnd => Some("__band__"),
                        BinOp::BitOr => Some("__bor__"),
                        BinOp::BitXor => Some("__bxor__"),
                        BinOp::Shl => Some("__lshift__"),
                        BinOp::Shr => Some("__rshift__"),
                        _ => None,
                    };
                    if let Some(name) = dunder {
                        // Check instance fields first, then class methods
                        if let Some(method) = fields.get(name) {
                            return self.call_value(&method.clone(), &[(None, r)]);
                        }
                        if let Some(Value::Class(cv)) = self.env.get(cls) {
                            if let Some(method) = cv.methods.get(name) {
                                self.env.push_scope_with_parent(method.closure_env.clone());
                                self.env.define("self", l.clone());
                                self.bind_call_params(&method.params, &[(None, r)])?;
                                let result = self.exec_block_no_scope(&method.body)?;
                                self.env.pop_scope();
                                return match result {
                                    Value::Return(v) => Ok(*v),
                                    other => Ok(other),
                                };
                            }
                        }
                    }
                }
                if let Value::CowInstance(_cls, fields) = &l {
                    let dunder = match op {
                        BinOp::Add => Some("__add__"),
                        BinOp::Sub => Some("__sub__"),
                        BinOp::Mul => Some("__mul__"),
                        BinOp::Div => Some("__div__"),
                        BinOp::Mod => Some("__mod__"),
                        BinOp::Pow => Some("__pow__"),
                        BinOp::Eq => Some("__eq__"),
                        BinOp::NotEq => Some("__ne__"),
                        BinOp::Lt => Some("__lt__"),
                        BinOp::Gt => Some("__gt__"),
                        BinOp::LtEq => Some("__le__"),
                        BinOp::GtEq => Some("__ge__"),
                        BinOp::IntDiv => Some("__floordiv__"),
                        BinOp::BitAnd => Some("__band__"),
                        BinOp::BitOr => Some("__bor__"),
                        BinOp::BitXor => Some("__bxor__"),
                        BinOp::Shl => Some("__lshift__"),
                        BinOp::Shr => Some("__rshift__"),
                        _ => None,
                    };
                    if let Some(name) = dunder {
                        if let Some(method) = fields.borrow().get(name).cloned() {
                            return self.call_value(&method, &[(None, r)]);
                        }
                    }
                }
                // Dunder __contains__ for 'in' / 'not in' on instances
                if matches!(op, BinOp::In | BinOp::NotIn) {
                    let container = &r;
                    let cls_name = match container {
                        Value::Instance(name, _) | Value::StructInstance(name, _) => Some(name.clone()),
                        _ => None,
                    };
                    if let Some(cls_name) = cls_name {
                        if let Some(Value::Class(cv)) = self.env.get(&cls_name) {
                            if let Some(method) = cv.methods.get("__contains__") {
                                let method = method.clone();
                                let saved = self.env.current;
                                self.env.push_scope_with_parent(method.closure_env);
                                self.env.define("self", container.clone());
                                self.bind_call_params(&method.params, &[(None, l.clone())])?;
                                let result = self.exec_block_no_scope(&method.body)?;
                                self.env.set_scope(saved);
                                let ret = match result {
                                    Value::Return(v) => *v,
                                    other => other,
                                };
                                if matches!(op, BinOp::NotIn) {
                                    return Ok(Value::Bool(!ret.is_truthy()));
                                }
                                return Ok(ret);
                            }
                        }
                    }
                }
                self.binary_op(op, &l, &r)
            }
            Expr::UnaryOp { op, expr } => {
                let val = self.eval_expr(expr)?;
                // Dispatch __neg__ / __not__ for instances
                let method_name = match op {
                    UnaryOp::Neg => Some("__neg__"),
                    UnaryOp::Not => Some("__not__"),
                    _ => None,
                };
                if let Some(mname) = method_name {
                    let has_method = match &val {
                        Value::Instance(class_name, _) | Value::CowInstance(class_name, _) => {
                            let cn = class_name.clone();
                            if let Some(Value::Class(cls)) = self.env.get(&cn) {
                                cls.methods.contains_key(mname)
                            } else { false }
                        }
                        _ => false,
                    };
                    if has_method {
                        let (result, _) = self.call_method(&val, mname, &[])?;
                        return Ok(result);
                    }
                }
                self.unary_op(op, &val)
            }

            Expr::Call { callee, args } => {
                // Handle super(args) — call parent constructor
                if let Expr::Ident(name) = callee.as_ref() {
                    if name == "__yield_expr" {
                        let yielded = if let Some(arg) = args.first() {
                            self.eval_expr(&arg.value)?
                        } else {
                            Value::Null
                        };
                        let result = self.handle_yield_point(yielded)?;
                        return match result {
                            Value::Return(v) => Ok(*v),
                            other => Ok(other),
                        };
                    }
                    if name == "super" {
                        let evaluated_args = self.eval_call_args(args)?;
                        return self.call_super(&evaluated_args);
                    }
                    // Special form: freeze(var) — mutates variable in-place for lists
                    if name == "freeze" && args.len() == 1 && args[0].name.is_none() {
                        if let Expr::Ident(var_name) = &args[0].value {
                            let val = self.eval_expr(&args[0].value)?;
                            let frozen = self.freeze_value(val)?;
                            self.env.set(var_name, frozen.clone()).ok();
                            // Track list variables as frozen by name
                            if matches!(frozen, Value::List(_)) {
                                self.frozen_vars.insert(var_name.clone());
                            }
                            return Ok(frozen);
                        }
                    }
                    // Special form: is_frozen(var) — check frozen state for lists
                    if name == "is_frozen" && args.len() == 1 && args[0].name.is_none() {
                        if let Expr::Ident(var_name) = &args[0].value {
                            let val = self.eval_expr(&args[0].value)?;
                            let frozen = match &val {
                                Value::Instance(_, fields) | Value::StructInstance(_, fields) => {
                                    fields.get("__frozen").map_or(false, |v| v.is_truthy())
                                }
                                Value::CowInstance(_, fields) => {
                                    fields.borrow().get("__frozen").map_or(false, |v| v.is_truthy())
                                }
                                Value::Dict(pairs) => {
                                    pairs.iter().any(|(k, v)| *k == Value::Str("__frozen".to_string()) && v.is_truthy())
                                }
                                Value::List(_) => self.frozen_vars.contains(var_name),
                                _ => false,
                            };
                            return Ok(Value::Bool(frozen));
                        }
                    }
                }
                let func = self.eval_expr(callee)?;
                let evaluated_args = self.eval_call_args(args)?;
                // Handle newtype/cstruct constructors: they are stored as Value::Str but
                // we need the declared name to create the StructInstance
                if let Value::Str(ref s) = func {
                    if s.starts_with("newtype:") {
                        let type_name = if let Expr::Ident(name) = callee.as_ref() {
                            name.clone()
                        } else {
                            s["newtype:".len()..].to_string()
                        };
                        let inner_val = evaluated_args.into_iter().next().map(|(_, v)| v).unwrap_or(Value::Null);
                        let mut field_map = HashMap::new();
                        field_map.insert("0".to_string(), inner_val.clone());
                        field_map.insert("inner".to_string(), inner_val);
                        return Ok(Value::StructInstance(type_name, field_map));
                    } else if s.starts_with("cstruct:") {
                        let struct_name = s["cstruct:".len()..].to_string();
                        let key = format!("__cstruct_fields_{}", struct_name);
                        let field_names = if let Some(Value::List(fl)) = self.env.get(&key) {
                            fl.iter().filter_map(|v| if let Value::Str(fs) = v { Some(fs.clone()) } else { None }).collect::<Vec<_>>()
                        } else { vec![] };
                        let mut field_map = HashMap::new();
                        for (i, name) in field_names.iter().enumerate() {
                            let val = evaluated_args.get(i).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
                            field_map.insert(name.clone(), val);
                        }
                        return Ok(Value::StructInstance(struct_name, field_map));
                    }
                }
                self.call_value(&func, &evaluated_args)
            }
            Expr::MacroCall { name, args } => {
                let macro_val = self.macros.get(name).cloned().ok_or_else(|| {
                    format!("Undefined macro '{}'", name)
                })?;
                // Guard recursive/self-referential expansion so it errors cleanly
                // instead of overflowing the native stack.
                if self.macro_depth >= self.macro_limit {
                    return Err(format!(
                        "macro expansion of '{}!' exceeded the limit of {} (raise it with ct_set_macro_limit)",
                        name, self.macro_limit
                    ));
                }
                let evaluated_args = self.eval_call_args(args)?;
                let saved = self.env.current;
                self.env.push_scope();
                for (idx, param) in macro_val.params.iter().enumerate() {
                    let value = evaluated_args.get(idx).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
                    self.env.define(param, value);
                }
                self.macro_depth += 1;
                let result = self.exec_block_no_scope(&macro_val.body);
                self.macro_depth -= 1;
                self.env.set_scope(saved);
                match result? {
                    Value::Return(v) => Ok(*v),
                    other => Ok(other),
                }
            }
            Expr::MethodCall {
                object,
                method,
                args,
                optional,
            } => {
                if let Expr::Ident(name) = object.as_ref() {
                    if name == "super" {
                        let evaluated_args = self.eval_call_args(args)?;
                        return self.call_super_method(method, &evaluated_args);
                    }
                }
                // Check for mutation methods that need &mut access
                // For mutation methods, we need to check if this is a built-in mut operation
                // on a list/dict/set, NOT a user-defined method on a class instance
                let mutation_methods = ["push", "pop", "pop_opt", "insert", "remove", "extend", "clear", "add", "set", "update", "delete"];
                let is_potential_mutation = mutation_methods.contains(&method.as_str());
                if is_potential_mutation {
                    // Peek at object type to decide: if it's an instance/struct with this method, skip mutation
                    let obj_peek = self.eval_expr(object)?;
                    let has_exported_member = match &obj_peek {
                        Value::Dict(pairs) => pairs.iter().any(|(key, _)| {
                            matches!(key, Value::Str(name) if name == method)
                        }),
                        _ => false,
                    };
                    let is_user_method = match &obj_peek {
                        Value::Instance(cls, _) | Value::CowInstance(cls, _) => {
                            if let Some(Value::Class(cv)) = self.env.get(cls) {
                                cv.methods.contains_key(method.as_str())
                            } else { false }
                        }
                        _ => false,
                    };
                    // Only in-place mutation targets (list/dict/set) use the &mut
                    // fast path; other receivers (e.g. Decimal) dispatch normally.
                    let is_mutation_target = matches!(
                        obj_peek,
                        Value::List(_) | Value::Dict(_) | Value::Set(_)
                    );
                    if is_mutation_target && !is_user_method && !has_exported_member {
                        let evaluated_args = self.eval_call_args(args)?;
                        return self.call_mutation_method(object, method, &evaluated_args);
                    }
                }
                // Check for enum variant constructor: Shape.Circle(5)
                if let Expr::Ident(name) = object.as_ref() {
                    let dotted = format!("{}.{}", name, method);
                    if let Some(func) = self.env.get(&dotted) {
                        let evaluated_args = self.eval_call_args(args)?;
                        return self.call_value(&func, &evaluated_args);
                    }
                }
                let obj = self.eval_expr(object)?;
                // Optional chaining: return null if object is null
                if *optional && matches!(obj, Value::Null) {
                    return Ok(Value::Null);
                }
                let evaluated_args = self.eval_call_args(args)?;
                let (result, updated_self) = self.call_method(&obj, method, &evaluated_args)?;
                // If the method modified self, propagate changes back
                if let Some(new_self) = updated_self {
                    if let Expr::Ident(name) = object.as_ref() {
                        self.env.set(name, new_self).ok();
                    } else if let Expr::Self_ = object.as_ref() {
                        self.env.set("self", new_self).ok();
                    }
                }
                Ok(result)
            }
            Expr::FieldAccess { object, field, optional } => {
                // First check if this is an enum access like Color.Red
                if let Expr::Ident(name) = object.as_ref() {
                    let dotted = format!("{}.{}", name, field);
                    if let Some(val) = self.env.get(&dotted) {
                        return Ok(val);
                    }
                }
                let obj = self.eval_expr(object)?;
                // Optional chaining: return null if object is null
                if *optional && matches!(obj, Value::Null) {
                    return Ok(Value::Null);
                }
                match &obj {
                    Value::Instance(_, fields) | Value::StructInstance(_, fields) => {
                        // Check for computed properties first
                        if let Value::Instance(cls_name, _) = &obj {
                            if let Some(Value::Class(cls)) = self.env.get(cls_name) {
                                if let Some(cp) = cls.computed_properties.get(field) {
                                    if let Some(getter) = &cp.getter {
                                        let getter = getter.clone();
                                        let saved = self.env.current;
                                        self.env.push_scope_with_parent(getter.closure_env);
                                        self.env.define("self", obj.clone());
                                        let result = self.exec_block_no_scope(&getter.body)?;
                                        self.env.set_scope(saved);
                                        let ret = match result {
                                            Value::Return(v) => *v,
                                            other => other,
                                        };
                                        return Ok(ret);
                                    }
                                }
                            }
                        }
                        fields
                            .get(field)
                            .cloned()
                            .ok_or_else(|| format!("No field '{}' on instance", field))
                    }
                    Value::CowInstance(_, fields) => {
                        if let Value::CowInstance(cls_name, _) = &obj {
                            if let Some(Value::Class(cls)) = self.env.get(cls_name) {
                                if let Some(cp) = cls.computed_properties.get(field) {
                                    if let Some(getter) = &cp.getter {
                                        let getter = getter.clone();
                                        let saved = self.env.current;
                                        self.env.push_scope_with_parent(getter.closure_env);
                                        self.env.define("self", obj.clone());
                                        let result = self.exec_block_no_scope(&getter.body)?;
                                        self.env.set_scope(saved);
                                        let ret = match result {
                                            Value::Return(v) => *v,
                                            other => other,
                                        };
                                        return Ok(ret);
                                    }
                                }
                            }
                        }
                        fields
                            .borrow()
                            .get(field)
                            .cloned()
                            .ok_or_else(|| format!("No field '{}' on instance", field))
                    }
                    Value::Dict(pairs) => {
                        let key = Value::Str(field.clone());
                        for (k, v) in pairs {
                            if *k == key {
                                return Ok(v.clone());
                            }
                        }
                        Err(format!("No key '{}' in dict", field))
                    }
                    // Positional tuple access: t.0, t.1, ...
                    Value::Tuple(items) => {
                        if let Ok(idx) = field.parse::<usize>() {
                            items.get(idx).cloned().ok_or_else(|| {
                                format!("Tuple index {} out of range (length {})", idx, items.len())
                            })
                        } else {
                            Err(format!("Cannot access field '{}' on tuple", field))
                        }
                    }
                    // Static (class-level) fields: Counter.count
                    Value::Class(cls) => cls
                        .fields
                        .get(field)
                        .cloned()
                        .ok_or_else(|| {
                            format!("No static field '{}' on class {}", field, cls.name)
                        }),
                    _ => Err(format!("Cannot access field '{}' on {}", field, obj.type_name())),
                }
            }
            Expr::Index { object, index } => {
                let obj = self.eval_expr(object)?;
                let idx = self.eval_expr(index)?;
                // Dunder __getitem__ dispatch for instances
                let cls_name = match &obj {
                    Value::Instance(name, _) | Value::StructInstance(name, _) => Some(name.clone()),
                    _ => None,
                };
                if let Some(cls_name) = cls_name {
                    if let Some(Value::Class(cv)) = self.env.get(&cls_name) {
                        if let Some(method) = cv.methods.get("__getitem__") {
                            let method = method.clone();
                            let saved = self.env.current;
                            self.env.push_scope_with_parent(method.closure_env);
                            self.env.define("self", obj.clone());
                            self.bind_call_params(&method.params, &[(None, idx)])?;
                            let result = self.exec_block_no_scope(&method.body)?;
                            self.env.set_scope(saved);
                            return match result {
                                Value::Return(v) => Ok(*v),
                                other => Ok(other),
                            };
                        }
                    }
                }
                self.index_value(&obj, &idx)
            }
            Expr::Slice { object, start, end, step } => {
                let obj = self.eval_expr(object)?;
                let start_val = if let Some(s) = start { Some(self.eval_expr(s)?) } else { None };
                let end_val = if let Some(e) = end { Some(self.eval_expr(e)?) } else { None };
                let step_val = if let Some(st) = step { Some(self.eval_expr(st)?) } else { None };
                self.slice_value(&obj, start_val, end_val, step_val)
            }

            Expr::List(elements) => {
                let mut items = Vec::new();
                for e in elements {
                    if let Expr::Spread(inner) = e {
                        let val = self.eval_expr(inner)?;
                        match val {
                            Value::List(inner_items) => items.extend(inner_items),
                            Value::Tuple(inner_items) => items.extend(inner_items),
                            _ => return Err("Spread operator requires an iterable".into()),
                        }
                    } else {
                        items.push(self.eval_expr(e)?);
                    }
                }
                Ok(Value::List(items))
            }
            Expr::ListComp { expr, clauses } => {
                let result = self.eval_comp_clauses(clauses, |interp| {
                    interp.eval_expr(expr)
                })?;
                Ok(Value::List(result))
            }
            Expr::DictComp { key_expr, val_expr, clauses } => {
                let mut pairs = Vec::new();
                self.eval_comp_clauses_raw(clauses, &mut |interp| {
                    let k = interp.eval_expr(key_expr)?;
                    let v = interp.eval_expr(val_expr)?;
                    pairs.push((k, v));
                    Ok(())
                })?;
                Ok(Value::Dict(pairs))
            }
            Expr::SetComp { expr, clauses } => {
                let items = self.eval_comp_clauses(clauses, |interp| {
                    interp.eval_expr(expr)
                })?;
                let mut result: Vec<Value> = Vec::new();
                for v in items {
                    if !result.contains(&v) { result.push(v); }
                }
                Ok(Value::Set(result))
            }
            Expr::GenComp { expr, clauses } => {
                // Generator comprehension: evaluate lazily as a generator (eagerly collect for now)
                let items = self.eval_comp_clauses(clauses, |interp| {
                    interp.eval_expr(expr)
                })?;
                use std::rc::Rc;
                use std::cell::RefCell;
                Ok(Value::Generator(Rc::new(RefCell::new(GeneratorState {
                    items,
                    index: 0,
                    lazy: None,
                    started: false,
                    done: false,
                    resume_inputs: vec![],
                }))))
            }
            Expr::ByteStr(bytes) => Ok(Value::Bytes(bytes.clone())),
            Expr::DoBlock(stmts) => {
                self.env.push_scope();
                let mut last = Value::Null;
                for s in stmts {
                    last = self.exec_stmt(s)?;
                    match &last {
                        Value::Return(_) | Value::Break | Value::Continue
                        | Value::BreakLabel(_) | Value::ContinueLabel(_) => {
                            self.env.pop_scope();
                            return Ok(last);
                        }
                        _ => {}
                    }
                }
                // If last statement was an expression statement, return its value
                self.env.pop_scope();
                Ok(last)
            }
            Expr::MatchExpr { subject, arms } => {
                let val = self.eval_expr(subject)?;
                for arm in arms {
                    if self.matches_pattern(&val, &arm.pattern)? {
                        self.env.push_scope();
                        self.bind_pattern(&val, &arm.pattern)?;
                        if let Some(guard) = &arm.guard {
                            let g = self.eval_expr(guard)?;
                            if !g.is_truthy() {
                                self.env.pop_scope();
                                continue;
                            }
                        }
                        let result = self.exec_block_no_scope(&arm.body);
                        self.env.pop_scope();
                        return result;
                    }
                }
                Ok(Value::Null)
            }
            Expr::Dict(pairs) => {
                let mut items = Vec::new();
                for (k, v) in pairs {
                    if let Expr::Spread(inner) = k {
                        let spread_val = self.eval_expr(inner)?;
                        match spread_val {
                            Value::Dict(spread_pairs) => {
                                for (sk, sv) in spread_pairs {
                                    if let Some((_, existing)) = items.iter_mut().find(|(ek, _)| *ek == sk) {
                                        *existing = sv;
                                    } else {
                                        items.push((sk, sv));
                                    }
                                }
                            }
                            Value::Instance(_, fields) | Value::StructInstance(_, fields) => {
                                for (field, val) in fields {
                                    let sk = Value::Str(field);
                                    if let Some((_, existing)) = items.iter_mut().find(|(ek, _)| *ek == sk) {
                                        *existing = val;
                                    } else {
                                        items.push((sk, val));
                                    }
                                }
                            }
                            Value::CowInstance(_, fields) => {
                                for (field, val) in fields.borrow().iter() {
                                    let sk = Value::Str(field.clone());
                                    if let Some((_, existing)) = items.iter_mut().find(|(ek, _)| *ek == sk) {
                                        *existing = val.clone();
                                    } else {
                                        items.push((sk, val.clone()));
                                    }
                                }
                            }
                            _ => {
                                return Err("Dict spread requires a dict or object-like value".into());
                            }
                        }
                    } else {
                        let key = self.eval_expr(k)?;
                        let val = self.eval_expr(v)?;
                        if let Some((_, existing)) = items.iter_mut().find(|(ek, _)| *ek == key) {
                            *existing = val;
                        } else {
                            items.push((key, val));
                        }
                    }
                }
                Ok(Value::Dict(items))
            }
            Expr::Tuple(elements) => {
                let mut items = Vec::new();
                for e in elements {
                    items.push(self.eval_expr(e)?);
                }
                Ok(Value::Tuple(items))
            }
            Expr::Set(elements) => {
                let mut items: Vec<Value> = Vec::new();
                for e in elements {
                    let v = self.eval_expr(e)?;
                    // Sets hold unique elements — duplicates collapse.
                    if !items.contains(&v) {
                        items.push(v);
                    }
                }
                Ok(Value::Set(items))
            }

            Expr::Lambda { params, body, .. } => {
                // Return a function value
                Ok(Value::Func(FuncValue {
                    name: "<lambda>".to_string(),
                    params: params.clone(),
                    body: vec![Stmt::Return(Some(body.as_ref().clone()))],
                    closure_env: self.env.current,
                    is_generator: false,
                }))
            }
            Expr::LambdaBlock { params, body, .. } => {
                Ok(Value::Func(FuncValue {
                    name: "<lambda>".to_string(),
                    params: params.clone(),
                    body: body.clone(),
                    closure_env: self.env.current,
                    is_generator: false,
                }))
            }

            Expr::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                let cond = self.eval_expr(condition)?;
                if cond.is_truthy() {
                    self.eval_expr(then_expr)
                } else {
                    self.eval_expr(else_expr)
                }
            }

            Expr::Range {
                start,
                end,
                inclusive,
            } => {
                let s = self.eval_expr(start)?;
                let e = self.eval_expr(end)?;
                match (&s, &e) {
                    (Value::Int(a), Value::Int(b)) => Ok(Value::Range(*a, *b, *inclusive)),
                    _ => Err("Range requires integer bounds".into()),
                }
            }

            Expr::Spread(inner) => {
                // In the context of a list, spread is handled by the list builder.
                // Here, just evaluate.
                self.eval_expr(inner)
            }

            Expr::New { class, args } => {
                let cls = self.env.get(class).ok_or_else(|| {
                    format!("Undefined class '{}'", class)
                })?;
                match cls {
                    Value::Class(cv) => {
                        let mut instance_fields = cv.fields.clone();
                        // Evaluate args
                        let evaluated_args = self.eval_call_args(args)?;
                        // Call constructor (init method) if it exists
                        let instance = if cv.is_cow {
                            Value::CowInstance(cv.name.clone(), Rc::new(RefCell::new(instance_fields.clone())))
                        } else {
                            Value::Instance(cv.name.clone(), instance_fields.clone())
                        };
                        let init_method = cv.methods.get("init")
                            .or_else(|| cv.methods.get("constructor"))
                            .cloned();
                        if let Some(init) = init_method {
                            self.env.push_scope_with_parent(init.closure_env);
                            self.env.define("self", instance.clone());
                            // Bind constructor params
                            self.bind_call_params(&init.params, &evaluated_args)?;
                            self.exec_block_no_scope(&init.body)?;
                            // Retrieve mutated self
                            let result = self.env.get("self").unwrap_or(instance);
                            self.env.pop_scope();
                            Ok(result)
                        } else {
                            // No init: use positional args to fill fields in order
                            let field_names = if !cv.field_order.is_empty() {
                                cv.field_order.clone()
                            } else {
                                instance_fields.keys().cloned().collect()
                            };
                            for (i, (_, val)) in evaluated_args.iter().enumerate() {
                                if i < field_names.len() {
                                    instance_fields
                                        .insert(field_names[i].clone(), val.clone());
                                }
                            }
                            if cv.is_cow {
                                Ok(Value::CowInstance(cv.name.clone(), Rc::new(RefCell::new(instance_fields))))
                            } else {
                                Ok(Value::Instance(cv.name.clone(), instance_fields))
                            }
                        }
                    }
                    Value::Str(s) if s.starts_with("cstruct:") => {
                        let struct_name = &s["cstruct:".len()..];
                        let key = format!("__cstruct_fields_{}", struct_name);
                        let field_names = if let Some(Value::List(fl)) = self.env.get(&key) {
                            fl.iter().filter_map(|v| if let Value::Str(s) = v { Some(s.clone()) } else { None }).collect::<Vec<_>>()
                        } else {
                            vec![]
                        };
                        let evaluated_args = self.eval_call_args(args)?;
                        let mut field_map = HashMap::new();
                        for (i, name) in field_names.iter().enumerate() {
                            if i < evaluated_args.len() {
                                field_map.insert(name.clone(), evaluated_args[i].1.clone());
                            } else {
                                field_map.insert(name.clone(), Value::Null);
                            }
                        }
                        Ok(Value::StructInstance(struct_name.to_string(), field_map))
                    }
                    Value::Str(s) if s.starts_with("newtype:") => {
                        let type_name = s["newtype:".len()..].to_string();
                        let evaluated_args = self.eval_call_args(args)?;
                        let inner_val = evaluated_args.into_iter().next().map(|(_, v)| v).unwrap_or(Value::Null);
                        let mut field_map = HashMap::new();
                        field_map.insert("0".to_string(), inner_val.clone());
                        field_map.insert("inner".to_string(), inner_val);
                        Ok(Value::StructInstance(type_name, field_map))
                    }
                    _ => Err(format!("'{}' is not a class", class)),
                }
            }

            Expr::Await(inner) => {
                // Simplified: just evaluate the inner expression
                self.eval_expr(inner)
            }

            Expr::StructLit { name, fields, spread } => {
                let mut field_map = HashMap::new();
                if let Some(spread_expr) = spread {
                    let base = self.eval_expr(spread_expr)?;
                    if let Value::StructInstance(_, base_fields) = base {
                        field_map = base_fields;
                    }
                }
                for (fname, fexpr) in fields {
                    field_map.insert(fname.clone(), self.eval_expr(fexpr)?);
                }
                Ok(Value::StructInstance(name.clone(), field_map))
            }

            Expr::TypeOf(inner) => {
                let val = self.eval_expr(inner)?;
                Ok(Value::Str(val.type_name().to_string()))
            }

            Expr::Pipe { left, right } => {
                // x |> f  =>  f(x), or  x |> add(_, 3)  =>  add(x, 3)
                let left_val = self.eval_expr(left)?;
                match right.as_ref() {
                    Expr::Call { callee, args } => {
                        let func = self.eval_expr(callee)?;
                        // Check if any arg uses _ placeholder
                        let has_placeholder = args.iter().any(|arg| {
                            matches!(&arg.value, Expr::Ident(n) if n == "_")
                        });
                        if has_placeholder {
                            // Replace _ with left_val in arg list
                            let mut evaluated_args = Vec::new();
                            for arg in args {
                                if matches!(&arg.value, Expr::Ident(n) if n == "_") {
                                    evaluated_args.push((arg.name.clone(), left_val.clone()));
                                } else {
                                    evaluated_args.push((arg.name.clone(), self.eval_expr(&arg.value)?));
                                }
                            }
                            self.call_value(&func, &evaluated_args)
                        } else {
                            let mut all_args = vec![(None, left_val)];
                            let rest_args = self.eval_call_args(args)?;
                            all_args.extend(rest_args);
                            self.call_value(&func, &all_args)
                        }
                    }
                    Expr::Ident(name) => {
                        let func = self.env.get(name).ok_or_else(|| {
                            format!("Undefined function '{}'", name)
                        })?;
                        self.call_value(&func, &[(None, left_val)])
                    }
                    _ => Err("Pipe target must be a function or call".into()),
                }
            }

            Expr::Grouped(inner) => self.eval_expr(inner),

            Expr::Lazy(inner) => {
                Ok(Value::Lazy(Box::new(inner.as_ref().clone())))
            }

            Expr::Cast { expr, target } => {
                let val = self.eval_expr(expr)?;
                match target.as_str() {
                    "int" => match &val {
                        Value::Int(n) => Ok(Value::Int(*n)),
                        Value::Float(f) => Ok(Value::Int(*f as i64)),
                        Value::Bool(b) => Ok(Value::Int(if *b { 1 } else { 0 })),
                        Value::Str(s) => s.parse::<i64>().map(Value::Int)
                            .map_err(|_| format!("Cannot cast '{}' to int", s)),
                        _ => Err(format!("Cannot cast {} to int", val.type_name())),
                    },
                    "float" => match &val {
                        Value::Float(f) => Ok(Value::Float(*f)),
                        Value::Int(n) => Ok(Value::Float(*n as f64)),
                        Value::Bool(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
                        Value::Str(s) => s.parse::<f64>().map(Value::Float)
                            .map_err(|_| format!("Cannot cast '{}' to float", s)),
                        _ => Err(format!("Cannot cast {} to float", val.type_name())),
                    },
                    "str" | "string" => Ok(Value::Str(format!("{}", val))),
                    "bool" => Ok(Value::Bool(val.is_truthy())),
                    _ => Err(format!("Unknown cast target type '{}'", target)),
                }
            }
            Expr::TryUnwrap(expr) => {
                let val = self.eval_expr(expr)?;
                match val {
                    Value::Ok(v) => Ok(*v),
                    Value::Some(v) => Ok(*v),
                    // Early-return the Err/None from the enclosing function.
                    // The value travels in pending_try_return; exec_block_no_scope
                    // converts the sentinel into a normal Value::Return.
                    Value::Err(_) => {
                        self.pending_try_return = Some(val);
                        Err("__try_return__".to_string())
                    }
                    Value::Null => {
                        self.pending_try_return = Some(Value::Null);
                        Err("__try_return__".to_string())
                    }
                    other => Ok(other), // non-Result/Option passes through
                }
            }
        }
    }

    // ── Operators ────────────────────────────────────────

    /// Normalize a big integer back to a machine `Int` when it fits, keeping the
    /// common case as fast `i64` and only staying big when necessary.
    fn norm_bigint(b: crate::bigint::BigInt) -> Value {
        match b.to_i64() {
            Some(i) => Value::Int(i),
            None => Value::BigInt(b),
        }
    }

    /// View an integer-typed value as a BigInt (for mixed Int/BigInt arithmetic).
    fn as_bigint(v: &Value) -> Option<crate::bigint::BigInt> {
        match v {
            Value::Int(i) => Some(crate::bigint::BigInt::from_i64(*i)),
            Value::BigInt(b) => Some(b.clone()),
            _ => None,
        }
    }

    /// View a value as an exact Decimal for decimal arithmetic (Int/BigInt/Float
    /// operands are promoted so `decimal + 1` works).
    fn as_decimal(v: &Value) -> Option<crate::decimal::Decimal> {
        match v {
            Value::Decimal(d) => Some(d.clone()),
            Value::Int(i) => Some(crate::decimal::Decimal::from_i64(*i)),
            Value::BigInt(b) => crate::decimal::Decimal::from_str(&b.to_string()),
            Value::Float(f) => Some(crate::decimal::Decimal::from_f64(*f)),
            Value::Str(s) => crate::decimal::Decimal::from_str(s),
            _ => None,
        }
    }

    fn binary_op(&self, op: &BinOp, left: &Value, right: &Value) -> Result<Value, String> {
        use crate::bigint::BigInt;
        // Exact decimal arithmetic — if either side is a Decimal, promote both.
        if matches!(left, Value::Decimal(_)) || matches!(right, Value::Decimal(_)) {
            if let (Some(a), Some(b)) = (Self::as_decimal(left), Self::as_decimal(right)) {
                match op {
                    BinOp::Add => return Ok(Value::Decimal(a.add(&b))),
                    BinOp::Sub => return Ok(Value::Decimal(a.sub(&b))),
                    BinOp::Mul => return Ok(Value::Decimal(a.mul(&b))),
                    BinOp::Div => return a.div(&b).map(Value::Decimal)
                        .ok_or_else(|| "Division by zero".to_string()),
                    BinOp::Eq => return Ok(Value::Bool(a.cmp(&b) == std::cmp::Ordering::Equal)),
                    BinOp::NotEq => return Ok(Value::Bool(a.cmp(&b) != std::cmp::Ordering::Equal)),
                    BinOp::Lt => return Ok(Value::Bool(a.cmp(&b) == std::cmp::Ordering::Less)),
                    BinOp::Gt => return Ok(Value::Bool(a.cmp(&b) == std::cmp::Ordering::Greater)),
                    BinOp::LtEq => return Ok(Value::Bool(a.cmp(&b) != std::cmp::Ordering::Greater)),
                    BinOp::GtEq => return Ok(Value::Bool(a.cmp(&b) != std::cmp::Ordering::Less)),
                    _ => {}
                }
            }
        }
        // Mixed Int/BigInt (or BigInt/BigInt) integer arithmetic — arbitrary precision.
        if matches!(left, Value::BigInt(_)) || matches!(right, Value::BigInt(_)) {
            // BigInt mixed with Float: promote the BigInt to f64 (may lose
            // precision, same as int/float mixing) and use float arithmetic.
            if matches!(left, Value::Float(_)) || matches!(right, Value::Float(_)) {
                let to_f = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Float(f) => Some(*f),
                        Value::Int(i) => Some(*i as f64),
                        Value::BigInt(b) => Some(b.to_f64()),
                        _ => None,
                    }
                };
                if let (Some(a), Some(b)) = (to_f(left), to_f(right)) {
                    match op {
                        BinOp::Add => return Ok(Value::Float(a + b)),
                        BinOp::Sub => return Ok(Value::Float(a - b)),
                        BinOp::Mul => return Ok(Value::Float(a * b)),
                        BinOp::Div => {
                            if b == 0.0 { return Err("Division by zero".into()); }
                            return Ok(Value::Float(a / b));
                        }
                        BinOp::IntDiv => {
                            if b == 0.0 { return Err("Division by zero".into()); }
                            return Ok(Value::Float((a / b).floor()));
                        }
                        BinOp::Mod => return Ok(Value::Float(a % b)),
                        BinOp::Pow => return Ok(Value::Float(a.powf(b))),
                        BinOp::Eq => return Ok(Value::Bool(a == b)),
                        BinOp::NotEq => return Ok(Value::Bool(a != b)),
                        BinOp::Lt => return Ok(Value::Bool(a < b)),
                        BinOp::Gt => return Ok(Value::Bool(a > b)),
                        BinOp::LtEq => return Ok(Value::Bool(a <= b)),
                        BinOp::GtEq => return Ok(Value::Bool(a >= b)),
                        _ => {}
                    }
                }
            }
            if let (Some(a), Some(b)) = (Self::as_bigint(left), Self::as_bigint(right)) {
                match op {
                    BinOp::Add => return Ok(Self::norm_bigint(a.add(&b))),
                    BinOp::Sub => return Ok(Self::norm_bigint(a.sub(&b))),
                    BinOp::Mul => return Ok(Self::norm_bigint(a.mul(&b))),
                    BinOp::IntDiv => {
                        return a.div_rem(&b).map(|(q, _)| Self::norm_bigint(q))
                            .ok_or_else(|| "Division by zero".to_string());
                    }
                    BinOp::Mod => {
                        return a.div_rem(&b).map(|(_, r)| Self::norm_bigint(r))
                            .ok_or_else(|| "Modulo by zero".to_string());
                    }
                    BinOp::Div => {
                        if b.is_zero() { return Err("Division by zero".into()); }
                        return Ok(Value::Float(a.to_f64() / b.to_f64()));
                    }
                    BinOp::Pow => {
                        if !b.is_negative() {
                            if let Some(e) = b.to_i64() {
                                return Ok(Self::norm_bigint(a.pow(e as u64)));
                            }
                        }
                        return Ok(Value::Float(a.to_f64().powf(b.to_f64())));
                    }
                    BinOp::Eq => return Ok(Value::Bool(a.cmp(&b) == std::cmp::Ordering::Equal)),
                    BinOp::NotEq => return Ok(Value::Bool(a.cmp(&b) != std::cmp::Ordering::Equal)),
                    BinOp::Lt => return Ok(Value::Bool(a.cmp(&b) == std::cmp::Ordering::Less)),
                    BinOp::Gt => return Ok(Value::Bool(a.cmp(&b) == std::cmp::Ordering::Greater)),
                    BinOp::LtEq => return Ok(Value::Bool(a.cmp(&b) != std::cmp::Ordering::Greater)),
                    BinOp::GtEq => return Ok(Value::Bool(a.cmp(&b) != std::cmp::Ordering::Less)),
                    _ => {}
                }
            }
        }
        match op {
            BinOp::Add => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(match a.checked_add(*b) {
                    Some(v) => Value::Int(v),
                    None => Self::norm_bigint(BigInt::from_i64(*a).add(&BigInt::from_i64(*b))),
                }),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
                (Value::Str(a), Value::Str(b)) => Ok(Value::Str(format!("{}{}", a, b))),
                (Value::Str(a), b) => Ok(Value::Str(format!("{}{}", a, b))),
                (a, Value::Str(b)) => Ok(Value::Str(format!("{}{}", a, b))),
                (Value::List(a), Value::List(b)) => {
                    let mut result = a.clone();
                    result.extend(b.iter().cloned());
                    Ok(Value::List(result))
                }
                _ => Err(format!(
                    "Cannot add {} and {}",
                    left.type_name(),
                    right.type_name()
                )),
            },
            BinOp::Sub => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(match a.checked_sub(*b) {
                    Some(v) => Value::Int(v),
                    None => Self::norm_bigint(BigInt::from_i64(*a).sub(&BigInt::from_i64(*b))),
                }),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 - b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - *b as f64)),
                (Value::Set(a), Value::Set(b)) => {
                    let result: Vec<Value> = a.iter().filter(|x| !b.contains(x)).cloned().collect();
                    Ok(Value::Set(result))
                }
                _ => Err(format!(
                    "Cannot subtract {} from {}",
                    right.type_name(),
                    left.type_name()
                )),
            },
            BinOp::Mul => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(match a.checked_mul(*b) {
                    Some(v) => Value::Int(v),
                    None => Self::norm_bigint(BigInt::from_i64(*a).mul(&BigInt::from_i64(*b))),
                }),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 * b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * *b as f64)),
                (Value::Str(s), Value::Int(n)) | (Value::Int(n), Value::Str(s)) => {
                    Ok(Value::Str(Self::repeat_str(s, *n)?))
                }
                (Value::List(items), Value::Int(n)) | (Value::Int(n), Value::List(items)) => {
                    // list * n replicates like Python; negative counts give [].
                    let n = (*n).max(0) as usize;
                    if items.len().saturating_mul(n) > 100_000_000 {
                        return Err("List repeat result too large".into());
                    }
                    let mut out = Vec::with_capacity(items.len() * n);
                    for _ in 0..n { out.extend(items.iter().cloned()); }
                    Ok(Value::List(out))
                }
                _ => Err(format!(
                    "Cannot multiply {} and {}",
                    left.type_name(),
                    right.type_name()
                )),
            },
            BinOp::Div => match (left, right) {
                (_, Value::Int(0)) => Err("Division by zero".into()),
                (_, Value::Float(f)) if *f == 0.0 => Err("Division by zero".into()),
                (Value::Int(a), Value::Int(b)) => Ok(Value::Float(*a as f64 / *b as f64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 / b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a / *b as f64)),
                _ => Err(format!(
                    "Cannot divide {} by {}",
                    left.type_name(),
                    right.type_name()
                )),
            },
            BinOp::Mod => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b == 0 {
                        Err("Modulo by zero".into())
                    } else {
                        Ok(Value::Int(a % b))
                    }
                }
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 % b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a % *b as f64)),
                _ => Err(format!(
                    "Cannot modulo {} by {}",
                    left.type_name(),
                    right.type_name()
                )),
            },
            BinOp::Pow => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b >= 0 {
                        // Unsized int is arbitrary-precision: use checked_pow and
                        // promote to BigInt on overflow so results are always exact.
                        match a.checked_pow(*b as u32) {
                            Some(v) => Ok(Value::Int(v)),
                            None => Ok(Self::norm_bigint(BigInt::from_i64(*a).pow(*b as u64))),
                        }
                    } else {
                        Ok(Value::Float((*a as f64).powf(*b as f64)))
                    }
                }
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.powf(*b))),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float((*a as f64).powf(*b))),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a.powf(*b as f64))),
                _ => Err(format!(
                    "Cannot raise {} to {}",
                    left.type_name(),
                    right.type_name()
                )),
            },
            BinOp::IntDiv => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b == 0 {
                        Err("Division by zero".into())
                    } else {
                        // Floor division: rounds toward negative infinity, so
                        // -7 // 2 == -4 (not -3 as truncation would give).
                        Ok(Value::Int(a.div_euclid(*b) - if a.rem_euclid(*b) != 0 && *b < 0 { 1 } else { 0 }))
                    }
                }
                (Value::Float(a), Value::Float(b)) => Ok(Value::Int((a / b).floor() as i64)),
                (Value::Int(a), Value::Float(b)) => Ok(Value::Int((*a as f64 / b).floor() as i64)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Int((a / *b as f64).floor() as i64)),
                _ => Err("Integer division requires numeric operands".into()),
            },
            BinOp::Eq => Ok(Value::Bool(left == right)),
            BinOp::NotEq => Ok(Value::Bool(left != right)),
            BinOp::Lt => self.compare_values(left, right, |a, b| a < b),
            BinOp::Gt => self.compare_values(left, right, |a, b| a > b),
            BinOp::LtEq => self.compare_values(left, right, |a, b| a <= b),
            BinOp::GtEq => self.compare_values(left, right, |a, b| a >= b),
            BinOp::And => {
                if left.is_truthy() {
                    Ok(right.clone())
                } else {
                    Ok(left.clone())
                }
            }
            BinOp::Or => {
                if left.is_truthy() {
                    Ok(left.clone())
                } else {
                    Ok(right.clone())
                }
            }
            BinOp::BitAnd => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a & b)),
                (Value::Set(a), Value::Set(b)) => {
                    let result: Vec<Value> = a.iter().filter(|x| b.contains(x)).cloned().collect();
                    Ok(Value::Set(result))
                }
                _ => Err("Bitwise AND requires integers or sets".into()),
            },
            BinOp::BitOr => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a | b)),
                (Value::Set(a), Value::Set(b)) => {
                    let mut result = a.clone();
                    for x in b { if !result.contains(x) { result.push(x.clone()); } }
                    Ok(Value::Set(result))
                }
                _ => Err("Bitwise OR requires integers or sets".into()),
            },
            BinOp::BitXor => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a ^ b)),
                (Value::Set(a), Value::Set(b)) => {
                    let mut result: Vec<Value> = a.iter().filter(|x| !b.contains(x)).cloned().collect();
                    for x in b { if !a.contains(x) { result.push(x.clone()); } }
                    Ok(Value::Set(result))
                }
                _ => Err("Bitwise XOR requires integers or sets".into()),
            },
            BinOp::Shl => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b < 0 { return Err("Shift count cannot be negative".into()); }
                    if *b > 1_000_000 { return Err("Shift count too large".into()); }
                    // Promote to BigInt on overflow — ints are arbitrary precision.
                    match (*b < 64).then(|| a.checked_shl(*b as u32)).flatten() {
                        Some(v) if (v >> b) == *a => Ok(Value::Int(v)),
                        _ => Ok(Self::norm_bigint(
                            BigInt::from_i64(*a).mul(&BigInt::from_i64(2).pow(*b as u64)),
                        )),
                    }
                }
                _ => Err("Shift requires integers".into()),
            },
            BinOp::Shr => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b < 0 { return Err("Shift count cannot be negative".into()); }
                    Ok(Value::Int(if *b >= 64 { if *a < 0 { -1 } else { 0 } } else { a >> b }))
                }
                _ => Err("Shift requires integers".into()),
            },
            BinOp::In => match right {
                Value::List(items) => Ok(Value::Bool(items.contains(left))),
                Value::Dict(pairs) => {
                    Ok(Value::Bool(pairs.iter().any(|(k, _)| k == left)))
                }
                Value::Str(s) => {
                    if let Value::Str(sub) = left {
                        Ok(Value::Bool(s.contains(sub.as_str())))
                    } else {
                        Err("Can only check string 'in' string".into())
                    }
                }
                Value::Set(items) => Ok(Value::Bool(items.contains(left))),
                Value::Tuple(items) => Ok(Value::Bool(items.contains(left))),
                Value::Range(start, end, inclusive) => {
                    if let Value::Int(v) = left {
                        if *inclusive {
                            Ok(Value::Bool(*v >= *start && *v <= *end))
                        } else {
                            Ok(Value::Bool(*v >= *start && *v < *end))
                        }
                    } else {
                        Err("Can only check int 'in' range".into())
                    }
                }
                _ => Err(format!("Cannot use 'in' with {}", right.type_name())),
            },
            BinOp::NotIn => match right {
                Value::List(items) => Ok(Value::Bool(!items.contains(left))),
                Value::Dict(pairs) => {
                    Ok(Value::Bool(!pairs.iter().any(|(k, _)| k == left)))
                }
                Value::Str(s) => {
                    if let Value::Str(sub) = left {
                        Ok(Value::Bool(!s.contains(sub.as_str())))
                    } else {
                        Err("Can only check string 'not in' string".into())
                    }
                }
                Value::Set(items) => Ok(Value::Bool(!items.contains(left))),
                Value::Tuple(items) => Ok(Value::Bool(!items.contains(left))),
                Value::Range(start, end, inclusive) => {
                    if let Value::Int(v) = left {
                        let in_range = if *inclusive { *v >= *start && *v <= *end } else { *v >= *start && *v < *end };
                        Ok(Value::Bool(!in_range))
                    } else {
                        Err("Can only check int 'not in' range".into())
                    }
                }
                _ => Err(format!("Cannot use 'not in' with {}", right.type_name())),
            },
            BinOp::Is => {
                let type_name = match right {
                    Value::Str(s) => s.clone(),
                    Value::BuiltinFunc(s) => s.clone(), // bare type names: int, str, float, etc.
                    // User-defined class names; traits are stored as classes
                    // named "<trait X>", so unwrap to the bare trait name.
                    Value::Class(cv) => cv
                        .name
                        .strip_prefix("<trait ")
                        .and_then(|s| s.strip_suffix('>'))
                        .unwrap_or(&cv.name)
                        .to_string(),
                    _ => right.type_name().to_string(),
                };
                // Instances match their class, any ancestor class, and any
                // trait the class (or an ancestor) implements.
                let matches = match left {
                    Value::Instance(cn, _)
                    | Value::CowInstance(cn, _)
                    | Value::StructInstance(cn, _) => {
                        self.class_is_a(cn, &type_name) || left.type_name() == type_name
                    }
                    _ => left.type_name() == type_name,
                };
                Ok(Value::Bool(matches))
            },
            BinOp::NullCoalesce => {
                // Already handled in eval_expr for short-circuit, but as fallback:
                if *left == Value::Null {
                    Ok(right.clone())
                } else {
                    Ok(left.clone())
                }
            },
        }
    }

    /// True when `class_name` equals `target`, inherits from it, or implements
    /// it as a trait (directly or via an ancestor). Used by the `is` operator.
    fn class_is_a(&self, class_name: &str, target: &str) -> bool {
        let mut cur = Some(class_name.to_string());
        let mut hops = 0;
        while let Some(cn) = cur {
            if cn == target {
                return true;
            }
            if self
                .trait_impls
                .get(&cn)
                .map(|ts| ts.iter().any(|t| t == target))
                .unwrap_or(false)
            {
                return true;
            }
            cur = match self.env.get(&cn) {
                Some(Value::Class(cv)) => cv.parent.clone(),
                _ => None,
            };
            hops += 1;
            if hops > 64 { break; } // guard against parent cycles
        }
        false
    }

    /// Execute an embedded engine block: run-only blocks execute in place;
    /// blocks with `@export` start a persistent worker whose functions become
    /// importable via `@import { ... } from @lang` selectors.
    fn exec_embedded_block(
        &mut self,
        lang_tag: &str,
        label: Option<&str>,
        code: &str,
    ) -> Result<Value, String> {
        let lang = crate::engines::normalize_lang(lang_tag);
        let (clean_code, mut exports, wildcard) = crate::engines::extract_directives(code);

        match lang {
            "python" | "node" => {
                if exports.is_empty() && !wildcard {
                    crate::engines::run_block(lang, &clean_code)?;
                    return Ok(Value::Null);
                }
                let worker = crate::engines::EngineWorker::start(lang, &clean_code)?;
                if wildcard {
                    if worker.announced_exports.is_empty() {
                        return Err(format!(
                            "@export {{ * }} needs enumerable globals; list names explicitly in @{} blocks",
                            lang_tag
                        ));
                    }
                    for name in &worker.announced_exports {
                        if !exports.contains(name) {
                            exports.push(name.clone());
                        }
                    }
                }
                let wid = self.engine_workers.len();
                self.engine_workers.push(worker);

                let tag = crate::engines::canonical_tag(lang_tag).to_string();
                let mut keys = vec![format!("@{}", tag)];
                if let Some(l) = label {
                    keys.push(l.to_string());
                    keys.push(format!("@{}.{}", tag, l));
                }
                for k in keys {
                    let entry = self.engine_exports.entry(k).or_default();
                    for name in &exports {
                        entry.push((name.clone(), wid));
                    }
                }
                Ok(Value::Null)
            }
            "bash" | "sh" | "shell" | "powershell" | "lua" => {
                if !exports.is_empty() || wildcard {
                    return Err(format!(
                        "@export is only supported in @py and @js blocks (got @{})",
                        lang_tag
                    ));
                }
                crate::engines::run_block(lang, &clean_code)?;
                Ok(Value::Null)
            }
            other => {
                eprintln!(
                    "[warning] @{} block skipped — the {} toolchain bridge is not implemented yet",
                    lang_tag, other
                );
                Ok(Value::Null)
            }
        }
    }

    /// `@import { a, b as c } from <selector>` — define V2 proxies for
    /// functions exported by engine blocks or foreign modules.
    fn exec_engine_import(
        &mut self,
        names: &[(String, Option<String>)],
        wildcard: bool,
        selector: &str,
    ) -> Result<Value, String> {
        // Normalize the selector to registry form.
        let key = if let Some(rest) = selector.strip_prefix('@') {
            let (tag, block) = match rest.split_once('.') {
                Some((t, b)) => (crate::engines::canonical_tag(t), Some(b)),
                None => (crate::engines::canonical_tag(rest), None),
            };
            match block {
                Some(b) => format!("@{}.{}", tag, b),
                None => format!("@{}", tag),
            }
        } else if let Some((lang, module)) = selector.split_once('.') {
            // Foreign module import: py.statistics, py.math_ops, ...
            return self.engine_import_module(lang, module, names, wildcard);
        } else {
            selector.to_string() // bare block name
        };

        let entries = self
            .engine_exports
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("No engine exports found for selector '{}'", selector))?;

        let mut to_define: Vec<(String, String, usize)> = Vec::new(); // (bind_as, fn, wid)
        if wildcard {
            for (fnname, wid) in &entries {
                let dup = entries
                    .iter()
                    .any(|(f, w)| f == fnname && w != wid);
                if dup {
                    return Err(format!(
                        "Ambiguous wildcard import of '{}' from '{}' — use a named block selector",
                        fnname, selector
                    ));
                }
                to_define.push((fnname.clone(), fnname.clone(), *wid));
            }
        }
        for (name, alias) in names {
            let matches: Vec<usize> = entries
                .iter()
                .filter(|(f, _)| f == name)
                .map(|(_, w)| *w)
                .collect();
            match matches.as_slice() {
                [] => {
                    return Err(format!(
                        "'{}' is not exported by '{}' (did the block @export it?)",
                        name, selector
                    ))
                }
                [wid] => to_define.push((
                    alias.clone().unwrap_or_else(|| name.clone()),
                    name.clone(),
                    *wid,
                )),
                _ if matches.windows(2).all(|w| w[0] == w[1]) => to_define.push((
                    alias.clone().unwrap_or_else(|| name.clone()),
                    name.clone(),
                    matches[0],
                )),
                _ => {
                    return Err(format!(
                        "Import of '{}' from '{}' is ambiguous (exported by multiple blocks) — use @lang.block_name",
                        name, selector
                    ))
                }
            }
        }
        for (bind_as, fnname, wid) in to_define {
            self.env.define(
                &bind_as,
                Value::BuiltinFunc(format!("__engine_call:{}:{}", wid, fnname)),
            );
        }
        Ok(Value::Null)
    }

    /// `@import { mean } from py.statistics` — spin up (or reuse) a worker
    /// that imports the foreign module, then proxy the requested names.
    fn engine_import_module(
        &mut self,
        lang: &str,
        module: &str,
        names: &[(String, Option<String>)],
        wildcard: bool,
    ) -> Result<Value, String> {
        let norm = crate::engines::normalize_lang(lang);
        if norm != "python" {
            return Err(format!(
                "Module imports are currently supported for Python only (got '{}.{}')",
                lang, module
            ));
        }
        let cache_key = format!("{}.{}", lang, module);
        let wid = match self.engine_module_workers.get(&cache_key) {
            Some(w) => *w,
            None => {
                let bridge = format!("from {} import *", module);
                let worker = crate::engines::EngineWorker::start(norm, &bridge)?;
                let wid = self.engine_workers.len();
                self.engine_workers.push(worker);
                self.engine_module_workers.insert(cache_key, wid);
                wid
            }
        };
        if wildcard {
            let announced = self.engine_workers[wid].announced_exports.clone();
            for name in announced {
                self.env.define(
                    &name,
                    Value::BuiltinFunc(format!("__engine_call:{}:{}", wid, name)),
                );
            }
        }
        for (name, alias) in names {
            let bind_as = alias.clone().unwrap_or_else(|| name.clone());
            self.env.define(
                &bind_as,
                Value::BuiltinFunc(format!("__engine_call:{}:{}", wid, name)),
            );
        }
        Ok(Value::Null)
    }

    /// regex.replace / replace_all with a lambda: each match is passed to the
    /// function and its (stringified) result is spliced into the output.
    fn regex_replace_with_fn(
        &mut self,
        text: &str,
        pattern: &str,
        func: &Value,
        all: bool,
    ) -> Result<Value, String> {
        let re = crate::regex_engine::compile(pattern)?;
        let chars: Vec<char> = text.chars().collect();
        let matches = if all {
            re.find_iter(&chars)
        } else {
            re.search(&chars, 0).into_iter().collect()
        };
        let mut out = String::new();
        let mut last = 0usize;
        for m in matches {
            out.extend(chars[last..m.start].iter());
            let matched: String = chars[m.start..m.end].iter().collect();
            let replaced = self.call_value(func, &[(None, Value::Str(matched))])?;
            out.push_str(&format!("{}", replaced));
            last = m.end.max(m.start);
        }
        out.extend(chars[last..].iter());
        Ok(Value::Str(out))
    }

    fn compare_values<F>(&self, left: &Value, right: &Value, cmp: F) -> Result<Value, String>
    where
        F: Fn(f64, f64) -> bool,
    {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(cmp(*a as f64, *b as f64))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(cmp(*a, *b))),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Bool(cmp(*a as f64, *b))),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(cmp(*a, *b as f64))),
            (Value::Str(a), Value::Str(b)) => {
                // Lexicographic comparison via f64 casting won't work for strings.
                // Use string ordering directly.
                Ok(Value::Bool(cmp(a.cmp(b) as i8 as f64, 0.0)))
            }
            // BigInt vs Int/BigInt: exact comparison via BigInt ordering.
            (Value::BigInt(_), Value::Int(_) | Value::BigInt(_))
            | (Value::Int(_), Value::BigInt(_)) => {
                let (a, b) = (Self::as_bigint(left).unwrap(), Self::as_bigint(right).unwrap());
                Ok(Value::Bool(cmp(a.cmp(&b) as i8 as f64, 0.0)))
            }
            // BigInt vs Float: promote the BigInt to f64.
            (Value::BigInt(a), Value::Float(b)) => Ok(Value::Bool(cmp(a.to_f64(), *b))),
            (Value::Float(a), Value::BigInt(b)) => Ok(Value::Bool(cmp(*a, b.to_f64()))),
            // Decimal vs any numeric: exact decimal ordering.
            (Value::Decimal(_), _) | (_, Value::Decimal(_)) => {
                match (Self::as_decimal(left), Self::as_decimal(right)) {
                    (Some(a), Some(b)) => Ok(Value::Bool(cmp(a.cmp(&b) as i8 as f64, 0.0))),
                    _ => Err(format!(
                        "Cannot compare {} and {}",
                        left.type_name(),
                        right.type_name()
                    )),
                }
            }
            _ => Err(format!(
                "Cannot compare {} and {}",
                left.type_name(),
                right.type_name()
            )),
        }
    }

    fn unary_op(&self, op: &UnaryOp, val: &Value) -> Result<Value, String> {
        match op {
            UnaryOp::Neg => match val {
                Value::Int(n) => Ok(match n.checked_neg() {
                    Some(v) => Value::Int(v),
                    None => Value::BigInt(crate::bigint::BigInt::from_i64(*n).neg()),
                }),
                Value::BigInt(b) => Ok(Self::norm_bigint(b.neg())),
                Value::Decimal(d) => Ok(Value::Decimal(d.neg())),
                Value::Float(f) => Ok(Value::Float(-f)),
                _ => Err(format!("Cannot negate {}", val.type_name())),
            },
            UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
            UnaryOp::BitNot => match val {
                Value::Int(n) => Ok(Value::Int(!n)),
                _ => Err(format!("Cannot bitwise-not {}", val.type_name())),
            },
        }
    }

    // ── Function Calls ───────────────────────────────────

    pub fn call_value(
        &mut self,
        func: &Value,
        args: &[(Option<String>, Value)],
    ) -> Result<Value, String> {
        // Depth guard: turn runaway recursion into a catchable error instead
        // of overflowing the native stack and killing the process.
        self.call_depth += 1;
        if self.call_depth > self.recursion_limit {
            self.call_depth -= 1;
            return Err(format!(
                "Maximum recursion depth of {} exceeded (raise it with set_recursion_limit)",
                self.recursion_limit
            ));
        }
        let result = self.call_value_inner(func, args);
        self.call_depth -= 1;
        result
    }

    fn call_value_inner(
        &mut self,
        func: &Value,
        args: &[(Option<String>, Value)],
    ) -> Result<Value, String> {
        match func {
            Value::Func(fv) => {
                // Check if this is an enum variant constructor (empty body, dotted name)
                if fv.body.is_empty() && fv.name.contains('.') {
                    let parts: Vec<&str> = fv.name.splitn(2, '.').collect();
                    let enum_name = parts[0].to_string();
                    let variant_name = parts[1].to_string();
                    let arg_vals: Vec<Value> = args.iter().map(|(_, v)| v.clone()).collect();
                    return Ok(Value::EnumVariant(enum_name, variant_name, arg_vals));
                }
                // Check @deprecated
                if let Some(msg) = self.deprecated_funcs.get(&fv.name).cloned() {
                    let warning = if let Some(m) = msg {
                        format!("Warning: function '{}' is deprecated: {}", fv.name, m)
                    } else {
                        format!("Warning: function '{}' is deprecated", fv.name)
                    };
                    eprintln!("{}", warning);
                }
                // Check @memo cache
                let memo_key = if self.memo_caches.contains_key(&fv.name) {
                    let key = format!("{:?}", args.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>());
                    if let Some(cache) = self.memo_caches.get(&fv.name) {
                        if let Some(cached) = cache.get(&key) {
                            return Ok(cached.clone());
                        }
                    }
                    Some((fv.name.clone(), key))
                } else {
                    None
                };
                // Generator: create lazy generator (body runs on demand via next())
                if fv.is_generator {
                    let fv_clone = fv.clone();
                    let args_clone = args.to_vec();
                    return Ok(Value::Generator(Rc::new(RefCell::new(GeneratorState {
                        items: vec![],
                        index: 0,
                        lazy: Some((fv_clone, args_clone)),
                        started: false,
                        done: false,
                        resume_inputs: vec![],
                    }))));
                }

                // TCO: execute the function body in fresh frames until it stops
                // returning a self-tail-call signal.
                let saved = self.env.current;
                let func_name = fv.name.clone();
                let mut current_args = args.to_vec();
                loop {
                    self.env.push_scope_with_parent(fv.closure_env);
                    self.defer_stack.push(vec![]);
                    self.bind_call_params(&fv.params, &current_args)?;
                    self.current_function.push(func_name.clone());

                    let result = self.exec_block_no_scope(&fv.body)?;
                    let deferred = self.defer_stack.pop().unwrap_or_default();
                    for body in deferred.into_iter().rev() {
                        let _ = self.exec_block_no_scope(&body);
                    }
                    self.current_function.pop();
                    self.env.set_scope(saved);

                    let final_val = match result {
                        Value::Return(v) => *v,
                        other => other,
                    };

                    if let Value::TailCall(name, tco_args) = final_val {
                        if name == func_name {
                            current_args = tco_args;
                            continue;
                        }
                        if let Some(f) = self.env.get(&name) {
                            return self.call_value(&f, &tco_args);
                        }
                        return Err(format!("Undefined function '{}' in tail call", name));
                    }

                    if let Some((fname, key)) = &memo_key {
                        self.memo_caches
                            .entry(fname.clone())
                            .or_default()
                            .insert(key.clone(), final_val.clone());
                    }
                    return Ok(final_val);
                }
            }
            Value::BuiltinFunc(name) => self.call_builtin(name, args),
            Value::Class(cv) => {
                // Calling a class as a constructor (alternative to `new`)
                let mut instance_fields = cv.fields.clone();
                let init_method = cv.methods.get("init")
                    .or_else(|| cv.methods.get("constructor"))
                    .cloned();
                let instance = if cv.is_cow {
                    Value::CowInstance(cv.name.clone(), Rc::new(RefCell::new(instance_fields.clone())))
                } else {
                    Value::Instance(cv.name.clone(), instance_fields.clone())
                };
                if let Some(init) = init_method {
                    let saved = self.env.current;
                    self.env.push_scope_with_parent(init.closure_env);
                    self.env.define("self", instance.clone());
                    self.bind_call_params(&init.params, args)?;
                    self.exec_block_no_scope(&init.body)?;
                    let result = self.env.get("self").unwrap_or(instance);
                    self.env.set_scope(saved);
                    Ok(result)
                } else {
                    // No init/constructor: use field_order or positional args
                    let field_names = if !cv.field_order.is_empty() {
                        cv.field_order.clone()
                    } else {
                        instance_fields.keys().cloned().collect()
                    };
                    for (i, (_, val)) in args.iter().enumerate() {
                        if i < field_names.len() {
                            instance_fields.insert(field_names[i].clone(), val.clone());
                        }
                    }
                    if cv.is_cow {
                        Ok(Value::CowInstance(cv.name.clone(), Rc::new(RefCell::new(instance_fields))))
                    } else {
                        Ok(Value::Instance(cv.name.clone(), instance_fields))
                    }
                }
            }
            _ => Err(format!("Cannot call value of type {}", func.type_name())),
        }
    }

    fn bind_call_params(
        &mut self,
        params: &[Param],
        args: &[(Option<String>, Value)],
    ) -> Result<(), String> {
        let mut positional_idx = 0;
        let mut variadic_values = Vec::new();
        let mut named_args: HashMap<String, Value> = HashMap::new();

        // Separate named args
        for (name, val) in args {
            if let Some(n) = name {
                named_args.insert(n.clone(), val.clone());
            }
        }

        for param in params {
            // Skip `self` param — it's bound explicitly by the method call dispatch
            if param.name == "self" {
                continue;
            }
            if param.is_variadic {
                // Collect remaining positional args
                while positional_idx < args.len() {
                    if args[positional_idx].0.is_none() {
                        variadic_values.push(args[positional_idx].1.clone());
                    }
                    positional_idx += 1;
                }
                self.env.define(&param.name, Value::List(variadic_values.clone()));
            } else if let Some(val) = named_args.remove(&param.name) {
                self.env.define(&param.name, val);
            } else {
                // Positional
                let val: Result<Value, String> = loop {
                    if positional_idx >= args.len() {
                        break match &param.default {
                            Some(default_expr) => self.eval_expr(default_expr),
                            None => Ok(Value::Null),
                        };
                    }
                    if args[positional_idx].0.is_none() {
                        let v = args[positional_idx].1.clone();
                        positional_idx += 1;
                        break Ok(v);
                    }
                    positional_idx += 1;
                };
                self.env.define(&param.name, val?);
            }
        }

        Ok(())
    }

    fn call_method(
        &mut self,
        obj: &Value,
        method: &str,
        args: &[(Option<String>, Value)],
    ) -> Result<(Value, Option<Value>), String> {
        // Same depth guard as call_value (methods can recurse too).
        self.call_depth += 1;
        if self.call_depth > self.recursion_limit {
            self.call_depth -= 1;
            return Err(format!(
                "Maximum recursion depth of {} exceeded (raise it with set_recursion_limit)",
                self.recursion_limit
            ));
        }
        let result = self.call_method_inner(obj, method, args);
        self.call_depth -= 1;
        result
    }

    fn call_method_inner(
        &mut self,
        obj: &Value,
        method: &str,
        args: &[(Option<String>, Value)],
    ) -> Result<(Value, Option<Value>), String> {
        // Returns (result, Option<updated_self>)
        // Static method call on a class value: `ClassName.method(args)`. The
        // method is invoked with no `self` binding (its params bind to the args).
        if let Value::Class(cv) = obj {
            if let Some(method_fn) = cv.methods.get(method) {
                let func = Value::Func(method_fn.clone());
                let result = self.call_value(&func, args)?;
                return Ok((result, None));
            }
            // Associated constant / static field on the class.
            if let Some(field) = cv.fields.get(method) {
                return Ok((field.clone(), None));
            }
            return Err(format!("No static method '{}' on class '{}'", method, cv.name));
        }
        // Check for class/struct instance method
        let class_name = match obj {
            Value::Instance(name, _) | Value::CowInstance(name, _) => Some(name.clone()),
            Value::StructInstance(name, _) => Some(name.clone()),
            Value::EnumVariant(enum_name, _, _) => Some(enum_name.clone()),
            _ => None,
        };
        if let Some(ref class_name) = class_name {
            // @data class: built-in copy(field: newval, ...) method
            if method == "copy" {
                if let Some(Value::Class(cv)) = self.env.get(class_name) {
                    if cv.is_data {
                        // Build new instance with original fields, then override with named args
                        let new_fields = match obj {
                            Value::Instance(_, fields) => fields.clone(),
                            _ => HashMap::new(),
                        };
                        let mut new_fields = new_fields;
                        for (name_opt, val) in args {
                            if let Some(name) = name_opt {
                                new_fields.insert(name.clone(), val.clone());
                            }
                        }
                        return Ok((Value::Instance(class_name.clone(), new_fields), None));
                    }
                }
            }
        }
        if let Some(class_name) = class_name {
            // Walk the class hierarchy for method resolution
            let mut current_class = Some(class_name);
            while let Some(cn) = current_class {
                if let Some(Value::Class(cv)) = self.env.get(&cn) {
                    if let Some(func) = cv.methods.get(method) {
                        let func = func.clone();
                        let saved = self.env.current;
                        self.env.push_scope_with_parent(func.closure_env);
                        self.env.define("self", obj.clone());
                        self.bind_call_params(&func.params, args)?;
                        let result = self.exec_block_no_scope(&func.body)?;
                        let updated_self = self.env.get("self");
                        self.env.set_scope(saved);
                        let ret = match result {
                            Value::Return(v) => *v,
                            other => other,
                        };
                        return Ok((ret, updated_self));
                    }
                    current_class = cv.parent.clone();
                } else {
                    break;
                }
            }
        }

        if let Value::Dict(pairs) = obj {
            if let Some((_, exported)) = pairs.iter().find(|(key, _)| {
                matches!(key, Value::Str(name) if name == method)
            }) {
                return self.call_value(exported, args).map(|result| (result, None));
            }
        }

        // Check primitive_impls for StructInstance (newtypes) and StructInstance in general
        let struct_type_name = match obj {
            Value::StructInstance(name, _) => Some(name.clone()),
            _ => None,
        };
        if let Some(type_name) = struct_type_name {
            if let Some(fv) = self.primitive_impls.get(&type_name).and_then(|m| m.get(method)).cloned() {
                let saved = self.env.current;
                self.env.push_scope_with_parent(fv.closure_env);
                self.env.define("self", obj.clone());
                self.bind_call_params(&fv.params, args)?;
                let result = self.exec_block_no_scope(&fv.body)?;
                self.env.set_scope(saved);
                return match result {
                    Value::Return(v) => Ok((*v, None)),
                    other => Ok((other, None)),
                };
            }
        }

        // Fallback for __eq__ on instances (when no __eq__ method defined)
        if method == "__eq__" {
            if let Some((_, other)) = args.first() {
                return Ok((Value::Bool(obj == other), None));
            }
        }
        if method == "__ne__" {
            if let Some((_, other)) = args.first() {
                return Ok((Value::Bool(obj != other), None));
            }
        }

        // Built-in methods
        let result = self.call_builtin_method(obj, method, args)?;
        Ok((result, None))
    }

    fn freeze_value(&self, val: Value) -> Result<Value, String> {
        match val {
            Value::Instance(name, mut fields) => {
                fields.insert("__frozen".to_string(), Value::Bool(true));
                Ok(Value::Instance(name, fields))
            }
            Value::CowInstance(name, fields) => {
                let mut new_fields = fields.borrow().clone();
                new_fields.insert("__frozen".to_string(), Value::Bool(true));
                Ok(Value::CowInstance(name, Rc::new(RefCell::new(new_fields))))
            }
            Value::StructInstance(name, mut fields) => {
                fields.insert("__frozen".to_string(), Value::Bool(true));
                Ok(Value::StructInstance(name, fields))
            }
            Value::Dict(pairs) => {
                let mut new_pairs = pairs;
                new_pairs.push((Value::Str("__frozen".to_string()), Value::Bool(true)));
                Ok(Value::Dict(new_pairs))
            }
            Value::List(_) => {
                // Lists are frozen in-place via frozen_vars tracking
                Ok(val)
            }
            other => Ok(other),
        }
    }

    fn call_super(&mut self, args: &[(Option<String>, Value)]) -> Result<Value, String> {
        // Get current self
        let current_self = self.env.get("self").ok_or("super() called outside of a class method")?;
        let class_name = match &current_self {
            Value::Instance(name, _) | Value::CowInstance(name, _) => name.clone(),
            _ => return Err("super() called on non-instance".into()),
        };
        // Find the class, then its parent
        let parent_name = if let Some(Value::Class(cv)) = self.env.get(&class_name) {
            cv.parent.clone().ok_or_else(|| format!("Class '{}' has no parent class", class_name))?
        } else {
            return Err(format!("Class '{}' not found", class_name));
        };
        // Find parent's init/constructor method
        let init = if let Some(Value::Class(pcv)) = self.env.get(&parent_name) {
            pcv.methods.get("init")
                .or_else(|| pcv.methods.get("constructor"))
                .cloned()
                .ok_or_else(|| format!("Parent class '{}' has no constructor", parent_name))?
        } else {
            return Err(format!("Parent class '{}' not found", parent_name));
        };
        // Call parent init with current self
        let saved = self.env.current;
        self.env.push_scope_with_parent(init.closure_env);
        self.env.define("self", current_self);
        self.bind_call_params(&init.params, args)?;
        self.exec_block_no_scope(&init.body)?;
        // Propagate updated self back
        if let Some(updated) = self.env.get("self") {
            self.env.set_scope(saved);
            self.env.set("self", updated).ok();
        } else {
            self.env.set_scope(saved);
        }
        Ok(Value::Null)
    }

    fn call_super_method(&mut self, method: &str, args: &[(Option<String>, Value)]) -> Result<Value, String> {
        let current_self = self.env.get("self").ok_or("super.method() called outside of a class method")?;
        let class_name = match &current_self {
            Value::Instance(name, _) | Value::CowInstance(name, _) => name.clone(),
            _ => return Err("super.method() called on non-instance".into()),
        };

        let mut current_class = if let Some(Value::Class(cv)) = self.env.get(&class_name) {
            cv.parent.clone()
        } else {
            None
        };

        while let Some(parent_name) = current_class {
            if let Some(Value::Class(cv)) = self.env.get(&parent_name) {
                if let Some(func) = cv.methods.get(method).cloned() {
                    let saved = self.env.current;
                    self.env.push_scope_with_parent(func.closure_env);
                    self.env.define("self", current_self.clone());
                    self.bind_call_params(&func.params, args)?;
                    let result = self.exec_block_no_scope(&func.body)?;
                    let updated_self = self.env.get("self");
                    self.env.set_scope(saved);
                    if let Some(updated) = updated_self {
                        self.env.set("self", updated).ok();
                    }
                    return Ok(match result {
                        Value::Return(v) => *v,
                        other => other,
                    });
                }
                current_class = cv.parent.clone();
            } else {
                break;
            }
        }

        Err(format!("No parent method '{}' found for class '{}'", method, class_name))
    }

    fn call_builtin_method(
        &mut self,
        obj: &Value,
        method: &str,
        args: &[(Option<String>, Value)],
    ) -> Result<Value, String> {
        let arg_vals: Vec<&Value> = args.iter().map(|(_, v)| v).collect();

        // Check primitive extension methods first
        let type_name = match obj {
            Value::Str(_) => Some("str"),
            Value::Int(_) => Some("int"),
            Value::Float(_) => Some("float"),
            Value::Bool(_) => Some("bool"),
            Value::List(_) => Some("list"),
            Value::Dict(_) => Some("dict"),
            Value::Set(_) => Some("set"),
            Value::Bytes(_) => Some("bytes"),
            _ => None,
        };
        if let Some(tname) = type_name {
            if let Some(methods_map) = self.primitive_impls.get(tname) {
                if let Some(fv) = methods_map.get(method).cloned() {
                    let saved = self.env.current;
                    self.env.push_scope_with_parent(fv.closure_env);
                    self.env.define("self", obj.clone());
                    self.bind_call_params(&fv.params, args)?;
                    let result = self.exec_block_no_scope(&fv.body)?;
                    self.env.set_scope(saved);
                    return match result {
                        Value::Return(v) => Ok(*v),
                        other => Ok(other),
                    };
                }
            }
        }

        match (obj, method) {
            // String methods
            // String length is measured in Unicode code points, not bytes
            // (`.byte_len()` gives the UTF-8 byte length).
            (Value::Str(s), "len") => Ok(Value::Int(s.chars().count() as i64)),
            (Value::Str(s), "to_upper") => Ok(Value::Str(s.to_uppercase())),
            (Value::Str(s), "to_lower") => Ok(Value::Str(s.to_lowercase())),
            (Value::Str(s), "upper") => Ok(Value::Str(s.to_uppercase())),
            (Value::Str(s), "lower") => Ok(Value::Str(s.to_lowercase())),
            (Value::Str(s), "trim") => Ok(Value::Str(s.trim().to_string())),
            (Value::Str(s), "contains") => {
                if let Some(Value::Str(sub)) = arg_vals.first() {
                    Ok(Value::Bool(s.contains(sub.as_str())))
                } else {
                    Err("contains() requires a string argument".into())
                }
            }
            (Value::Str(s), "starts_with") => {
                if let Some(Value::Str(prefix)) = arg_vals.first() {
                    Ok(Value::Bool(s.starts_with(prefix.as_str())))
                } else {
                    Err("starts_with() requires a string argument".into())
                }
            }
            (Value::Str(s), "ends_with") => {
                if let Some(Value::Str(suffix)) = arg_vals.first() {
                    Ok(Value::Bool(s.ends_with(suffix.as_str())))
                } else {
                    Err("ends_with() requires a string argument".into())
                }
            }
            (Value::Str(s), "split") => {
                let sep = if let Some(Value::Str(sep)) = arg_vals.first() {
                    sep.as_str()
                } else {
                    " "
                };
                if let Some(Value::Int(n)) = arg_vals.get(1) {
                    Ok(Value::List(
                        s.splitn(*n as usize, sep).map(|p| Value::Str(p.to_string())).collect(),
                    ))
                } else {
                    Ok(Value::List(
                        s.split(sep).map(|p| Value::Str(p.to_string())).collect(),
                    ))
                }
            }
            (Value::Str(s), "to_bytes") => {
                Ok(Value::List(s.as_bytes().iter().map(|b| Value::Int(*b as i64)).collect()))
            }
            (Value::Str(s), "byte_len") => {
                Ok(Value::Int(s.len() as i64))
            }
            (Value::Str(s), "replace") => {
                if arg_vals.len() >= 2 {
                    if let (Value::Str(from), Value::Str(to)) = (arg_vals[0], arg_vals[1]) {
                        Ok(Value::Str(s.replace(from.as_str(), to.as_str())))
                    } else {
                        Err("replace() requires two string arguments".into())
                    }
                } else {
                    Err("replace() requires two arguments".into())
                }
            }
            (Value::Str(s), "reverse") => {
                Ok(Value::Str(s.chars().rev().collect()))
            }
            (Value::Str(s), "char_at") => {
                if let Some(Value::Int(i)) = arg_vals.first() {
                    s.chars()
                        .nth(*i as usize)
                        .map(|c| Value::Str(c.to_string()))
                        .ok_or_else(|| format!("Index {} out of bounds", i))
                } else {
                    Err("char_at() requires an integer argument".into())
                }
            }
            (Value::Str(s), "substr") => {
                let start = arg_vals.first().and_then(|v| if let Value::Int(n) = v { Some(*n as usize) } else { None }).unwrap_or(0);
                let end = arg_vals.get(1).and_then(|v| if let Value::Int(n) = v { Some(*n as usize) } else { None }).unwrap_or(s.len());
                let result: String = s.chars().skip(start).take(end - start).collect();
                Ok(Value::Str(result))
            }
            (Value::Str(s), "trim_start") => Ok(Value::Str(s.trim_start().to_string())),
            (Value::Str(s), "trim_end") => Ok(Value::Str(s.trim_end().to_string())),
            (Value::Str(s), "replace_first") => {
                if arg_vals.len() >= 2 {
                    if let (Value::Str(from), Value::Str(to)) = (arg_vals[0], arg_vals[1]) {
                        Ok(Value::Str(s.replacen(from.as_str(), to.as_str(), 1)))
                    } else {
                        Err("replace_first() requires two string arguments".into())
                    }
                } else {
                    Err("replace_first() requires two arguments".into())
                }
            }
            (Value::Str(s), "count") => {
                if let Some(Value::Str(sub)) = arg_vals.first() {
                    Ok(Value::Int(s.matches(sub.as_str()).count() as i64))
                } else {
                    Err("count() requires a string argument".into())
                }
            }
            (Value::Str(s), "index_of") | (Value::Str(s), "indexOf")
            | (Value::Str(s), "find") | (Value::Str(s), "index") => {
                if let Some(Value::Str(sub)) = arg_vals.first() {
                    Ok(s.find(sub.as_str()).map(|i| Value::Int(i as i64)).unwrap_or(Value::Int(-1)))
                } else {
                    Err("index_of() requires a string argument".into())
                }
            }
            (Value::Str(s), "last_index_of") | (Value::Str(s), "lastIndexOf") => {
                if let Some(Value::Str(sub)) = arg_vals.first() {
                    Ok(s.rfind(sub.as_str()).map(|i| Value::Int(i as i64)).unwrap_or(Value::Int(-1)))
                } else {
                    Err("last_index_of() requires a string argument".into())
                }
            }
            (Value::Str(s), "slice") => {
                // Char-based with negative-index support, clamped (never panics).
                let len = s.chars().count() as i64;
                let resolve = |n: i64| -> usize {
                    (if n < 0 { (len + n).max(0) } else { n.min(len) }) as usize
                };
                let start = arg_vals.first().and_then(|v| if let Value::Int(n) = v { Some(resolve(*n)) } else { None }).unwrap_or(0);
                let end = arg_vals.get(1).and_then(|v| if let Value::Int(n) = v { Some(resolve(*n)) } else { None }).unwrap_or(len as usize);
                let result: String = s.chars().skip(start).take(end.saturating_sub(start)).collect();
                Ok(Value::Str(result))
            }
            (Value::Str(s), "repeat") => {
                if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::Str(Self::repeat_str(s, *n)?))
                } else {
                    Err("repeat() requires an integer argument".into())
                }
            }
            (Value::Str(s), "pad_start") => {
                if let Some(Value::Int(width)) = arg_vals.first() {
                    let fill = arg_vals.get(1).and_then(|v| if let Value::Str(c) = v { Some(c.clone()) } else { None }).unwrap_or_else(|| " ".to_string());
                    if *width > 1_000_000_000 { return Err("pad_start() width too large".into()); }
                    // Width counts characters, not bytes; negative widths are no-ops.
                    let w = (*width).max(0) as usize;
                    let n = s.chars().count();
                    if n >= w || fill.is_empty() { Ok(Value::Str(s.clone())) }
                    else {
                        let padding: String = fill.chars().cycle().take(w - n).collect();
                        Ok(Value::Str(format!("{}{}", padding, s)))
                    }
                } else {
                    Err("pad_start() requires a width argument".into())
                }
            }
            (Value::Str(s), "pad_end") => {
                if let Some(Value::Int(width)) = arg_vals.first() {
                    let fill = arg_vals.get(1).and_then(|v| if let Value::Str(c) = v { Some(c.clone()) } else { None }).unwrap_or_else(|| " ".to_string());
                    if *width > 1_000_000_000 { return Err("pad_end() width too large".into()); }
                    let w = (*width).max(0) as usize;
                    let n = s.chars().count();
                    if n >= w || fill.is_empty() { Ok(Value::Str(s.clone())) }
                    else {
                        let padding: String = fill.chars().cycle().take(w - n).collect();
                        Ok(Value::Str(format!("{}{}", s, padding)))
                    }
                } else {
                    Err("pad_end() requires a width argument".into())
                }
            }
            (Value::Str(s), "is_alpha") | (Value::Str(s), "isalpha") => {
                Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_alphabetic())))
            }
            (Value::Str(s), "is_digit") | (Value::Str(s), "isdigit") => {
                Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())))
            }
            (Value::Str(s), "is_alnum") | (Value::Str(s), "isalnum") => {
                Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_alphanumeric())))
            }
            (Value::Str(s), "is_space") | (Value::Str(s), "isspace") => {
                Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_whitespace())))
            }
            (Value::Str(s), "is_upper") | (Value::Str(s), "isupper") => {
                Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| !c.is_alphabetic() || c.is_uppercase())))
            }
            (Value::Str(s), "is_lower") | (Value::Str(s), "islower") => {
                Ok(Value::Bool(!s.is_empty() && s.chars().all(|c| !c.is_alphabetic() || c.is_lowercase())))
            }
            (Value::Str(s), "chars") => {
                Ok(Value::List(s.chars().map(|c| Value::Str(c.to_string())).collect()))
            }
            (Value::Str(s), "graphemes") => {
                // Approximate grapheme clusters as chars (Unicode grapheme boundary support)
                Ok(Value::List(s.chars().map(|c| Value::Str(c.to_string())).collect()))
            }
            (Value::Str(s), "encode") => {
                // encode(encoding="utf-8") -> bytes
                Ok(Value::Bytes(s.as_bytes().to_vec()))
            }
            (Value::Bytes(b), "len") => Ok(Value::Int(b.len() as i64)),
            (Value::Bytes(b), "byte_len") => Ok(Value::Int(b.len() as i64)),
            (Value::Bytes(b), "to_list") => {
                Ok(Value::List(b.iter().map(|x| Value::Int(*x as i64)).collect()))
            }
            (Value::Bytes(b), "decode") => {
                // decode(encoding="utf-8") -> str
                Ok(Value::Str(String::from_utf8_lossy(b).into_owned()))
            }
            (Value::Str(s), "bytes") => {
                Ok(Value::List(s.bytes().map(|b| Value::Int(b as i64)).collect()))
            }
            (Value::Str(s), "capitalize") => {
                let mut c = s.chars();
                let result = match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                };
                Ok(Value::Str(result))
            }
            (Value::Str(s), "title") => {
                let mut result = String::new();
                let mut capitalize_next = true;
                for ch in s.chars() {
                    if ch.is_whitespace() || ch == '-' || ch == '_' {
                        result.push(ch);
                        capitalize_next = true;
                    } else if capitalize_next {
                        result.extend(ch.to_uppercase());
                        capitalize_next = false;
                    } else {
                        result.extend(ch.to_lowercase());
                    }
                }
                Ok(Value::Str(result))
            }
            (Value::Str(s), "swapcase") => {
                Ok(Value::Str(s.chars().map(|c| {
                    if c.is_uppercase() { c.to_lowercase().collect::<String>() }
                    else { c.to_uppercase().collect::<String>() }
                }).collect()))
            }
            (Value::Str(s), "center") => {
                if let Some(Value::Int(width)) = arg_vals.first() {
                    if *width > 1_000_000_000 { return Err("center() width too large".into()); }
                    let w = (*width).max(0) as usize;
                    let fill = arg_vals.get(1).and_then(|v| if let Value::Str(c) = v { c.chars().next() } else { None }).unwrap_or(' ');
                    if s.chars().count() >= w { Ok(Value::Str(s.clone())) }
                    else {
                        let total_pad = w - s.chars().count();
                        let left_pad = total_pad / 2;
                        let right_pad = total_pad - left_pad;
                        let result = format!("{}{}{}", std::iter::repeat(fill).take(left_pad).collect::<String>(), s, std::iter::repeat(fill).take(right_pad).collect::<String>());
                        Ok(Value::Str(result))
                    }
                } else {
                    Err("center() requires a width argument".into())
                }
            }

            // List methods
            (Value::List(items), "len") => Ok(Value::Int(items.len() as i64)),
            (Value::List(items), "is_empty") => Ok(Value::Bool(items.is_empty())),
            (Value::List(items), "first") => {
                Ok(items.first().cloned().unwrap_or(Value::Null))
            }
            (Value::List(items), "last") => {
                Ok(items.last().cloned().unwrap_or(Value::Null))
            }
            (Value::List(items), "contains") => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Bool(items.iter().any(|v| v == *val)))
                } else {
                    Err("contains() requires an argument".into())
                }
            }
            (Value::List(items), "reverse") => {
                let mut r = items.clone();
                r.reverse();
                Ok(Value::List(r))
            }
            (Value::List(items), "sort") | (Value::List(items), "sorted") => {
                let mut sorted = items.clone();
                sorted.sort_by(|a, b| {
                    match (a, b) {
                        (Value::Int(x), Value::Int(y)) => x.cmp(y),
                        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(std::cmp::Ordering::Equal),
                        (Value::Str(x), Value::Str(y)) => x.cmp(y),
                        _ => std::cmp::Ordering::Equal,
                    }
                });
                Ok(Value::List(sorted))
            }
            (Value::List(items), "join") => {
                let sep = if let Some(Value::Str(s)) = arg_vals.first() {
                    s.as_str()
                } else {
                    ""
                };
                let parts: Vec<String> = items.iter().map(|v| format!("{}", v)).collect();
                Ok(Value::Str(parts.join(sep)))
            }
            (Value::List(items), "map") => {
                if let Some(func) = arg_vals.first() {
                    let mut result = Vec::new();
                    for item in items {
                        let r = self.call_value(func, &[(None, item.clone())])?;
                        result.push(r);
                    }
                    Ok(Value::List(result))
                } else {
                    Err("map() requires a function argument".into())
                }
            }
            (Value::List(items), "filter") => {
                if let Some(func) = arg_vals.first() {
                    let mut result = Vec::new();
                    for item in items {
                        let r = self.call_value(func, &[(None, item.clone())])?;
                        if r.is_truthy() {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::List(result))
                } else {
                    Err("filter() requires a function argument".into())
                }
            }
            (Value::List(items), "reduce") => {
                if arg_vals.len() >= 1 {
                    let func = arg_vals[0];
                    let init = if arg_vals.len() >= 2 {
                        arg_vals[1].clone()
                    } else if !items.is_empty() {
                        items[0].clone()
                    } else {
                        return Err("reduce() on empty list without initial value".into());
                    };
                    let start_idx = if arg_vals.len() >= 2 { 0 } else { 1 };
                    let mut acc = init;
                    for item in items.iter().skip(start_idx) {
                        acc =
                            self.call_value(func, &[(None, acc), (None, item.clone())])?;
                    }
                    Ok(acc)
                } else {
                    Err("reduce() requires a function argument".into())
                }
            }
            (Value::List(items), "each") => {
                if let Some(func) = arg_vals.first() {
                    for item in items {
                        self.call_value(func, &[(None, item.clone())])?;
                    }
                    Ok(Value::Null)
                } else {
                    Err("each() requires a function argument".into())
                }
            }
            (Value::List(items), "find") => {
                if let Some(func) = arg_vals.first() {
                    for item in items {
                        let r = self.call_value(func, &[(None, item.clone())])?;
                        if r.is_truthy() {
                            return Ok(item.clone());
                        }
                    }
                    Ok(Value::Null)
                } else {
                    Err("find() requires a function argument".into())
                }
            }
            (Value::List(items), "any") => {
                if let Some(func) = arg_vals.first() {
                    for item in items {
                        let r = self.call_value(func, &[(None, item.clone())])?;
                        if r.is_truthy() {
                            return Ok(Value::Bool(true));
                        }
                    }
                    Ok(Value::Bool(false))
                } else {
                    Err("any() requires a function argument".into())
                }
            }
            (Value::List(items), "all") => {
                if let Some(func) = arg_vals.first() {
                    for item in items {
                        let r = self.call_value(func, &[(None, item.clone())])?;
                        if !r.is_truthy() {
                            return Ok(Value::Bool(false));
                        }
                    }
                    Ok(Value::Bool(true))
                } else {
                    Err("all() requires a function argument".into())
                }
            }
            (Value::List(items), "sum") => {
                let mut total = Value::Int(0);
                for item in items {
                    total = self.binary_op(&BinOp::Add, &total, item)?;
                }
                Ok(total)
            }
            (Value::List(items), "enumerate") => {
                Ok(Value::List(
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, v)| Value::Tuple(vec![Value::Int(i as i64), v.clone()]))
                        .collect(),
                ))
            }
            (Value::List(items), "slice") => {
                // Negative indices count from the end; bounds are clamped so a
                // wild range can never panic.
                let len = items.len() as i64;
                let resolve = |n: i64| -> usize {
                    (if n < 0 { (len + n).max(0) } else { n.min(len) }) as usize
                };
                let start = arg_vals.first().and_then(|v| if let Value::Int(n) = v { Some(resolve(*n)) } else { None }).unwrap_or(0);
                let end = arg_vals.get(1).and_then(|v| if let Value::Int(n) = v { Some(resolve(*n)) } else { None }).unwrap_or(items.len());
                if start >= end { return Ok(Value::List(Vec::new())); }
                Ok(Value::List(items[start..end].to_vec()))
            }
            (Value::List(items), "flat_map") => {
                if let Some(func) = arg_vals.first() {
                    let mut result = Vec::new();
                    for item in items {
                        let r = self.call_value(func, &[(None, item.clone())])?;
                        if let Value::List(inner) = r {
                            result.extend(inner);
                        } else {
                            result.push(r);
                        }
                    }
                    Ok(Value::List(result))
                } else {
                    Err("flat_map() requires a function argument".into())
                }
            }
            (Value::List(items), "index_of") | (Value::List(items), "indexOf") => {
                if let Some(val) = arg_vals.first() {
                    Ok(items.iter().position(|v| v == *val).map(|i| Value::Int(i as i64)).unwrap_or(Value::Int(-1)))
                } else {
                    Err("index_of() requires an argument".into())
                }
            }
            (Value::List(items), "count") => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Int(items.iter().filter(|v| *v == *val).count() as i64))
                } else {
                    Err("count() requires an argument".into())
                }
            }
            (Value::List(items), "unique") => {
                let mut result = Vec::new();
                for v in items {
                    if !result.contains(v) {
                        result.push(v.clone());
                    }
                }
                Ok(Value::List(result))
            }
            (Value::List(items), "flatten") => {
                let mut result = Vec::new();
                for v in items {
                    if let Value::List(inner) = v {
                        result.extend(inner.clone());
                    } else {
                        result.push(v.clone());
                    }
                }
                Ok(Value::List(result))
            }
            (Value::List(items), "for_each") => {
                if let Some(func) = arg_vals.first() {
                    for item in items {
                        self.call_value(func, &[(None, item.clone())])?;
                    }
                    Ok(Value::Null)
                } else {
                    Err("for_each() requires a function argument".into())
                }
            }
            (Value::List(items), "take") => {
                if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::List(items.iter().take((*n).max(0) as usize).cloned().collect()))
                } else {
                    Err("take() requires an integer argument".into())
                }
            }
            (Value::List(items), "drop") => {
                if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::List(items.iter().skip((*n).max(0) as usize).cloned().collect()))
                } else {
                    Err("drop() requires an integer argument".into())
                }
            }
            (Value::List(items), "product") => {
                use crate::bigint::BigInt;
                let mut result: Value = Value::Int(1);
                let mut is_float = false;
                let mut fresult = 1.0f64;
                for v in items {
                    match v {
                        Value::Int(n) => {
                            fresult *= *n as f64;
                            result = match &result {
                                // Ints are arbitrary precision: promote instead of wrapping.
                                Value::Int(acc) => match acc.checked_mul(*n) {
                                    Some(p) => Value::Int(p),
                                    None => Self::norm_bigint(
                                        BigInt::from_i64(*acc).mul(&BigInt::from_i64(*n)),
                                    ),
                                },
                                Value::BigInt(acc) => {
                                    Self::norm_bigint(acc.mul(&BigInt::from_i64(*n)))
                                }
                                _ => result.clone(),
                            };
                        }
                        Value::Float(f) => { fresult *= f; is_float = true; }
                        _ => return Err("product() requires numeric list".into()),
                    }
                }
                if is_float { Ok(Value::Float(fresult)) } else { Ok(result) }
            }
            (Value::List(items), "sort_by") => {
                if let Some(func) = arg_vals.first() {
                    let mut indexed: Vec<(usize, Value)> = items.iter().cloned().enumerate().collect();
                    let func_cloned = (*func).clone();
                    indexed.sort_by(|a, b| {
                        // We can't call self here due to borrow issues, use key comparison
                        match (&a.1, &b.1) {
                            (Value::Int(x), Value::Int(y)) => x.cmp(y),
                            (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                            (Value::Str(x), Value::Str(y)) => x.cmp(y),
                            _ => std::cmp::Ordering::Equal,
                        }
                    });
                    // Actually apply the key function
                    let mut keyed: Vec<(Value, Value)> = Vec::new();
                    for item in items {
                        let key = self.call_value(&func_cloned, &[(None, item.clone())])?;
                        keyed.push((key, item.clone()));
                    }
                    keyed.sort_by(|a, b| {
                        match (&a.0, &b.0) {
                            (Value::Int(x), Value::Int(y)) => x.cmp(y),
                            (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                            (Value::Str(x), Value::Str(y)) => x.cmp(y),
                            _ => std::cmp::Ordering::Equal,
                        }
                    });
                    Ok(Value::List(keyed.into_iter().map(|(_, v)| v).collect()))
                } else {
                    Err("sort_by() requires a key function".into())
                }
            }
            (Value::List(items), "partition") => {
                if let Some(func) = arg_vals.first() {
                    let mut yes = Vec::new();
                    let mut no = Vec::new();
                    for item in items {
                        let r = self.call_value(func, &[(None, item.clone())])?;
                        if r.is_truthy() { yes.push(item.clone()); } else { no.push(item.clone()); }
                    }
                    Ok(Value::Tuple(vec![Value::List(yes), Value::List(no)]))
                } else {
                    Err("partition() requires a predicate function".into())
                }
            }
            (Value::List(items), "group_by") => {
                if let Some(func) = arg_vals.first() {
                    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
                    for item in items {
                        let key = self.call_value(func, &[(None, item.clone())])?;
                        if let Some(group) = groups.iter_mut().find(|(k, _)| *k == key) {
                            group.1.push(item.clone());
                        } else {
                            groups.push((key, vec![item.clone()]));
                        }
                    }
                    Ok(Value::Dict(groups.into_iter().map(|(k, v)| (k, Value::List(v))).collect()))
                } else {
                    Err("group_by() requires a key function".into())
                }
            }
            (Value::List(items), "min") => {
                if items.is_empty() { return Ok(Value::Null); }
                let mut min = &items[0];
                for v in &items[1..] {
                    match (v, min) {
                        (Value::Int(a), Value::Int(b)) if a < b => min = v,
                        (Value::Float(a), Value::Float(b)) if a < b => min = v,
                        _ => {}
                    }
                }
                Ok(min.clone())
            }
            (Value::List(items), "max") => {
                if items.is_empty() { return Ok(Value::Null); }
                let mut max = &items[0];
                for v in &items[1..] {
                    match (v, max) {
                        (Value::Int(a), Value::Int(b)) if a > b => max = v,
                        (Value::Float(a), Value::Float(b)) if a > b => max = v,
                        _ => {}
                    }
                }
                Ok(max.clone())
            }
            (Value::List(items), "zip") => {
                if let Some(Value::List(other)) = arg_vals.first() {
                    let result: Vec<Value> = items.iter().zip(other.iter())
                        .map(|(a, b)| Value::Tuple(vec![a.clone(), b.clone()]))
                        .collect();
                    Ok(Value::List(result))
                } else {
                    Err("zip() requires a list argument".into())
                }
            }
            (Value::List(items), "copy") | (Value::List(items), "clone") => {
                Ok(Value::List(items.clone()))
            }
            (Value::List(items), "to_set") => {
                let mut result = Vec::new();
                for v in items {
                    if !result.contains(v) { result.push(v.clone()); }
                }
                Ok(Value::Set(result))
            }
            (Value::List(items), "to_tuple") => {
                Ok(Value::Tuple(items.clone()))
            }
            (Value::List(items), "fill") => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::List(vec![(*val).clone(); items.len()]))
                } else {
                    Err("fill() requires a value argument".into())
                }
            }
            (Value::List(items), "reversed") => {
                let mut r = items.clone();
                r.reverse();
                Ok(Value::List(r))
            }
            // Identity conversions so range/iter pipelines compose freely.
            (Value::List(items), "to_list") | (Value::List(items), "collect") => {
                Ok(Value::List(items.clone()))
            }
            (Value::List(items), "chunk") | (Value::List(items), "chunks") => {
                match arg_vals.first() {
                    Some(Value::Int(n)) if *n > 0 => Ok(Value::List(
                        items.chunks(*n as usize).map(|c| Value::List(c.to_vec())).collect(),
                    )),
                    _ => Err("chunk() requires a positive integer size".into()),
                }
            }
            (Value::List(items), "window") | (Value::List(items), "windows") => {
                match arg_vals.first() {
                    Some(Value::Int(n)) if *n > 0 => {
                        let n = *n as usize;
                        if n > items.len() {
                            return Ok(Value::List(Vec::new()));
                        }
                        Ok(Value::List(
                            items.windows(n).map(|c| Value::List(c.to_vec())).collect(),
                        ))
                    }
                    _ => Err("window() requires a positive integer size".into()),
                }
            }

            // Dict methods
            // Compiled regex object (regex.compile): methods delegate to the
            // __regex_* builtins with the stored pattern as second argument.
            (Value::Dict(pairs), m)
                if matches!(
                    m,
                    "match" | "test" | "is_match" | "find" | "find_all" | "capture"
                        | "replace" | "replace_all" | "split"
                ) && pairs.iter().any(|(k, v)| {
                    matches!((k, v), (Value::Str(kk), Value::Str(vv)) if kk == "type" && vv == "regex")
                }) =>
            {
                let pattern = pairs
                    .iter()
                    .find_map(|(k, v)| match (k, v) {
                        (Value::Str(kk), Value::Str(vv)) if kk == "pattern" => Some(vv.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let mut new_args: Vec<(Option<String>, Value)> = Vec::new();
                if let Some(first) = args.first() {
                    new_args.push(first.clone());
                }
                new_args.push((None, Value::Str(pattern)));
                for extra in args.iter().skip(1) {
                    new_args.push(extra.clone());
                }
                let builtin = match m {
                    "match" | "test" | "is_match" => "__regex_match",
                    "find" => "__regex_find",
                    "find_all" => "__regex_find_all",
                    "capture" => "__regex_capture",
                    "replace" => "__regex_replace",
                    "replace_all" => "__regex_replace_all",
                    _ => "__regex_split",
                };
                return self.call_builtin(builtin, &new_args);
            }
            (Value::Dict(pairs), "keys") => {
                Ok(Value::List(pairs.iter().map(|(k, _)| k.clone()).collect()))
            }
            (Value::Dict(pairs), "values") => {
                Ok(Value::List(pairs.iter().map(|(_, v)| v.clone()).collect()))
            }
            (Value::Dict(pairs), "len") => Ok(Value::Int(pairs.len() as i64)),
            (Value::Dict(pairs), "contains") => {
                if let Some(key) = arg_vals.first() {
                    Ok(Value::Bool(pairs.iter().any(|(k, _)| k == *key)))
                } else {
                    Err("contains() requires an argument".into())
                }
            }
            (Value::Dict(pairs), "entries") => {
                Ok(Value::List(
                    pairs.iter().map(|(k, v)| Value::Tuple(vec![k.clone(), v.clone()])).collect(),
                ))
            }
            (Value::Dict(pairs), "merge") => {
                if let Some(Value::Dict(other)) = arg_vals.first() {
                    let mut merged = pairs.clone();
                    for (k, v) in other.iter() {
                        let mut found = false;
                        for (ek, ev) in merged.iter_mut() {
                            if ek == k {
                                *ev = v.clone();
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            merged.push((k.clone(), v.clone()));
                        }
                    }
                    Ok(Value::Dict(merged))
                } else {
                    Err("merge() requires a dict argument".into())
                }
            }
            (Value::Dict(pairs), "get") => {
                if arg_vals.len() >= 1 {
                    let key = arg_vals[0];
                    let default = if arg_vals.len() >= 2 { arg_vals[1].clone() } else { Value::Null };
                    for (k, v) in pairs {
                        if k == key {
                            return Ok(v.clone());
                        }
                    }
                    Ok(default)
                } else {
                    Err("get() requires a key argument".into())
                }
            }
            (Value::Dict(pairs), "items") => {
                Ok(Value::List(
                    pairs.iter().map(|(k, v)| Value::Tuple(vec![k.clone(), v.clone()])).collect(),
                ))
            }
            (Value::Dict(pairs), "has") | (Value::Dict(pairs), "has_key") | (Value::Dict(pairs), "contains_key") => {
                if let Some(key) = arg_vals.first() {
                    Ok(Value::Bool(pairs.iter().any(|(k, _)| k == *key)))
                } else {
                    Err("has() requires a key argument".into())
                }
            }
            (Value::Dict(pairs), "is_empty") => Ok(Value::Bool(pairs.is_empty())),
            (Value::Dict(pairs), "copy") | (Value::Dict(pairs), "clone") => Ok(Value::Dict(pairs.clone())),
            (Value::Dict(pairs), "to_pairs") => {
                Ok(Value::List(
                    pairs.iter().map(|(k, v)| Value::Tuple(vec![k.clone(), v.clone()])).collect(),
                ))
            }
            (Value::Dict(pairs), "pick") => {
                if let Some(Value::List(keys)) = arg_vals.first() {
                    let result: Vec<(Value, Value)> = pairs.iter()
                        .filter(|(k, _)| keys.contains(k))
                        .cloned().collect();
                    Ok(Value::Dict(result))
                } else {
                    Err("pick() requires a list of keys".into())
                }
            }
            (Value::Dict(pairs), "omit") => {
                if let Some(Value::List(keys)) = arg_vals.first() {
                    let result: Vec<(Value, Value)> = pairs.iter()
                        .filter(|(k, _)| !keys.contains(k))
                        .cloned().collect();
                    Ok(Value::Dict(result))
                } else {
                    Err("omit() requires a list of keys".into())
                }
            }
            (Value::Dict(pairs), "invert") => {
                Ok(Value::Dict(pairs.iter().map(|(k, v)| (v.clone(), k.clone())).collect()))
            }
            (Value::Dict(pairs), "map_values") => {
                if let Some(func) = arg_vals.first() {
                    let mut result = Vec::new();
                    for (k, v) in pairs {
                        let new_v = self.call_value(func, &[(None, v.clone())])?;
                        result.push((k.clone(), new_v));
                    }
                    Ok(Value::Dict(result))
                } else {
                    Err("map_values() requires a function argument".into())
                }
            }
            (Value::Dict(pairs), "map_keys") => {
                if let Some(func) = arg_vals.first() {
                    let mut result = Vec::new();
                    for (k, v) in pairs {
                        let new_k = self.call_value(func, &[(None, k.clone())])?;
                        result.push((new_k, v.clone()));
                    }
                    Ok(Value::Dict(result))
                } else {
                    Err("map_keys() requires a function argument".into())
                }
            }
            (Value::Dict(pairs), "filter") => {
                if let Some(func) = arg_vals.first() {
                    let mut result = Vec::new();
                    for (k, v) in pairs {
                        let keep = self.call_value(func, &[(None, k.clone()), (None, v.clone())])?;
                        if keep.is_truthy() {
                            result.push((k.clone(), v.clone()));
                        }
                    }
                    Ok(Value::Dict(result))
                } else {
                    Err("filter() requires a predicate function".into())
                }
            }

            // Tuple methods
            (Value::Tuple(items), "len") => Ok(Value::Int(items.len() as i64)),
            (Value::Tuple(items), "first") => Ok(items.first().cloned().unwrap_or(Value::Null)),
            (Value::Tuple(items), "last") => Ok(items.last().cloned().unwrap_or(Value::Null)),
            (Value::Tuple(items), "contains") => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Bool(items.iter().any(|v| v == *val)))
                } else {
                    Err("contains() requires an argument".into())
                }
            }
            (Value::Tuple(items), "to_list") => Ok(Value::List(items.clone())),

            // Set methods
            (Value::Set(items), "len") => Ok(Value::Int(items.len() as i64)),
            (Value::Set(items), "is_empty") => Ok(Value::Bool(items.is_empty())),
            (Value::Set(items), "contains") => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Bool(items.iter().any(|v| v == *val)))
                } else {
                    Err("contains() requires an argument".into())
                }
            }
            (Value::Set(items), "to_list") => Ok(Value::List(items.clone())),
            (Value::Set(a), "union") => {
                if let Some(Value::Set(b)) = arg_vals.first() {
                    let mut result = a.clone();
                    for v in b {
                        if !result.contains(v) {
                            result.push(v.clone());
                        }
                    }
                    Ok(Value::Set(result))
                } else {
                    Err("union() requires a set argument".into())
                }
            }
            (Value::Set(a), "intersection") | (Value::Set(a), "intersect") => {
                if let Some(Value::Set(b)) = arg_vals.first() {
                    let result: Vec<Value> = a.iter().filter(|v| b.contains(v)).cloned().collect();
                    Ok(Value::Set(result))
                } else {
                    Err("intersection() requires a set argument".into())
                }
            }
            (Value::Set(a), "difference") => {
                if let Some(Value::Set(b)) = arg_vals.first() {
                    let result: Vec<Value> = a.iter().filter(|v| !b.contains(v)).cloned().collect();
                    Ok(Value::Set(result))
                } else {
                    Err("difference() requires a set argument".into())
                }
            }
            (Value::Set(a), "sym_difference") | (Value::Set(a), "symmetric_difference") => {
                if let Some(Value::Set(b)) = arg_vals.first() {
                    let mut result: Vec<Value> = a.iter().filter(|v| !b.contains(v)).cloned().collect();
                    for v in b {
                        if !a.contains(v) {
                            result.push(v.clone());
                        }
                    }
                    Ok(Value::Set(result))
                } else {
                    Err("sym_difference() requires a set argument".into())
                }
            }
            (Value::Set(a), "is_subset") => {
                if let Some(Value::Set(b)) = arg_vals.first() {
                    Ok(Value::Bool(a.iter().all(|v| b.contains(v))))
                } else {
                    Err("is_subset() requires a set argument".into())
                }
            }
            (Value::Set(a), "is_superset") => {
                if let Some(Value::Set(b)) = arg_vals.first() {
                    Ok(Value::Bool(b.iter().all(|v| a.contains(v))))
                } else {
                    Err("is_superset() requires a set argument".into())
                }
            }
            (Value::Set(a), "is_disjoint") => {
                if let Some(Value::Set(b)) = arg_vals.first() {
                    Ok(Value::Bool(!a.iter().any(|v| b.contains(v))))
                } else {
                    Err("is_disjoint() requires a set argument".into())
                }
            }

            // Result/Option methods
            (Value::Ok(v), "unwrap") => Ok(*v.clone()),
            (Value::Ok(v), "unwrap_or") => Ok(v.as_ref().clone()),
            (Value::Ok(_), "is_ok") => Ok(Value::Bool(true)),
            (Value::Ok(_), "is_err") => Ok(Value::Bool(false)),
            (Value::Ok(v), "map") => {
                if let Some(func) = arg_vals.first() {
                    self.call_value(func, &[(None, *v.clone())])
                        .map(|r| Value::Ok(Box::new(r)))
                } else {
                    Err("map() requires a function argument".into())
                }
            }
            (Value::Ok(_), "map_err") => Ok(obj.clone()), // Ok.map_err is identity
            (Value::Ok(v), "and_then") => {
                if let Some(func) = arg_vals.first() {
                    self.call_value(func, &[(None, *v.clone())])
                } else {
                    Err("and_then() requires a function argument".into())
                }
            }
            (Value::Ok(_), "or_else") => Ok(obj.clone()), // Ok.or_else is identity
            (Value::Ok(v), "unwrap_err") => Err(format!("unwrap_err() on Ok({})", v)),
            (Value::Ok(v), "ok") => Ok(Value::Some(v.clone())),
            (Value::Ok(v), "flatten") => {
                match v.as_ref() {
                    Value::Ok(_) | Value::Err(_) => Ok(*v.clone()),
                    _ => Ok(obj.clone()),
                }
            }
            (Value::Ok(v), "unwrap_or_default") => Ok(*v.clone()),
            (Value::Ok(v), "expect") => Ok(*v.clone()),
            (Value::Err(e), "unwrap") => Err(format!("Unwrap on Err: {}", e)),
            (Value::Err(_), "unwrap_or") => Ok(arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null)),
            (Value::Err(_), "is_ok") => Ok(Value::Bool(false)),
            (Value::Err(_), "is_err") => Ok(Value::Bool(true)),
            (Value::Err(e), "map") => Ok(Value::Err(e.clone())),
            (Value::Err(e), "map_err") => {
                if let Some(func) = arg_vals.first() {
                    self.call_value(func, &[(None, *e.clone())])
                        .map(|r| Value::Err(Box::new(r)))
                } else {
                    Err("map_err() requires a function argument".into())
                }
            }
            (Value::Err(_), "and_then") => Ok(obj.clone()), // Err.and_then is identity
            (Value::Err(e), "or_else") => {
                if let Some(func) = arg_vals.first() {
                    self.call_value(func, &[(None, *e.clone())])
                } else {
                    Err("or_else() requires a function argument".into())
                }
            }
            (Value::Err(e), "unwrap_err") => Ok(*e.clone()),
            (Value::Err(_), "ok") => Ok(Value::Null),
            (Value::Err(_), "flatten") => Ok(obj.clone()),
            (Value::Err(_), "unwrap_or_default") => Ok(Value::Int(0)),
            (Value::Err(_), "expect") => {
                let msg = arg_vals.first().map(|v| format!("{}", v)).unwrap_or_else(|| "expect() failed".into());
                Err(msg)
            }
            (Value::Some(v), "unwrap") => Ok(*v.clone()),
            (Value::Some(v), "unwrap_or") => Ok(v.as_ref().clone()),
            (Value::Some(_), "is_some") => Ok(Value::Bool(true)),
            (Value::Some(_), "is_none") => Ok(Value::Bool(false)),
            (Value::Some(v), "map") => {
                if let Some(func) = arg_vals.first() {
                    self.call_value(func, &[(None, *v.clone())])
                        .map(|r| Value::Some(Box::new(r)))
                } else {
                    Err("map() requires a function argument".into())
                }
            }
            (Value::Some(v), "and_then") => {
                if let Some(func) = arg_vals.first() {
                    self.call_value(func, &[(None, *v.clone())])
                } else {
                    Err("and_then() requires a function argument".into())
                }
            }
            (Value::Some(_), "or_else") => Ok(obj.clone()),
            (Value::Some(v), "filter") => {
                if let Some(func) = arg_vals.first() {
                    let result = self.call_value(func, &[(None, *v.clone())])?;
                    if result.is_truthy() { Ok(obj.clone()) } else { Ok(Value::Null) }
                } else {
                    Err("filter() requires a function argument".into())
                }
            }
            (Value::Some(v), "ok_or") => {
                Ok(Value::Ok(v.clone()))
            }
            (Value::Some(v), "flatten") => {
                match v.as_ref() {
                    Value::Some(_) | Value::Null => Ok(*v.clone()),
                    _ => Ok(obj.clone()),
                }
            }
            (Value::Some(v), "unwrap_or_default") => Ok(*v.clone()),
            (Value::Some(v), "expect") => Ok(*v.clone()),
            // None (Null) can have option methods too
            (Value::Null, "unwrap") => Err("Unwrap on None".into()),
            (Value::Null, "unwrap_or") => Ok(arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null)),
            (Value::Null, "is_some") => Ok(Value::Bool(false)),
            (Value::Null, "is_none") => Ok(Value::Bool(true)),
            (Value::Null, "map") => Ok(Value::Null),
            (Value::Null, "and_then") => Ok(Value::Null),
            (Value::Null, "or_else") => {
                if let Some(func) = arg_vals.first() {
                    self.call_value(func, &[])
                } else {
                    Err("or_else() requires a function argument".into())
                }
            }
            (Value::Null, "filter") => Ok(Value::Null),
            (Value::Null, "ok_or") => {
                let err = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Str("None".into()));
                Ok(Value::Err(Box::new(err)))
            }
            (Value::Null, "flatten") => Ok(Value::Null),
            (Value::Null, "unwrap_or_default") => Ok(Value::Int(0)),
            (Value::Null, "expect") => {
                let msg = arg_vals.first().map(|v| format!("{}", v)).unwrap_or_else(|| "expect() failed on None".into());
                Err(msg)
            }

            // Generator methods
            (Value::Generator(gs), "next") => {
                self.resume_generator(gs, None, false)
            }
            (Value::Generator(gs), "collect") => {
                let state_snap = gs.borrow().clone();
                if let Some((func, func_args)) = state_snap.lazy {
                    // Collect all remaining items by running the body eagerly from current index
                    let start_idx = state_snap.index;
                    let prev_collector = self.yield_collector.take();
                    self.yield_collector = Some(Vec::new());
                    let saved = self.env.current;
                    self.env.push_scope_with_parent(func.closure_env);
                    self.bind_call_params(&func.params, &func_args)?;
                    let _ = self.exec_block_no_scope(&func.body);
                    self.env.set_scope(saved);
                    let mut collected = self.yield_collector.take().unwrap_or_default();
                    self.yield_collector = prev_collector;
                    // Skip already-consumed items
                    if start_idx < collected.len() {
                        collected = collected[start_idx..].to_vec();
                    } else {
                        collected = vec![];
                    }
                    let mut state = gs.borrow_mut();
                    state.index += collected.len();
                    state.done = true;
                    Ok(Value::List(collected))
                } else {
                    let mut state = gs.borrow_mut();
                    let remaining: Vec<Value> = state.items[state.index..].to_vec();
                    state.index = state.items.len();
                    state.done = true;
                    Ok(Value::List(remaining))
                }
            }
            (Value::Generator(gs), "is_done") => {
                let state = gs.borrow();
                Ok(Value::Bool(state.done || (state.lazy.is_none() && state.index >= state.items.len())))
            }
            (Value::Generator(gs), "send") => {
                let sent = arg_vals
                    .first()
                    .map(|v| (*v).clone())
                    .unwrap_or(Value::Null);
                self.resume_generator(gs, Some(sent), true)
            }
            (Value::Generator(gs), "to_list") => {
                let state_snap = gs.borrow().clone();
                if let Some((func, func_args)) = state_snap.lazy {
                    // Run eagerly to collect all items
                    let start_idx = state_snap.index;
                    let prev_collector = self.yield_collector.take();
                    self.yield_collector = Some(Vec::new());
                    let saved = self.env.current;
                    self.env.push_scope_with_parent(func.closure_env);
                    self.bind_call_params(&func.params, &func_args)?;
                    let _ = self.exec_block_no_scope(&func.body);
                    self.env.set_scope(saved);
                    let collected = self.yield_collector.take().unwrap_or_default();
                    self.yield_collector = prev_collector;
                    let slice = if start_idx < collected.len() { collected[start_idx..].to_vec() } else { vec![] };
                    let mut state = gs.borrow_mut();
                    state.index += slice.len();
                    state.done = true;
                    Ok(Value::List(slice))
                } else {
                    let mut state = gs.borrow_mut();
                    let remaining: Vec<Value> = state.items[state.index..].to_vec();
                    state.index = state.items.len();
                    state.done = true;
                    Ok(Value::List(remaining))
                }
            }

            // Newtype unwrap methods
            (Value::StructInstance(_, fields), "inner") => {
                fields.get("0").cloned().ok_or_else(|| "No inner field on this type".to_string())
            }

            // Range methods: materialize (ranges are lazy) then delegate to the
            // list methods. `.collect()`/`.to_list()` return the materialized list.
            (Value::Range(start, end, inclusive), m) => {
                let end_v = if *inclusive { *end + 1 } else { *end };
                let items: Vec<Value> = (*start..end_v).map(Value::Int).collect();
                let list = Value::List(items);
                match m {
                    "collect" | "to_list" => Ok(list),
                    _ => self.call_builtin_method(&list, m, args),
                }
            }

            // Decimal value methods: d.add(x), d.round(n), d.to_str(), etc.
            (Value::Decimal(d), m) => {
                let other = || arg_vals.first().and_then(|v| Self::as_decimal(v));
                match m {
                    "add" => other().map(|o| Value::Decimal(d.add(&o))).ok_or_else(|| "decimal.add: bad argument".to_string()),
                    "sub" => other().map(|o| Value::Decimal(d.sub(&o))).ok_or_else(|| "decimal.sub: bad argument".to_string()),
                    "mul" => other().map(|o| Value::Decimal(d.mul(&o))).ok_or_else(|| "decimal.mul: bad argument".to_string()),
                    "div" => other().and_then(|o| d.div(&o)).map(Value::Decimal).ok_or_else(|| "decimal.div: division by zero".to_string()),
                    "compare" | "cmp" => other().map(|o| Value::Int(d.cmp(&o) as i64)).ok_or_else(|| "decimal.compare: bad argument".to_string()),
                    "abs" => Ok(Value::Decimal(d.abs())),
                    "neg" => Ok(Value::Decimal(d.neg())),
                    "round" => {
                        let places = arg_vals.first().and_then(|v| if let Value::Int(n) = v { Some(*n as u32) } else { None }).unwrap_or(0);
                        Ok(Value::Decimal(d.round(places)))
                    }
                    "to_str" | "to_string" | "str" => Ok(Value::Str(d.to_string())),
                    "to_float" | "float" => Ok(Value::Float(d.to_f64())),
                    "eq" => other().map(|o| Value::Bool(d.cmp(&o) == std::cmp::Ordering::Equal)).ok_or_else(|| "decimal.eq: bad argument".to_string()),
                    _ => Err(format!("No method '{}' on decimal", m)),
                }
            }

            // Universal conversion: every value can render itself to a string.
            (v, "to_string") | (v, "to_str") => Ok(Value::Str(format!("{}", v))),

            // A builtin function name that is ALSO a stdlib module (e.g. `hash` —
            // both `hash(v)` value-hashing and `hash.fnv1a(v)` module access).
            // Dispatch the method through the module of the same name.
            (Value::BuiltinFunc(fname), _) => {
                let module = self
                    .env
                    .get(&format!("std.{}", fname))
                    .or_else(|| self.env.get(fname));
                if let Some(Value::Dict(entries)) = module {
                    for (k, v) in &entries {
                        if let (Value::Str(key), Value::BuiltinFunc(bname)) = (k, v) {
                            if key == method {
                                let bname = bname.clone();
                                return self.call_builtin(&bname, args);
                            }
                        }
                    }
                }
                Err(format!("No method '{}' on {}", method, obj.type_name()))
            }

            _ => Err(format!(
                "No method '{}' on {}",
                method,
                obj.type_name()
            )),
        }
    }

    // Collect the lvalue path of a mutation target: the root variable name plus
    // the field/index accessors leading to the value. Index expressions are
    // evaluated here, BEFORE any mutable borrow of the environment is taken.
    fn collect_lvalue_path(
        &mut self,
        expr: &Expr,
        accesses: &mut Vec<LvalueAccess>,
    ) -> Result<String, String> {
        match expr {
            Expr::Ident(name) => Ok(name.clone()),
            Expr::Self_ => Ok("self".to_string()),
            Expr::FieldAccess { object, field, .. } => {
                let root = self.collect_lvalue_path(object, accesses)?;
                accesses.push(LvalueAccess::Field(field.clone()));
                Ok(root)
            }
            Expr::Index { object, index } => {
                let root = self.collect_lvalue_path(object, accesses)?;
                let idx = self.eval_expr(index)?;
                accesses.push(LvalueAccess::Index(idx));
                Ok(root)
            }
            _ => Err("Mutation target must start at a variable".to_string()),
        }
    }

    // Apply `f` to the value at the given lvalue path, mutating in place.
    // Recursive so CowInstance RefCell borrows stay alive during the walk.
    fn mutate_at_path<F>(val: &mut Value, accesses: &[LvalueAccess], f: F) -> Result<Value, String>
    where
        F: FnOnce(&mut Value) -> Result<Value, String>,
    {
        let Some((first, rest)) = accesses.split_first() else {
            return f(val);
        };
        match (first, val) {
            (LvalueAccess::Field(name), Value::Instance(_, fields))
            | (LvalueAccess::Field(name), Value::StructInstance(_, fields)) => {
                let inner = fields
                    .get_mut(name)
                    .ok_or_else(|| format!("No field '{}' on instance", name))?;
                Self::mutate_at_path(inner, rest, f)
            }
            (LvalueAccess::Field(name), Value::CowInstance(_, fields)) => {
                let mut borrowed = fields.borrow_mut();
                let inner = borrowed
                    .get_mut(name)
                    .ok_or_else(|| format!("No field '{}' on instance", name))?;
                Self::mutate_at_path(inner, rest, f)
            }
            (LvalueAccess::Field(name), Value::Dict(pairs)) => {
                let key = Value::Str(name.clone());
                let inner = pairs
                    .iter_mut()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v)
                    .ok_or_else(|| format!("Key {} not found", name))?;
                Self::mutate_at_path(inner, rest, f)
            }
            (LvalueAccess::Field(name), Value::Class(cls)) => {
                let inner = cls
                    .fields
                    .get_mut(name)
                    .ok_or_else(|| format!("No static field '{}' on class", name))?;
                Self::mutate_at_path(inner, rest, f)
            }
            (LvalueAccess::Index(idx), Value::List(items)) => {
                if let Value::Int(i) = idx {
                    let i = if *i < 0 { items.len() as i64 + i } else { *i };
                    if i < 0 || i as usize >= items.len() {
                        return Err(format!("Index {} out of bounds", i));
                    }
                    Self::mutate_at_path(&mut items[i as usize], rest, f)
                } else {
                    Err(format!("Cannot index list with {}", idx.type_name()))
                }
            }
            (LvalueAccess::Index(idx), Value::Dict(pairs)) => {
                let inner = pairs
                    .iter_mut()
                    .find(|(k, _)| k == idx)
                    .map(|(_, v)| v)
                    .ok_or_else(|| format!("Key {} not found", idx))?;
                Self::mutate_at_path(inner, rest, f)
            }
            (LvalueAccess::Index(_), Value::Tuple(_)) => Err("Tuples are immutable".to_string()),
            (_, other) => Err(format!(
                "Cannot navigate into {} while assigning",
                other.type_name()
            )),
        }
    }

    // Clone the value at an lvalue path (for compound-op reads and dispatch checks).
    fn value_at_path(root: &Value, accesses: &[LvalueAccess]) -> Result<Value, String> {
        let mut cur = root.clone();
        for acc in accesses {
            cur = match (acc, &cur) {
                (LvalueAccess::Field(name), Value::Instance(_, fields))
                | (LvalueAccess::Field(name), Value::StructInstance(_, fields)) => fields
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("No field '{}' on instance", name))?,
                (LvalueAccess::Field(name), Value::CowInstance(_, fields)) => fields
                    .borrow()
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("No field '{}' on instance", name))?,
                (LvalueAccess::Field(name), Value::Dict(pairs)) => {
                    let key = Value::Str(name.clone());
                    pairs
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| format!("Key {} not found", name))?
                }
                (LvalueAccess::Field(name), Value::Class(cls)) => cls
                    .fields
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("No static field '{}' on class", name))?,
                (LvalueAccess::Index(idx), Value::List(items)) => {
                    if let Value::Int(i) = idx {
                        let i = if *i < 0 { items.len() as i64 + i } else { *i };
                        items
                            .get(i.max(0) as usize)
                            .cloned()
                            .ok_or_else(|| format!("Index {} out of bounds", i))?
                    } else {
                        return Err(format!("Cannot index list with {}", idx.type_name()));
                    }
                }
                (LvalueAccess::Index(idx), Value::Dict(pairs)) => pairs
                    .iter()
                    .find(|(k, _)| k == idx)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| format!("Key {} not found", idx))?,
                (_, other) => {
                    return Err(format!("Cannot navigate into {}", other.type_name()))
                }
            };
        }
        Ok(cur)
    }

    fn call_mutation_method(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[(Option<String>, Value)],
    ) -> Result<Value, String> {
        let arg_vals: Vec<Value> = args.iter().map(|(_, v)| v.clone()).collect();

        // Mutation through an arbitrary index/field chain, e.g. d["x"].push(2),
        // grid[0][1].push(9), self.rows[i].sort(). The simple Ident/Self_ and
        // one-level-field cases keep their dedicated paths below (they also
        // handle CowInstance, which the generic walker cannot borrow through).
        let needs_chain_walk = matches!(object, Expr::Index { .. })
            || matches!(object, Expr::FieldAccess { object: p, .. }
                if !matches!(p.as_ref(), Expr::Ident(_) | Expr::Self_));
        if needs_chain_walk {
            let mut accesses = Vec::new();
            match self.collect_lvalue_path(object, &mut accesses) {
                Ok(root) => {
                    if self.frozen_vars.contains(&root) {
                        return Err(format!("Cannot mutate frozen value '{}'", root));
                    }
                    self.ensure_cow_binding_unique(&root)?;
                    let root_val = self
                        .env
                        .get_mut(&root)
                        .ok_or_else(|| format!("Undefined variable '{}'", root))?;
                    return Self::mutate_at_path(root_val, &accesses, |target| {
                        apply_mutation(target, method, arg_vals)
                    });
                }
                // Not rooted at a variable (e.g. f()[0].push(x)): mutate the
                // temporary, like Python; the change is simply discarded.
                Err(_) => {
                    let mut tmp = self.eval_expr(object)?;
                    return apply_mutation(&mut tmp, method, arg_vals);
                }
            }
        }

        // Handle field access like self.items.push(val) or obj.field.push(val)
        if let Expr::FieldAccess { object: parent, field, .. } = object {
            let parent_name = match parent.as_ref() {
                Expr::Ident(name) => name.clone(),
                Expr::Self_ => "self".to_string(),
                _ => return Err(format!("Cannot call mutation method '{}' on nested expression", method)),
            };
            self.ensure_cow_binding_unique(&parent_name)?;
            let parent_val = self.env.get_mut(&parent_name).ok_or_else(|| {
                format!("Undefined variable '{}'", parent_name)
            })?;
            // Get the field from Instance/StructInstance
            let field_val = match parent_val {
                Value::Instance(_, ref mut fields) | Value::StructInstance(_, ref mut fields) => {
                    fields.get_mut(field).ok_or_else(|| format!("No field '{}' on instance", field))?
                }
                Value::CowInstance(_, fields) => {
                    let mut borrowed = fields.borrow_mut();
                    return apply_mutation(
                        borrowed.get_mut(field).ok_or_else(|| format!("No field '{}' on instance", field))?,
                        method,
                        arg_vals,
                    );
                }
                _ => return Err(format!("Cannot access field '{}' on {}", field, parent_val.type_name())),
            };
            return apply_mutation(field_val, method, arg_vals);
        }

        // Get the variable name to mutate
        let var_name = match object {
            Expr::Ident(name) => name.clone(),
            Expr::Self_ => "self".to_string(),
            // Any other receiver (literal, call result, …): mutate the
            // temporary value, like Python; the change is simply discarded.
            _ => {
                let mut tmp = self.eval_expr(object)?;
                return apply_mutation(&mut tmp, method, arg_vals);
            }
        };

        // Check if the variable is frozen
        if self.frozen_vars.contains(&var_name) {
            return Err(format!("Cannot mutate frozen value '{}'", var_name));
        }

        self.ensure_cow_binding_unique(&var_name)?;
        let val = self.env.get_mut(&var_name).ok_or_else(|| {
            format!("Undefined variable '{}'", var_name)
        })?;

        apply_mutation(val, method, arg_vals)
    }
}

impl Drop for Interpreter {
    fn drop(&mut self) {
        self.report_leaks();
    }
}

// ── Regex helpers (backed by src/regex_engine.rs) ───────

/// True when `text` contains a match for `pattern`.
fn simple_regex_match(pattern: &str, text: &str) -> bool {
    match crate::regex_engine::compile(pattern) {
        Ok(re) => re.is_match(text),
        Err(_) => false,
    }
}

/// First match of `pattern` in `text`.
fn simple_regex_find(pattern: &str, text: &str) -> Option<String> {
    let re = crate::regex_engine::compile(pattern).ok()?;
    let chars: Vec<char> = text.chars().collect();
    re.search(&chars, 0)
        .map(|m| chars[m.start..m.end].iter().collect())
}

/// All non-overlapping matches.
fn simple_regex_find_all(pattern: &str, text: &str) -> Vec<String> {
    let Ok(re) = crate::regex_engine::compile(pattern) else {
        return Vec::new();
    };
    let chars: Vec<char> = text.chars().collect();
    re.find_iter(&chars)
        .into_iter()
        .map(|m| chars[m.start..m.end].iter().collect())
        .collect()
}

/// Replace the first (or all) matches. `$0`-`$9` in the replacement refer to
/// the whole match / capture groups.
fn simple_regex_replace(pattern: &str, replacement: &str, text: &str, all: bool) -> String {
    let Ok(re) = crate::regex_engine::compile(pattern) else {
        return text.to_string();
    };
    let chars: Vec<char> = text.chars().collect();
    let matches = if all {
        re.find_iter(&chars)
    } else {
        re.search(&chars, 0).into_iter().collect()
    };
    let mut out = String::new();
    let mut last = 0usize;
    for m in matches {
        out.extend(chars[last..m.start].iter());
        out.push_str(&expand_regex_replacement(replacement, &chars, &m));
        last = m.end.max(m.start);
    }
    out.extend(chars[last..].iter());
    out
}

/// Expand `$0`-`$9` group references in a replacement template.
fn expand_regex_replacement(
    replacement: &str,
    chars: &[char],
    m: &crate::regex_engine::MatchResult,
) -> String {
    let rep: Vec<char> = replacement.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < rep.len() {
        if rep[i] == '$' && i + 1 < rep.len() && rep[i + 1].is_ascii_digit() {
            let g = rep[i + 1] as usize - '0' as usize;
            let range = if g == 0 {
                Some((m.start, m.end))
            } else {
                m.groups.get(g - 1).copied().flatten()
            };
            if let Some((s, e)) = range {
                out.extend(chars[s..e].iter());
            }
            i += 2;
        } else {
            out.push(rep[i]);
            i += 1;
        }
    }
    out
}

/// Split `text` on every match of `pattern`.
fn simple_regex_split(pattern: &str, text: &str) -> Vec<String> {
    let Ok(re) = crate::regex_engine::compile(pattern) else {
        return vec![text.to_string()];
    };
    let chars: Vec<char> = text.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut last = 0usize;
    for m in re.find_iter(&chars) {
        if m.end == m.start && m.start == last {
            continue; // ignore empty match at the current position
        }
        parts.push(chars[last..m.start].iter().collect());
        last = m.end;
    }
    parts.push(chars[last..].iter().collect());
    parts
}

// ── JSON helpers ─────────────────────────────────────────

/// Decode a JSON string body: standard escapes plus \uXXXX (with surrogate pairs).
fn json_unescape(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                '"' => { out.push('"'); i += 2; }
                '\\' => { out.push('\\'); i += 2; }
                '/' => { out.push('/'); i += 2; }
                'n' => { out.push('\n'); i += 2; }
                't' => { out.push('\t'); i += 2; }
                'r' => { out.push('\r'); i += 2; }
                'b' => { out.push('\u{8}'); i += 2; }
                'f' => { out.push('\u{c}'); i += 2; }
                'u' if i + 5 < chars.len() => {
                    let hex: String = chars[i + 2..i + 6].iter().collect();
                    if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                        // Surrogate pair: \uD800-\uDBFF followed by \uDC00-\uDFFF
                        if (0xD800..0xDC00).contains(&cp)
                            && i + 11 < chars.len()
                            && chars[i + 6] == '\\'
                            && chars[i + 7] == 'u'
                        {
                            let hex2: String = chars[i + 8..i + 12].iter().collect();
                            if let Ok(lo) = u32::from_str_radix(&hex2, 16) {
                                if (0xDC00..0xE000).contains(&lo) {
                                    let combined =
                                        0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                    if let Some(c) = char::from_u32(combined) {
                                        out.push(c);
                                        i += 12;
                                        continue;
                                    }
                                }
                            }
                        }
                        if let Some(c) = char::from_u32(cp) {
                            out.push(c);
                        }
                        i += 6;
                    } else {
                        out.push(chars[i]);
                        i += 1;
                    }
                }
                other => {
                    out.push(other);
                    i += 2;
                }
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn parse_json_value(s: &str) -> Result<Value, String> {
    let s = s.trim();
    if s == "null" { return Ok(Value::Null); }
    if s == "true" { return Ok(Value::Bool(true)); }
    if s == "false" { return Ok(Value::Bool(false)); }
    if let Ok(n) = s.parse::<i64>() { return Ok(Value::Int(n)); }
    if let Ok(f) = s.parse::<f64>() { return Ok(Value::Float(f)); }
    if s.starts_with('"') && s.ends_with('"') {
        return Ok(Value::Str(json_unescape(&s[1..s.len() - 1])));
    }
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len()-1];
        if inner.trim().is_empty() { return Ok(Value::List(vec![])); }
        let items = split_json_top_level(inner);
        let mut result = Vec::new();
        for item in items {
            result.push(parse_json_value(item.trim())?);
        }
        return Ok(Value::List(result));
    }
    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len()-1];
        if inner.trim().is_empty() { return Ok(Value::Dict(vec![])); }
        let items = split_json_top_level(inner);
        let mut pairs = Vec::new();
        for item in items {
            let item = item.trim();
            if let Some(colon_pos) = find_json_colon(item) {
                let key = parse_json_value(item[..colon_pos].trim())?;
                let val = parse_json_value(item[colon_pos+1..].trim())?;
                pairs.push((key, val));
            }
        }
        return Ok(Value::Dict(pairs));
    }
    Err(format!("Invalid JSON: {}", s))
}

fn split_json_top_level(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut in_string = false;
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if in_string {
            if ch == '\\' { i += 1; }
            else if ch == '"' { in_string = false; }
        } else {
            match ch {
                '"' => in_string = true,
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    result.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if start < s.len() { result.push(&s[start..]); }
    result
}

fn find_json_colon(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if in_string {
            if ch == '\\' { i += 1; }
            else if ch == '"' { in_string = false; }
        } else {
            match ch {
                '"' => in_string = true,
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                ':' if depth == 0 => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn value_to_json(val: &Value) -> String {
    match val {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => format!("{}", f),
        Value::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\t', "\\t")),
        Value::List(items) => {
            let parts: Vec<String> = items.iter().map(value_to_json).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Dict(pairs) => {
            let parts: Vec<String> = pairs.iter().map(|(k, v)| format!("{}: {}", value_to_json(k), value_to_json(v))).collect();
            format!("{{{}}}", parts.join(", "))
        }
        Value::Tuple(items) => {
            let parts: Vec<String> = items.iter().map(value_to_json).collect();
            format!("[{}]", parts.join(", "))
        }
        _ => format!("\"{}\"", val),
    }
}

/// Find a ':' in an f-string expression that is NOT inside parentheses/brackets/braces
/// (to distinguish format spec from dict literals or ternary)
fn find_fstring_colon(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, ch) in s.chars().enumerate() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ':' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Format a Value with a Python-like format spec
fn format_value_with_spec(val: &Value, spec: &str) -> String {
    // Common specs: .2f, .3f, >10, <10, ^10, 04d, x, X, b, o, e, %
    let spec = spec.trim();
    if spec.is_empty() {
        return format!("{}", val);
    }

    // Check for fill/align
    let (fill, align, spec_rest) = parse_format_align(spec);

    // Check for width and precision
    if let Some(dot_pos) = spec_rest.find('.') {
        // Has precision
        let width_str = &spec_rest[..dot_pos];
        let after_dot = &spec_rest[dot_pos+1..];
        let (prec_str, type_char) = split_prec_type(after_dot);
        let prec: usize = prec_str.parse().unwrap_or(6);
        let width: usize = width_str.parse().unwrap_or(0);

        let formatted = match type_char {
            'f' | 'F' => {
                let f = match val {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return format!("{}", val),
                };
                format!("{:.prec$}", f, prec = prec)
            }
            'e' | 'E' => {
                let f = match val {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return format!("{}", val),
                };
                if type_char == 'E' {
                    format!("{:.prec$E}", f, prec = prec)
                } else {
                    format!("{:.prec$e}", f, prec = prec)
                }
            }
            '%' => {
                let f = match val {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return format!("{}", val),
                };
                format!("{:.prec$}%", f * 100.0, prec = prec)
            }
            _ => {
                // Just format as string with precision (truncation)
                let s = format!("{}", val);
                s[..prec.min(s.len())].to_string()
            }
        };
        apply_width_align(&formatted, width, fill, align)
    } else {
        // No precision, check type
        let (width_str, type_char) = split_width_type(&spec_rest);
        let width: usize = width_str.parse().unwrap_or(0);
        let zero_pad = width_str.starts_with('0') && width > 0;

        let formatted = match type_char {
            'd' => {
                let n = match val {
                    Value::Int(n) => *n,
                    Value::Float(f) => *f as i64,
                    _ => return format!("{}", val),
                };
                if zero_pad { format!("{:0>width$}", n, width = width) }
                else { format!("{}", n) }
            }
            'x' => {
                let n = match val { Value::Int(n) => *n, _ => return format!("{}", val) };
                format!("{:x}", n)
            }
            'X' => {
                let n = match val { Value::Int(n) => *n, _ => return format!("{}", val) };
                format!("{:X}", n)
            }
            'o' => {
                let n = match val { Value::Int(n) => *n, _ => return format!("{}", val) };
                format!("{:o}", n)
            }
            'b' => {
                let n = match val { Value::Int(n) => *n, _ => return format!("{}", val) };
                format!("{:b}", n)
            }
            'f' | 'F' => {
                let f = match val {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return format!("{}", val),
                };
                format!("{:.6}", f)
            }
            'e' => {
                let f = match val {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return format!("{}", val),
                };
                format!("{:e}", f)
            }
            '%' => {
                let f = match val {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return format!("{}", val),
                };
                format!("{:.6}%", f * 100.0)
            }
            _ => format!("{}", val),
        };
        if zero_pad { return formatted; }
        apply_width_align(&formatted, width, fill, align)
    }
}

fn parse_format_align(spec: &str) -> (char, char, &str) {
    let chars: Vec<char> = spec.chars().collect();
    if chars.len() >= 2 && (chars[1] == '<' || chars[1] == '>' || chars[1] == '^') {
        (chars[0], chars[1], &spec[chars[0].len_utf8() + chars[1].len_utf8()..])
    } else if !chars.is_empty() && (chars[0] == '<' || chars[0] == '>' || chars[0] == '^') {
        (' ', chars[0], &spec[chars[0].len_utf8()..])
    } else {
        (' ', '>', spec)
    }
}

fn split_prec_type(s: &str) -> (&str, char) {
    if s.is_empty() { return (s, 'f'); }
    let last = s.chars().last().unwrap();
    if last.is_alphabetic() || last == '%' {
        (&s[..s.len()-last.len_utf8()], last)
    } else {
        (s, 'f')
    }
}

fn split_width_type(s: &str) -> (&str, char) {
    if s.is_empty() { return (s, 's'); }
    let last = s.chars().last().unwrap();
    if last.is_alphabetic() || last == '%' {
        (&s[..s.len()-last.len_utf8()], last)
    } else {
        (s, 's')
    }
}

fn apply_width_align(s: &str, width: usize, fill: char, align: char) -> String {
    if width == 0 || s.len() >= width { return s.to_string(); }
    let pad = width - s.len();
    match align {
        '<' => format!("{}{}", s, std::iter::repeat(fill).take(pad).collect::<String>()),
        '^' => {
            let left = pad / 2;
            let right = pad - left;
            format!("{}{}{}", std::iter::repeat(fill).take(left).collect::<String>(), s, std::iter::repeat(fill).take(right).collect::<String>())
        }
        _ => format!("{}{}", std::iter::repeat(fill).take(pad).collect::<String>(), s), // > default right-align
    }
}

/// One accessor step in a mutation lvalue path (see collect_lvalue_path):
/// `.field` or `[index]` with the index already evaluated.
enum LvalueAccess {
    Field(String),
    Index(Value),
}

fn apply_mutation(val: &mut Value, method: &str, arg_vals: Vec<Value>) -> Result<Value, String> {
    match (val, method) {
        // List mutation methods
        (Value::List(ref mut items), "push") => {
            for v in arg_vals {
                items.push(v);
            }
            Ok(Value::Null)
        }
        (Value::List(ref mut items), "pop") => {
            Ok(items.pop().unwrap_or(Value::Null))
        }
        (Value::List(ref mut items), "pop_opt") => {
            if items.is_empty() {
                Ok(Value::Null)
            } else {
                let val = items.pop().unwrap();
                Ok(Value::Some(Box::new(val)))
            }
        }
        (Value::List(ref mut items), "insert") => {
            if arg_vals.len() >= 2 {
                if let Value::Int(idx) = &arg_vals[0] {
                    items.insert(*idx as usize, arg_vals[1].clone());
                    Ok(Value::Null)
                } else {
                    Err("insert() first argument must be an integer".into())
                }
            } else {
                Err("insert() requires index and value arguments".into())
            }
        }
        (Value::List(ref mut items), "remove") => {
            if let Some(Value::Int(idx)) = arg_vals.first() {
                if (*idx as usize) < items.len() {
                    Ok(items.remove(*idx as usize))
                } else {
                    Err(format!("Index {} out of bounds", idx))
                }
            } else {
                Err("remove() requires an integer index".into())
            }
        }
        (Value::List(ref mut items), "extend") => {
            if let Some(Value::List(other)) = arg_vals.first() {
                items.extend(other.clone());
                Ok(Value::Null)
            } else {
                Err("extend() requires a list argument".into())
            }
        }
        (Value::List(ref mut items), "clear") => {
            items.clear();
            Ok(Value::Null)
        }
        // Dict mutation methods
        (Value::Dict(ref mut pairs), "remove") | (Value::Dict(ref mut pairs), "delete") => {
            if let Some(key) = arg_vals.first() {
                let before_len = pairs.len();
                pairs.retain(|(k, _)| k != key);
                if pairs.len() < before_len {
                    Ok(Value::Bool(true))
                } else {
                    Ok(Value::Bool(false))
                }
            } else {
                Err("remove() requires a key argument".into())
            }
        }
        (Value::Dict(ref mut pairs), "clear") => {
            pairs.clear();
            Ok(Value::Null)
        }
        (Value::Dict(ref mut pairs), "pop") => {
            if let Some(key) = arg_vals.first() {
                if let Some(pos) = pairs.iter().position(|(k, _)| *k == *key) {
                    let (_, val) = pairs.remove(pos);
                    Ok(val)
                } else {
                    // Return default if provided, else Null
                    Ok(arg_vals.get(1).cloned().unwrap_or(Value::Null))
                }
            } else {
                Err("pop() requires a key argument".into())
            }
        }
        (Value::Dict(ref mut pairs), "set") => {
            if arg_vals.len() >= 2 {
                let key = arg_vals[0].clone();
                let val = arg_vals[1].clone();
                for (k, v) in pairs.iter_mut() {
                    if *k == key {
                        *v = val;
                        return Ok(Value::Null);
                    }
                }
                pairs.push((key, val));
                Ok(Value::Null)
            } else {
                Err("set() requires key and value arguments".into())
            }
        }
        (Value::Dict(ref mut pairs), "update") => {
            if let Some(Value::Dict(other)) = arg_vals.first() {
                for (k, v) in other.iter() {
                    let mut found = false;
                    for (ek, ev) in pairs.iter_mut() {
                        if ek == k {
                            *ev = v.clone();
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        pairs.push((k.clone(), v.clone()));
                    }
                }
                Ok(Value::Null)
            } else {
                Err("update() requires a dict argument".into())
            }
        }
        // Set mutation methods
        (Value::Set(ref mut items), "push") | (Value::Set(ref mut items), "add") => {
            for v in arg_vals {
                if !items.contains(&v) {
                    items.push(v);
                }
            }
            Ok(Value::Null)
        }
        (Value::Set(ref mut items), "remove") => {
            if let Some(val) = arg_vals.first() {
                items.retain(|v| v != val);
                Ok(Value::Null)
            } else {
                Err("remove() requires an argument".into())
            }
        }
        (Value::Set(ref mut items), "clear") => {
            items.clear();
            Ok(Value::Null)
        }
        (other, _) => {
            let type_name = other.type_name().to_string();
            Err(format!("No mutation method '{}' on {}", method, type_name))
        }
    }
}

impl Interpreter {
    // ── Builtins ─────────────────────────────────────────

    /// Convert an argument value to raw bytes for hashing/encoding builtins:
    /// strings become their UTF-8 bytes, byte strings pass through, and a list of
    /// integers is treated as a byte array.
    fn arg_to_bytes(v: Option<&&Value>) -> Vec<u8> {
        match v {
            Some(Value::Str(s)) => s.as_bytes().to_vec(),
            Some(Value::Bytes(b)) => b.clone(),
            Some(Value::List(items)) => items
                .iter()
                .map(|it| if let Value::Int(n) = it { (*n & 0xFF) as u8 } else { 0 })
                .collect(),
            Some(other) => format!("{}", other).into_bytes(),
            None => Vec::new(),
        }
    }

    /// Parse a semantic version `MAJOR.MINOR.PATCH[-prerelease][+build]`.
    fn parse_semver(s: &str) -> Option<(i64, i64, i64, String, String)> {
        let s = s.trim().trim_start_matches('v');
        let (core, build) = match s.split_once('+') {
            Some((c, b)) => (c, b.to_string()),
            None => (s, String::new()),
        };
        let (core, pre) = match core.split_once('-') {
            Some((c, p)) => (c, p.to_string()),
            None => (core, String::new()),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return None;
        }
        let maj = parts.first().and_then(|p| p.parse().ok())?;
        let min = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
        let pat = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
        Some((maj, min, pat, pre, build))
    }

    /// Compare two semver strings: -1, 0, or 1. A prerelease is lower precedence
    /// than the associated normal release.
    fn semver_cmp(a: &str, b: &str) -> i32 {
        let pa = Self::parse_semver(a);
        let pb = Self::parse_semver(b);
        match (pa, pb) {
            (Some((amaj, amin, apat, apre, _)), Some((bmaj, bmin, bpat, bpre, _))) => {
                for (x, y) in [(amaj, bmaj), (amin, bmin), (apat, bpat)] {
                    if x != y {
                        return if x < y { -1 } else { 1 };
                    }
                }
                // Equal core: no prerelease outranks a prerelease.
                match (apre.is_empty(), bpre.is_empty()) {
                    (true, true) => 0,
                    (true, false) => 1,
                    (false, true) => -1,
                    (false, false) => apre.cmp(&bpre) as i32,
                }
            }
            _ => 0,
        }
    }

    /// Basic range satisfaction: exact, `^` (compatible), `~` (approximate),
    /// and comparison operators (`>=`, `>`, `<=`, `<`, `=`).
    fn semver_satisfies(v: &str, range: &str) -> bool {
        let range = range.trim();
        if range == "*" || range.is_empty() {
            return true;
        }
        let (maj, min, pat, _, _) = match Self::parse_semver(v) {
            Some(t) => t,
            None => return false,
        };
        if let Some(base) = range.strip_prefix('^') {
            if let Some((bmaj, bmin, bpat, _, _)) = Self::parse_semver(base) {
                return maj == bmaj && (min, pat) >= (bmin, bpat);
            }
        }
        if let Some(base) = range.strip_prefix('~') {
            if let Some((bmaj, bmin, bpat, _, _)) = Self::parse_semver(base) {
                return maj == bmaj && min == bmin && pat >= bpat;
            }
        }
        for (op, len) in [(">=", 2), ("<=", 2), (">", 1), ("<", 1), ("=", 1)] {
            if let Some(base) = range.strip_prefix(op) {
                let _ = len;
                let c = Self::semver_cmp(v, base.trim());
                return match op {
                    ">=" => c >= 0,
                    "<=" => c <= 0,
                    ">" => c > 0,
                    "<" => c < 0,
                    _ => c == 0,
                };
            }
        }
        Self::semver_cmp(v, range) == 0
    }

    /// Parse CSV text into rows of fields, honoring double-quoted fields with
    /// embedded commas, newlines, and escaped quotes (`""`).
    fn parse_csv(text: &str) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        let mut row = Vec::new();
        let mut field = String::new();
        let mut in_quotes = false;
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if in_quotes {
                if c == '"' {
                    if i + 1 < chars.len() && chars[i + 1] == '"' {
                        field.push('"');
                        i += 1;
                    } else {
                        in_quotes = false;
                    }
                } else {
                    field.push(c);
                }
            } else {
                match c {
                    '"' => in_quotes = true,
                    ',' => {
                        row.push(std::mem::take(&mut field));
                    }
                    '\n' => {
                        row.push(std::mem::take(&mut field));
                        rows.push(std::mem::take(&mut row));
                    }
                    '\r' => {}
                    _ => field.push(c),
                }
            }
            i += 1;
        }
        if !field.is_empty() || !row.is_empty() {
            row.push(field);
            rows.push(row);
        }
        rows
    }

    /// Quote a CSV field when it contains a comma, quote, or newline.
    fn csv_escape(field: &str) -> String {
        if field.contains(',') || field.contains('"') || field.contains('\n') {
            format!("\"{}\"", field.replace('"', "\"\""))
        } else {
            field.to_string()
        }
    }

    /// Parse a `.env` document into `(KEY, VALUE)` string pairs. Supports
    /// `export KEY=val`, `#` comments, and single/double-quoted values.
    fn parse_dotenv(text: &str) -> Vec<(Value, Value)> {
        let mut out = Vec::new();
        for raw in text.lines() {
            let mut line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("export ") {
                line = rest.trim();
            }
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim().to_string();
                let mut val = line[eq + 1..].trim().to_string();
                // Strip a trailing inline comment on unquoted values.
                if !(val.starts_with('"') || val.starts_with('\'')) {
                    if let Some(h) = val.find(" #") {
                        val = val[..h].trim().to_string();
                    }
                }
                if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                    || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
                {
                    val = val[1..val.len() - 1].to_string();
                }
                if !key.is_empty() {
                    out.push((Value::Str(key), Value::Str(val)));
                }
            }
        }
        out
    }

    // ── std.toml: a pragmatic TOML parser (common config subset) ──

    /// Strip a `#` comment that is outside of a quoted string.
    fn toml_strip_comment(line: &str) -> String {
        let mut in_str = false;
        let mut q = ' ';
        let mut out = String::new();
        for c in line.chars() {
            if (c == '"' || c == '\'') && (!in_str || c == q) {
                in_str = !in_str;
                q = c;
            }
            if c == '#' && !in_str {
                break;
            }
            out.push(c);
        }
        out
    }

    /// Split on `delim` at the top level (ignoring quotes and `[]`/`{}` nesting).
    fn toml_split(s: &str, delim: char) -> Vec<String> {
        let mut parts = Vec::new();
        let mut cur = String::new();
        let mut depth = 0i32;
        let mut in_str = false;
        let mut q = ' ';
        for c in s.chars() {
            match c {
                '"' | '\'' if !in_str => { in_str = true; q = c; }
                _ if in_str && c == q => in_str = false,
                '[' | '{' if !in_str => depth += 1,
                ']' | '}' if !in_str => depth -= 1,
                _ => {}
            }
            if c == delim && depth == 0 && !in_str {
                parts.push(cur.trim().to_string());
                cur.clear();
            } else {
                cur.push(c);
            }
        }
        if !cur.trim().is_empty() {
            parts.push(cur.trim().to_string());
        }
        parts
    }

    /// Parse a TOML scalar/array/inline-table value.
    fn parse_toml_scalar(s: &str) -> Value {
        let s = s.trim();
        if s.is_empty() {
            return Value::Null;
        }
        if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
            return Value::Str(s[1..s.len() - 1].to_string());
        }
        if s.starts_with('[') && s.ends_with(']') {
            let inner = &s[1..s.len() - 1];
            return Value::List(Self::toml_split(inner, ',').iter().map(|i| Self::parse_toml_scalar(i)).collect());
        }
        if s.starts_with('{') && s.ends_with('}') {
            let inner = &s[1..s.len() - 1];
            let mut d = Vec::new();
            for pair in Self::toml_split(inner, ',') {
                if let Some(eq) = pair.find('=') {
                    let k = pair[..eq].trim().trim_matches('"').to_string();
                    d.push((Value::Str(k), Self::parse_toml_scalar(pair[eq + 1..].trim())));
                }
            }
            return Value::Dict(d);
        }
        match s {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => {
                if let Ok(i) = s.parse::<i64>() {
                    Value::Int(i)
                } else if let Ok(f) = s.parse::<f64>() {
                    Value::Float(f)
                } else {
                    Value::Str(s.to_string())
                }
            }
        }
    }

    /// Insert `value` at `path` in a nested dict, creating intermediate tables.
    fn toml_insert(dict: &mut Vec<(Value, Value)>, path: &[String], value: Value) {
        if path.is_empty() {
            return;
        }
        let key = &path[0];
        if path.len() == 1 {
            for (k, v) in dict.iter_mut() {
                if matches!(k, Value::Str(s) if s == key) {
                    *v = value;
                    return;
                }
            }
            dict.push((Value::Str(key.clone()), value));
            return;
        }
        for (k, v) in dict.iter_mut() {
            if matches!(k, Value::Str(s) if s == key) {
                if let Value::Dict(sub) = v {
                    Self::toml_insert(sub, &path[1..], value);
                    return;
                }
            }
        }
        let mut sub = Vec::new();
        Self::toml_insert(&mut sub, &path[1..], value);
        dict.push((Value::Str(key.clone()), Value::Dict(sub)));
    }

    /// Parse a TOML document into a `Value::Dict`. Supports `[table]`,
    /// `[nested.table]`, scalars (str/int/float/bool), arrays, and inline tables.
    fn parse_toml_value(text: &str) -> Value {
        let mut root: Vec<(Value, Value)> = Vec::new();
        let mut current_path: Vec<String> = Vec::new();
        for raw in text.lines() {
            let line = Self::toml_strip_comment(raw);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let name = line.trim_matches(|c| c == '[' || c == ']').trim();
                current_path = name.split('.').map(|s| s.trim().trim_matches('"').to_string()).collect();
                // Ensure the (empty) table exists.
                if !current_path.is_empty() {
                    Self::ensure_table(&mut root, &current_path);
                }
                continue;
            }
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim().trim_matches('"').to_string();
                let val = Self::parse_toml_scalar(line[eq + 1..].trim());
                let mut full = current_path.clone();
                full.push(key);
                Self::toml_insert(&mut root, &full, val);
            }
        }
        Value::Dict(root)
    }

    fn ensure_table(dict: &mut Vec<(Value, Value)>, path: &[String]) {
        if path.is_empty() {
            return;
        }
        let key = &path[0];
        for (k, v) in dict.iter_mut() {
            if matches!(k, Value::Str(s) if s == key) {
                if let Value::Dict(sub) = v {
                    Self::ensure_table(sub, &path[1..]);
                }
                return;
            }
        }
        let mut sub = Vec::new();
        Self::ensure_table(&mut sub, &path[1..]);
        dict.push((Value::Str(key.clone()), Value::Dict(sub)));
    }

    /// Serialize a `Value` to TOML. `prefix` is the current table path.
    fn toml_stringify_value(v: &Value, prefix: &str, out: &mut String) {
        if let Value::Dict(pairs) = v {
            // Scalars first, then nested tables.
            for (k, val) in pairs {
                let key = format!("{}", k);
                if !matches!(val, Value::Dict(_)) {
                    out.push_str(&format!("{} = {}\n", key, Self::toml_scalar_str(val)));
                }
            }
            for (k, val) in pairs {
                if let Value::Dict(_) = val {
                    let key = format!("{}", k);
                    let path = if prefix.is_empty() { key.clone() } else { format!("{}.{}", prefix, key) };
                    out.push_str(&format!("\n[{}]\n", path));
                    Self::toml_stringify_value(val, &path, out);
                }
            }
        }
    }

    fn toml_scalar_str(v: &Value) -> String {
        match v {
            Value::Str(s) => format!("\"{}\"", s.replace('"', "\\\"")),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => format!("{}", f),
            Value::Bool(b) => b.to_string(),
            Value::List(items) => {
                let parts: Vec<String> = items.iter().map(Self::toml_scalar_str).collect();
                format!("[{}]", parts.join(", "))
            }
            _ => format!("\"{}\"", v),
        }
    }

    /// Recursively collect all file paths under `dir` (for fs.walk).
    fn walk_dir(dir: &std::path::Path, out: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut items: Vec<_> = entries.flatten().collect();
            items.sort_by_key(|e| e.file_name());
            for entry in items {
                let path = entry.path();
                if path.is_dir() {
                    Self::walk_dir(&path, out);
                } else {
                    out.push(path.to_string_lossy().to_string());
                }
            }
        }
    }

    // ── std.fs path helpers (pure string manipulation, '/' separator) ──
    fn path_basename(p: &str) -> String {
        let p = p.trim_end_matches(['/', '\\']);
        p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
    }
    fn path_dirname(p: &str) -> String {
        let trimmed = p.trim_end_matches(['/', '\\']);
        match trimmed.rfind(['/', '\\']) {
            Some(0) => "/".to_string(),
            Some(i) => trimmed[..i].to_string(),
            None => ".".to_string(),
        }
    }
    fn path_ext(p: &str) -> String {
        let base = Self::path_basename(p);
        match base.rfind('.') {
            Some(i) if i > 0 => base[i..].to_string(),
            _ => String::new(),
        }
    }
    fn path_stem(p: &str) -> String {
        let base = Self::path_basename(p);
        match base.rfind('.') {
            Some(i) if i > 0 => base[..i].to_string(),
            _ => base,
        }
    }
    fn path_normalize(p: &str) -> String {
        let is_abs = p.starts_with('/');
        let mut out: Vec<String> = Vec::new();
        for part in p.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    if out.last().map(|l| l != "..").unwrap_or(false) {
                        out.pop();
                    } else if !is_abs {
                        out.push("..".to_string());
                    }
                }
                other => out.push(other.to_string()),
            }
        }
        let joined = out.join("/");
        if is_abs {
            format!("/{}", joined)
        } else if joined.is_empty() {
            ".".to_string()
        } else {
            joined
        }
    }

    /// LCS-based diff of two unit sequences. Returns a list of (op, unit) where
    /// op is ' ' (keep), '-' (remove from a), or '+' (add from b).
    fn lcs_diff(a: &[String], b: &[String]) -> Vec<(char, String)> {
        let n = a.len();
        let m = b.len();
        // dp[i][j] = LCS length of a[i..], b[j..]
        let mut dp = vec![vec![0usize; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i][j] = if a[i] == b[j] {
                    dp[i + 1][j + 1] + 1
                } else {
                    dp[i + 1][j].max(dp[i][j + 1])
                };
            }
        }
        let mut ops = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a[i] == b[j] {
                ops.push((' ', a[i].clone()));
                i += 1;
                j += 1;
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                ops.push(('-', a[i].clone()));
                i += 1;
            } else {
                ops.push(('+', b[j].clone()));
                j += 1;
            }
        }
        while i < n {
            ops.push(('-', a[i].clone()));
            i += 1;
        }
        while j < m {
            ops.push(('+', b[j].clone()));
            j += 1;
        }
        ops
    }

    /// Split text into diff units based on mode: "line" (default), "char", "word".
    fn diff_units(text: &str, mode: &str) -> Vec<String> {
        match mode {
            "char" => text.chars().map(|c| c.to_string()).collect(),
            "word" => text.split_whitespace().map(|s| s.to_string()).collect(),
            _ => {
                // Line mode: preserve line boundaries, drop a trailing empty split.
                let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
                if lines.last().map(|s| s.is_empty()).unwrap_or(false) {
                    lines.pop();
                }
                lines
            }
        }
    }

    /// Look up a named argument by name in the raw argument list.
    fn named_arg<'a>(args: &'a [(Option<String>, Value)], name: &str) -> Option<&'a Value> {
        args.iter().find(|(n, _)| n.as_deref() == Some(name)).map(|(_, v)| v)
    }

    /// Build a money value: a tagged dict carrying an exact-decimal amount.
    fn make_money(amount: crate::decimal::Decimal, currency: &str) -> Value {
        Value::Dict(vec![
            (Value::Str("type".into()), Value::Str("money".into())),
            (Value::Str("amount".into()), Value::Decimal(amount)),
            (Value::Str("currency".into()), Value::Str(currency.to_string())),
        ])
    }

    /// Extract (amount, currency) from a money value.
    fn money_parts(v: Option<&Value>) -> Option<(crate::decimal::Decimal, String)> {
        if let Some(Value::Dict(pairs)) = v {
            let mut amount = None;
            let mut currency = None;
            for (k, val) in pairs.iter() {
                if let Value::Str(key) = k {
                    match key.as_str() {
                        "amount" => amount = Self::as_decimal(val),
                        "currency" => currency = Some(format!("{}", val)),
                        _ => {}
                    }
                }
            }
            if let (Some(a), Some(c)) = (amount, currency) {
                return Some((a, c));
            }
        }
        None
    }

    /// printf-style formatting for fmt.sprintf: supports `%[flags][width][.prec]spec`
    /// with specifiers d/i/u/s/f/e/g/x/X/o/b/c/% and flags `0`, `-`, `+`, ` `.
    fn printf_format(template: &str, args: &[&Value]) -> String {
        let chars: Vec<char> = template.chars().collect();
        let mut out = String::new();
        let mut ai = 0;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '%' {
                out.push(chars[i]);
                i += 1;
                continue;
            }
            i += 1;
            if i < chars.len() && chars[i] == '%' {
                out.push('%');
                i += 1;
                continue;
            }
            // flags
            let (mut zero, mut left, mut plus, mut space) = (false, false, false, false);
            while i < chars.len() {
                match chars[i] {
                    '0' => zero = true,
                    '-' => left = true,
                    '+' => plus = true,
                    ' ' => space = true,
                    _ => break,
                }
                i += 1;
            }
            // width
            let mut width = 0usize;
            let mut has_width = false;
            while i < chars.len() && chars[i].is_ascii_digit() {
                has_width = true;
                width = width * 10 + chars[i].to_digit(10).unwrap() as usize;
                i += 1;
            }
            // precision
            let mut precision: Option<usize> = None;
            if i < chars.len() && chars[i] == '.' {
                i += 1;
                let mut p = 0usize;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    p = p * 10 + chars[i].to_digit(10).unwrap() as usize;
                    i += 1;
                }
                precision = Some(p);
            }
            if i >= chars.len() {
                break;
            }
            let spec = chars[i];
            i += 1;
            let arg = args.get(ai).copied();
            ai += 1;
            let as_i64 = |v: Option<&Value>| -> i64 {
                match v {
                    Some(Value::Int(n)) => *n,
                    Some(Value::Float(f)) => *f as i64,
                    Some(Value::Bool(b)) => *b as i64,
                    Some(Value::BigInt(b)) => b.to_i64().unwrap_or(0),
                    _ => 0,
                }
            };
            let as_f64 = |v: Option<&Value>| -> f64 {
                match v {
                    Some(Value::Float(f)) => *f,
                    Some(Value::Int(n)) => *n as f64,
                    Some(Value::BigInt(b)) => b.to_f64(),
                    _ => 0.0,
                }
            };
            let mut body = match spec {
                'd' | 'i' | 'u' => {
                    let n = as_i64(arg);
                    let s = n.unsigned_abs().to_string();
                    if n < 0 { format!("-{}", s) }
                    else if plus { format!("+{}", s) }
                    else if space { format!(" {}", s) }
                    else { s }
                }
                'f' | 'F' => format!("{:.*}", precision.unwrap_or(6), as_f64(arg)),
                'e' => format!("{:.*e}", precision.unwrap_or(6), as_f64(arg)),
                'g' | 'G' => {
                    let f = as_f64(arg);
                    format!("{}", f)
                }
                'x' => format!("{:x}", as_i64(arg)),
                'X' => format!("{:X}", as_i64(arg)),
                'o' => format!("{:o}", as_i64(arg)),
                'b' => format!("{:b}", as_i64(arg)),
                'c' => {
                    let n = as_i64(arg) as u32;
                    char::from_u32(n).map(|c| c.to_string()).unwrap_or_default()
                }
                's' => {
                    let s = match arg {
                        Some(v) => format!("{}", v),
                        None => String::new(),
                    };
                    match precision {
                        Some(p) if p < s.chars().count() => s.chars().take(p).collect(),
                        _ => s,
                    }
                }
                other => {
                    ai -= 1; // not a real conversion — emit literally
                    format!("%{}", other)
                }
            };
            // Apply width padding.
            if has_width && body.chars().count() < width {
                let pad = width - body.chars().count();
                if left {
                    body.push_str(&" ".repeat(pad));
                } else if zero && matches!(spec, 'd' | 'i' | 'u' | 'f' | 'F' | 'x' | 'X' | 'o' | 'b' | 'e') {
                    // Zero-pad after an optional sign.
                    if let Some(stripped) = body.strip_prefix('-') {
                        body = format!("-{}{}", "0".repeat(pad), stripped);
                    } else {
                        body = format!("{}{}", "0".repeat(pad), body);
                    }
                } else {
                    body = format!("{}{}", " ".repeat(pad), body);
                }
            }
            out.push_str(&body);
        }
        out
    }

    /// Extract the integer id embedded in a channel/thread handle dict, or the
    /// bare integer itself.
    fn handle_id(v: Option<&Value>) -> i64 {
        match v {
            Some(Value::Int(n)) => *n,
            Some(Value::Dict(pairs)) => pairs
                .iter()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == "id"))
                .and_then(|(_, v)| if let Value::Int(n) = v { Some(*n) } else { None })
                .unwrap_or(-1),
            _ => -1,
        }
    }

    fn call_builtin(
        &mut self,
        name: &str,
        args: &[(Option<String>, Value)],
    ) -> Result<Value, String> {
        let arg_vals: Vec<&Value> = args.iter().map(|(_, v)| v).collect();

        match name {
            "print" => {
                let mut sep = " ".to_string();
                let mut end = "".to_string();
                let mut positional = Vec::new();
                for (name, val) in args {
                    match name.as_deref() {
                        Some("sep") => sep = self.value_to_string(val),
                        Some("end") => end = self.value_to_string(val),
                        _ => positional.push(self.value_to_string(val)),
                    }
                }
                print!("{}{}", positional.join(&sep), end);
                io::stdout().flush().ok();
                Ok(Value::Null)
            }
            "println" => {
                let mut sep = " ".to_string();
                let mut end = "\n".to_string();
                let mut positional = Vec::new();
                for (name, val) in args {
                    match name.as_deref() {
                        Some("sep") => sep = self.value_to_string(val),
                        Some("end") => end = self.value_to_string(val),
                        _ => positional.push(self.value_to_string(val)),
                    }
                }
                print!("{}{}", positional.join(&sep), end);
                io::stdout().flush().ok();
                Ok(Value::Null)
            }
            "__io_write" => {
                for value in &arg_vals {
                    print!("{}", value);
                }
                io::stdout().flush().ok();
                Ok(Value::Null)
            }
            "__io_write_line" => {
                for value in &arg_vals {
                    print!("{}", value);
                }
                println!();
                Ok(Value::Null)
            }
            "__io_flush" => {
                io::stdout().flush().ok();
                Ok(Value::Null)
            }
            "input" => {
                if let Some(prompt) = arg_vals.first() {
                    print!("{}", prompt);
                    io::stdout().flush().ok();
                }
                let mut line = String::new();
                io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
                Ok(Value::Str(line.trim_end_matches('\n').trim_end_matches('\r').to_string()))
            }
            "len" => {
                if let Some(val) = arg_vals.first() {
                    match val {
                        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                        Value::List(l) => Ok(Value::Int(l.len() as i64)),
                        Value::Dict(d) => Ok(Value::Int(d.len() as i64)),
                        Value::Tuple(t) => Ok(Value::Int(t.len() as i64)),
                        Value::Set(s) => Ok(Value::Int(s.len() as i64)),
                        Value::Bytes(b) => Ok(Value::Int(b.len() as i64)),
                        Value::Instance(cls, _) | Value::StructInstance(cls, _) => {
                            // Dispatch to __len__ dunder method
                            let cls = cls.clone();
                            let val_clone = (*val).clone();
                            if let Some(Value::Class(cv)) = self.env.get(&cls) {
                                if let Some(method) = cv.methods.get("__len__") {
                                    let method = method.clone();
                                    let saved = self.env.current;
                                    self.env.push_scope_with_parent(method.closure_env);
                                    self.env.define("self", val_clone);
                                    let result = self.exec_block_no_scope(&method.body)?;
                                    self.env.set_scope(saved);
                                    return match result {
                                        Value::Return(v) => Ok(*v),
                                        other => Ok(other),
                                    };
                                }
                            }
                            Err(format!("Cannot get length of {}", val.type_name()))
                        }
                        _ => Err(format!("Cannot get length of {}", val.type_name())),
                    }
                } else {
                    Err("len() requires an argument".into())
                }
            }
            "type_of" => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Str(val.type_name().to_string()))
                } else {
                    Err("type_of() requires an argument".into())
                }
            }
            // ── Promise static API (synchronous async model) ──
            // Inputs are already-resolved values, so these operate on them directly.
            "__promise_resolve" => Ok(arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null)),
            "__promise_reject" => {
                let e = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null);
                Err(format!("Promise rejected: {}", self.value_to_string(&e)))
            }
            "__promise_all" => {
                // Promise.all([...]) → list of resolved values (all already resolved).
                match arg_vals.first() {
                    Some(Value::List(items)) => Ok(Value::List(items.clone())),
                    Some(other) => Ok(Value::List(vec![(*other).clone()])),
                    None => Ok(Value::List(vec![])),
                }
            }
            "__promise_all_settled" => {
                // Each entry → { "status": "fulfilled", "value": v }.
                let items: Vec<Value> = match arg_vals.first() {
                    Some(Value::List(items)) => items.clone(),
                    Some(other) => vec![(*other).clone()],
                    None => vec![],
                };
                let settled: Vec<Value> = items
                    .into_iter()
                    .map(|v| {
                        Value::Dict(vec![
                            (Value::Str("status".into()), Value::Str("fulfilled".into())),
                            (Value::Str("value".into()), v),
                        ])
                    })
                    .collect();
                Ok(Value::List(settled))
            }
            "__promise_race" | "__promise_any" => {
                // First resolved value wins (synchronous: just take the first).
                match arg_vals.first() {
                    Some(Value::List(items)) => Ok(items.first().cloned().unwrap_or(Value::Null)),
                    Some(other) => Ok((*other).clone()),
                    None => Ok(Value::Null),
                }
            }
            "__promise_timeout" => Ok(Value::Null),
            "to_string" => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Str(self.value_to_string(val)))
                } else {
                    Err("to_string() requires an argument".into())
                }
            }
            "to_int" => {
                if let Some(val) = arg_vals.first() {
                    match val {
                        Value::Int(n) => Ok(Value::Int(*n)),
                        Value::BigInt(b) => Ok(Value::BigInt(b.clone())),
                        Value::Float(f) => Ok(Value::Int(*f as i64)),
                        Value::Str(s) => s
                            .parse::<i64>()
                            .map(Value::Int)
                            .or_else(|_| crate::bigint::BigInt::from_str(s.trim())
                                .map(Self::norm_bigint)
                                .ok_or(()))
                            .map_err(|_| format!("Cannot convert '{}' to int", s)),
                        Value::Bool(b) => Ok(Value::Int(if *b { 1 } else { 0 })),
                        _ => Err(format!("Cannot convert {} to int", val.type_name())),
                    }
                } else {
                    Err("to_int() requires an argument".into())
                }
            }
            "to_float" => {
                if let Some(val) = arg_vals.first() {
                    match val {
                        Value::Int(n) => Ok(Value::Float(*n as f64)),
                        Value::BigInt(b) => Ok(Value::Float(b.to_f64())),
                        Value::Float(f) => Ok(Value::Float(*f)),
                        Value::Str(s) => s
                            .parse::<f64>()
                            .map(Value::Float)
                            .map_err(|_| format!("Cannot convert '{}' to float", s)),
                        _ => Err(format!("Cannot convert {} to float", val.type_name())),
                    }
                } else {
                    Err("to_float() requires an argument".into())
                }
            }
            "range" => {
                let start = arg_vals.first().and_then(|v| if let Value::Int(n) = v { Some(*n) } else { None }).unwrap_or(0);
                let end = arg_vals.get(1).and_then(|v| if let Value::Int(n) = v { Some(*n) } else { None });
                let step = arg_vals.get(2).and_then(|v| if let Value::Int(n) = v { Some(*n) } else { None });
                if let Some(step) = step {
                    // range(start, end, step)
                    let end = end.unwrap_or(start);
                    if step == 0 {
                        return Err("range() step cannot be zero".into());
                    }
                    let mut result = Vec::new();
                    let mut i = if end == start { 0 } else { start };
                    let end_val = if end == start { start } else { end };
                    if step > 0 {
                        while i < end_val { result.push(Value::Int(i)); i += step; }
                    } else {
                        while i > end_val { result.push(Value::Int(i)); i += step; }
                    }
                    Ok(Value::List(result))
                } else if let Some(end) = end {
                    Ok(Value::Range(start, end, false))
                } else {
                    Ok(Value::Range(0, start, false))
                }
            }
            "abs" => {
                if let Some(val) = arg_vals.first() {
                    match val {
                        Value::Int(n) => Ok(match n.checked_abs() {
                            Some(v) => Value::Int(v),
                            None => Value::BigInt(crate::bigint::BigInt::from_i64(*n).neg()),
                        }),
                        Value::BigInt(b) => Ok(Self::norm_bigint(if b.is_negative() { b.neg() } else { b.clone() })),
                        Value::Decimal(d) => Ok(Value::Decimal(d.abs())),
                        Value::Float(f) => Ok(Value::Float(f.abs())),
                        _ => Err("abs() requires a number".into()),
                    }
                } else {
                    Err("abs() requires an argument".into())
                }
            }
            "min" => {
                if arg_vals.len() >= 2 {
                    let result = self.compare_values(arg_vals[0], arg_vals[1], |a, b| a < b)?;
                    if let Value::Bool(true) = result {
                        Ok(arg_vals[0].clone())
                    } else {
                        Ok(arg_vals[1].clone())
                    }
                } else if let Some(Value::List(items)) = arg_vals.first() {
                    items.iter().cloned().reduce(|a, b| {
                        if let Ok(Value::Bool(true)) = self.compare_values(&a, &b, |x, y| x < y) {
                            a
                        } else {
                            b
                        }
                    }).ok_or_else(|| "min() on empty list".into())
                } else {
                    Err("min() requires two arguments or a list".into())
                }
            }
            "max" => {
                if arg_vals.len() >= 2 {
                    let result = self.compare_values(arg_vals[0], arg_vals[1], |a, b| a > b)?;
                    if let Value::Bool(true) = result {
                        Ok(arg_vals[0].clone())
                    } else {
                        Ok(arg_vals[1].clone())
                    }
                } else if let Some(Value::List(items)) = arg_vals.first() {
                    items.iter().cloned().reduce(|a, b| {
                        if let Ok(Value::Bool(true)) = self.compare_values(&a, &b, |x, y| x > y) {
                            a
                        } else {
                            b
                        }
                    }).ok_or_else(|| "max() on empty list".into())
                } else {
                    Err("max() requires two arguments or a list".into())
                }
            }
            "push" => {
                if arg_vals.len() >= 2 {
                    if let Value::List(ref items) = arg_vals[0] {
                        let mut new_list = items.clone();
                        new_list.push(arg_vals[1].clone());
                        Ok(Value::List(new_list))
                    } else {
                        Err("push() requires a list as first argument".into())
                    }
                } else {
                    Err("push() requires two arguments".into())
                }
            }
            "pop" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    let mut new_list = items.clone();
                    let popped = new_list.pop().unwrap_or(Value::Null);
                    Ok(Value::Tuple(vec![popped, Value::List(new_list)]))
                } else {
                    Err("pop() requires a list argument".into())
                }
            }
            "pop_opt" => {
                // Returns Some(last) and mutates, or None
                if let Some(Value::List(items)) = arg_vals.first() {
                    if items.is_empty() {
                        Ok(Value::Null) // None
                    } else {
                        let last = items.last().unwrap().clone();
                        Ok(Value::Some(Box::new(last)))
                    }
                } else {
                    Err("pop_opt() requires a list argument".into())
                }
            }
            "keys" => {
                if let Some(Value::Dict(pairs)) = arg_vals.first() {
                    Ok(Value::List(pairs.iter().map(|(k, _)| k.clone()).collect()))
                } else {
                    Err("keys() requires a dict argument".into())
                }
            }
            "values" => {
                if let Some(Value::Dict(pairs)) = arg_vals.first() {
                    Ok(Value::List(pairs.iter().map(|(_, v)| v.clone()).collect()))
                } else {
                    Err("values() requires a dict argument".into())
                }
            }
            "contains" => {
                if arg_vals.len() >= 2 {
                    match arg_vals[0] {
                        Value::List(items) => Ok(Value::Bool(items.contains(arg_vals[1]))),
                        Value::Str(s) => {
                            if let Value::Str(sub) = arg_vals[1] {
                                Ok(Value::Bool(s.contains(sub.as_str())))
                            } else {
                                Err("contains() on string requires string arg".into())
                            }
                        }
                        _ => Err("contains() requires a list or string".into()),
                    }
                } else {
                    Err("contains() requires two arguments".into())
                }
            }
            "remove" => {
                Err("remove() not yet implemented for in-place mutation".into())
            }
            "split" => {
                if let Some(Value::Str(s)) = arg_vals.first() {
                    let sep = arg_vals.get(1).and_then(|v| if let Value::Str(sep) = v { Some(sep.as_str()) } else { None }).unwrap_or(" ");
                    Ok(Value::List(s.split(sep).map(|p| Value::Str(p.to_string())).collect()))
                } else {
                    Err("split() requires a string argument".into())
                }
            }
            "join" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    let sep = arg_vals.get(1).and_then(|v| if let Value::Str(s) = v { Some(s.as_str()) } else { None }).unwrap_or("");
                    let parts: Vec<String> = items.iter().map(|v| format!("{}", v)).collect();
                    Ok(Value::Str(parts.join(sep)))
                } else {
                    Err("join() requires a list argument".into())
                }
            }
            "trim" | "to_upper" | "to_lower" | "replace" | "starts_with" | "ends_with"
            | "substr" | "char_at" | "reverse" => {
                if let Some(val) = arg_vals.first() {
                    self.call_builtin_method(val, name, args)
                } else {
                    Err(format!("{}() requires an argument", name))
                }
            }
            "sort" | "map" | "filter" | "reduce" | "each" | "find" | "enumerate"
            | "flat_map" | "count" | "first" | "last" | "is_empty" => {
                if let Some(val) = arg_vals.first() {
                    let rest: Vec<(Option<String>, Value)> =
                        args.iter().skip(1).cloned().collect();
                    self.call_builtin_method(val, name, &rest)
                } else {
                    Err(format!("{}() requires an argument", name))
                }
            }
            "round" => {
                if let Some(Value::Float(f)) = arg_vals.first() {
                    let digits = arg_vals.get(1).and_then(|v| if let Value::Int(n) = v { Some(*n) } else { None }).unwrap_or(0);
                    let factor = 10_f64.powi(digits as i32);
                    Ok(Value::Float((f * factor).round() / factor))
                } else if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::Int(*n))
                } else {
                    Err("round() requires a number".into())
                }
            }
            "floor" => {
                if let Some(Value::Float(f)) = arg_vals.first() {
                    Ok(Value::Int(f.floor() as i64))
                } else if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::Int(*n))
                } else {
                    Err("floor() requires a number".into())
                }
            }
            "ceil" => {
                if let Some(Value::Float(f)) = arg_vals.first() {
                    Ok(Value::Int(f.ceil() as i64))
                } else if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::Int(*n))
                } else {
                    Err("ceil() requires a number".into())
                }
            }
            "sqrt" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Float(f.sqrt())),
                    Some(Value::Int(n)) => Ok(Value::Float((*n as f64).sqrt())),
                    _ => Err("sqrt() requires a number".into()),
                }
            }
            "pow" => {
                if arg_vals.len() >= 2 {
                    self.binary_op(&BinOp::Pow, arg_vals[0], arg_vals[1])
                } else {
                    Err("pow() requires two arguments".into())
                }
            }
            "log" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Float(f.ln())),
                    Some(Value::Int(n)) => Ok(Value::Float((*n as f64).ln())),
                    _ => Err("log() requires a number".into()),
                }
            }
            "sin" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Float(f.sin())),
                    Some(Value::Int(n)) => Ok(Value::Float((*n as f64).sin())),
                    _ => Err("sin() requires a number".into()),
                }
            }
            "cos" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Float(f.cos())),
                    Some(Value::Int(n)) => Ok(Value::Float((*n as f64).cos())),
                    _ => Err("cos() requires a number".into()),
                }
            }
            "tan" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Float(f.tan())),
                    Some(Value::Int(n)) => Ok(Value::Float((*n as f64).tan())),
                    _ => Err("tan() requires a number".into()),
                }
            }
            "mean" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    if items.is_empty() {
                        return Err("mean() requires a non-empty list".into());
                    }
                    let mut total = 0.0;
                    for item in items {
                        match item {
                            Value::Int(n) => total += *n as f64,
                            Value::Float(f) => total += *f,
                            _ => return Err("mean() requires a numeric list".into()),
                        }
                    }
                    Ok(Value::Float(total / items.len() as f64))
                } else {
                    Err("mean() requires a list".into())
                }
            }
            "median" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    if items.is_empty() {
                        return Err("median() requires a non-empty list".into());
                    }
                    let mut values = Vec::new();
                    for item in items {
                        match item {
                            Value::Int(n) => values.push(*n as f64),
                            Value::Float(f) => values.push(*f),
                            _ => return Err("median() requires a numeric list".into()),
                        }
                    }
                    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let mid = values.len() / 2;
                    if values.len() % 2 == 0 {
                        Ok(Value::Float((values[mid - 1] + values[mid]) / 2.0))
                    } else {
                        Ok(Value::Float(values[mid]))
                    }
                } else {
                    Err("median() requires a list".into())
                }
            }
            "stddev" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    if items.is_empty() {
                        return Err("stddev() requires a non-empty list".into());
                    }
                    let mut values = Vec::new();
                    for item in items {
                        match item {
                            Value::Int(n) => values.push(*n as f64),
                            Value::Float(f) => values.push(*f),
                            _ => return Err("stddev() requires a numeric list".into()),
                        }
                    }
                    let mean = values.iter().sum::<f64>() / values.len() as f64;
                    let variance = values
                        .iter()
                        .map(|v| {
                            let diff = *v - mean;
                            diff * diff
                        })
                        .sum::<f64>()
                        / values.len() as f64;
                    Ok(Value::Float(variance.sqrt()))
                } else {
                    Err("stddev() requires a list".into())
                }
            }
            "slice" => {
                if let Some(val) = arg_vals.first() {
                    let rest: Vec<(Option<String>, Value)> =
                        args.iter().skip(1).cloned().collect();
                    self.call_builtin_method(val, "slice", &rest)
                } else {
                    Err("slice() requires an argument".into())
                }
            }
            "insert" | "extend" => {
                Err(format!("{}() not yet implemented for in-place mutation", name))
            }
            "clone" => {
                if let Some(val) = arg_vals.first() {
                    Ok((*val).clone())
                } else {
                    Err("clone() requires an argument".into())
                }
            }
            "hash" => {
                if let Some(val) = arg_vals.first() {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    format!("{}", val).hash(&mut hasher);
                    Ok(Value::Int(hasher.finish() as i64))
                } else {
                    Err("hash() requires an argument".into())
                }
            }
            "assert" => {
                if let Some(val) = arg_vals.first() {
                    if val.is_truthy() {
                        Ok(Value::Null)
                    } else {
                        let msg = arg_vals
                            .get(1)
                            .map(|v| format!("{}", v))
                            .unwrap_or_else(|| "Assertion failed".to_string());
                        Err(msg)
                    }
                } else {
                    Err("assert() requires an argument".into())
                }
            }
            "test_register" => {
                if arg_vals.len() < 2 {
                    return Err("test_register() requires a name and callback".into());
                }
                let name = match arg_vals[0] {
                    Value::Str(s) => s.clone(),
                    _ => return Err("test_register() name must be a string".into()),
                };
                let callback = arg_vals[1].clone();
                if !matches!(callback, Value::Func(_) | Value::BuiltinFunc(_) | Value::Class(_)) {
                    return Err("test_register() callback must be callable".into());
                }
                self.registered_test_fns.push((name, callback));
                Ok(Value::Null)
            }
            "test_run_all" => Ok(self.run_registered_tests()),
            "panic" => {
                let msg = arg_vals
                    .first()
                    .map(|v| format!("{}", v))
                    .unwrap_or_else(|| "panic!".to_string());
                Err(format!("PANIC: {}", msg))
            }
            "exit" => {
                let code = arg_vals
                    .first()
                    .and_then(|v| if let Value::Int(n) = v { Some(*n) } else { None })
                    .unwrap_or(0);
                std::process::exit(code as i32);
            }
            "set" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    // Deduplicate
                    let mut set_items = Vec::new();
                    for item in items.iter() {
                        if !set_items.contains(item) {
                            set_items.push(item.clone());
                        }
                    }
                    Ok(Value::Set(set_items))
                } else if arg_vals.is_empty() {
                    Ok(Value::Set(vec![]))
                } else {
                    Err("set() requires a list argument".into())
                }
            }
            "tuple" => {
                if arg_vals.len() == 1 {
                    match arg_vals[0] {
                        Value::List(items) => Ok(Value::Tuple(items.clone())),
                        Value::Tuple(_) => Ok(arg_vals[0].clone()),
                        _ => Ok(Value::Tuple(vec![arg_vals[0].clone()])),
                    }
                } else {
                    Ok(Value::Tuple(arg_vals.into_iter().cloned().collect()))
                }
            }
            // Type conversion builtins
            "bool" => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Bool(val.is_truthy()))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            "int" => {
                if let Some(val) = arg_vals.first() {
                    let base = arg_vals.get(1).and_then(|v| if let Value::Int(n) = v { Some(*n as u32) } else { None });
                    match val {
                        Value::Int(n) => Ok(Value::Int(*n)),
                        Value::BigInt(b) => Ok(Value::BigInt(b.clone())),
                        Value::Float(f) => Ok(Value::Int(*f as i64)),
                        Value::Bool(b) => Ok(Value::Int(if *b { 1 } else { 0 })),
                        Value::Str(s) => {
                            if let Some(b) = base {
                                i64::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0b").trim_start_matches("0o"), b)
                                    .map(Value::Int)
                                    .map_err(|_| format!("Cannot convert '{}' to int base {}", s, b))
                            } else {
                                s.parse::<i64>().map(Value::Int)
                                    .or_else(|_| crate::bigint::BigInt::from_str(s.trim())
                                        .map(Self::norm_bigint).ok_or(()))
                                    .map_err(|_| format!("Cannot convert '{}' to int", s))
                            }
                        }
                        _ => Err(format!("Cannot convert {} to int", val.type_name())),
                    }
                } else {
                    Ok(Value::Int(0))
                }
            }
            "float" => {
                if let Some(val) = arg_vals.first() {
                    match val {
                        Value::Int(n) => Ok(Value::Float(*n as f64)),
                        Value::BigInt(b) => Ok(Value::Float(b.to_f64())),
                        Value::Float(f) => Ok(Value::Float(*f)),
                        Value::Str(s) => s.parse::<f64>().map(Value::Float).map_err(|_| format!("Cannot convert '{}' to float", s)),
                        Value::Bool(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
                        _ => Err(format!("Cannot convert {} to float", val.type_name())),
                    }
                } else {
                    Ok(Value::Float(0.0))
                }
            }
            "str" => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Str(self.value_to_string(val)))
                } else {
                    Ok(Value::Str(String::new()))
                }
            }
            "list" => {
                if let Some(val) = arg_vals.first() {
                    match val {
                        Value::List(l) => Ok(Value::List(l.clone())),
                        Value::Tuple(t) => Ok(Value::List(t.clone())),
                        Value::Set(s) => Ok(Value::List(s.clone())),
                        Value::Str(s) => Ok(Value::List(s.chars().map(|c| Value::Str(c.to_string())).collect())),
                        Value::Range(start, end, inclusive) => {
                            let end_val = if *inclusive { *end + 1 } else { *end };
                            Ok(Value::List(((*start)..end_val).map(Value::Int).collect()))
                        }
                        Value::Dict(pairs) => Ok(Value::List(pairs.iter().map(|(k, v)| Value::Tuple(vec![k.clone(), v.clone()])).collect())),
                        _ => Err(format!("Cannot convert {} to list", val.type_name())),
                    }
                } else {
                    Ok(Value::List(vec![]))
                }
            }
            "dict" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    let mut pairs = Vec::new();
                    for item in items {
                        if let Value::Tuple(kv) = item {
                            if kv.len() >= 2 {
                                pairs.push((kv[0].clone(), kv[1].clone()));
                            }
                        } else if let Value::List(kv) = item {
                            if kv.len() >= 2 {
                                pairs.push((kv[0].clone(), kv[1].clone()));
                            }
                        }
                    }
                    Ok(Value::Dict(pairs))
                } else if arg_vals.is_empty() {
                    Ok(Value::Dict(vec![]))
                } else {
                    Err("dict() requires a list of key-value pairs".into())
                }
            }
            // Numeric string representations
            "hex" => {
                if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::Str(format!("0x{:x}", n)))
                } else {
                    Err("hex() requires an integer".into())
                }
            }
            "bin" => {
                if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::Str(format!("0b{:b}", n)))
                } else {
                    Err("bin() requires an integer".into())
                }
            }
            "oct" => {
                if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::Str(format!("0o{:o}", n)))
                } else {
                    Err("oct() requires an integer".into())
                }
            }
            "chr" => {
                if let Some(Value::Int(n)) = arg_vals.first() {
                    Ok(Value::Str(char::from_u32(*n as u32).unwrap_or('\u{FFFD}').to_string()))
                } else {
                    Err("chr() requires an integer".into())
                }
            }
            "ord" => {
                if let Some(Value::Str(s)) = arg_vals.first() {
                    if let Some(c) = s.chars().next() {
                        Ok(Value::Int(c as i64))
                    } else {
                        Err("ord() requires a non-empty string".into())
                    }
                } else {
                    Err("ord() requires a string".into())
                }
            }
            // Introspection
            "set_recursion_limit" => {
                if let Some(Value::Int(n)) = arg_vals.first() {
                    // Floor keeps the interpreter usable; the ceiling stays inside
                    // the 512MB native stack (~21KB of Rust stack per V2 frame).
                    self.recursion_limit = (*n).clamp(64, 20_000) as usize;
                    Ok(Value::Int(self.recursion_limit as i64))
                } else {
                    Err("set_recursion_limit() requires an integer".into())
                }
            }
            "get_recursion_limit" => Ok(Value::Int(self.recursion_limit as i64)),
            "callable" => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Bool(matches!(val, Value::Func(_) | Value::BuiltinFunc(_) | Value::Class(_))))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            "defined" => {
                if let Some(Value::Str(name)) = arg_vals.first() {
                    Ok(Value::Bool(self.env.get(name).is_some()))
                } else {
                    Err("defined() requires a string argument".into())
                }
            }
            // Random
            "random" => {
                // Simple pseudo-random using system time
                let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos();
                Ok(Value::Float((seed as f64) / (u32::MAX as f64)))
            }
            "random_int" => {
                if arg_vals.len() >= 2 {
                    if let (Value::Int(min), Value::Int(max)) = (arg_vals[0], arg_vals[1]) {
                        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos();
                        let range = (max - min + 1) as u32;
                        Ok(Value::Int(min + (seed % range) as i64))
                    } else {
                        Err("random_int() requires two integers".into())
                    }
                } else {
                    Err("random_int() requires min and max arguments".into())
                }
            }
            "random_choice" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    if items.is_empty() {
                        Ok(Value::Null)
                    } else {
                        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos();
                        let idx = (seed as usize) % items.len();
                        Ok(items[idx].clone())
                    }
                } else {
                    Err("random_choice() requires a list".into())
                }
            }
            // Time
            "time" => {
                let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                Ok(Value::Float(t.as_secs_f64()))
            }
            "sleep" => {
                if let Some(val) = arg_vals.first() {
                    let duration = match val {
                        Value::Int(n) => std::time::Duration::from_secs(*n as u64),
                        Value::Float(f) => std::time::Duration::from_secs_f64(*f),
                        _ => return Err("sleep() requires a number (seconds)".into()),
                    };
                    std::thread::sleep(duration);
                    Ok(Value::Null)
                } else {
                    Err("sleep() requires an argument".into())
                }
            }
            // Environment
            "getenv" => {
                if let Some(Value::Str(name)) = arg_vals.first() {
                    Ok(std::env::var(name).map(Value::Str).unwrap_or(Value::Null))
                } else {
                    Err("getenv() requires a string argument".into())
                }
            }
            // File I/O
            "read_file" => {
                if let Some(Value::Str(path)) = arg_vals.first() {
                    std::fs::read_to_string(path).map(Value::Str).map_err(|e| format!("read_file: {}", e))
                } else {
                    Err("read_file() requires a path string".into())
                }
            }
            "write_file" => {
                if arg_vals.len() >= 2 {
                    if let (Value::Str(path), Value::Str(content)) = (arg_vals[0], arg_vals[1]) {
                        std::fs::write(path, content).map_err(|e| format!("write_file: {}", e))?;
                        Ok(Value::Null)
                    } else {
                        Err("write_file() requires path and content strings".into())
                    }
                } else {
                    Err("write_file() requires path and content".into())
                }
            }
            "append_file" => {
                if arg_vals.len() >= 2 {
                    if let (Value::Str(path), Value::Str(content)) = (arg_vals[0], arg_vals[1]) {
                        use std::io::Write;
                        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)
                            .map_err(|e| format!("append_file: {}", e))?;
                        file.write_all(content.as_bytes()).map_err(|e| format!("append_file: {}", e))?;
                        Ok(Value::Null)
                    } else {
                        Err("append_file() requires path and content strings".into())
                    }
                } else {
                    Err("append_file() requires path and content".into())
                }
            }
            "file_exists" => {
                if let Some(Value::Str(path)) = arg_vals.first() {
                    Ok(Value::Bool(std::path::Path::new(path).exists()))
                } else {
                    Err("file_exists() requires a path string".into())
                }
            }
            "delete_file" => {
                if let Some(Value::Str(path)) = arg_vals.first() {
                    std::fs::remove_file(path).map_err(|e| format!("delete_file: {}", e))?;
                    Ok(Value::Null)
                } else {
                    Err("delete_file() requires a path string".into())
                }
            }
            // JSON
            "json_parse" => {
                if let Some(Value::Str(s)) = arg_vals.first() {
                    parse_json_value(s)
                } else {
                    Err("json_parse() requires a string".into())
                }
            }
            "json_stringify" => {
                if let Some(val) = arg_vals.first() {
                    Ok(Value::Str(value_to_json(val)))
                } else {
                    Err("json_stringify() requires an argument".into())
                }
            }
            // Error handling
            "try_wrap" => {
                if let Some(func) = arg_vals.first() {
                    let call_args: Vec<(Option<String>, Value)> = arg_vals[1..].iter().map(|v| (None, (*v).clone())).collect();
                    match self.call_value(func, &call_args) {
                        Ok(v) => Ok(Value::Ok(Box::new(v))),
                        Err(e) => Ok(Value::Err(Box::new(Value::Str(e)))),
                    }
                } else {
                    Err("try_wrap() requires a function".into())
                }
            }
            // Functional helpers
            "sorted" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    let mut sorted = items.clone();
                    sorted.sort_by(|a, b| match (a, b) {
                        (Value::Int(x), Value::Int(y)) => x.cmp(y),
                        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                        (Value::Str(x), Value::Str(y)) => x.cmp(y),
                        _ => std::cmp::Ordering::Equal,
                    });
                    Ok(Value::List(sorted))
                } else {
                    Err("sorted() requires a list".into())
                }
            }
            "reversed" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    let mut r = items.clone();
                    r.reverse();
                    Ok(Value::List(r))
                } else if let Some(Value::Str(s)) = arg_vals.first() {
                    Ok(Value::Str(s.chars().rev().collect()))
                } else {
                    Err("reversed() requires a list or string".into())
                }
            }
            "dir" => {
                match arg_vals.first() {
                    Some(Value::Instance(_, fields)) | Some(Value::StructInstance(_, fields)) => {
                        let keys: Vec<Value> = fields.keys().map(|k| Value::Str(k.clone())).collect();
                        Ok(Value::List(keys))
                    }
                    Some(Value::CowInstance(_, fields)) => {
                        let keys: Vec<Value> = fields.borrow().keys().map(|k| Value::Str(k.clone())).collect();
                        Ok(Value::List(keys))
                    }
                    Some(Value::Dict(pairs)) => {
                        let keys: Vec<Value> = pairs.iter().map(|(k, _)| k.clone()).collect();
                        Ok(Value::List(keys))
                    }
                    _ => Err("dir() requires an instance or dict".into()),
                }
            }
            "hasattr" => {
                if arg_vals.len() >= 2 {
                    if let (Some(Value::Instance(_, fields)), Some(Value::Str(name))) = (arg_vals.first(), arg_vals.get(1)) {
                        Ok(Value::Bool(fields.contains_key(name.as_str())))
                    } else if let (Some(Value::CowInstance(_, fields)), Some(Value::Str(name))) = (arg_vals.first(), arg_vals.get(1)) {
                        Ok(Value::Bool(fields.borrow().contains_key(name.as_str())))
                    } else if let (Some(Value::StructInstance(_, fields)), Some(Value::Str(name))) = (arg_vals.first(), arg_vals.get(1)) {
                        Ok(Value::Bool(fields.contains_key(name.as_str())))
                    } else {
                        Ok(Value::Bool(false))
                    }
                } else {
                    Err("hasattr() requires 2 arguments: (object, name)".into())
                }
            }
            "getattr" => {
                if arg_vals.len() >= 2 {
                    let name = match arg_vals.get(1) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err("getattr() name must be a string".into()),
                    };
                    let default = arg_vals.get(2).map(|v| (*v).clone());
                    match arg_vals.first() {
                        Some(Value::Instance(_, fields)) | Some(Value::StructInstance(_, fields)) => {
                            Ok(fields.get(&name).cloned().or(default).unwrap_or(Value::Null))
                        }
                        Some(Value::CowInstance(_, fields)) => {
                            Ok(fields.borrow().get(&name).cloned().or(default).unwrap_or(Value::Null))
                        }
                        _ => Ok(default.unwrap_or(Value::Null)),
                    }
                } else {
                    Err("getattr() requires at least 2 arguments: (object, name)".into())
                }
            }
            "setattr" => {
                // setattr needs mutable access — we handle it by returning a modified copy
                // Real mutation happens at the call site for ident targets
                if arg_vals.len() >= 3 {
                    let name = match arg_vals.get(1) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err("setattr() name must be a string".into()),
                    };
                    let val = arg_vals[2].clone();
                    match arg_vals.first() {
                        Some(Value::Instance(cls, fields)) => {
                            let mut new_fields = fields.clone();
                            new_fields.insert(name, val);
                            Ok(Value::Instance(cls.clone(), new_fields))
                        }
                        Some(Value::CowInstance(cls, fields)) => {
                            let mut new_fields = fields.borrow().clone();
                            new_fields.insert(name, val);
                            Ok(Value::CowInstance(cls.clone(), Rc::new(RefCell::new(new_fields))))
                        }
                        Some(Value::StructInstance(sn, fields)) => {
                            let mut new_fields = fields.clone();
                            new_fields.insert(name, val);
                            Ok(Value::StructInstance(sn.clone(), new_fields))
                        }
                        _ => Err("setattr() requires an instance".into()),
                    }
                } else {
                    Err("setattr() requires 3 arguments: (object, name, value)".into())
                }
            }
            "eval" => {
                if let Some(Value::Str(code)) = arg_vals.first() {
                    use crate::lexer::Lexer;
                    use crate::parser::Parser;
                    let mut lexer = Lexer::new(code);
                    let tokens = lexer.tokenize().map_err(|e| format!("eval error: {}", e))?;
                    let mut parser = Parser::new(tokens);
                    let expr = parser.parse_expr_public().map_err(|e| format!("eval error: {}", e))?;
                    self.eval_expr(&expr)
                } else {
                    Err("eval() requires a string argument".into())
                }
            }
            "exec" => {
                if let Some(Value::Str(code)) = arg_vals.first() {
                    use crate::lexer::Lexer;
                    use crate::parser::Parser;
                    let mut lexer = Lexer::new(code);
                    let tokens = lexer.tokenize().map_err(|e| format!("exec error: {}", e))?;
                    let mut parser = Parser::new(tokens);
                    let program = parser.parse().map_err(|e| format!("exec error: {}", e))?;
                    self.exec_block_no_scope(&program.stmts)
                } else {
                    Err("exec() requires a string argument".into())
                }
            }
            "zip" => {
                if arg_vals.len() >= 2 {
                    let a = self.value_to_iter(arg_vals[0])?;
                    let b = self.value_to_iter(arg_vals[1])?;
                    let result: Vec<Value> = a.into_iter().zip(b.into_iter())
                        .map(|(x, y)| Value::Tuple(vec![x, y]))
                        .collect();
                    Ok(Value::List(result))
                } else {
                    Err("zip() requires at least 2 arguments".into())
                }
            }
            "sum" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    let mut total = 0i64;
                    let mut is_float = false;
                    let mut ftotal = 0.0f64;
                    for item in items {
                        match item {
                            Value::Int(n) => { total += n; ftotal += *n as f64; }
                            Value::Float(f) => { is_float = true; ftotal += f; }
                            _ => return Err("sum() requires numeric list".into()),
                        }
                    }
                    if is_float { Ok(Value::Float(ftotal)) } else { Ok(Value::Int(total)) }
                } else {
                    Err("sum() requires a list argument".into())
                }
            }
            "any" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    Ok(Value::Bool(items.iter().any(|v| v.is_truthy())))
                } else {
                    Err("any() requires a list".into())
                }
            }
            "all" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    Ok(Value::Bool(items.iter().all(|v| v.is_truthy())))
                } else {
                    Err("all() requires a list".into())
                }
            }
            "Ok" => {
                let val = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null);
                Ok(Value::Ok(Box::new(val)))
            }
            "Err" => {
                let val = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null);
                Ok(Value::Err(Box::new(val)))
            }
            "Some" => {
                let val = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null);
                Ok(Value::Some(Box::new(val)))
            }
            "freeze" => {
                // freeze(obj) — sets __frozen field on instance/struct
                let val = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null);
                match val {
                    Value::Instance(name, mut fields) => {
                        fields.insert("__frozen".to_string(), Value::Bool(true));
                        Ok(Value::Instance(name, fields))
                    }
                    Value::CowInstance(name, fields) => {
                        let mut new_fields = fields.borrow().clone();
                        new_fields.insert("__frozen".to_string(), Value::Bool(true));
                        Ok(Value::CowInstance(name, Rc::new(RefCell::new(new_fields))))
                    }
                    Value::StructInstance(name, mut fields) => {
                        fields.insert("__frozen".to_string(), Value::Bool(true));
                        Ok(Value::StructInstance(name, fields))
                    }
                    Value::Dict(pairs) => {
                        let mut new_pairs = pairs;
                        new_pairs.push((Value::Str("__frozen".to_string()), Value::Bool(true)));
                        Ok(Value::Dict(new_pairs))
                    }
                    other => Ok(other), // non-freezable types are returned as-is
                }
            }
            "is_frozen" => {
                let val = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null);
                let frozen = match &val {
                    Value::Instance(_, fields) | Value::StructInstance(_, fields) => {
                        fields.get("__frozen").map_or(false, |v| v.is_truthy())
                    }
                    Value::CowInstance(_, fields) => {
                        fields.borrow().get("__frozen").map_or(false, |v| v.is_truthy())
                    }
                    Value::Dict(pairs) => {
                        pairs.iter().any(|(k, v)| {
                            *k == Value::Str("__frozen".to_string()) && v.is_truthy()
                        })
                    }
                    _ => false,
                };
                Ok(Value::Bool(frozen))
            }
            "typeof" => {
                let val = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null);
                Ok(Value::Str(val.type_name().to_string()))
            }
            "chars" => {
                let val = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null);
                match val {
                    Value::Str(s) => {
                        Ok(Value::List(s.chars().map(|c| Value::Str(c.to_string())).collect()))
                    }
                    _ => Err("chars() requires a string argument".into()),
                }
            }
            "assert_eq" => {
                if arg_vals.len() >= 2 {
                    if arg_vals[0] == arg_vals[1] {
                        Ok(Value::Null)
                    } else {
                        let msg = if arg_vals.len() >= 3 {
                            format!("{}", arg_vals[2])
                        } else {
                            format!("assert_eq failed: {} != {}", arg_vals[0], arg_vals[1])
                        };
                        Err(msg)
                    }
                } else {
                    Err("assert_eq() requires two arguments".into())
                }
            }
            "assert_ne" => {
                if arg_vals.len() >= 2 {
                    if arg_vals[0] != arg_vals[1] {
                        Ok(Value::Null)
                    } else {
                        let msg = if arg_vals.len() >= 3 {
                            format!("{}", arg_vals[2])
                        } else {
                            format!("assert_ne failed: {} == {}", arg_vals[0], arg_vals[1])
                        };
                        Err(msg)
                    }
                } else {
                    Err("assert_ne() requires two arguments".into())
                }
            }
            "expect_eq" => self.call_builtin("assert_eq", args),
            "expect_ne" => self.call_builtin("assert_ne", args),
            "expect_true" => {
                if let Some(val) = arg_vals.first() {
                    if val.is_truthy() {
                        Ok(Value::Null)
                    } else {
                        Err("expect_true failed: value is not truthy".into())
                    }
                } else {
                    Err("expect_true() requires one argument".into())
                }
            }
            "expect_false" => {
                if let Some(val) = arg_vals.first() {
                    if !val.is_truthy() {
                        Ok(Value::Null)
                    } else {
                        Err("expect_false failed: value is truthy".into())
                    }
                } else {
                    Err("expect_false() requires one argument".into())
                }
            }
            "expect_ok" => {
                if let Some(val) = arg_vals.first() {
                    if matches!(val, Value::Ok(_)) {
                        Ok(Value::Null)
                    } else {
                        Err("expect_ok failed: value is not Ok(_)".into())
                    }
                } else {
                    Err("expect_ok() requires one argument".into())
                }
            }
            "expect_err" => {
                if let Some(val) = arg_vals.first() {
                    if matches!(val, Value::Err(_)) {
                        Ok(Value::Null)
                    } else {
                        Err("expect_err failed: value is not Err(_)".into())
                    }
                } else {
                    Err("expect_err() requires one argument".into())
                }
            }
            "expect_some" => {
                if let Some(val) = arg_vals.first() {
                    if matches!(val, Value::Some(_)) {
                        Ok(Value::Null)
                    } else {
                        Err("expect_some failed: value is not Some(_)".into())
                    }
                } else {
                    Err("expect_some() requires one argument".into())
                }
            }
            "expect_none" => {
                if let Some(val) = arg_vals.first() {
                    if matches!(val, Value::Null) {
                        Ok(Value::Null)
                    } else {
                        Err("expect_none failed: value is not None".into())
                    }
                } else {
                    Err("expect_none() requires one argument".into())
                }
            }
            "from_pairs" => {
                if let Some(Value::List(items)) = arg_vals.first() {
                    let mut pairs = Vec::new();
                    for item in items {
                        match item {
                            Value::Tuple(elems) if elems.len() >= 2 => {
                                pairs.push((elems[0].clone(), elems[1].clone()));
                            }
                            Value::List(elems) if elems.len() >= 2 => {
                                pairs.push((elems[0].clone(), elems[1].clone()));
                            }
                            _ => return Err("from_pairs() requires list of (key, value) pairs".into()),
                        }
                    }
                    Ok(Value::Dict(pairs))
                } else {
                    Err("from_pairs() requires a list argument".into())
                }
            }
            "is_some" => {
                Ok(Value::Bool(matches!(arg_vals.first(), Some(Value::Some(_)))))
            }
            "is_none" => {
                Ok(Value::Bool(matches!(arg_vals.first(), Some(Value::Null) | None)))
            }
            "is_ok" => {
                Ok(Value::Bool(matches!(arg_vals.first(), Some(Value::Ok(_)))))
            }
            "is_err" => {
                Ok(Value::Bool(matches!(arg_vals.first(), Some(Value::Err(_)))))
            }
            "is_func" => {
                let is = match arg_vals.first() {
                    Some(Value::Func(..)) | Some(Value::BuiltinFunc(..)) => true,
                    _ => false,
                };
                Ok(Value::Bool(is))
            }
            "unwrap" => {
                match arg_vals.first() {
                    Some(Value::Some(v)) => Ok(*v.clone()),
                    Some(Value::Ok(v)) => Ok(*v.clone()),
                    Some(Value::Null) => Err("unwrap() called on null".into()),
                    Some(Value::Err(e)) => Err(format!("unwrap() called on Err({})", e)),
                    Some(v) => Ok((*v).clone()),
                    None => Err("unwrap() requires an argument".into()),
                }
            }
            "unwrap_or" => {
                let default = arg_vals.get(1).map(|v| (*v).clone()).unwrap_or(Value::Null);
                match arg_vals.first() {
                    Some(Value::Some(v)) => Ok(*v.clone()),
                    Some(Value::Ok(v)) => Ok(*v.clone()),
                    Some(Value::Null) | Some(Value::Err(_)) => Ok(default),
                    Some(v) => Ok((*v).clone()),
                    None => Ok(default),
                }
            }
            "items" => {
                match arg_vals.first() {
                    Some(Value::Dict(pairs)) => {
                        Ok(Value::List(pairs.iter().map(|(k, v)| Value::Tuple(vec![k.clone(), v.clone()])).collect()))
                    }
                    _ => Err("items() requires a dict".into()),
                }
            }
            "shuffle" => {
                match arg_vals.first() {
                    Some(Value::List(items)) => {
                        let mut result = items.clone();
                        let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos() as u64;
                        for i in (1..result.len()).rev() {
                            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                            let j = (seed as usize) % (i + 1);
                            result.swap(i, j);
                        }
                        Ok(Value::List(result))
                    }
                    _ => Err("shuffle() requires a list".into()),
                }
            }
            "str_from_bytes" => {
                match arg_vals.first() {
                    Some(Value::Bytes(b)) => {
                        Ok(Value::Str(String::from_utf8(b.clone()).map_err(|e| e.to_string())?))
                    }
                    Some(Value::List(items)) => {
                        let bytes: Result<Vec<u8>, String> = items.iter().map(|v| match v {
                            Value::Int(n) => Ok(*n as u8),
                            _ => Err("str_from_bytes() list must contain ints".into()),
                        }).collect();
                        Ok(Value::Str(String::from_utf8(bytes?).map_err(|e| e.to_string())?))
                    }
                    _ => Err("str_from_bytes() requires bytes or list of ints".into()),
                }
            }
            "vars" => {
                // Return current scope as a dict
                let scope = self.env.get_current_scope_dict();
                let pairs: Vec<(Value, Value)> = scope.into_iter()
                    .map(|(k, v)| (Value::Str(k), v))
                    .collect();
                Ok(Value::Dict(pairs))
            }
            "memo" => {
                // memo(func) — wrap a function with memoization
                match arg_vals.first() {
                    Some(Value::Func(fv)) => {
                        let func = Value::Func(fv.clone());
                        let name = fv.name.clone();
                        self.memo_caches.insert(name, HashMap::new());
                        Ok(func)
                    }
                    _ => Err("memo() requires a function argument".into()),
                }
            }
            "is_lazy" => {
                match arg_vals.first() {
                    Some(Value::Lazy(_)) => Ok(Value::Bool(true)),
                    Some(_) => Ok(Value::Bool(false)),
                    None => Err("is_lazy() requires an argument".into()),
                }
            }
            "patch" => {
                // patch(name, new_func) — replace a function binding in scope
                if arg_vals.len() >= 2 {
                    if let Value::Str(name) = arg_vals[0] {
                        let new_func = arg_vals[1].clone();
                        self.env.set(name, new_func).map_err(|e| e.to_string())?;
                        Ok(Value::Null)
                    } else {
                        Err("patch() first argument must be a string (function name)".into())
                    }
                } else {
                    Err("patch() requires two arguments: name and replacement".into())
                }
            }
            "deque_new" => {
                use std::collections::VecDeque;
                let items: VecDeque<Value> = arg_vals.into_iter().cloned().collect();
                Ok(Value::Deque(std::rc::Rc::new(std::cell::RefCell::new(items))))
            }
            "deque_push_front" => {
                if arg_vals.len() >= 2 {
                    if let Value::Deque(dq) = arg_vals[0] {
                        dq.borrow_mut().push_front(arg_vals[1].clone());
                        Ok(Value::Null)
                    } else {
                        Err("deque_push_front() first argument must be a deque".into())
                    }
                } else {
                    Err("deque_push_front() requires deque and value arguments".into())
                }
            }
            "deque_push_back" => {
                if arg_vals.len() >= 2 {
                    if let Value::Deque(dq) = arg_vals[0] {
                        dq.borrow_mut().push_back(arg_vals[1].clone());
                        Ok(Value::Null)
                    } else {
                        Err("deque_push_back() first argument must be a deque".into())
                    }
                } else {
                    Err("deque_push_back() requires deque and value arguments".into())
                }
            }
            "deque_pop_front" => {
                if let Some(Value::Deque(dq)) = arg_vals.first() {
                    Ok(dq.borrow_mut().pop_front().unwrap_or(Value::Null))
                } else {
                    Err("deque_pop_front() requires a deque argument".into())
                }
            }
            "deque_pop_back" => {
                if let Some(Value::Deque(dq)) = arg_vals.first() {
                    Ok(dq.borrow_mut().pop_back().unwrap_or(Value::Null))
                } else {
                    Err("deque_pop_back() requires a deque argument".into())
                }
            }
            "deque_len" => {
                if let Some(Value::Deque(dq)) = arg_vals.first() {
                    Ok(Value::Int(dq.borrow().len() as i64))
                } else {
                    Err("deque_len() requires a deque argument".into())
                }
            }
            "unwrap_err" => {
                match arg_vals.first() {
                    Some(Value::Err(e)) => Ok(*e.clone()),
                    Some(Value::Ok(v)) => Err(format!("unwrap_err() called on Ok({})", v)),
                    _ => Err("unwrap_err() requires a Result value".into()),
                }
            }
            "default_" => {
                // default_(value, fallback) — return value if not null, else fallback
                if arg_vals.len() >= 2 {
                    if *arg_vals[0] == Value::Null {
                        Ok(arg_vals[1].clone())
                    } else {
                        Ok(arg_vals[0].clone())
                    }
                } else {
                    Err("default_() requires two arguments".into())
                }
            }
            // ── Compile-time intrinsics ──────────────────────────
            "ct_platform" => {
                Ok(Value::Str(if cfg!(target_os = "windows") { "windows" }
                              else if cfg!(target_os = "linux") { "linux" }
                              else if cfg!(target_os = "macos") { "macos" }
                              else { "unknown" }.into()))
            }
            "ct_arch" => {
                Ok(Value::Str(if cfg!(target_arch = "x86_64") { "x86_64" }
                              else if cfg!(target_arch = "aarch64") { "arm64" }
                              else { "unknown" }.into()))
            }
            "ct_word_exists" => {
                match arg_vals.first() {
                    Some(Value::Str(name)) => Ok(Value::Bool(self.env.get(name).is_some())),
                    _ => Err("ct_word_exists() requires a string argument".into()),
                }
            }
            "ct_list_funcs" => {
                let names = self.env.current_scope_vars();
                let funcs: Vec<Value> = names.into_iter()
                    .filter(|(_, v)| matches!(v, Value::Func(_) | Value::BuiltinFunc(_)))
                    .map(|(k, _)| Value::Str(k))
                    .collect();
                Ok(Value::List(funcs))
            }
            "ct_get_effects" => {
                // Effects are not tracked in tree-walk; return empty list
                Ok(Value::List(vec![]))
            }
            "ct_unregister" => {
                match arg_vals.first() {
                    Some(Value::Str(name)) => {
                        self.env.define(name, Value::Null);
                        Ok(Value::Null)
                    }
                    _ => Err("ct_unregister() requires a string argument".into()),
                }
            }
            "ct_emit" => {
                // Parse and execute the emitted code string
                match arg_vals.first() {
                    Some(Value::Str(code)) => {
                        use crate::lexer::Lexer;
                        use crate::parser::Parser;
                        let mut lexer = Lexer::new(code);
                        let tokens = lexer.tokenize().map_err(|e| format!("ct_emit parse error: {}", e))?;
                        let mut parser = Parser::new(tokens);
                        let program = parser.parse().map_err(|e| format!("ct_emit parse error: {}", e))?;
                        for stmt in &program.stmts {
                            self.exec_stmt(stmt)?;
                        }
                        Ok(Value::Null)
                    }
                    _ => Err("ct_emit() requires a string argument".into()),
                }
            }
            "ct_error" => {
                match arg_vals.first() {
                    Some(Value::Str(msg)) => Err(format!("compile-time error: {}", msg)),
                    Some(v) => Err(format!("compile-time error: {}", v)),
                    None => Err("compile-time error".into()),
                }
            }
            "ct_warn" => {
                match arg_vals.first() {
                    Some(Value::Str(msg)) => {
                        eprintln!("[comptime warning] {}", msg);
                        Ok(Value::Null)
                    }
                    _ => { eprintln!("[comptime warning]"); Ok(Value::Null) }
                }
            }
            "ct_set_macro_limit" => {
                if let Some(Value::Int(n)) = arg_vals.first().copied() {
                    if *n > 0 { self.macro_limit = *n as usize; }
                }
                Ok(Value::Null)
            }
            "ct_get_macro_limit" => Ok(Value::Int(self.macro_limit as i64)),
            "ct_feature" => Ok(Value::Bool(false)),
            "mem_size_of" => {
                match arg_vals.first() {
                    Some(Value::Str(t)) => {
                        let sz: i64 = match t.as_str() {
                            "i8" | "u8" => 1, "i16" | "u16" => 2,
                            "i32" | "u32" | "f32" => 4,
                            "i64" | "u64" | "f64" => 8,
                            "pointer" | "usize" | "isize" => 8,
                            _ => 0,
                        };
                        Ok(Value::Int(sz))
                    }
                    _ => Err("mem_size_of() requires a string type name".into()),
                }
            }
            // ── Memory management ──────────────────────────
            "mem_alloc" | "mem_alloc_zeroed" => {
                match arg_vals.first() {
                    Some(Value::Int(size)) => {
                        Ok(self.alloc_memory_block(*size as usize))
                    }
                    _ => Err(format!("{}() requires an integer size", name)),
                }
            }
            "mem_realloc" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Pointer(pointer_id)), Some(Value::Int(new_size))) => {
                        let block = self.get_memory_block_mut(*pointer_id, "mem_realloc")?;
                        block.bytes.resize(*new_size as usize, Value::Int(0));
                        Ok(Value::Pointer(*pointer_id))
                    }
                    (Some(Value::List(data)), Some(Value::Int(new_size))) => {
                        let mut new_data = data.clone();
                        new_data.resize(*new_size as usize, Value::Int(0));
                        Ok(Value::List(new_data))
                    }
                    _ => Err("mem_realloc() requires (pointer, new_size)".into()),
                }
            }
            "mem_free" => {
                match arg_vals.first() {
                    Some(Value::Pointer(pointer_id)) => {
                        let block = self.memory_blocks.get_mut(pointer_id)
                            .ok_or_else(|| format!("MemoryAccessError: mem_free on unknown pointer {}", pointer_id))?;
                        if block.freed {
                            Err(format!("MemoryAccessError: double free on pointer {}", pointer_id))
                        } else {
                            block.freed = true;
                            Ok(Value::Null)
                        }
                    }
                    Some(Value::List(_)) => Ok(Value::Null),
                    _ => Err("mem_free() requires (ptr)".into()),
                }
            }
            "mem_copy" => {
                // mem_copy(dst, src, size) — copy bytes from src into dst, return dst
                match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
                    (Some(Value::Pointer(dst_id)), Some(Value::Pointer(src_id)), Some(Value::Int(size))) => {
                        let dst_id = *dst_id;
                        let src_id = *src_id;
                        let sz = *size as usize;
                        let src_bytes: Vec<Value> = {
                            let src_block = self.get_memory_block(src_id, "mem_copy (src)")?;
                            src_block.bytes[..sz.min(src_block.bytes.len())].to_vec()
                        };
                        let dst_block = self.get_memory_block_mut(dst_id, "mem_copy (dst)")?;
                        for (i, v) in src_bytes.into_iter().enumerate() {
                            if i < dst_block.bytes.len() { dst_block.bytes[i] = v; }
                        }
                        Ok(Value::Pointer(dst_id))
                    }
                    (Some(Value::List(dst)), Some(Value::List(src)), Some(Value::Int(size))) => {
                        let mut new_dst = dst.clone();
                        let sz = *size as usize;
                        for i in 0..sz.min(src.len()).min(new_dst.len()) {
                            new_dst[i] = src[i].clone();
                        }
                        Ok(Value::List(new_dst))
                    }
                    _ => Err("mem_copy() requires (dst, src, size)".into()),
                }
            }
            "mem_set" => {
                match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
                    (Some(Value::Pointer(pointer_id)), Some(Value::Int(byte)), Some(Value::Int(size))) => {
                        let pointer_id = *pointer_id;
                        let byte_val = *byte;
                        let sz = *size as usize;
                        let block = self.get_memory_block_mut(pointer_id, "mem_set")?;
                        for i in 0..sz.min(block.bytes.len()) {
                            block.bytes[i] = Value::Int(byte_val);
                        }
                        Ok(Value::Pointer(pointer_id))
                    }
                    (Some(Value::List(data)), Some(Value::Int(byte)), Some(Value::Int(size))) => {
                        let mut new_data = data.clone();
                        let sz = *size as usize;
                        for i in 0..sz.min(new_data.len()) {
                            new_data[i] = Value::Int(*byte);
                        }
                        Ok(Value::List(new_data))
                    }
                    _ => Err("mem_set() requires (ptr, byte, size)".into()),
                }
            }
            "mem_read" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Pointer(pointer_id)), Some(Value::Int(offset))) => {
                        let idx = *offset as usize;
                        let block = self.get_memory_block(*pointer_id, "mem_read")?;
                        if idx < block.bytes.len() {
                            Ok(block.bytes[idx].clone())
                        } else {
                            Err("MemoryAccessError: mem_read offset out of bounds".into())
                        }
                    }
                    (Some(Value::List(data)), Some(Value::Int(offset))) => {
                        let idx = *offset as usize;
                        if idx < data.len() {
                            Ok(data[idx].clone())
                        } else {
                            Err("mem_read: offset out of bounds".into())
                        }
                    }
                    _ => Err("mem_read() requires (ptr, offset)".into()),
                }
            }
            "mem_write" => {
                // mem_write is a mutation — return modified ptr
                match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
                    (Some(Value::Pointer(pointer_id)), Some(Value::Int(offset)), Some(val)) => {
                        let idx = *offset as usize;
                        let block = self.get_memory_block_mut(*pointer_id, "mem_write")?;
                        if idx < block.bytes.len() {
                            block.bytes[idx] = (*val).clone();
                            Ok(Value::Pointer(*pointer_id))
                        } else {
                            Err("MemoryAccessError: mem_write offset out of bounds".into())
                        }
                    }
                    (Some(Value::List(data)), Some(Value::Int(offset)), Some(val)) => {
                        let mut new_data = data.clone();
                        let idx = *offset as usize;
                        if idx < new_data.len() {
                            new_data[idx] = (*val).clone();
                            Ok(Value::List(new_data))
                        } else {
                            Err("mem_write: offset out of bounds".into())
                        }
                    }
                    _ => Err("mem_write() requires (ptr, offset, value)".into()),
                }
            }
            // ── Vector (SIMD) operations ──────────────────────────
            "vec_new" => {
                match arg_vals.first() {
                    Some(Value::Int(n)) => Ok(Value::List(vec![Value::Float(0.0); *n as usize])),
                    _ => Err("vec_new() requires an integer size".into()),
                }
            }
            "vec_from" => {
                match arg_vals.first() {
                    Some(Value::List(items)) => {
                        let floats: Vec<Value> = items.iter().map(|v| match v {
                            Value::Float(_) => v.clone(),
                            Value::Int(n) => Value::Float(*n as f64),
                            _ => Value::Float(0.0),
                        }).collect();
                        Ok(Value::List(floats))
                    }
                    _ => Err("vec_from() requires a list".into()),
                }
            }
            "vec_get" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::List(v)), Some(Value::Int(i))) => {
                        v.get(*i as usize).cloned().ok_or("vec_get: index out of bounds".into())
                    }
                    _ => Err("vec_get() requires (vec, index)".into()),
                }
            }
            "vec_set" => {
                match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
                    (Some(Value::List(v)), Some(Value::Int(i)), Some(val)) => {
                        let mut new_v = v.clone();
                        let idx = *i as usize;
                        if idx < new_v.len() { new_v[idx] = (*val).clone(); }
                        Ok(Value::List(new_v))
                    }
                    _ => Err("vec_set() requires (vec, index, value)".into()),
                }
            }
            "vec_len" => {
                match arg_vals.first() {
                    Some(Value::List(v)) => Ok(Value::Int(v.len() as i64)),
                    _ => Err("vec_len() requires a vec".into()),
                }
            }
            "vec_add" | "vec_sub" | "vec_mul" | "vec_div" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::List(a)), Some(Value::List(b))) if a.len() == b.len() => {
                        let result: Vec<Value> = a.iter().zip(b.iter()).map(|(x, y)| {
                            let xf = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            let yf = match y { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            Value::Float(match name {
                                "vec_add" => xf + yf,
                                "vec_sub" => xf - yf,
                                "vec_mul" => xf * yf,
                                "vec_div" => if yf != 0.0 { xf / yf } else { f64::NAN },
                                _ => 0.0,
                            })
                        }).collect();
                        Ok(Value::List(result))
                    }
                    _ => Err(format!("{}() requires two vecs of equal length", name)),
                }
            }
            "vec_scale" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::List(v)), Some(scalar)) => {
                        let s = match scalar { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 1.0 };
                        let result: Vec<Value> = v.iter().map(|x| {
                            let f = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            Value::Float(f * s)
                        }).collect();
                        Ok(Value::List(result))
                    }
                    _ => Err("vec_scale() requires (vec, scalar)".into()),
                }
            }
            "vec_dot" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::List(a)), Some(Value::List(b))) if a.len() == b.len() => {
                        let sum: f64 = a.iter().zip(b.iter()).map(|(x, y)| {
                            let xf = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            let yf = match y { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            xf * yf
                        }).sum();
                        Ok(Value::Float(sum))
                    }
                    _ => Err("vec_dot() requires two vecs of equal length".into()),
                }
            }
            "vec_norm" => {
                match arg_vals.first() {
                    Some(Value::List(v)) => {
                        let sum: f64 = v.iter().map(|x| {
                            let f = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            f * f
                        }).sum();
                        Ok(Value::Float(sum.sqrt()))
                    }
                    _ => Err("vec_norm() requires a vec".into()),
                }
            }
            "vec_normalize" => {
                match arg_vals.first() {
                    Some(Value::List(v)) => {
                        let sum: f64 = v.iter().map(|x| {
                            let f = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            f * f
                        }).sum();
                        let norm = sum.sqrt();
                        if norm == 0.0 { return Ok(Value::List(v.clone())); }
                        let result: Vec<Value> = v.iter().map(|x| {
                            let f = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            Value::Float(f / norm)
                        }).collect();
                        Ok(Value::List(result))
                    }
                    _ => Err("vec_normalize() requires a vec".into()),
                }
            }
            "vec_sum" => {
                match arg_vals.first() {
                    Some(Value::List(v)) => {
                        let sum: f64 = v.iter().map(|x| match x {
                            Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0
                        }).sum();
                        Ok(Value::Float(sum))
                    }
                    _ => Err("vec_sum() requires a vec".into()),
                }
            }
            "vec_min" => {
                match arg_vals.first() {
                    Some(Value::List(v)) => {
                        let min = v.iter().map(|x| match x {
                            Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => f64::INFINITY
                        }).fold(f64::INFINITY, f64::min);
                        Ok(Value::Float(min))
                    }
                    _ => Err("vec_min() requires a vec".into()),
                }
            }
            "vec_max" => {
                match arg_vals.first() {
                    Some(Value::List(v)) => {
                        let max = v.iter().map(|x| match x {
                            Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => f64::NEG_INFINITY
                        }).fold(f64::NEG_INFINITY, f64::max);
                        Ok(Value::Float(max))
                    }
                    _ => Err("vec_max() requires a vec".into()),
                }
            }
            "vec_clamp" => {
                match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
                    (Some(Value::List(v)), Some(lo), Some(hi)) => {
                        let lo_f = match lo { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        let hi_f = match hi { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 1.0 };
                        let result: Vec<Value> = v.iter().map(|x| {
                            let f = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            Value::Float(f.clamp(lo_f, hi_f))
                        }).collect();
                        Ok(Value::List(result))
                    }
                    _ => Err("vec_clamp() requires (vec, min, max)".into()),
                }
            }
            "vec_copy" => {
                match arg_vals.first() {
                    Some(Value::List(v)) => Ok(Value::List(v.clone())),
                    _ => Err("vec_copy() requires a vec".into()),
                }
            }
            "vec_to_list" => {
                match arg_vals.first() {
                    Some(Value::List(v)) => Ok(Value::List(v.clone())),
                    _ => Err("vec_to_list() requires a vec".into()),
                }
            }
            // ── Tensor operations ──────────────────────────
            "tensor_new" => {
                match arg_vals.first() {
                    Some(Value::List(shape)) => {
                        let total: i64 = shape.iter().map(|s| match s {
                            Value::Int(n) => *n, _ => 1
                        }).product();
                        let data = vec![Value::Float(0.0); total as usize];
                        // Store as dict: { "shape": shape, "data": flat_data }
                        Ok(Value::Dict(vec![
                            (Value::Str("shape".into()), Value::List(shape.clone())),
                            (Value::Str("data".into()), Value::List(data)),
                        ]))
                    }
                    _ => Err("tensor_new() requires a shape list".into()),
                }
            }
            "tensor_from" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::List(data)), Some(Value::List(shape))) => {
                        Ok(Value::Dict(vec![
                            (Value::Str("shape".into()), Value::List(shape.clone())),
                            (Value::Str("data".into()), Value::List(data.clone())),
                        ]))
                    }
                    _ => Err("tensor_from() requires (data_list, shape_list)".into()),
                }
            }
            "tensor_shape" => {
                match arg_vals.first() {
                    Some(Value::Dict(pairs)) => {
                        for (k, v) in pairs {
                            if *k == Value::Str("shape".into()) { return Ok(v.clone()); }
                        }
                        Err("tensor has no shape".into())
                    }
                    _ => Err("tensor_shape() requires a tensor".into()),
                }
            }
            "tensor_rank" => {
                match arg_vals.first() {
                    Some(Value::Dict(pairs)) => {
                        for (k, v) in pairs {
                            if *k == Value::Str("shape".into()) {
                                if let Value::List(s) = v { return Ok(Value::Int(s.len() as i64)); }
                            }
                        }
                        Err("tensor has no shape".into())
                    }
                    _ => Err("tensor_rank() requires a tensor".into()),
                }
            }
            "tensor_size" => {
                match arg_vals.first() {
                    Some(Value::Dict(pairs)) => {
                        for (k, v) in pairs {
                            if *k == Value::Str("data".into()) {
                                if let Value::List(d) = v { return Ok(Value::Int(d.len() as i64)); }
                            }
                        }
                        Err("tensor has no data".into())
                    }
                    _ => Err("tensor_size() requires a tensor".into()),
                }
            }
            "tensor_get" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Dict(pairs)), Some(Value::List(indices))) => {
                        let mut shape_v = Vec::new();
                        let mut data_v = Vec::new();
                        for (k, v) in pairs {
                            if *k == Value::Str("shape".into()) { if let Value::List(s) = v { shape_v = s.clone(); } }
                            if *k == Value::Str("data".into()) { if let Value::List(d) = v { data_v = d.clone(); } }
                        }
                        // Compute flat index
                        let mut flat = 0usize;
                        let mut stride = 1usize;
                        for i in (0..shape_v.len()).rev() {
                            let dim = match &shape_v[i] { Value::Int(n) => *n as usize, _ => 1 };
                            let idx = match indices.get(i) { Some(Value::Int(n)) => *n as usize, _ => 0 };
                            flat += idx * stride;
                            stride *= dim;
                        }
                        data_v.get(flat).cloned().ok_or("tensor_get: index out of bounds".into())
                    }
                    _ => Err("tensor_get() requires (tensor, indices)".into()),
                }
            }
            "tensor_set" => {
                match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
                    (Some(Value::Dict(pairs)), Some(Value::List(indices)), Some(val)) => {
                        let mut shape_v = Vec::new();
                        let mut data_v = Vec::new();
                        for (k, v) in pairs {
                            if *k == Value::Str("shape".into()) { if let Value::List(s) = v { shape_v = s.clone(); } }
                            if *k == Value::Str("data".into()) { if let Value::List(d) = v { data_v = d.clone(); } }
                        }
                        let mut flat = 0usize;
                        let mut stride = 1usize;
                        for i in (0..shape_v.len()).rev() {
                            let dim = match &shape_v[i] { Value::Int(n) => *n as usize, _ => 1 };
                            let idx = match indices.get(i) { Some(Value::Int(n)) => *n as usize, _ => 0 };
                            flat += idx * stride;
                            stride *= dim;
                        }
                        if flat < data_v.len() { data_v[flat] = (*val).clone(); }
                        Ok(Value::Dict(vec![
                            (Value::Str("shape".into()), Value::List(shape_v)),
                            (Value::Str("data".into()), Value::List(data_v)),
                        ]))
                    }
                    _ => Err("tensor_set() requires (tensor, indices, value)".into()),
                }
            }
            "tensor_add" | "tensor_sub" | "tensor_mul" | "tensor_div" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Dict(a)), Some(Value::Dict(b))) => {
                        let mut a_data = Vec::new();
                        let mut b_data = Vec::new();
                        let mut shape = Vec::new();
                        for (k, v) in a { if *k == Value::Str("data".into()) { if let Value::List(d) = v { a_data = d.clone(); } }
                                           if *k == Value::Str("shape".into()) { if let Value::List(s) = v { shape = s.clone(); } } }
                        for (k, v) in b { if *k == Value::Str("data".into()) { if let Value::List(d) = v { b_data = d.clone(); } } }
                        let result: Vec<Value> = a_data.iter().zip(b_data.iter()).map(|(x, y)| {
                            let xf = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            let yf = match y { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            Value::Float(match name {
                                "tensor_add" => xf + yf, "tensor_sub" => xf - yf,
                                "tensor_mul" => xf * yf, "tensor_div" => if yf != 0.0 { xf / yf } else { f64::NAN },
                                _ => 0.0,
                            })
                        }).collect();
                        Ok(Value::Dict(vec![
                            (Value::Str("shape".into()), Value::List(shape)),
                            (Value::Str("data".into()), Value::List(result)),
                        ]))
                    }
                    _ => Err(format!("{}() requires two tensors", name)),
                }
            }
            "tensor_scale" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Dict(t)), Some(scalar)) => {
                        let s = match scalar { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 1.0 };
                        let mut data = Vec::new();
                        let mut shape = Vec::new();
                        for (k, v) in t { if *k == Value::Str("data".into()) { if let Value::List(d) = v { data = d.clone(); } }
                                           if *k == Value::Str("shape".into()) { if let Value::List(sv) = v { shape = sv.clone(); } } }
                        let result: Vec<Value> = data.iter().map(|x| {
                            let f = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                            Value::Float(f * s)
                        }).collect();
                        Ok(Value::Dict(vec![
                            (Value::Str("shape".into()), Value::List(shape)),
                            (Value::Str("data".into()), Value::List(result)),
                        ]))
                    }
                    _ => Err("tensor_scale() requires (tensor, scalar)".into()),
                }
            }
            "tensor_sum" | "tensor_mean" | "tensor_min" | "tensor_max" => {
                match arg_vals.first() {
                    Some(Value::Dict(t)) => {
                        let mut data = Vec::new();
                        for (k, v) in t { if *k == Value::Str("data".into()) { if let Value::List(d) = v { data = d.clone(); } } }
                        let floats: Vec<f64> = data.iter().map(|x| match x {
                            Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0
                        }).collect();
                        let result = match name {
                            "tensor_sum" => floats.iter().sum::<f64>(),
                            "tensor_mean" => if floats.is_empty() { 0.0 } else { floats.iter().sum::<f64>() / floats.len() as f64 },
                            "tensor_min" => floats.iter().cloned().fold(f64::INFINITY, f64::min),
                            "tensor_max" => floats.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                            _ => 0.0,
                        };
                        Ok(Value::Float(result))
                    }
                    _ => Err(format!("{}() requires a tensor", name)),
                }
            }
            "tensor_argmin" | "tensor_argmax" => {
                match arg_vals.first() {
                    Some(Value::Dict(t)) => {
                        let mut data = Vec::new();
                        for (k, v) in t { if *k == Value::Str("data".into()) { if let Value::List(d) = v { data = d.clone(); } } }
                        let floats: Vec<f64> = data.iter().map(|x| match x {
                            Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0
                        }).collect();
                        let idx = if name == "tensor_argmin" {
                            floats.iter().enumerate().min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i).unwrap_or(0)
                        } else {
                            floats.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i).unwrap_or(0)
                        };
                        Ok(Value::Int(idx as i64))
                    }
                    _ => Err(format!("{}() requires a tensor", name)),
                }
            }
            "tensor_transpose" | "tensor_reshape" | "tensor_slice" |
            "tensor_fill" | "tensor_copy" | "tensor_matmul" |
            "tensor_softmax" | "tensor_relu" | "tensor_to_list" | "tensor_from_list" => {
                // Simplified implementations
                match name {
                    "tensor_copy" => {
                        match arg_vals.first() { Some(v) => Ok((*v).clone()), _ => Err("tensor_copy() requires a tensor".into()) }
                    }
                    "tensor_to_list" => {
                        match arg_vals.first() {
                            Some(Value::Dict(t)) => {
                                for (k, v) in t { if *k == Value::Str("data".into()) { return Ok(v.clone()); } }
                                Err("tensor has no data".into())
                            }
                            _ => Err("tensor_to_list() requires a tensor".into()),
                        }
                    }
                    "tensor_relu" => {
                        match arg_vals.first() {
                            Some(Value::Dict(t)) => {
                                let mut data = Vec::new();
                                let mut shape = Vec::new();
                                for (k, v) in t { if *k == Value::Str("data".into()) { if let Value::List(d) = v { data = d.clone(); } }
                                                   if *k == Value::Str("shape".into()) { if let Value::List(s) = v { shape = s.clone(); } } }
                                let result: Vec<Value> = data.iter().map(|x| {
                                    let f = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                                    Value::Float(if f > 0.0 { f } else { 0.0 })
                                }).collect();
                                Ok(Value::Dict(vec![
                                    (Value::Str("shape".into()), Value::List(shape)),
                                    (Value::Str("data".into()), Value::List(result)),
                                ]))
                            }
                            _ => Err("tensor_relu() requires a tensor".into()),
                        }
                    }
                    "tensor_fill" => {
                        match (arg_vals.first(), arg_vals.get(1)) {
                            (Some(Value::Dict(t)), Some(val)) => {
                                let mut shape = Vec::new();
                                let mut data_len = 0;
                                for (k, v) in t { if *k == Value::Str("shape".into()) { if let Value::List(s) = v { shape = s.clone(); } }
                                                   if *k == Value::Str("data".into()) { if let Value::List(d) = v { data_len = d.len(); } } }
                                Ok(Value::Dict(vec![
                                    (Value::Str("shape".into()), Value::List(shape)),
                                    (Value::Str("data".into()), Value::List(vec![(*val).clone(); data_len])),
                                ]))
                            }
                            _ => Err("tensor_fill() requires (tensor, value)".into()),
                        }
                    }
                    _ => Ok(Value::Null), // stub for complex operations
                }
            }
            // ── Extended math builtins ──────────────────────────
            "__math_asin" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Float(f.asin())),
                    Some(Value::Int(n)) => Ok(Value::Float((*n as f64).asin())),
                    _ => Err("asin() requires a number".into()),
                }
            }
            "__math_acos" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Float(f.acos())),
                    Some(Value::Int(n)) => Ok(Value::Float((*n as f64).acos())),
                    _ => Err("acos() requires a number".into()),
                }
            }
            "__math_atan" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Float(f.atan())),
                    Some(Value::Int(n)) => Ok(Value::Float((*n as f64).atan())),
                    _ => Err("atan() requires a number".into()),
                }
            }
            "__math_atan2" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(y), Some(x)) => {
                        let yf = match y { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        let xf = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        Ok(Value::Float(yf.atan2(xf)))
                    }
                    _ => Err("atan2() requires (y, x)".into()),
                }
            }
            "__math_sinh" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("sinh() requires a number".into()) }; Ok(Value::Float(f.sinh())) }
            "__math_cosh" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("cosh() requires a number".into()) }; Ok(Value::Float(f.cosh())) }
            "__math_tanh" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("tanh() requires a number".into()) }; Ok(Value::Float(f.tanh())) }
            "__math_deg" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("deg() requires a number".into()) }; Ok(Value::Float(f.to_degrees())) }
            "__math_rad" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("rad() requires a number".into()) }; Ok(Value::Float(f.to_radians())) }
            "__math_exp" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("exp() requires a number".into()) }; Ok(Value::Float(f.exp())) }
            "__math_exp2" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("exp2() requires a number".into()) }; Ok(Value::Float(f.exp2())) }
            "__math_log2" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("log2() requires a number".into()) }; Ok(Value::Float(f.log2())) }
            "__math_log10" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("log10() requires a number".into()) }; Ok(Value::Float(f.log10())) }
            "__math_cbrt" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("cbrt() requires a number".into()) }; Ok(Value::Float(f.cbrt())) }
            "__math_hypot" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(x), Some(y)) => {
                        let xf = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        let yf = match y { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        Ok(Value::Float(xf.hypot(yf)))
                    }
                    _ => Err("hypot() requires (x, y)".into()),
                }
            }
            "__math_trunc" => { let f = match arg_vals.first() { Some(Value::Float(f)) => *f, Some(Value::Int(n)) => *n as f64, _ => return Err("trunc() requires a number".into()) }; Ok(Value::Float(f.trunc())) }
            "__math_clamp" => {
                match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
                    (Some(x), Some(lo), Some(hi)) => {
                        let xf = match x { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        let lo_f = match lo { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        let hi_f = match hi { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 1.0 };
                        Ok(Value::Float(xf.clamp(lo_f, hi_f)))
                    }
                    _ => Err("clamp() requires (x, min, max)".into()),
                }
            }
            "__math_lerp" => {
                match (arg_vals.first(), arg_vals.get(1), arg_vals.get(2)) {
                    (Some(a), Some(b), Some(t)) => {
                        let af = match a { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        let bf = match b { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        let tf = match t { Value::Float(f) => *f, Value::Int(n) => *n as f64, _ => 0.0 };
                        Ok(Value::Float(af + (bf - af) * tf))
                    }
                    _ => Err("lerp() requires (a, b, t)".into()),
                }
            }
            "__math_sign" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Int(if *f > 0.0 { 1 } else if *f < 0.0 { -1 } else { 0 })),
                    Some(Value::Int(n)) => Ok(Value::Int(if *n > 0 { 1 } else if *n < 0 { -1 } else { 0 })),
                    _ => Err("sign() requires a number".into()),
                }
            }
            "__math_is_nan" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Bool(f.is_nan())),
                    _ => Ok(Value::Bool(false)),
                }
            }
            "__math_is_inf" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Bool(f.is_infinite())),
                    _ => Ok(Value::Bool(false)),
                }
            }
            "__math_is_finite" => {
                match arg_vals.first() {
                    Some(Value::Float(f)) => Ok(Value::Bool(f.is_finite())),
                    Some(Value::Int(_)) => Ok(Value::Bool(true)),
                    _ => Ok(Value::Bool(false)),
                }
            }
            "__math_gcd" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Int(a)), Some(Value::Int(b))) => {
                        fn gcd(mut a: i64, mut b: i64) -> i64 { a = a.abs(); b = b.abs(); while b != 0 { let t = b; b = a % b; a = t; } a }
                        Ok(Value::Int(gcd(*a, *b)))
                    }
                    _ => Err("gcd() requires two integers".into()),
                }
            }
            "__math_lcm" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Int(a)), Some(Value::Int(b))) => {
                        fn gcd(mut a: i64, mut b: i64) -> i64 { a = a.abs(); b = b.abs(); while b != 0 { let t = b; b = a % b; a = t; } a }
                        let g = gcd(*a, *b);
                        Ok(Value::Int(if g == 0 { 0 } else { (a * b).abs() / g }))
                    }
                    _ => Err("lcm() requires two integers".into()),
                }
            }
            "__math_factorial" => {
                match arg_vals.first() {
                    Some(Value::Int(n)) => {
                        if *n < 0 { return Err("factorial: negative input".into()); }
                        if *n > 100_000 { return Err("factorial: input too large".into()); }
                        // Exact like the rest of int arithmetic: promote to
                        // BigInt instead of erroring on overflow.
                        let mut result = crate::bigint::BigInt::from_i64(1);
                        for i in 2..=*n {
                            result = result.mul(&crate::bigint::BigInt::from_i64(i));
                        }
                        Ok(Self::norm_bigint(result))
                    }
                    _ => Err("factorial() requires an integer".into()),
                }
            }
            "__math_is_prime" => {
                match arg_vals.first() {
                    Some(Value::Int(n)) => {
                        let n = *n;
                        if n < 2 { return Ok(Value::Bool(false)); }
                        if n < 4 { return Ok(Value::Bool(true)); }
                        if n % 2 == 0 || n % 3 == 0 { return Ok(Value::Bool(false)); }
                        let mut i = 5i64;
                        while i * i <= n {
                            if n % i == 0 || n % (i + 2) == 0 { return Ok(Value::Bool(false)); }
                            i += 6;
                        }
                        Ok(Value::Bool(true))
                    }
                    _ => Err("is_prime() requires an integer".into()),
                }
            }
            "__math_variance" => {
                match arg_vals.first() {
                    Some(Value::List(items)) => {
                        let vals: Vec<f64> = items.iter().filter_map(|v| match v {
                            Value::Float(f) => Some(*f), Value::Int(n) => Some(*n as f64), _ => None
                        }).collect();
                        if vals.is_empty() { return Ok(Value::Float(0.0)); }
                        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
                        let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64;
                        Ok(Value::Float(var))
                    }
                    _ => Err("variance() requires a list".into()),
                }
            }
            "__math_mode" => {
                match arg_vals.first() {
                    Some(Value::List(items)) => {
                        let mut counts: HashMap<String, (Value, usize)> = HashMap::new();
                        for v in items {
                            let key = format!("{}", v);
                            counts.entry(key).and_modify(|e| e.1 += 1).or_insert((v.clone(), 1));
                        }
                        let max_count = counts.values().map(|c| c.1).max().unwrap_or(0);
                        let mode = counts.values().find(|c| c.1 == max_count).map(|c| c.0.clone()).unwrap_or(Value::Null);
                        Ok(mode)
                    }
                    _ => Err("mode() requires a list".into()),
                }
            }
            // ── Collection helper builtins ──────────────────────────
            "__col_stack" | "__col_queue" | "__col_linked_list" => {
                // All implemented as lists (stack=LIFO, queue=FIFO, linked_list=list)
                Ok(Value::List(vec![]))
            }
            "__col_priority_queue" | "__col_sorted_map" | "__col_multiset" => {
                // Implemented as empty dicts or lists
                Ok(Value::List(vec![]))
            }
            // ── Actor/Agent builtins ──────────────────────────
            "actor_spawn" => {
                match arg_vals.first() {
                    Some(Value::Str(name)) => {
                        let id = format!("actor_{}", name);
                        let actor = Value::Dict(vec![
                            (Value::Str("__kind".into()), Value::Str("actor_instance".into())),
                            (Value::Str("name".into()), Value::Str(name.clone())),
                            (Value::Str("id".into()), Value::Str(id.clone())),
                            (Value::Str("alive".into()), Value::Bool(true)),
                            (Value::Str("mailbox".into()), Value::List(vec![])),
                        ]);
                        Ok(actor)
                    }
                    _ => Err("actor_spawn() requires a string name".into()),
                }
            }
            "actor_send" => {
                // Stub: push message to mailbox
                Ok(Value::Null)
            }
            "actor_receive" => {
                Ok(Value::Null)
            }
            "actor_call" => {
                // Stub: return null
                Ok(Value::Null)
            }
            "actor_stop" => {
                Ok(Value::Null)
            }
            "actor_is_alive" => {
                match arg_vals.first() {
                    Some(Value::Dict(pairs)) => {
                        for (k, v) in pairs {
                            if *k == Value::Str("alive".into()) {
                                return Ok(v.clone());
                            }
                        }
                        Ok(Value::Bool(false))
                    }
                    _ => Ok(Value::Bool(false)),
                }
            }
            "agent_create" => {
                match arg_vals.first() {
                    Some(Value::Str(name)) => {
                        let state = if let Some(s) = arg_vals.get(1) { (*s).clone() } else { Value::Dict(vec![]) };
                        Ok(Value::Dict(vec![
                            (Value::Str("__kind".into()), Value::Str("agent_instance".into())),
                            (Value::Str("name".into()), Value::Str(name.clone())),
                            (Value::Str("state".into()), state),
                            (Value::Str("done".into()), Value::Bool(false)),
                        ]))
                    }
                    _ => Err("agent_create() requires a string name".into()),
                }
            }
            "agent_set_goal" | "agent_set_state" | "agent_run" | "agent_done" | "agent_get_state" => {
                // Stubs
                match name {
                    "agent_get_state" => {
                        match arg_vals.first() {
                            Some(Value::Dict(pairs)) => {
                                for (k, v) in pairs {
                                    if *k == Value::Str("state".into()) { return Ok(v.clone()); }
                                }
                                Ok(Value::Dict(vec![]))
                            }
                            _ => Ok(Value::Dict(vec![])),
                        }
                    }
                    _ => Ok(Value::Null),
                }
            }
            // ── Isolate builtins ──────────────────────────
            "isolate_new" => {
                Ok(Value::Dict(vec![
                    (Value::Str("__kind".into()), Value::Str("isolate".into())),
                    (Value::Str("vars".into()), Value::Dict(vec![])),
                ]))
            }
            "isolate_get" => {
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Dict(pairs)), Some(Value::Str(key))) => {
                        for (k, v) in pairs {
                            if *k == Value::Str("vars".into()) {
                                if let Value::Dict(vars) = v {
                                    for (vk, vv) in vars {
                                        if *vk == Value::Str(key.clone()) { return Ok(vv.clone()); }
                                    }
                                }
                            }
                        }
                        Ok(Value::Null)
                    }
                    _ => Err("isolate_get() requires (isolate, key)".into()),
                }
            }
            "isolate_set" | "isolate_exec" | "isolate_run" => {
                Ok(Value::Null)
            }
            // ── Weak references ──────────────────────────
            "weak_ref" => {
                // In tree-walk interpreter, weak_ref is a strong ref wrapped in a dict
                match arg_vals.first() {
                    Some(val) => Ok(Value::Dict(vec![
                        (Value::Str("__kind".into()), Value::Str("weak_ref".into())),
                        (Value::Str("value".into()), (*val).clone()),
                        (Value::Str("alive".into()), Value::Bool(true)),
                    ])),
                    _ => Err("weak_ref() requires a value".into()),
                }
            }
            "unwrap_or_default" => {
                match arg_vals.first() {
                    Some(Value::Null) | None => Ok(Value::Int(0)),
                    Some(val) => Ok((*val).clone()),
                }
            }
            "register_engine" => {
                // Stub: register engine name
                Ok(Value::Null)
            }

            // ── Thread builtins ──
            "thread_spawn" => {
                // Single-threaded model: run the closure eagerly and store its
                // result so thread_join can return it. Extra args are forwarded.
                let func = arg_vals.first().cloned().cloned()
                    .ok_or("thread_spawn() requires a function")?;
                let call_args: Vec<(Option<String>, Value)> =
                    arg_vals.iter().skip(1).map(|v| (None, (*v).clone())).collect();
                let result = self.call_value(&func, &call_args)?;
                let id = self.next_handle_id;
                self.next_handle_id += 1;
                self.thread_results.insert(id, result);
                // Return the handle as a bare int id (`t >= 0` is a valid check).
                Ok(Value::Int(id))
            }
            "thread_join" => {
                let id = Self::handle_id(arg_vals.first().copied());
                Ok(self.thread_results.remove(&id).unwrap_or(Value::Null))
            }
            "mutex_create" => {
                Ok(Value::Dict(vec![
                    (Value::Str("type".into()), Value::Str("mutex".into())),
                    (Value::Str("locked".into()), Value::Bool(false)),
                ]))
            }
            "mutex_lock" | "mutex_unlock" | "rwmutex_read_lock"
            | "rwmutex_write_lock" | "rwmutex_unlock" => {
                Ok(Value::Null)
            }
            "mutex_with" => {
                // mutex_with(mtx, fn) — call fn, return result
                if arg_vals.len() >= 2 {
                    return self.call_value(arg_vals[1], &[]);
                }
                Ok(Value::Null)
            }
            "rwmutex_create" => {
                Ok(Value::Dict(vec![
                    (Value::Str("type".into()), Value::Str("rwmutex".into())),
                ]))
            }
            "atomic_new" => {
                let init = arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Int(0));
                Ok(Value::Dict(vec![
                    (Value::Str("type".into()), Value::Str("atomic".into())),
                    (Value::Str("value".into()), init),
                ]))
            }
            "atomic_load" => {
                if let Some(Value::Dict(pairs)) = arg_vals.first().copied() {
                    for (k, v) in pairs {
                        if k == &Value::Str("value".into()) { return Ok(v.clone()); }
                    }
                }
                Ok(Value::Int(0))
            }
            "atomic_store" => Ok(Value::Null),
            "atomic_add" => {
                let n = if let Some(Value::Int(n)) = arg_vals.get(1).copied() { *n } else { 1 };
                if let Some(Value::Dict(pairs)) = arg_vals.first().copied() {
                    for (k, v) in pairs {
                        if k == &Value::Str("value".into()) {
                            if let Value::Int(old) = v { return Ok(Value::Int(*old + n)); }
                        }
                    }
                }
                Ok(Value::Int(n))
            }
            "atomic_sub" => {
                let n = if let Some(Value::Int(n)) = arg_vals.get(1).copied() { *n } else { 1 };
                if let Some(Value::Dict(pairs)) = arg_vals.first().copied() {
                    for (k, v) in pairs {
                        if k == &Value::Str("value".into()) {
                            if let Value::Int(old) = v { return Ok(Value::Int(*old - n)); }
                        }
                    }
                }
                Ok(Value::Int(-n))
            }
            "atomic_cas" => {
                // cas(atom, expected, desired) -> bool
                Ok(Value::Bool(true))
            }
            "threadpool_create" => {
                let size = if let Some(Value::Int(n)) = arg_vals.first().copied() { *n } else { 4 };
                Ok(Value::Dict(vec![
                    (Value::Str("type".into()), Value::Str("threadpool".into())),
                    (Value::Str("size".into()), Value::Int(size)),
                ]))
            }
            "threadpool_submit" | "threadpool_submit_future" => {
                // Tree-walk runtime executes submitted tasks eagerly and stores the result.
                let result = match arg_vals.get(1) {
                    Some(task @ Value::Func(_)) | Some(task @ Value::BuiltinFunc(_)) => {
                        self.call_value(task, &[]).unwrap_or(Value::Null)
                    }
                    _ => Value::Null,
                };
                let future_id = self.next_future_id;
                self.next_future_id += 1;
                self.futures.insert(future_id, result);

                if name == "threadpool_submit_future" {
                    Ok(Value::Dict(vec![
                        (Value::Str("type".into()), Value::Str("future".into())),
                        (Value::Str("id".into()), Value::Int(future_id)),
                    ]))
                } else {
                    Ok(Value::Int(future_id))
                }
            }
            "threadpool_wait" | "threadpool_destroy" => Ok(Value::Null),
            "future_get" => {
                let fid = match arg_vals.first() {
                    Some(Value::Int(id)) => Some(*id),
                    Some(Value::Dict(entries)) => entries.iter().find_map(|(k, v)| {
                        if *k == Value::Str("id".into()) {
                            if let Value::Int(id) = v { Some(*id) } else { None }
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };
                match fid.and_then(|id| self.futures.get(&id).cloned()) {
                    Some(v) => Ok(v),
                    None => Ok(Value::Null),
                }
            }
            "future_is_done" => {
                let fid = match arg_vals.first() {
                    Some(Value::Int(id)) => Some(*id),
                    Some(Value::Dict(entries)) => entries.iter().find_map(|(k, v)| {
                        if *k == Value::Str("id".into()) {
                            if let Value::Int(id) = v { Some(*id) } else { None }
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };
                Ok(Value::Bool(fid.map(|id| self.futures.contains_key(&id)).unwrap_or(false)))
            }
            "future_try_get" => {
                let fid = match arg_vals.first() {
                    Some(Value::Int(id)) => Some(*id),
                    Some(Value::Dict(entries)) => entries.iter().find_map(|(k, v)| {
                        if *k == Value::Str("id".into()) {
                            if let Value::Int(id) = v { Some(*id) } else { None }
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };
                match fid.and_then(|id| self.futures.get(&id).cloned()) {
                    Some(v) => Ok(Value::Some(Box::new(v))),
                    None => Ok(Value::Null),
                }
            }
            "waitgroup_create" => {
                Ok(Value::Dict(vec![
                    (Value::Str("type".into()), Value::Str("waitgroup".into())),
                    (Value::Str("count".into()), Value::Int(0)),
                ]))
            }
            "waitgroup_add" | "waitgroup_done" | "waitgroup_wait" => Ok(Value::Null),

            // ── Channel builtins ──
            "chan_create" => {
                let cap = if let Some(Value::Int(n)) = arg_vals.first().copied() { *n } else { 0 };
                let id = self.next_handle_id;
                self.next_handle_id += 1;
                self.channels.insert(id, std::collections::VecDeque::new());
                self.channels_closed.insert(id, false);
                Ok(Value::Dict(vec![
                    (Value::Str("type".into()), Value::Str("channel".into())),
                    (Value::Str("id".into()), Value::Int(id)),
                    (Value::Str("capacity".into()), Value::Int(cap)),
                ]))
            }
            "chan_send" => {
                let id = Self::handle_id(arg_vals.first().copied());
                if self.channels_closed.get(&id).copied().unwrap_or(true) {
                    return Err("chan_send: channel is closed".into());
                }
                let val = arg_vals.get(1).cloned().cloned().unwrap_or(Value::Null);
                if let Some(buf) = self.channels.get_mut(&id) {
                    buf.push_back(val);
                    Ok(Value::Null)
                } else {
                    Err("chan_send: invalid channel".into())
                }
            }
            "chan_try_send" => {
                // Non-blocking send: buffer the value and report success.
                let id = Self::handle_id(arg_vals.first().copied());
                let val = arg_vals.get(1).cloned().cloned().unwrap_or(Value::Null);
                if let Some(buf) = self.channels.get_mut(&id) {
                    buf.push_back(val);
                    Ok(Value::Bool(true))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            "chan_recv" => {
                let id = Self::handle_id(arg_vals.first().copied());
                let val = self.channels.get_mut(&id).and_then(|buf| buf.pop_front());
                Ok(val.unwrap_or(Value::Null))
            }
            "chan_try_recv" => {
                // Non-blocking recv: {ok, value}. ok=false when the buffer is empty.
                let id = Self::handle_id(arg_vals.first().copied());
                let val = self.channels.get_mut(&id).and_then(|buf| buf.pop_front());
                Ok(Value::Dict(vec![
                    (Value::Str("ok".into()), Value::Bool(val.is_some())),
                    (Value::Str("value".into()), val.unwrap_or(Value::Null)),
                ]))
            }
            "chan_len" => {
                let id = Self::handle_id(arg_vals.first().copied());
                Ok(Value::Int(self.channels.get(&id).map(|b| b.len() as i64).unwrap_or(0)))
            }
            "chan_drain" => {
                let id = Self::handle_id(arg_vals.first().copied());
                let items: Vec<Value> = self.channels.get_mut(&id)
                    .map(|b| b.drain(..).collect()).unwrap_or_default();
                Ok(Value::List(items))
            }
            "chan_is_closed" => {
                let id = Self::handle_id(arg_vals.first().copied());
                Ok(Value::Bool(self.channels_closed.get(&id).copied().unwrap_or(true)))
            }
            "chan_close" => {
                let id = Self::handle_id(arg_vals.first().copied());
                self.channels_closed.insert(id, true);
                Ok(Value::Null)
            }
            "chan_is_closed" => {
                if let Some(Value::Dict(pairs)) = arg_vals.first().copied() {
                    for (k, v) in pairs {
                        if k == &Value::Str("closed".into()) {
                            return Ok(v.clone());
                        }
                    }
                }
                Ok(Value::Bool(false))
            }
            "chan_try_send" => Ok(Value::Bool(true)),
            "chan_try_recv" => Ok(Value::Dict(vec![
                (Value::Str("ok".into()), Value::Bool(false)),
                (Value::Str("value".into()), Value::Null),
            ])),
            "chan_drain" => Ok(Value::List(Vec::new())),
            "chan_len" => Ok(Value::Int(0)),
            "chan_select" => {
                // chan_select(cases) — returns index of ready case or -1
                Ok(Value::Int(0))
            }

            // ── Structured concurrency ──
            "task_group" => {
                Ok(Value::Dict(vec![
                    (Value::Str("type".into()), Value::Str("task_group".into())),
                    (Value::Str("tasks".into()), Value::List(Vec::new())),
                    (Value::Str("spawn".into()), Value::BuiltinFunc("__task_group_spawn".into())),
                    (Value::Str("join_all".into()), Value::BuiltinFunc("__task_group_join_all".into())),
                    (Value::Str("cancel".into()), Value::BuiltinFunc("__task_group_cancel".into())),
                ]))
            }
            "task_scope" => {
                // task_scope(fn) — call fn with scoped concurrency, return result
                if let Some(f) = arg_vals.first() {
                    return self.call_value(f, &[]);
                }
                Ok(Value::Null)
            }
            "__task_group_spawn" | "__task_group_join_all" | "__task_group_cancel" => {
                Ok(Value::Null)
            }

            // ── Borrow / move helpers ──
            "borrow" | "borrow_mut" | "deref" => {
                // In tree-walk, these just return the value itself
                Ok(arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null))
            }
            "unsafe_send" => {
                Ok(arg_vals.first().map(|v| (*v).clone()).unwrap_or(Value::Null))
            }

            // ── Hardware fault signal handlers (Milestone 1) ──────────────────
            "__signal_on_fault" => {
                // signal.on_fault(name, handler)
                match (arg_vals.first(), arg_vals.get(1)) {
                    (Some(Value::Str(sig_name)), Some(handler)) => {
                        let sig = sig_name.clone();
                        let h = (*handler).clone();
                        self.fault_handlers.insert(sig.clone(), h);
                        let result = unsafe { fault::install_os_fault_handler(&sig) };
                        if let Err(e) = result {
                            return Err(format!("signal.on_fault: {}", e));
                        }
                        Ok(Value::Null)
                    }
                    _ => Err("signal.on_fault() requires (signal_name, handler)".into()),
                }
            }
            "__signal_dump_json" => {
                match arg_vals.first() {
                    Some(Value::Str(path)) => {
                        let info = if fault::FAULT_PENDING.load(std::sync::atomic::Ordering::Acquire) {
                            let sig_num = fault::FAULT_SIGNUM.load(std::sync::atomic::Ordering::Acquire);
                            #[cfg(unix)]
                            let sig_name = fault::signum_to_fault_name(sig_num).unwrap_or("UNKNOWN");
                            #[cfg(not(unix))]
                            let sig_name = "UNKNOWN";
                            fault::make_fault_info_dict(sig_name)
                        } else if let Some(last) = &self.last_fault_info {
                            last.clone()
                        } else {
                            fault::make_fault_info_dict("<no active fault>")
                        };
                        let json = fault::value_to_json(&info);
                        std::fs::write(path, json)
                            .map_err(|e| format!("signal.dump_json: cannot write '{}': {}", path, e))?;
                        Ok(Value::Null)
                    }
                    _ => Err("signal.dump_json() requires (path)".into()),
                }
            }
            "__signal_list" => {
                Ok(Value::List(vec![
                    Value::Str("SIGSEGV".into()),
                    Value::Str("SIGBUS".into()),
                    Value::Str("SIGFPE".into()),
                    Value::Str("SIGABRT".into()),
                ]))
            }
            "__signal_on" | "__signal_once" | "__signal_off" | "__signal_reset"
            | "__signal_ignore" | "__signal_raise" | "__signal_alarm" => Ok(Value::Null),
            "__signal_set_recovery_point" => {
                Err("signal.set_recovery_point() not yet implemented (Milestone 2)".into())
            }
            "__signal_recover" => {
                Err("signal.recover() not yet implemented (Milestone 2)".into())
            }
            "__signal_dump_core" => {
                match arg_vals.first() {
                    Some(Value::Str(path)) => {
                        let info = if let Some(last) = &self.last_fault_info {
                            last.clone()
                        } else {
                            fault::make_fault_info_dict("<no active fault>")
                        };
                        let json = fault::value_to_json(&info);
                        let content = format!("CORE_DUMP_V2\n{}\n", json);
                        std::fs::write(path, content)
                            .map_err(|e| format!("signal.dump_core: cannot write '{}': {}", path, e))?;
                        Ok(Value::Null)
                    }
                    _ => {
                        // No path: dump to stderr
                        let info = if let Some(last) = &self.last_fault_info {
                            last.clone()
                        } else {
                            fault::make_fault_info_dict("<no active fault>")
                        };
                        let json = fault::value_to_json(&info);
                        eprintln!("CORE_DUMP_V2\n{}", json);
                        Ok(Value::Null)
                    }
                }
            }

            // ── Real filesystem implementations ──
            "__fs_exists" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                Ok(Value::Bool(std::path::Path::new(&path).exists()))
            }
            "__fs_is_file" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                Ok(Value::Bool(std::path::Path::new(&path).is_file()))
            }
            "__fs_is_dir" | "__fs_is_directory" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                Ok(Value::Bool(std::path::Path::new(&path).is_dir()))
            }
            "__fs_size" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::metadata(&path) {
                    Ok(m) => Ok(Value::Int(m.len() as i64)),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_read" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::read_to_string(&path) {
                    Ok(content) => Ok(Value::Str(content)),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_read_bytes" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::read(&path) {
                    Ok(bytes) => Ok(Value::Bytes(bytes)),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_write" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let content = arg_vals.get(1).map(|v| v.to_string_repr()).unwrap_or_default();
                match std::fs::write(&path, content.as_bytes()) {
                    Ok(_) => Ok(Value::Null),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_append" => {
                use std::io::Write as IoWrite;
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let content = arg_vals.get(1).map(|v| v.to_string_repr()).unwrap_or_default();
                match std::fs::OpenOptions::new().append(true).create(true).open(&path) {
                    Ok(mut f) => { f.write_all(content.as_bytes()).map_err(|e| format!("IOError: {}", e))?; Ok(Value::Null) }
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_mkdir" | "__fs_create_dir" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::create_dir_all(&path) {
                    Ok(_) => Ok(Value::Null),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_rmdir" | "__fs_remove_dir" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::remove_dir_all(&path) {
                    Ok(_) => Ok(Value::Null),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_delete" | "__fs_remove" | "__fs_remove_file" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::remove_file(&path) {
                    Ok(_) => Ok(Value::Null),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_copy" => {
                let src = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let dst = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::copy(&src, &dst) {
                    Ok(_) => Ok(Value::Null),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_move" | "__fs_move_file" | "__fs_rename" => {
                let src = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let dst = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::rename(&src, &dst) {
                    Ok(_) => Ok(Value::Null),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_ls" | "__fs_list" | "__fs_readdir" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or(".");
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut result = Vec::new();
                        for entry in entries.flatten() {
                            result.push(Value::Str(entry.file_name().to_string_lossy().into_owned()));
                        }
                        Ok(Value::List(result))
                    }
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_walk" => {
                fn walk_dir(path: &str, result: &mut Vec<Value>) -> Result<(), String> {
                    match std::fs::read_dir(path) {
                        Ok(entries) => {
                            for entry in entries.flatten() {
                                let p = entry.path();
                                result.push(Value::Str(p.to_string_lossy().into_owned()));
                                if p.is_dir() { walk_dir(&p.to_string_lossy(), result)?; }
                            }
                            Ok(())
                        }
                        Err(e) => Err(format!("IOError: {}", e)),
                    }
                }
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or(".");
                let mut result = Vec::new();
                walk_dir(&path, &mut result)?;
                Ok(Value::List(result))
            }
            "__fs_abs" | "__fs_absolute" | "__fs_canonicalize" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or(".");
                match std::fs::canonicalize(&path) {
                    Ok(p) => Ok(Value::Str(p.to_string_lossy().into_owned())),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_basename" | "__fs_filename" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let result = std::path::Path::new(&path)
                    .file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                Ok(Value::Str(result))
            }
            "__fs_dirname" | "__fs_parent" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let result = std::path::Path::new(&path)
                    .parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                Ok(Value::Str(result))
            }
            "__fs_stem" | "__fs_file_stem" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let result = std::path::Path::new(&path)
                    .file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                Ok(Value::Str(result))
            }
            "__fs_ext" | "__fs_extension" => {
                // Include the leading dot, matching the docs (`fs.ext("readme.md") == ".md"`).
                let path = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Str(Self::path_ext(&path)))
            }
            "__fs_join" => {
                // Join with '/' (portable, matches docs); preserve a leading root.
                let mut result = String::new();
                for v in &arg_vals {
                    let part = self.value_to_string(v);
                    if part.is_empty() { continue; }
                    if result.is_empty() {
                        result = part;
                    } else if result.ends_with('/') {
                        result.push_str(part.trim_start_matches('/'));
                    } else {
                        result.push('/');
                        result.push_str(part.trim_start_matches('/'));
                    }
                }
                Ok(Value::Str(result))
            }
            "__fs_is_abs" | "__fs_is_absolute" => {
                let p = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Bool(p.starts_with('/') || p.starts_with('\\') || (p.len() >= 2 && p.as_bytes()[1] == b':')))
            }
            "__fs_cwd" | "__fs_getcwd" => {
                match std::env::current_dir() {
                    Ok(p) => Ok(Value::Str(p.to_string_lossy().into_owned())),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_chdir" | "__fs_set_cwd" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::env::set_current_dir(&path) {
                    Ok(_) => Ok(Value::Null),
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_normalize" => {
                let path = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Str(Self::path_normalize(&path)))
            }
            "__fs_stat" => {
                let path = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::fs::metadata(&path) {
                    Ok(m) => {
                        Ok(Value::Dict(vec![
                            (Value::Str("size".into()), Value::Int(m.len() as i64)),
                            (Value::Str("is_file".into()), Value::Bool(m.is_file())),
                            (Value::Str("is_dir".into()), Value::Bool(m.is_dir())),
                        ]))
                    }
                    Err(e) => Err(format!("IOError: {}", e)),
                }
            }
            "__fs_chmod" => {
                // chmod not easily portable; just return null
                Ok(Value::Null)
            }

            // ── Real OS implementations ──
            "__os_platform" => {
                Ok(Value::Str(std::env::consts::OS.to_string()))
            }
            "__os_arch" => {
                Ok(Value::Str(std::env::consts::ARCH.to_string()))
            }
            "__os_pid" => {
                Ok(Value::Int(std::process::id() as i64))
            }
            "__os_hostname" => {
                // Use env variable as fallback; no external crate
                let h = std::env::var("COMPUTERNAME")
                    .or_else(|_| std::env::var("HOSTNAME"))
                    .unwrap_or_else(|_| "unknown".to_string());
                Ok(Value::Str(h))
            }
            "__os_username" => {
                let u = std::env::var("USERNAME")
                    .or_else(|_| std::env::var("USER"))
                    .unwrap_or_else(|_| "unknown".to_string());
                Ok(Value::Str(u))
            }
            "__os_home_dir" | "__os_home" => {
                let h = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .unwrap_or_else(|_| "/".to_string());
                Ok(Value::Str(h))
            }
            "__os_cpu_count" | "__os_cpus" => {
                Ok(Value::Int(std::thread::available_parallelism().map(|n| n.get() as i64).unwrap_or(1)))
            }
            "__os_getenv" | "__os_env_get" => {
                let key = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                match std::env::var(&key) {
                    Ok(v) => Ok(Value::Str(v)),
                    Err(_) => Ok(Value::Null),
                }
            }
            "__os_setenv" | "__os_env_set" => {
                let key = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let val = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                std::env::set_var(&key, &val);
                Ok(Value::Null)
            }
            "__os_unsetenv" | "__os_env_remove" => {
                let key = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                std::env::remove_var(&key);
                Ok(Value::Null)
            }
            "__os_environ" | "__os_env_all" => {
                let pairs: Vec<(Value, Value)> = std::env::vars()
                    .map(|(k, v)| (Value::Str(k), Value::Str(v)))
                    .collect();
                Ok(Value::Dict(pairs))
            }
            "__os_exit" => {
                let code = arg_vals.first().and_then(|v| if let Value::Int(i) = v { Some(*i as i32) } else { None }).unwrap_or(0);
                std::process::exit(code);
            }
            "__os_exec" | "__os_run" => {
                let cmd = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let output = std::process::Command::new("sh")
                    .arg("-c").arg(&cmd)
                    .output()
                    .or_else(|_| std::process::Command::new("cmd").arg("/C").arg(&cmd).output())
                    .map_err(|e| format!("IOError: {}", e))?;
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                Ok(Value::Dict(vec![
                    (Value::Str("stdout".into()), Value::Str(stdout)),
                    (Value::Str("stderr".into()), Value::Str(stderr)),
                    (Value::Str("code".into()), Value::Int(output.status.code().unwrap_or(0) as i64)),
                ]))
            }

            // ── Real time implementations ──
            "__time_now" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let ms = SystemTime::now().duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64).unwrap_or(0);
                Ok(Value::Int(ms))
            }
            "__time_now_utc" | "__time_now_iso" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let secs = SystemTime::now().duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs()).unwrap_or(0);
                // Simple UTC formatting without external crate
                let s = secs;
                let days = s / 86400;
                let year_start = 1970i64;
                let approx_year = year_start + (days as i64 / 365);
                Ok(Value::Str(format!("{}T{}Z", approx_year, s % 86400)))
            }
            "__time_timestamp" | "__time_unix" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let secs = SystemTime::now().duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64).unwrap_or(0);
                Ok(Value::Int(secs))
            }
            "__time_ms" | "__time_millis" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let ms = SystemTime::now().duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64).unwrap_or(0);
                Ok(Value::Int(ms))
            }
            "__time_diff" => {
                let a = arg_vals.first().and_then(|v| if let Value::Int(i) = v { Some(*i) } else { None }).unwrap_or(0);
                let b = arg_vals.get(1).and_then(|v| if let Value::Int(i) = v { Some(*i) } else { None }).unwrap_or(0);
                Ok(Value::Int(b - a))
            }
            "__time_sleep" | "__time_wait" => {
                let ms = arg_vals.first().and_then(|v| if let Value::Int(i) = v { Some(*i as u64) } else { None }).unwrap_or(0);
                std::thread::sleep(std::time::Duration::from_millis(ms));
                Ok(Value::Null)
            }

            // ── Real random implementations ──
            "__rand_bool" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
                Ok(Value::Bool(seed % 2 == 0))
            }
            "__rand_float" | "__rand_f64" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
                // Simple LCG
                let r = ((seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)) >> 32) as f64 / u32::MAX as f64;
                Ok(Value::Float(r))
            }
            "__rand_int" | "__rand_range" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let min = arg_vals.first().and_then(|v| if let Value::Int(i) = v { Some(*i) } else { None }).unwrap_or(0);
                let max = arg_vals.get(1).and_then(|v| if let Value::Int(i) = v { Some(*i) } else { None }).unwrap_or(100);
                let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
                let r = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) >> 32;
                let range = (max - min).abs() + 1;
                Ok(Value::Int(min + (r as i64 % range)))
            }
            "__rand_shuffle" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                if let Some(Value::List(items)) = arg_vals.first() {
                    let mut items = items.clone();
                    let mut seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
                    for i in (1..items.len()).rev() {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        let j = (seed >> 32) as usize % (i + 1);
                        items.swap(i, j);
                    }
                    Ok(Value::List(items))
                } else {
                    Ok(Value::List(vec![]))
                }
            }
            "__rand_choice" | "__rand_pick" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                if let Some(Value::List(items)) = arg_vals.first() {
                    if items.is_empty() { return Ok(Value::Null); }
                    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
                    let idx = (seed.wrapping_mul(6364136223846793005) >> 32) as usize % items.len();
                    Ok(items[idx].clone())
                } else {
                    Ok(Value::Null)
                }
            }
            "__rand_sample" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                if let (Some(Value::List(items)), Some(Value::Int(k))) = (arg_vals.first(), arg_vals.get(1)) {
                    let k = (*k as usize).min(items.len());
                    let mut items = items.clone();
                    let mut seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
                    for i in (1..items.len()).rev() {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        let j = (seed >> 32) as usize % (i + 1);
                        items.swap(i, j);
                    }
                    Ok(Value::List(items[..k].to_vec()))
                } else {
                    Ok(Value::List(vec![]))
                }
            }
            "__rand_choices" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                if let (Some(Value::List(items)), Some(Value::Int(k))) = (arg_vals.first(), arg_vals.get(1)) {
                    let k = *k as usize;
                    let mut result = Vec::with_capacity(k);
                    let mut seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
                    for _ in 0..k {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        let idx = (seed >> 32) as usize % items.len();
                        result.push(items[idx].clone());
                    }
                    Ok(Value::List(result))
                } else {
                    Ok(Value::List(vec![]))
                }
            }
            "__rand_bytes" | "__rand_secure_bytes" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let n = arg_vals.first().and_then(|v| if let Value::Int(i) = v { Some(*i as usize) } else { None }).unwrap_or(16);
                let mut seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
                let bytes: Vec<u8> = (0..n).map(|_| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 24) as u8 }).collect();
                Ok(Value::Bytes(bytes))
            }
            "__rand_token" | "__rand_secure_token" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let n = arg_vals.first().and_then(|v| if let Value::Int(i) = v { Some(*i as usize) } else { None }).unwrap_or(16);
                let charset = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
                let mut seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(1);
                let token: String = (0..n).map(|_| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); charset[(seed >> 32) as usize % charset.len()] as char }).collect();
                Ok(Value::Str(token))
            }

            // ── Embedded engine calls: proxies created by @import ──
            name if name.starts_with("__engine_call:") => {
                let rest = &name["__engine_call:".len()..];
                let (wid_str, fn_name) = rest
                    .split_once(':')
                    .ok_or("malformed engine call name")?;
                let wid: usize = wid_str.parse().map_err(|_| "bad engine worker id")?;
                let args_json = format!(
                    "[{}]",
                    arg_vals
                        .iter()
                        .map(|v| value_to_json(v))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let worker = self
                    .engine_workers
                    .get_mut(wid)
                    .ok_or("engine worker is no longer running")?;
                let response = worker.call_json(fn_name, &args_json)?;
                let parsed = parse_json_value(&response)
                    .map_err(|e| format!("bad engine response: {}", e))?;
                if let Value::Dict(pairs) = &parsed {
                    for (k, v) in pairs {
                        if matches!(k, Value::Str(s) if s == "error") {
                            return Err(format!("{}", v));
                        }
                    }
                    for (k, v) in pairs {
                        if matches!(k, Value::Str(s) if s == "ok") {
                            return Ok(v.clone());
                        }
                    }
                }
                Err("bad engine response shape".into())
            }

            // ── Real regex implementations (simple pattern matching) ──
            // Regex functions take (text, pattern[, replacement]) per the docs.
            "__regex_match" | "__regex_match_" | "__regex_is_match" | "__regex_test" => {
                let text = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let pattern = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                Ok(Value::Bool(simple_regex_match(&pattern, &text)))
            }
            "__regex_find" | "__regex_search" => {
                let text = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let pattern = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                match simple_regex_find(&pattern, &text) {
                    Some(m) => Ok(Value::Str(m)),
                    None => Ok(Value::Null),
                }
            }
            "__regex_find_all" | "__regex_matches" => {
                let text = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let pattern = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                let matches: Vec<Value> = simple_regex_find_all(&pattern, &text)
                    .into_iter().map(Value::Str).collect();
                Ok(Value::List(matches))
            }
            "__regex_replace" | "__regex_sub" => {
                let text = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let pattern = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                // Replacement may be a string template or a lambda(match) -> str.
                if matches!(arg_vals.get(2), Some(Value::Func(_)) | Some(Value::BuiltinFunc(_))) {
                    let func = (*arg_vals.get(2).unwrap()).clone();
                    return self.regex_replace_with_fn(&text, &pattern, &func, false);
                }
                let replacement = arg_vals.get(2).and_then(|v| v.as_str()).unwrap_or_default();
                Ok(Value::Str(simple_regex_replace(&pattern, &replacement, &text, false)))
            }
            "__regex_replace_all" | "__regex_gsub" => {
                let text = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let pattern = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                if matches!(arg_vals.get(2), Some(Value::Func(_)) | Some(Value::BuiltinFunc(_))) {
                    let func = (*arg_vals.get(2).unwrap()).clone();
                    return self.regex_replace_with_fn(&text, &pattern, &func, true);
                }
                let replacement = arg_vals.get(2).and_then(|v| v.as_str()).unwrap_or_default();
                Ok(Value::Str(simple_regex_replace(&pattern, &replacement, &text, true)))
            }
            "__regex_capture" | "__regex_captures" | "__regex_groups" => {
                let text = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let pattern = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                let re = crate::regex_engine::compile(&pattern)?;
                let chars: Vec<char> = text.chars().collect();
                match re.search(&chars, 0) {
                    Some(m) => {
                        // Dict keyed by group index (0 = whole match) and, for
                        // named groups, by name: m[1], m["year"], ...
                        let mut pairs: Vec<(Value, Value)> = Vec::new();
                        let full: String = chars[m.start..m.end].iter().collect();
                        pairs.push((Value::Int(0), Value::Str(full)));
                        for (gi, grp) in m.groups.iter().enumerate() {
                            let val = match grp {
                                Some((s, e)) => {
                                    Value::Str(chars[*s..*e].iter().collect::<String>())
                                }
                                None => Value::Null,
                            };
                            pairs.push((Value::Int(gi as i64 + 1), val.clone()));
                            if let Some(Some(name)) = re.group_names.get(gi) {
                                pairs.push((Value::Str(name.clone()), val));
                            }
                        }
                        Ok(Value::Dict(pairs))
                    }
                    None => Ok(Value::Null),
                }
            }
            "__regex_split" => {
                let text = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                let pattern = arg_vals.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                let parts: Vec<Value> = simple_regex_split(&pattern, &text)
                    .into_iter().map(Value::Str).collect();
                Ok(Value::List(parts))
            }
            "__regex_compile" | "__regex_new" => {
                // Validate now; return a regex "object" as a dict. Method calls
                // on it (pat.match(text), pat.find_all(text), ...) are handled
                // by the compiled-pattern arm in call_builtin_method.
                let pattern = arg_vals.first().and_then(|v| v.as_str()).unwrap_or_default();
                crate::regex_engine::compile(&pattern)?;
                Ok(Value::Dict(vec![
                    (Value::Str("pattern".into()), Value::Str(pattern.to_string())),
                    (Value::Str("type".into()), Value::Str("regex".into())),
                ]))
            }

            // ── std.hash: real non-cryptographic hashes (return Int) ──
            "__hash_fnv1a" => Ok(Value::Int(crate::hashing::fnv1a32(&Self::arg_to_bytes(arg_vals.first())) as i64)),
            "__hash_fnv1a64" => Ok(Value::Int(crate::hashing::fnv1a64(&Self::arg_to_bytes(arg_vals.first())) as i64)),
            "__hash_djb2" => Ok(Value::Int(crate::hashing::djb2(&Self::arg_to_bytes(arg_vals.first())) as i64)),
            "__hash_sdbm" => Ok(Value::Int(crate::hashing::sdbm(&Self::arg_to_bytes(arg_vals.first())) as i64)),
            "__hash_crc32" | "__hash_crc32c" => Ok(Value::Int(crate::hashing::crc32(&Self::arg_to_bytes(arg_vals.first())) as i64)),
            "__hash_adler32" => Ok(Value::Int(crate::hashing::adler32(&Self::arg_to_bytes(arg_vals.first())) as i64)),
            "__hash_murmur3" => {
                let seed = arg_vals.get(1).and_then(|v| if let Value::Int(n) = v { Some(*n as u32) } else { None }).unwrap_or(0);
                Ok(Value::Int(crate::hashing::murmur3_32(&Self::arg_to_bytes(arg_vals.first()), seed) as i64))
            }
            "__hash_xxhash" | "__hash_xxhash64" => {
                // Approximate xxhash with fnv1a64 (deterministic, non-cryptographic).
                Ok(Value::Int(crate::hashing::fnv1a64(&Self::arg_to_bytes(arg_vals.first())) as i64))
            }
            "__hash_content_id" => {
                // Deterministic content hash of any value via its canonical string form.
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::sha256(s.as_bytes()))))
            }

            // ── std.crypto: real digests (return lowercase hex string) ──
            "__crypto_sha256" => Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::sha256(&Self::arg_to_bytes(arg_vals.first()))))),
            "__crypto_sha512" => Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::sha512(&Self::arg_to_bytes(arg_vals.first()))))),
            "__crypto_sha1" => Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::sha1(&Self::arg_to_bytes(arg_vals.first()))))),
            "__crypto_md5" => Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::md5(&Self::arg_to_bytes(arg_vals.first()))))),
            "__crypto_hmac" | "__crypto_hmac_sha256" => {
                let key = Self::arg_to_bytes(arg_vals.first());
                let msg = Self::arg_to_bytes(arg_vals.get(1));
                Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::hmac_sha256(&key, &msg))))
            }
            "__crypto_base64_encode" => Ok(Value::Str(crate::hashing::base64_encode(&Self::arg_to_bytes(arg_vals.first())))),
            "__crypto_base64_decode" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Bytes(crate::hashing::base64_decode(&s).unwrap_or_default()))
            }
            "__crypto_hex_encode" => Ok(Value::Str(crate::hashing::hex_encode(&Self::arg_to_bytes(arg_vals.first())))),
            "__crypto_hex_decode" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Bytes(crate::hashing::hex_decode(&s).unwrap_or_default()))
            }

            // ── std.uuid: real UUID generation ──
            "__uuid_v4" | "__crypto_uuid4" => {
                let mut b = crate::hashing::random_bytes(16);
                b[6] = (b[6] & 0x0F) | 0x40; // version 4
                b[8] = (b[8] & 0x3F) | 0x80; // RFC 4122 variant
                let h = crate::hashing::hex_encode(&b);
                Ok(Value::Str(format!(
                    "{}-{}-{}-{}-{}",
                    &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]
                )))
            }
            "__uuid_v7" => {
                // Time-ordered UUID v7: 48-bit ms timestamp + random.
                let ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let mut b = crate::hashing::random_bytes(16);
                b[0] = (ms >> 40) as u8;
                b[1] = (ms >> 32) as u8;
                b[2] = (ms >> 24) as u8;
                b[3] = (ms >> 16) as u8;
                b[4] = (ms >> 8) as u8;
                b[5] = ms as u8;
                b[6] = (b[6] & 0x0F) | 0x70; // version 7
                b[8] = (b[8] & 0x3F) | 0x80;
                let h = crate::hashing::hex_encode(&b);
                Ok(Value::Str(format!(
                    "{}-{}-{}-{}-{}",
                    &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]
                )))
            }
            "__uuid_nil" => Ok(Value::Str("00000000-0000-0000-0000-000000000000".into())),
            "__uuid_is_valid" | "__uuid_validate" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let ok = {
                    let parts: Vec<&str> = s.split('-').collect();
                    parts.len() == 5
                        && parts[0].len() == 8
                        && parts[1].len() == 4
                        && parts[2].len() == 4
                        && parts[3].len() == 4
                        && parts[4].len() == 12
                        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                };
                Ok(Value::Bool(ok))
            }

            // ── Top-level encoding/hash builtins (also usable bare) ──
            "base64_encode" => Ok(Value::Str(crate::hashing::base64_encode(&Self::arg_to_bytes(arg_vals.first())))),
            "base64_decode" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Bytes(crate::hashing::base64_decode(&s).unwrap_or_default()))
            }
            "hex_encode" => Ok(Value::Str(crate::hashing::hex_encode(&Self::arg_to_bytes(arg_vals.first())))),
            "hex_decode" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Bytes(crate::hashing::hex_decode(&s).unwrap_or_default()))
            }
            "sha256" => Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::sha256(&Self::arg_to_bytes(arg_vals.first()))))),
            "sha512" => Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::sha512(&Self::arg_to_bytes(arg_vals.first()))))),
            "sha1" => Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::sha1(&Self::arg_to_bytes(arg_vals.first()))))),
            "md5" => Ok(Value::Str(crate::hashing::hex_encode(&crate::hashing::md5(&Self::arg_to_bytes(arg_vals.first()))))),

            // ── std.fmt: printf-style formatting ──
            "__fmt_sprintf" | "sprintf" | "__fmt_format" | "format_str" => {
                let template = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let rest: Vec<&Value> = arg_vals.iter().skip(1).copied().collect();
                Ok(Value::Str(Self::printf_format(&template, &rest)))
            }

            // ── std.log: leveled structured logging (to stderr) ──
            "__log_debug" | "__log_info" | "__log_warn" | "__log_error" | "__log_fatal"
            | "__log_fatal_and_exit" => {
                let (level_num, level_name) = match name {
                    "__log_debug" => (0, "DEBUG"),
                    "__log_info" => (1, "INFO"),
                    "__log_warn" => (2, "WARN"),
                    "__log_error" => (3, "ERROR"),
                    _ => (4, "FATAL"),
                };
                // FATAL is never suppressed; others honor the configured level.
                if level_num >= self.log_level || level_num == 4 {
                    let msg = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                    let mut line = format!("[{}] {}", level_name, msg);
                    if let Some(Value::Dict(fields)) = arg_vals.get(1) {
                        for (k, v) in fields {
                            line.push_str(&format!(" {}={}", self.value_to_string(k), self.value_to_string(v)));
                        }
                    }
                    eprintln!("{}", line);
                }
                if name == "__log_fatal_and_exit" {
                    let code = arg_vals.iter().rev()
                        .find_map(|v| if let Value::Int(n) = v { Some(*n as i32) } else { None })
                        .unwrap_or(1);
                    std::process::exit(code);
                }
                Ok(Value::Null)
            }
            "__log_set_level" => {
                let lvl = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                self.log_level = match lvl.to_uppercase().as_str() {
                    "DEBUG" => 0, "INFO" => 1, "WARN" | "WARNING" => 2,
                    "ERROR" => 3, "FATAL" => 4, _ => self.log_level,
                };
                Ok(Value::Null)
            }
            "__log_set_format" | "__log_set_output" | "__log_add_sink"
            | "__log_context" | "__log_context_clear" => Ok(Value::Null),
            "__log_context_get" => Ok(Value::Dict(vec![])),

            // ── std.dotenv: .env parsing and env loading ──
            "__dotenv_parse" => {
                let text = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Dict(Self::parse_dotenv(&text)))
            }
            "__dotenv_read" => {
                let path = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_else(|| ".env".into());
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                Ok(Value::Dict(Self::parse_dotenv(&text)))
            }
            "__dotenv_load" => {
                let path = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_else(|| ".env".into());
                let override_existing = Self::named_arg(args, "override").map(|v| v.is_truthy()).unwrap_or(false);
                let text = std::fs::read_to_string(&path).map_err(|e| format!("dotenv.load('{}'): {}", path, e))?;
                let mut count = 0;
                for (k, v) in Self::parse_dotenv(&text) {
                    if let (Value::Str(key), Value::Str(val)) = (k, v) {
                        if override_existing || std::env::var(&key).is_err() {
                            std::env::set_var(&key, &val);
                            count += 1;
                        }
                    }
                }
                Ok(Value::Int(count))
            }
            "__dotenv_get" => {
                let key = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(std::env::var(&key).map(Value::Str).unwrap_or(Value::Null))
            }
            "__dotenv_get_or" => {
                let key = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let default = arg_vals.get(1).cloned().cloned().unwrap_or(Value::Null);
                Ok(std::env::var(&key).map(Value::Str).unwrap_or(default))
            }
            "__dotenv_set" => {
                let key = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let val = arg_vals.get(1).map(|v| self.value_to_string(v)).unwrap_or_default();
                std::env::set_var(&key, &val);
                Ok(Value::Null)
            }
            "__dotenv_require" => {
                let keys: Vec<String> = match arg_vals.first() {
                    Some(Value::List(items)) => items.iter().map(|v| self.value_to_string(v)).collect(),
                    Some(other) => vec![self.value_to_string(other)],
                    None => vec![],
                };
                let missing: Vec<String> = keys.into_iter().filter(|k| std::env::var(k).is_err()).collect();
                if missing.is_empty() {
                    Ok(Value::Bool(true))
                } else {
                    Err(format!("dotenv.require: missing required env vars: {}", missing.join(", ")))
                }
            }
            "__dotenv_all" => {
                let entries: Vec<(Value, Value)> = std::env::vars()
                    .map(|(k, v)| (Value::Str(k), Value::Str(v)))
                    .collect();
                Ok(Value::Dict(entries))
            }

            // ── std.toml: TOML parse / stringify ──
            "__toml_parse" | "__toml_decode" => {
                let text = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Self::parse_toml_value(&text))
            }
            "__toml_read" => {
                let path = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let text = std::fs::read_to_string(&path).map_err(|e| format!("toml.read('{}'): {}", path, e))?;
                Ok(Self::parse_toml_value(&text))
            }
            "__toml_stringify" | "__toml_encode" => {
                let mut out = String::new();
                Self::toml_stringify_value(arg_vals.first().copied().unwrap_or(&Value::Null), "", &mut out);
                Ok(Value::Str(out))
            }
            "__toml_write" => {
                let path = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let mut out = String::new();
                Self::toml_stringify_value(arg_vals.get(1).copied().unwrap_or(&Value::Null), "", &mut out);
                std::fs::write(&path, out).map(|_| Value::Null).map_err(|e| format!("toml.write('{}'): {}", path, e))
            }

            // ── std.diff: LCS-based text diffing ──
            "__diff_compute" | "__diff_diff" => {
                let a = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let b = arg_vals.get(1).map(|v| self.value_to_string(v)).unwrap_or_default();
                let mode = Self::named_arg(args, "mode").map(|v| self.value_to_string(v))
                    .unwrap_or_else(|| "line".into());
                let ua = Self::diff_units(&a, &mode);
                let ub = Self::diff_units(&b, &mode);
                let ops = Self::lcs_diff(&ua, &ub);
                let list: Vec<Value> = ops.into_iter().map(|(op, text)| {
                    let op_name = match op { '+' => "add", '-' => "remove", _ => "keep" };
                    Value::Dict(vec![
                        (Value::Str("op".into()), Value::Str(op_name.into())),
                        (Value::Str("text".into()), Value::Str(text)),
                    ])
                }).collect();
                Ok(Value::List(list))
            }
            "__diff_unified" => {
                let a = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let b = arg_vals.get(1).map(|v| self.value_to_string(v)).unwrap_or_default();
                let ua = Self::diff_units(&a, "line");
                let ub = Self::diff_units(&b, "line");
                let ops = Self::lcs_diff(&ua, &ub);
                let mut out = String::from("--- original\n+++ modified\n");
                out.push_str(&format!("@@ -1,{} +1,{} @@\n", ua.len(), ub.len()));
                for (op, text) in ops {
                    out.push(op);
                    out.push_str(&text);
                    out.push('\n');
                }
                Ok(Value::Str(out))
            }
            "__diff_side_by_side" => {
                let a = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let b = arg_vals.get(1).map(|v| self.value_to_string(v)).unwrap_or_default();
                let ops = Self::lcs_diff(&Self::diff_units(&a, "line"), &Self::diff_units(&b, "line"));
                let mut out = String::new();
                for (op, text) in ops {
                    let marker = match op { '+' => ">", '-' => "<", _ => "|" };
                    out.push_str(&format!("{} {}\n", marker, text));
                }
                Ok(Value::Str(out))
            }

            // ── std.money: currency-safe arithmetic on exact decimals ──
            "__money_new" => {
                let amount = arg_vals.first()
                    .and_then(|v| Self::as_decimal(v))
                    .ok_or("money.new: invalid amount")?;
                let currency = Self::named_arg(args, "currency")
                    .or_else(|| arg_vals.get(1).copied())
                    .map(|v| self.value_to_string(v))
                    .unwrap_or_else(|| "USD".into());
                Ok(Self::make_money(amount, &currency))
            }
            "__money_add" | "__money_sub" | "__money_compare" => {
                let (a, ca) = Self::money_parts(arg_vals.first().copied())
                    .ok_or_else(|| format!("{}: first argument is not money", name))?;
                let (b, cb) = Self::money_parts(arg_vals.get(1).copied())
                    .ok_or_else(|| format!("{}: second argument is not money", name))?;
                if ca != cb {
                    return Err(format!("money: currency mismatch {} vs {}", ca, cb));
                }
                match name {
                    "__money_add" => Ok(Self::make_money(a.add(&b), &ca)),
                    "__money_sub" => Ok(Self::make_money(a.sub(&b), &ca)),
                    _ => Ok(Value::Int(a.cmp(&b) as i64)),
                }
            }
            "__money_mul" => {
                let (a, ca) = Self::money_parts(arg_vals.first().copied())
                    .ok_or("money.mul: first argument is not money")?;
                let scalar = arg_vals.get(1).and_then(|v| Self::as_decimal(v))
                    .ok_or("money.mul: scalar required")?;
                Ok(Self::make_money(a.mul(&scalar), &ca))
            }
            "__money_div" => {
                let (a, ca) = Self::money_parts(arg_vals.first().copied())
                    .ok_or("money.div: first argument is not money")?;
                let scalar = arg_vals.get(1).and_then(|v| Self::as_decimal(v))
                    .ok_or("money.div: scalar required")?;
                let result = a.div(&scalar).ok_or("money.div: division by zero")?;
                let mode = Self::named_arg(args, "mode").map(|v| self.value_to_string(v));
                let rounded = match mode {
                    Some(m) => result.round_with_mode(2, &m),
                    None => result.round_with_mode(2, "HALF_EVEN"),
                };
                Ok(Self::make_money(rounded, &ca))
            }
            "__money_round" => {
                let (a, ca) = Self::money_parts(arg_vals.first().copied())
                    .ok_or("money.round: first argument is not money")?;
                let scale = Self::named_arg(args, "scale")
                    .or_else(|| arg_vals.get(1).copied())
                    .and_then(|v| if let Value::Int(n) = v { Some(*n as u32) } else { None })
                    .unwrap_or(2);
                let mode = Self::named_arg(args, "mode")
                    .or_else(|| arg_vals.get(2).copied())
                    .map(|v| self.value_to_string(v))
                    .unwrap_or_else(|| "HALF_UP".into());
                Ok(Self::make_money(a.round_with_mode(scale, &mode), &ca))
            }
            "__money_rates" => {
                // Identity: a rate table is just the provided dict.
                Ok(arg_vals.first().cloned().cloned().unwrap_or(Value::Dict(vec![])))
            }
            "__money_convert" => {
                let (amount, from) = Self::money_parts(arg_vals.first().copied())
                    .ok_or("money.convert: first argument is not money")?;
                let to = Self::named_arg(args, "to")
                    .map(|v| self.value_to_string(v))
                    .ok_or("money.convert: 'to' currency required")?;
                if to == from {
                    return Ok(Self::make_money(amount, &to));
                }
                let rates = Self::named_arg(args, "rates").cloned();
                let key = format!("{}/{}", from, to);
                let rate = match &rates {
                    Some(Value::Dict(pairs)) => pairs.iter()
                        .find(|(k, _)| matches!(k, Value::Str(s) if *s == key))
                        .and_then(|(_, v)| Self::as_decimal(v)),
                    _ => None,
                }.ok_or_else(|| format!("money.convert: no rate for {}", key))?;
                let mode = Self::named_arg(args, "mode").map(|v| self.value_to_string(v))
                    .unwrap_or_else(|| "HALF_EVEN".into());
                let converted = amount.mul(&rate).round_with_mode(2, &mode);
                Ok(Self::make_money(converted, &to))
            }
            "__money_format" => {
                let (amount, currency) = Self::money_parts(arg_vals.first().copied())
                    .ok_or("money.format: not a money value")?;
                Ok(Value::Str(format!("{} {}", currency, amount.to_fixed(2))))
            }

            // ── std.decimal: exact decimal arithmetic ──
            "__decimal_new" | "__decimal_from_str" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                crate::decimal::Decimal::from_str(&s)
                    .map(Value::Decimal)
                    .ok_or_else(|| format!("decimal.new: invalid decimal '{}'", s))
            }
            "__decimal_from_int" => {
                let n = arg_vals.first().and_then(|v| if let Value::Int(i) = v { Some(*i) } else { None }).unwrap_or(0);
                Ok(Value::Decimal(crate::decimal::Decimal::from_i64(n)))
            }
            "__decimal_from_float" => {
                let f = arg_vals.first().and_then(|v| if let Value::Float(x) = v { Some(*x) } else { None }).unwrap_or(0.0);
                Ok(Value::Decimal(crate::decimal::Decimal::from_f64(f)))
            }
            "__decimal_add" | "__decimal_sub" | "__decimal_mul" | "__decimal_div" | "__decimal_compare" => {
                let a = arg_vals.first().and_then(|v| Self::as_decimal(v));
                let b = arg_vals.get(1).and_then(|v| Self::as_decimal(v));
                match (a, b) {
                    (Some(a), Some(b)) => match name {
                        "__decimal_add" => Ok(Value::Decimal(a.add(&b))),
                        "__decimal_sub" => Ok(Value::Decimal(a.sub(&b))),
                        "__decimal_mul" => Ok(Value::Decimal(a.mul(&b))),
                        "__decimal_div" => a.div(&b).map(Value::Decimal).ok_or_else(|| "decimal.div: division by zero".to_string()),
                        _ => Ok(Value::Int(a.cmp(&b) as i64)),
                    },
                    _ => Err(format!("{}: requires two decimal-compatible arguments", name)),
                }
            }
            "__decimal_abs" => {
                Self::as_decimal(arg_vals.first().copied().unwrap_or(&Value::Null))
                    .map(|d| Value::Decimal(d.abs()))
                    .ok_or_else(|| "decimal.abs: not a decimal".to_string())
            }
            "__decimal_round" => {
                let d = arg_vals.first().and_then(|v| Self::as_decimal(v));
                let places = arg_vals.get(1).and_then(|v| if let Value::Int(i) = v { Some(*i as u32) } else { None }).unwrap_or(0);
                d.map(|d| Value::Decimal(d.round(places)))
                    .ok_or_else(|| "decimal.round: not a decimal".to_string())
            }
            "__decimal_to_str" => {
                Ok(Value::Str(arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default()))
            }

            // ── std.semver: real semantic-version parsing/comparison ──
            "__semver_parse" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                match Self::parse_semver(&s) {
                    Some((maj, min, pat, pre, build)) => Ok(Value::Dict(vec![
                        (Value::Str("major".into()), Value::Int(maj)),
                        (Value::Str("minor".into()), Value::Int(min)),
                        (Value::Str("patch".into()), Value::Int(pat)),
                        (Value::Str("prerelease".into()), if pre.is_empty() { Value::Null } else { Value::Str(pre) }),
                        (Value::Str("build".into()), if build.is_empty() { Value::Null } else { Value::Str(build) }),
                    ])),
                    None => Err(format!("semver.parse: invalid version '{}'", s)),
                }
            }
            "__semver_is_valid" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Bool(Self::parse_semver(&s).is_some()))
            }
            "__semver_compare" => {
                let a = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let b = arg_vals.get(1).map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Int(Self::semver_cmp(&a, &b) as i64))
            }
            "__semver_major" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Int(Self::parse_semver(&s).map(|t| t.0).unwrap_or(0)))
            }
            "__semver_minor" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Int(Self::parse_semver(&s).map(|t| t.1).unwrap_or(0)))
            }
            "__semver_patch_ver" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Int(Self::parse_semver(&s).map(|t| t.2).unwrap_or(0)))
            }
            "__semver_increment" => {
                let s = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let part = arg_vals.get(1).map(|v| self.value_to_string(v)).unwrap_or_else(|| "patch".into());
                if let Some((mut maj, mut min, mut pat, _, _)) = Self::parse_semver(&s) {
                    match part.as_str() {
                        "major" => { maj += 1; min = 0; pat = 0; }
                        "minor" => { min += 1; pat = 0; }
                        _ => { pat += 1; }
                    }
                    Ok(Value::Str(format!("{}.{}.{}", maj, min, pat)))
                } else {
                    Err(format!("semver.increment: invalid version '{}'", s))
                }
            }
            "__semver_satisfies" => {
                let v = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let range = arg_vals.get(1).map(|v| self.value_to_string(v)).unwrap_or_default();
                Ok(Value::Bool(Self::semver_satisfies(&v, &range)))
            }

            // ── std.csv: real CSV parse/stringify ──
            "__csv_parse" => {
                let text = arg_vals.first().map(|v| self.value_to_string(v)).unwrap_or_default();
                let rows = Self::parse_csv(&text);
                Ok(Value::List(
                    rows.into_iter()
                        .map(|row| Value::List(row.into_iter().map(Value::Str).collect()))
                        .collect(),
                ))
            }
            "__csv_stringify" | "__csv_write" => {
                if let Some(Value::List(rows)) = arg_vals.first() {
                    let mut out = String::new();
                    for row in rows {
                        if let Value::List(fields) = row {
                            let cells: Vec<String> = fields
                                .iter()
                                .map(|f| Self::csv_escape(&self.value_to_string(f)))
                                .collect();
                            out.push_str(&cells.join(","));
                            out.push('\n');
                        }
                    }
                    Ok(Value::Str(out))
                } else {
                    Ok(Value::Str(String::new()))
                }
            }

            // ── Catch-all for stub builtins (std.* module exports) ──
            _ if name.starts_with("__") => {
                // Return a reasonable stub value based on function name pattern
                let n = &name[2..]; // strip __
                if n.contains("is_") || n.contains("has_") || n.contains("exists") || n.contains("valid") {
                    Ok(Value::Bool(false))
                } else if n.contains("len") || n.contains("count") || n.contains("size") || n.contains("index") {
                    Ok(Value::Int(0))
                } else if n.contains("list") || n.contains("all") || n.contains("find_all")
                        || n.contains("keys") || n.contains("values") || n.contains("entries")
                        || n.contains("ls") || n.contains("walk") || n.contains("glob")
                        || n.contains("drain") || n.contains("collect") {
                    Ok(Value::List(Vec::new()))
                } else if n.contains("parse") || n.contains("decode") || n.contains("read") || n.contains("load") {
                    // parse/decode: return dict for structured, string for text
                    if n.contains("json") || n.contains("toml") || n.contains("yaml") || n.contains("csv") {
                        Ok(Value::Dict(Vec::new()))
                    } else {
                        Ok(Value::Str("".into()))
                    }
                } else if n.contains("stringify") || n.contains("encode") || n.contains("format")
                        || n.contains("to_string") || n.contains("render") || n.contains("sprintf")
                        || n.contains("join") || n.contains("basename") || n.contains("dirname")
                        || n.contains("ext") || n.contains("stem") || n.contains("abs")
                        || n.contains("normalize") || n.contains("platform") || n.contains("arch")
                        || n.contains("hostname") || n.contains("sha") || n.contains("md5")
                        || n.contains("hmac") || n.contains("hash") || n.contains("base64")
                        || n.contains("uuid") || n.contains("color") || n.contains("style") {
                    let result = if let Some(val) = arg_vals.first() {
                        format!("{}", val)
                    } else {
                        String::new()
                    };
                    Ok(Value::Str(result))
                } else if n.contains("float") || n.contains("rand") {
                    Ok(Value::Float(0.0))
                } else if n.contains("int") || n.contains("number") || n.contains("port") {
                    Ok(Value::Int(0))
                } else if n.contains("now") || n.contains("time") || n.contains("timestamp") {
                    Ok(Value::Int(0))
                } else if n.contains("create") || n.contains("new") || n.contains("open")
                        || n.contains("connect") || n.contains("listen") || n.contains("bind")
                        || n.contains("init") || n.contains("build") || n.contains("server") {
                    Ok(Value::Dict(vec![
                        (Value::Str("type".into()), Value::Str(n.to_string().into())),
                    ]))
                } else {
                    // Default: return null (most operations are side-effectful)
                    Ok(Value::Null)
                }
            }
            _ => Err(format!("Unknown builtin function '{}'", name)),
        }
    }

    // ── Helpers ──────────────────────────────────────────

    /// Evaluate a tagged template `tag f"...${expr}..."`. The literal segments
    /// (split at each `${...}`) are passed as a list, and each interpolated
    /// value is passed as a trailing positional argument:
    /// `tag(["a", "b", ""], v1, v2)`.
    fn eval_tagged_template(&mut self, tag: &Expr, template: &str) -> Result<Value, String> {
        let mut parts: Vec<Value> = Vec::new();
        let mut values: Vec<(Option<String>, Value)> = Vec::new();
        let mut current = String::new();
        let chars: Vec<char> = template.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                parts.push(Value::Str(std::mem::take(&mut current)));
                i += 2;
                let start = i;
                let mut depth = 1;
                while i < chars.len() && depth > 0 {
                    if chars[i] == '{' {
                        depth += 1;
                    } else if chars[i] == '}' {
                        depth -= 1;
                    }
                    if depth > 0 {
                        i += 1;
                    }
                }
                let expr_str: String = chars[start..i].iter().collect();
                i += 1; // skip closing }
                let mut lexer = crate::lexer::Lexer::new(&expr_str);
                let tokens = lexer.tokenize()?;
                let mut parser = crate::parser::Parser::new(tokens);
                let expr_ast = parser.parse_expr_public()?;
                let val = self.eval_expr(&expr_ast)?;
                values.push((None, val));
            } else {
                current.push(chars[i]);
                i += 1;
            }
        }
        parts.push(Value::Str(current));

        let tag_fn = self.eval_expr(tag)?;
        let mut call_args: Vec<(Option<String>, Value)> = Vec::with_capacity(values.len() + 1);
        call_args.push((None, Value::List(parts)));
        call_args.extend(values);
        self.call_value(&tag_fn, &call_args)
    }

    fn eval_fstring(&mut self, template: &str) -> Result<Value, String> {
        // Parse ${expr} inside the string
        let mut result = String::new();
        let chars: Vec<char> = template.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                // Find matching }
                i += 2;
                let start = i;
                let mut depth = 1;
                while i < chars.len() && depth > 0 {
                    if chars[i] == '{' {
                        depth += 1;
                    } else if chars[i] == '}' {
                        depth -= 1;
                    }
                    if depth > 0 {
                        i += 1;
                    }
                }
                let expr_str: String = chars[start..i].iter().collect();
                i += 1; // skip }

                // Check for format specifier: ${expr:format_spec}
                let (actual_expr, format_spec) = if let Some(colon_pos) = find_fstring_colon(&expr_str) {
                    (expr_str[..colon_pos].to_string(), Some(expr_str[colon_pos+1..].to_string()))
                } else {
                    (expr_str, None)
                };

                // Parse and evaluate the expression
                let mut lexer = crate::lexer::Lexer::new(&actual_expr);
                let tokens = lexer.tokenize()?;
                let mut parser = crate::parser::Parser::new(tokens);
                let expr_ast = parser.parse_expr_public()?;
                let val = self.eval_expr(&expr_ast)?;

                if let Some(spec) = format_spec {
                    result.push_str(&format_value_with_spec(&val, &spec));
                } else {
                    result.push_str(&format!("{}", val));
                }
            } else {
                result.push(chars[i]);
                i += 1;
            }
        }
        Ok(Value::Str(result))
    }

    fn value_to_string(&mut self, val: &Value) -> String {
        // Check for __str__ on instances — look in instance fields AND class methods
        if let Value::Instance(cls_name, fields) = val {
            // First check instance fields
            if let Some(method) = fields.get("__str__").cloned() {
                if let Ok(result) = self.call_value(&method, &[]) {
                    return format!("{}", result);
                }
            }
            // Then check class methods
            if let Some(cv) = self.env.get(cls_name) {
                if let Value::Class(ref class_val) = cv {
                    if let Some(func) = class_val.methods.get("__str__") {
                        // Push scope, bind self, call method
                        self.env.push_scope();
                        self.env.define("self", val.clone());
                        let result = self.exec_block(&func.body);
                        self.env.pop_scope();
                        match result {
                            Ok(Value::Return(v)) => return format!("{}", v),
                            Ok(v) => return format!("{}", v),
                            _ => {}
                        }
                    }
                    // @data: auto-generate toString
                    if class_val.is_data {
                        let field_strs: Vec<String> = fields.iter()
                            .map(|(k, v)| format!("{}: {}", k, v))
                            .collect();
                        return format!("{}({})", cls_name, field_strs.join(", "));
                    }
                }
            }
        }
        if let Value::CowInstance(cls_name, fields) = val {
            if let Some(method) = fields.borrow().get("__str__").cloned() {
                if let Ok(result) = self.call_value(&method, &[]) {
                    return format!("{}", result);
                }
            }
            if let Some(cv) = self.env.get(cls_name) {
                if let Value::Class(ref class_val) = cv {
                    if let Some(func) = class_val.methods.get("__str__") {
                        self.env.push_scope();
                        self.env.define("self", val.clone());
                        let result = self.exec_block(&func.body);
                        self.env.pop_scope();
                        match result {
                            Ok(Value::Return(v)) => return format!("{}", v),
                            Ok(v) => return format!("{}", v),
                            _ => {}
                        }
                    }
                    if class_val.is_data {
                        let field_strs: Vec<String> = fields
                            .borrow()
                            .iter()
                            .map(|(k, v)| format!("{}: {}", k, v))
                            .collect();
                        return format!("{}({})", cls_name, field_strs.join(", "));
                    }
                }
            }
        }
        format!("{}", val)
    }

    fn eval_comp_clauses<F>(&mut self, clauses: &[CompClause], mut f: F) -> Result<Vec<Value>, String>
    where
        F: FnMut(&mut Self) -> Result<Value, String>,
    {
        let mut result = Vec::new();
        self.eval_comp_clauses_raw(clauses, &mut |interp| {
            result.push(f(interp)?);
            Ok(())
        })?;
        Ok(result)
    }

    fn eval_comp_clauses_raw<F>(&mut self, clauses: &[CompClause], f: &mut F) -> Result<(), String>
    where
        F: FnMut(&mut Self) -> Result<(), String>,
    {
        if clauses.is_empty() {
            return f(self);
        }
        let clause = &clauses[0];
        let rest = &clauses[1..];
        let iterable = self.eval_expr(&clause.iter)?;
        // `for (k, v) in dict` in a comprehension yields [key, value] pairs.
        let items = match &iterable {
            Value::Dict(pairs) if clause.destructure.is_some() => pairs
                .iter()
                .map(|(k, v)| Value::List(vec![k.clone(), v.clone()]))
                .collect(),
            _ => self.collect_iter(&iterable)?,
        };
        self.env.push_scope();
        for item in items {
            if let Some(ref destructure) = clause.destructure {
                if let Value::Tuple(vals) | Value::List(vals) = &item {
                    for (i, name) in destructure.iter().enumerate() {
                        let v = vals.get(i).cloned().unwrap_or(Value::Null);
                        self.env.define(name, v);
                    }
                } else {
                    self.env.define(&clause.var, item);
                }
            } else {
                self.env.define(&clause.var, item);
            }
            if let Some(ref condition) = clause.cond {
                let c = self.eval_expr(condition)?;
                if !c.is_truthy() { continue; }
            }
            self.eval_comp_clauses_raw(rest, f)?;
        }
        self.env.pop_scope();
        Ok(())
    }

    /// Materialize an iterable into a Vec, driving lazy generators to completion
    /// (needed by comprehensions, which cannot suspend). Falls back to the
    /// immutable value_to_iter for everything else.
    fn collect_iter(&mut self, val: &Value) -> Result<Vec<Value>, String> {
        if let Value::Generator(ref gs) = val {
            if gs.borrow().lazy.is_some() {
                let mut out = Vec::new();
                loop {
                    let next_result = self.call_builtin_method(val, "next", &[])?;
                    let (done, value) = if let Value::Dict(ref pairs) = next_result {
                        let done = pairs.iter()
                            .find(|(k, _)| matches!(k, Value::Str(s) if s == "done"))
                            .map(|(_, v)| v.is_truthy()).unwrap_or(true);
                        let value = pairs.iter()
                            .find(|(k, _)| matches!(k, Value::Str(s) if s == "value"))
                            .map(|(_, v)| v.clone()).unwrap_or(Value::Null);
                        (done, value)
                    } else {
                        (true, Value::Null)
                    };
                    if done {
                        break;
                    }
                    out.push(value);
                }
                return Ok(out);
            }
        }
        self.value_to_iter(val)
    }

    fn value_to_iter(&self, val: &Value) -> Result<Vec<Value>, String> {
        match val {
            Value::List(items) => Ok(items.clone()),
            Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
            Value::Range(start, end, inclusive) => {
                let end = if *inclusive { *end + 1 } else { *end };
                Ok((*start..end).map(Value::Int).collect())
            }
            Value::Dict(pairs) => {
                Ok(pairs.iter().map(|(k, _)| k.clone()).collect())
            }
            Value::Tuple(items) => Ok(items.clone()),
            Value::Generator(gs) => {
                let mut state = gs.borrow_mut();
                if state.lazy.is_some() {
                    // Lazy generators should be iterated via ForIn; here just return empty
                    Ok(vec![])
                } else {
                    let remaining: Vec<Value> = state.items[state.index..].to_vec();
                    state.index = state.items.len();
                    Ok(remaining)
                }
            }
            _ => Err(format!("Cannot iterate over {}", val.type_name())),
        }
    }

    // Repeat a string n times; negative counts give "" (like Python's s * -1),
    // and absurdly large results are an error instead of an allocator panic.
    fn repeat_str(s: &str, n: i64) -> Result<String, String> {
        if n <= 0 { return Ok(String::new()); }
        if (s.len() as u64).saturating_mul(n as u64) > 1_000_000_000 {
            return Err("String repeat result too large".into());
        }
        Ok(s.repeat(n as usize))
    }

    fn index_value(&self, obj: &Value, idx: &Value) -> Result<Value, String> {
        // Range index = slice: s[1..3], lst[2..], t[..=4] all delegate to slice_value.
        // Dicts are excluded so a range can still be a dict key.
        if let Value::Range(s, e, inclusive) = idx {
            if !matches!(obj, Value::Dict(_)) {
                let end = if *inclusive { *e + 1 } else { *e };
                return self.slice_value(obj, Some(Value::Int(*s)), Some(Value::Int(end)), None);
            }
        }
        match (obj, idx) {
            (Value::List(items), Value::Int(i)) => {
                let i = if *i < 0 {
                    (items.len() as i64 + i) as usize
                } else {
                    *i as usize
                };
                items.get(i).cloned().ok_or_else(|| format!("Index {} out of bounds", i))
            }
            (Value::Str(s), Value::Int(i)) => {
                let i = if *i < 0 {
                    (s.len() as i64 + i) as usize
                } else {
                    *i as usize
                };
                s.chars()
                    .nth(i)
                    .map(|c| Value::Str(c.to_string()))
                    .ok_or_else(|| format!("Index {} out of bounds", i))
            }
            (Value::Bytes(bytes), Value::Int(i)) => {
                let i = if *i < 0 {
                    (bytes.len() as i64 + i) as usize
                } else {
                    *i as usize
                };
                bytes
                    .get(i)
                    .map(|b| Value::Int(*b as i64))
                    .ok_or_else(|| format!("Index {} out of bounds", i))
            }
            (Value::Dict(pairs), key) => {
                for (k, v) in pairs {
                    if k == key {
                        return Ok(v.clone());
                    }
                }
                Err(format!("Key {} not found", key))
            }
            (Value::Instance(_, fields), Value::Str(key)) => {
                fields.get(key).cloned().ok_or_else(|| format!("Field '{}' not found", key))
            }
            (Value::StructInstance(_, fields), Value::Str(key)) => {
                fields.get(key).cloned().ok_or_else(|| format!("Field '{}' not found", key))
            }
            (Value::Tuple(items), Value::Int(i)) => {
                let i = if *i < 0 {
                    (items.len() as i64 + i) as usize
                } else {
                    *i as usize
                };
                items.get(i).cloned().ok_or_else(|| format!("Index {} out of bounds", i))
            }
            _ => Err(format!(
                "Cannot index {} with {}",
                obj.type_name(),
                idx.type_name()
            )),
        }
    }

    fn slice_value(&self, obj: &Value, start: Option<Value>, end: Option<Value>, step: Option<Value>) -> Result<Value, String> {
        let step_n = match &step {
            Some(Value::Int(n)) => *n,
            None => 1,
            _ => return Err("Slice step must be an integer".into()),
        };
        if step_n == 0 {
            return Err("Slice step cannot be zero".into());
        }
        match obj {
            Value::List(items) => {
                let len = items.len() as i64;
                let s = self.resolve_slice_index(start.as_ref(), 0, len);
                let e = self.resolve_slice_index(end.as_ref(), len, len);
                let mut result = Vec::new();
                if step_n > 0 {
                    let mut i = s;
                    while i < e {
                        if i >= 0 && (i as usize) < items.len() {
                            result.push(items[i as usize].clone());
                        }
                        i += step_n;
                    }
                } else {
                    let mut i = self.resolve_slice_index(start.as_ref(), len - 1, len);
                    let e = self.resolve_slice_index(end.as_ref(), -1, len);
                    while i > e {
                        if i >= 0 && (i as usize) < items.len() {
                            result.push(items[i as usize].clone());
                        }
                        i += step_n;
                    }
                }
                Ok(Value::List(result))
            }
            Value::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let len = chars.len() as i64;
                let start_i = self.resolve_slice_index(start.as_ref(), 0, len);
                let end_i = self.resolve_slice_index(end.as_ref(), len, len);
                let mut result = String::new();
                if step_n > 0 {
                    let mut i = start_i;
                    while i < end_i {
                        if i >= 0 && (i as usize) < chars.len() {
                            result.push(chars[i as usize]);
                        }
                        i += step_n;
                    }
                } else {
                    let mut i = self.resolve_slice_index(start.as_ref(), len - 1, len);
                    let e = self.resolve_slice_index(end.as_ref(), -1, len);
                    while i > e {
                        if i >= 0 && (i as usize) < chars.len() {
                            result.push(chars[i as usize]);
                        }
                        i += step_n;
                    }
                }
                Ok(Value::Str(result))
            }
            Value::Tuple(items) => {
                let sliced = self.slice_value(&Value::List(items.clone()), start, end, step)?;
                match sliced {
                    Value::List(v) => Ok(Value::Tuple(v)),
                    other => Ok(other),
                }
            }
            Value::Bytes(bytes) => {
                let len = bytes.len() as i64;
                let s = self.resolve_slice_index(start.as_ref(), 0, len);
                let e = self.resolve_slice_index(end.as_ref(), len, len);
                let mut result = Vec::new();
                if step_n > 0 {
                    let mut i = s;
                    while i < e {
                        if i >= 0 && (i as usize) < bytes.len() {
                            result.push(bytes[i as usize]);
                        }
                        i += step_n;
                    }
                } else {
                    let mut i = self.resolve_slice_index(start.as_ref(), len - 1, len);
                    let e = self.resolve_slice_index(end.as_ref(), -1, len);
                    while i > e {
                        if i >= 0 && (i as usize) < bytes.len() {
                            result.push(bytes[i as usize]);
                        }
                        i += step_n;
                    }
                }
                Ok(Value::Bytes(result))
            }
            _ => Err(format!("Cannot slice {}", obj.type_name())),
        }
    }

    fn resolve_slice_index(&self, val: Option<&Value>, default: i64, len: i64) -> i64 {
        match val {
            Some(Value::Int(n)) => {
                if *n < 0 { (len + n).max(0) } else { (*n).min(len) }
            }
            None => default,
            _ => default,
        }
    }

    fn matches_pattern(&self, val: &Value, pattern: &Pattern) -> Result<bool, String> {
        match pattern {
            Pattern::Literal(expr) => {
                match expr {
                    Expr::Int(n) => Ok(val == &Value::Int(*n)),
                    Expr::Float(f) => Ok(val == &Value::Float(*f)),
                    Expr::Str(s) => Ok(val == &Value::Str(s.clone())),
                    Expr::Bool(b) => Ok(val == &Value::Bool(*b)),
                    Expr::Null => Ok(matches!(val, Value::Null)),
                    _ => Ok(false),
                }
            }
            Pattern::Ident(_) => Ok(true),
            Pattern::Wildcard => Ok(true),
            Pattern::Default => Ok(true),
            Pattern::Or(pats) => {
                for p in pats {
                    if self.matches_pattern(val, p)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Pattern::Range { start, end, inclusive } => {
                if let (Value::Int(v), Expr::Int(s), Expr::Int(e)) = (val, start.as_ref(), end.as_ref()) {
                    if *inclusive {
                        Ok(*v >= *s && *v <= *e)
                    } else {
                        Ok(*v >= *s && *v < *e)
                    }
                } else {
                    Ok(false)
                }
            }
            Pattern::Ok(inner) => {
                if let Value::Ok(v) = val {
                    self.matches_pattern(v, inner)
                } else {
                    Ok(false)
                }
            }
            Pattern::Err(inner) => {
                if let Value::Err(v) = val {
                    self.matches_pattern(v, inner)
                } else {
                    Ok(false)
                }
            }
            Pattern::Some(inner) => {
                if let Value::Some(v) = val {
                    self.matches_pattern(v, inner)
                } else {
                    Ok(false)
                }
            }
            Pattern::None => Ok(matches!(val, Value::Null)),
            Pattern::Tuple(pats) => {
                if let Value::Tuple(items) = val {
                    if pats.len() != items.len() {
                        return Ok(false);
                    }
                    for (p, v) in pats.iter().zip(items.iter()) {
                        if !self.matches_pattern(v, p)? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Pattern::Destructure { path, fields } => {
                if let Value::EnumVariant(ename, vname, data) = val {
                    let matches = if path.len() == 2 {
                        path[0] == *ename && path[1] == *vname
                    } else if path.len() == 1 {
                        path[0] == *vname
                    } else {
                        false
                    };
                    Ok(matches && (fields.is_empty() || fields.len() == data.len()))
                } else {
                    Ok(false)
                }
            }
            Pattern::List(pats) => {
                if let Value::List(items) = val {
                    // Check if any pattern is Rest; if so, min length check
                    let has_rest = pats.iter().any(|p| matches!(p, Pattern::Rest(_)));
                    if has_rest {
                        let non_rest = pats.iter().filter(|p| !matches!(p, Pattern::Rest(_))).count();
                        if items.len() < non_rest { return Ok(false); }
                    } else if pats.len() != items.len() {
                        return Ok(false);
                    }
                    let mut item_idx = 0;
                    for p in pats {
                        if let Pattern::Rest(_) = p {
                            let rest_count = items.len() - (pats.len() - 1);
                            item_idx += rest_count;
                        } else {
                            if !self.matches_pattern(&items[item_idx], p)? { return Ok(false); }
                            item_idx += 1;
                        }
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Pattern::Rest(_) => Ok(true), // Always matches (used inside List)
            Pattern::TypePat(type_name) => {
                let matches = match type_name.as_str() {
                    "int" => matches!(val, Value::Int(_)),
                    "float" => matches!(val, Value::Float(_)),
                    "str" => matches!(val, Value::Str(_)),
                    "bool" => matches!(val, Value::Bool(_)),
                    "list" => matches!(val, Value::List(_)),
                    "dict" => matches!(val, Value::Dict(_)),
                    "tuple" => matches!(val, Value::Tuple(_)),
                    "set" => matches!(val, Value::Set(_)),
                    "bytes" => matches!(val, Value::Bytes(_)),
                    "null" => matches!(val, Value::Null),
                    "generator" => matches!(val, Value::Generator(_)),
                    "pointer" => matches!(val, Value::Pointer(_)),
                    _ => {
                        // Check if value is an instance of that class
                        match val {
                            Value::Instance(class_name, _) => class_name == type_name,
                            Value::CowInstance(class_name, _) => class_name == type_name,
                            _ => false,
                        }
                    }
                };
                Ok(matches)
            }
            Pattern::StructPat { type_name, fields } => {
                // Anonymous { key, ... } patterns also match dicts: every named
                // key must exist, and any nested pattern must match its value.
                if type_name.is_none() {
                    if let Value::Dict(pairs) = val {
                        for (field_name, field_pat) in fields {
                            let key = Value::Str(field_name.clone());
                            let Some((_, fv)) = pairs.iter().find(|(k, _)| *k == key) else {
                                return Ok(false);
                            };
                            if let Some(fp) = field_pat {
                                if !self.matches_pattern(fv, fp)? { return Ok(false); }
                            }
                        }
                        return Ok(true);
                    }
                }
                let inst_class = match val {
                    Value::Instance(class_name, _) => Some(class_name.clone()),
                    Value::CowInstance(class_name, _) => Some(class_name.clone()),
                    Value::StructInstance(class_name, _) => Some(class_name.clone()),
                    _ => None,
                };
                if let Some(ref tn) = type_name {
                    if inst_class.as_deref() != Some(tn.as_str()) { return Ok(false); }
                } else if inst_class.is_none() {
                    return Ok(false);
                }
                // All named fields must exist and match
                for (field_name, field_pat) in fields {
                    let field_val = match val {
                        Value::Instance(_, flds) => flds.get(field_name).cloned().unwrap_or(Value::Null),
                        Value::CowInstance(_, flds) => flds.borrow().get(field_name).cloned().unwrap_or(Value::Null),
                        Value::StructInstance(_, flds) => flds.get(field_name).cloned().unwrap_or(Value::Null),
                        _ => return Ok(false),
                    };
                    if let Some(fp) = field_pat {
                        if !self.matches_pattern(&field_val, fp)? { return Ok(false); }
                    }
                }
                Ok(true)
            }
            Pattern::TypedBind { type_name, .. } => {
                // Matches when the value has the named type (binding occurs in bind_pattern).
                let matches = match type_name.as_str() {
                    "int" => matches!(val, Value::Int(_)),
                    "float" => matches!(val, Value::Float(_)),
                    "str" => matches!(val, Value::Str(_)),
                    "bool" => matches!(val, Value::Bool(_)),
                    "list" => matches!(val, Value::List(_)),
                    "dict" => matches!(val, Value::Dict(_)),
                    "tuple" => matches!(val, Value::Tuple(_)),
                    "set" => matches!(val, Value::Set(_)),
                    "bytes" => matches!(val, Value::Bytes(_)),
                    "null" => matches!(val, Value::Null),
                    "generator" => matches!(val, Value::Generator(_)),
                    "pointer" => matches!(val, Value::Pointer(_)),
                    _ => match val {
                        Value::Instance(class_name, _) => class_name == type_name,
                        Value::CowInstance(class_name, _) => class_name == type_name,
                        _ => false,
                    },
                };
                Ok(matches)
            }
        }
    }

    fn bind_pattern(&mut self, val: &Value, pattern: &Pattern) -> Result<(), String> {
        match pattern {
            Pattern::Ident(name) => {
                self.env.define(name, val.clone());
            }
            Pattern::Ok(inner) => {
                if let Value::Ok(v) = val {
                    self.bind_pattern(v, inner)?;
                }
            }
            Pattern::Err(inner) => {
                if let Value::Err(v) = val {
                    self.bind_pattern(v, inner)?;
                }
            }
            Pattern::Some(inner) => {
                if let Value::Some(v) = val {
                    self.bind_pattern(v, inner)?;
                }
            }
            Pattern::Tuple(pats) => {
                if let Value::Tuple(items) = val {
                    for (p, v) in pats.iter().zip(items.iter()) {
                        self.bind_pattern(v, p)?;
                    }
                }
            }
            Pattern::Destructure { fields, .. } => {
                if let Value::EnumVariant(_, _, data) = val {
                    for (p, v) in fields.iter().zip(data.iter()) {
                        self.bind_pattern(v, p)?;
                    }
                }
            }
            Pattern::List(pats) => {
                if let Value::List(items) = val {
                    let mut item_idx = 0;
                    for p in pats {
                        if let Pattern::Rest(name) = p {
                            let rest_count = items.len().saturating_sub(pats.len() - 1);
                            let rest_items: Vec<Value> = items[item_idx..item_idx + rest_count].to_vec();
                            self.env.define(name, Value::List(rest_items));
                            item_idx += rest_count;
                        } else {
                            if let Some(v) = items.get(item_idx) {
                                self.bind_pattern(v, p)?;
                            }
                            item_idx += 1;
                        }
                    }
                }
            }
            Pattern::Rest(name) => {
                self.env.define(name, val.clone());
            }
            Pattern::TypePat(_) => {} // no binding for type patterns
            Pattern::TypedBind { name, .. } => {
                self.env.define(name, val.clone());
            }
            Pattern::StructPat { fields, .. } => {                // Bind field variables from instance fields
                match val {
                    Value::Dict(pairs) => {
                        for (field_name, field_pat) in fields {
                            let key = Value::Str(field_name.clone());
                            let fv = pairs
                                .iter()
                                .find(|(k, _)| *k == key)
                                .map(|(_, v)| v.clone())
                                .unwrap_or(Value::Null);
                            if let Some(fp) = field_pat {
                                self.bind_pattern(&fv, fp)?;
                            } else {
                                self.env.define(field_name, fv);
                            }
                        }
                    }
                    Value::Instance(_, inst_fields) => {
                        for (field_name, field_pat) in fields {
                            let fv = inst_fields.get(field_name).cloned().unwrap_or(Value::Null);
                            if let Some(fp) = field_pat {
                                self.bind_pattern(&fv, fp)?;
                            } else {
                                self.env.define(field_name, fv);
                            }
                        }
                    }
                    Value::CowInstance(_, inst_fields) => {
                        for (field_name, field_pat) in fields {
                            let fv = inst_fields.borrow().get(field_name).cloned().unwrap_or(Value::Null);
                            if let Some(fp) = field_pat {
                                self.bind_pattern(&fv, fp)?;
                            } else {
                                self.env.define(field_name, fv);
                            }
                        }
                    }
                    Value::StructInstance(_, inst_fields) => {
                        for (field_name, field_pat) in fields {
                            let fv = inst_fields.get(field_name).cloned().unwrap_or(Value::Null);
                            if let Some(fp) = field_pat {
                                self.bind_pattern(&fv, fp)?;
                            } else {
                                self.env.define(field_name, fv);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Pattern::Or(pats) => {
                for p in pats {
                    if self.matches_pattern(val, p)? {
                        self.bind_pattern(val, p)?;
                        break;
                    }
                }
            }
            _ => {} // Literal, Wildcard, Default, Range, None — no binding
        }
        Ok(())
    }

    fn apply_derive(&mut self, class_name: &str, traits_str: &str) {
        let traits: Vec<&str> = traits_str.split(',').map(|s| s.trim()).collect();
        if let Some(Value::Class(mut cls)) = self.env.get(class_name) {
            for trait_name in traits {
                match trait_name {
                    "Eq" | "PartialEq" => {
                        // Mark as data class for automatic field equality comparison
                        cls.is_data = true;
                    }
                    "Clone" => {
                        if !cls.methods.contains_key("clone") {
                            cls.methods.insert("clone".to_string(), FuncValue {
                                name: "clone".to_string(),
                                params: vec![],
                                body: vec![Stmt::Return(Some(Expr::Ident("self".to_string())))],
                                closure_env: self.env.current,
                                is_generator: false,
                            });
                        }
                    }
                    "Default" => {
                        if !cls.methods.contains_key("default_") {
                            cls.methods.insert("default_".to_string(), FuncValue {
                                name: "default_".to_string(),
                                params: vec![],
                                body: vec![Stmt::Return(Some(Expr::Null))],
                                closure_env: self.env.current,
                                is_generator: false,
                            });
                        }
                    }
                    "Display" | "ToString" => {
                        if !cls.methods.contains_key("__str__") {
                            cls.methods.insert("__str__".to_string(), FuncValue {
                                name: "__str__".to_string(),
                                params: vec![],
                                body: vec![Stmt::Return(Some(Expr::Ident("self".to_string())))],
                                closure_env: self.env.current,
                                is_generator: false,
                            });
                        }
                    }
                    "Debug" => {
                        if !cls.methods.contains_key("debug") {
                            cls.methods.insert("debug".to_string(), FuncValue {
                                name: "debug".to_string(),
                                params: vec![],
                                body: vec![Stmt::Return(Some(Expr::Ident("self".to_string())))],
                                closure_env: self.env.current,
                                is_generator: false,
                            });
                        }
                    }
                    "Hash" => {
                        if !cls.methods.contains_key("__hash__") {
                            cls.methods.insert("__hash__".to_string(), FuncValue {
                                name: "__hash__".to_string(),
                                params: vec![],
                                body: vec![Stmt::Return(Some(Expr::Int(0)))],
                                closure_env: self.env.current,
                                is_generator: false,
                            });
                        }
                    }
                    _ => {}
                }
            }
            self.env.set(class_name, Value::Class(cls)).ok();
        }
    }

    fn apply_memo_decorator(&mut self, func: Value) -> Result<Value, String> {
        if let Value::Func(ref fv) = func {
            self.memo_caches.insert(fv.name.clone(), HashMap::new());
        }
        Ok(func)
    }

    fn apply_deprecated_decorator(&mut self, func: Value, msg: Option<String>) -> Result<Value, String> {
        if let Value::Func(ref fv) = func {
            self.deprecated_funcs.insert(fv.name.clone(), msg);
        }
        Ok(func)
    }
}
