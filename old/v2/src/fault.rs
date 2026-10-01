//! Hardware fault handling runtime for V2.
//!
//! Implements OS-level signal/exception registration (POSIX `sigaction` and
//! Windows VEH), fault context capture, V2 callback dispatch, and JSON crash
//! report generation.
//!
//! This is **Milestone 1**: registration + capture + termination-only dispatch
//! + `dump_json`. Recovery (`set_recovery_point`/`recover`) and native core
//! dumps are planned for later milestones.
//!
//! # Safety
//! All public functions in this module are intentionally unsafe-adjacent: fault
//! handlers run in a restricted context and make no guarantees about heap
//! consistency. The V2 language surface gates all of these behind `unsafe {}`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::cell::Cell;

use crate::value::Value;

// ─── Fault context (async-signal-safe storage) ────────────────────────────
// Written only by OS handlers; read by the dispatch path.

/// Set when an OS fault has been captured and the V2 dispatch is pending.
pub static FAULT_PENDING: AtomicBool = AtomicBool::new(false);

/// POSIX signal number (or Windows exception code cast to i32) of the fault.
pub static FAULT_SIGNUM: AtomicI32 = AtomicI32::new(0);

/// Faulting memory address (0 if unavailable).
pub static FAULT_ADDR: AtomicI64 = AtomicI64::new(0);

/// Program counter at fault time (0 if unavailable in Milestone 1).
pub static FAULT_PC: AtomicI64 = AtomicI64::new(0);

/// Whether this fault was a stack overflow.
pub static FAULT_IS_STACK_OVERFLOW: AtomicBool = AtomicBool::new(false);

// ─── Thread-local interpreter back-pointer ────────────────────────────────
// We store a raw pointer to the Interpreter that is executing on this thread.
// Hardware faults always fire on the faulting thread, so the thread-local is
// always valid when the OS handler runs (as long as the interpreter is live).

use crate::interpreter::Interpreter;

thread_local! {
    static INTERP_PTR: Cell<*mut Interpreter> = Cell::new(std::ptr::null_mut());
}

/// Store a back-pointer to the interpreter executing on this thread.
/// Called from `Interpreter::exec` before beginning execution.
pub fn set_interpreter_ptr(interp: *mut Interpreter) {
    INTERP_PTR.with(|c| c.set(interp));
}

/// Clear the interpreter back-pointer.
/// Called from `Interpreter::exec` after execution completes or on panic.
pub fn clear_interpreter_ptr() {
    INTERP_PTR.with(|c| c.set(std::ptr::null_mut()));
}

// ─── Platform helpers ──────────────────────────────────────────────────────

/// Return the OS thread ID of the calling thread.
pub fn current_thread_id() -> u64 {
    #[cfg(unix)]
    unsafe {
        libc::pthread_self() as u64
    }
    #[cfg(windows)]
    {
        extern "system" { fn GetCurrentThreadId() -> u32; }
        unsafe { GetCurrentThreadId() as u64 }
    }
    #[cfg(not(any(unix, windows)))]
    { 0 }
}

// ─── Signal name mapping ───────────────────────────────────────────────────

/// Supported V2 hardware fault signal names.
pub const FAULT_SIGNAL_NAMES: &[&str] = &["SIGSEGV", "SIGBUS", "SIGFPE", "SIGABRT"];

/// Map a POSIX signal number to a V2 fault name.
#[cfg(unix)]
pub fn signum_to_fault_name(signum: i32) -> Option<&'static str> {
    if signum == libc::SIGSEGV { return Some("SIGSEGV"); }
    if signum == libc::SIGBUS  { return Some("SIGBUS");  }
    if signum == libc::SIGFPE  { return Some("SIGFPE");  }
    if signum == libc::SIGABRT { return Some("SIGABRT"); }
    None
}

/// Map a V2 fault name to a POSIX signal number.
#[cfg(unix)]
pub fn fault_name_to_signum(name: &str) -> Option<i32> {
    match name {
        "SIGSEGV" => Some(libc::SIGSEGV),
        "SIGBUS"  => Some(libc::SIGBUS),
        "SIGFPE"  => Some(libc::SIGFPE),
        "SIGABRT" => Some(libc::SIGABRT),
        _ => None,
    }
}

// ─── FaultInfo dict builder ────────────────────────────────────────────────

/// Build a `FaultInfo` dict from the currently captured fault context.
/// This is the value passed to the V2 fault handler.
pub fn make_fault_info_dict(signal_name: &str) -> Value {
    let addr = FAULT_ADDR.load(Ordering::Acquire);
    let pc   = FAULT_PC.load(Ordering::Acquire);
    let is_so = FAULT_IS_STACK_OVERFLOW.load(Ordering::Acquire);
    Value::Dict(vec![
        (Value::Str("signal".into()),            Value::Str(signal_name.into())),
        (Value::Str("address".into()),           if addr == 0 { Value::Null } else { Value::Int(addr) }),
        (Value::Str("pc".into()),                if pc   == 0 { Value::Null } else { Value::Int(pc) }),
        (Value::Str("thread_id".into()),         Value::Int(current_thread_id() as i64)),
        (Value::Str("backtrace".into()),         Value::Str("<not available in milestone-1>".into())),
        (Value::Str("registers".into()),         Value::Dict(vec![])),
        (Value::Str("is_stack_overflow".into()), Value::Bool(is_so)),
    ])
}

// ─── Minimal JSON serialiser for Value ────────────────────────────────────

/// Serialize a `Value` to a JSON string.
/// Handles all scalar types, `Dict`, and `List`; everything else becomes a
/// quoted string.
pub fn value_to_json(v: &Value) -> String {
    match v {
        Value::Int(n)  => n.to_string(),
        Value::Float(f) => {
            if f.fract() == 0.0 { format!("{:.1}", f) } else { f.to_string() }
        }
        Value::Str(s) => {
            let escaped = s
                .replace('\\', "\\\\")
                .replace('"',  "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t");
            format!("\"{}\"", escaped)
        }
        Value::Bool(b)   => b.to_string(),
        Value::Null      => "null".into(),
        Value::Dict(kv)  => {
            let pairs: Vec<String> = kv.iter().map(|(k, v)| {
                let key_json = match k {
                    Value::Str(s) => {
                        let esc = s.replace('\\', "\\\\").replace('"', "\\\"");
                        format!("\"{}\"", esc)
                    }
                    other => value_to_json(other),
                };
                format!("{}: {}", key_json, value_to_json(v))
            }).collect();
            format!("{{{}}}", pairs.join(", "))
        }
        Value::List(items) => {
            let items: Vec<String> = items.iter().map(value_to_json).collect();
            format!("[{}]", items.join(", "))
        }
        other => format!("\"{}\"", other),
    }
}

// ─── Fault dispatch ────────────────────────────────────────────────────────

/// Dispatch a fault to the registered V2 handler (if any), then exit(1).
///
/// This function is called from OS signal/exception handlers. It runs on the
/// faulting thread and uses the thread-local interpreter pointer to call back
/// into V2 code. Since the process always terminates after this returns, we
/// accept the risks of calling non-AS-safe code from within the OS handler.
pub fn dispatch_fault(signal_name: &str) -> ! {
    // Prevent recursive fault dispatch (e.g., fault inside the fault handler).
    static DISPATCHING: AtomicBool = AtomicBool::new(false);
    if DISPATCHING.swap(true, Ordering::AcqRel) {
        // Recursive fault — abort immediately to avoid infinite recursion.
        eprintln!("[v2] recursive fault in fault handler — aborting");
        std::process::abort();
    }

    let fault_info = make_fault_info_dict(signal_name);

    // Try to invoke the registered V2 fault handler on this thread.
    INTERP_PTR.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            // Safety: the interpreter is valid on this thread; we are the
            // only code running right now (signal handler context).
            let interp = unsafe { &mut *ptr };
            if let Some(handler) = interp.fault_handlers.get(signal_name).cloned() {
                let args = vec![(None, fault_info.clone())];
                let _ = interp.call_value(&handler, &args);
            } else {
                // No handler registered — print a default message.
                eprintln!("[v2] Unhandled hardware fault: {}", signal_name);
                let addr = FAULT_ADDR.load(Ordering::Acquire);
                if addr != 0 {
                    eprintln!("[v2]   at address: {:#018x}", addr as u64);
                }
            }
        }
    });

    std::process::exit(1);
}

// ─── OS handler installation ───────────────────────────────────────────────

/// Install an OS-level handler for the given V2 fault signal name.
///
/// Safe to call multiple times for the same signal — subsequent calls are
/// no-ops (the handler is already installed).
///
/// # Errors
/// Returns an error string if the signal name is not a supported hardware
/// fault signal, or if the OS call fails.
pub unsafe fn install_os_fault_handler(signal_name: &str) -> Result<(), String> {
    if !FAULT_SIGNAL_NAMES.contains(&signal_name) {
        return Err(format!(
            "'{}' is not a hardware fault signal; supported: {:?}",
            signal_name, FAULT_SIGNAL_NAMES
        ));
    }

    #[cfg(unix)]
    { install_posix_handler(signal_name) }

    #[cfg(windows)]
    { install_windows_veh() }

    #[cfg(not(any(unix, windows)))]
    { Err(format!("platform does not support fault handler installation for '{}'", signal_name)) }
}

// ─── POSIX implementation ──────────────────────────────────────────────────

#[cfg(unix)]
unsafe fn install_posix_handler(signal_name: &str) -> Result<(), String> {
    let signum = fault_name_to_signum(signal_name)
        .ok_or_else(|| format!("unsupported POSIX signal '{}'", signal_name))?;

    let mut sa: libc::sigaction = std::mem::zeroed();
    sa.sa_sigaction = posix_fault_handler as libc::sighandler_t;
    libc::sigemptyset(&mut sa.sa_mask);
    sa.sa_flags = libc::SA_SIGINFO;

    if libc::sigaction(signum, &sa, std::ptr::null_mut()) != 0 {
        return Err(format!("sigaction() failed for signal '{}' (errno: {})",
                           signal_name, *libc::__errno_location()));
    }
    Ok(())
}

#[cfg(unix)]
extern "C" fn posix_fault_handler(
    signum: libc::c_int,
    siginfo: *mut libc::siginfo_t,
    _context: *mut libc::c_void,
) {
    // Write fault context using only async-signal-safe atomic stores.
    FAULT_SIGNUM.store(signum, Ordering::Release);

    let addr = if siginfo.is_null() { 0i64 } else {
        unsafe { (*siginfo).si_addr() as i64 }
    };
    FAULT_ADDR.store(addr, Ordering::Release);
    FAULT_PC.store(0, Ordering::Release);
    // Treat zero-address SIGSEGV as likely stack overflow.
    FAULT_IS_STACK_OVERFLOW.store(signum == libc::SIGSEGV && addr == 0, Ordering::Release);
    FAULT_PENDING.store(true, Ordering::Release);

    let name = signum_to_fault_name(signum).unwrap_or("SIGSEGV");
    dispatch_fault(name);
}

// ─── Windows VEH implementation ────────────────────────────────────────────

#[cfg(windows)]
#[allow(non_snake_case)]
mod win_types {
    // Windows exception codes we care about.
    pub const EXCEPTION_ACCESS_VIOLATION:   u32 = 0xC000_0005;
    pub const EXCEPTION_STACK_OVERFLOW:     u32 = 0xC000_00FD;
    pub const EXCEPTION_INT_DIVIDE_BY_ZERO: u32 = 0xC000_0094;
    pub const EXCEPTION_FLT_DIVIDE_BY_ZERO: u32 = 0xC000_008E;
    pub const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xC000_001D;
    pub const EXCEPTION_PRIV_INSTRUCTION:   u32 = 0xC000_0096;

    // VEH return values.
    pub const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

    #[repr(C)]
    pub struct ExceptionRecord {
        pub ExceptionCode:    u32,
        pub ExceptionFlags:   u32,
        pub ExceptionRecord:  *mut ExceptionRecord,
        pub ExceptionAddress: *mut std::ffi::c_void,
        pub NumberParameters: u32,
        pub ExceptionInformation: [usize; 15],
    }

    #[repr(C)]
    pub struct ExceptionPointers {
        pub ExceptionRecord: *mut ExceptionRecord,
        pub ContextRecord:   *mut std::ffi::c_void,
    }
}

#[cfg(windows)]
extern "system" {
    fn AddVectoredExceptionHandler(
        First:   u32,
        Handler: unsafe extern "system" fn(*mut win_types::ExceptionPointers) -> i32,
    ) -> *mut std::ffi::c_void;
}

#[cfg(windows)]
unsafe extern "system" fn windows_veh_handler(
    exc: *mut win_types::ExceptionPointers,
) -> i32 {
    use win_types::*;

    if exc.is_null() { return EXCEPTION_CONTINUE_SEARCH; }
    let record = (*exc).ExceptionRecord;
    if record.is_null() { return EXCEPTION_CONTINUE_SEARCH; }

    let code = (*record).ExceptionCode;
    let addr = (*record).ExceptionAddress as i64;

    let (fault_name, is_so) = match code {
        EXCEPTION_ACCESS_VIOLATION                                  => ("SIGSEGV", false),
        EXCEPTION_STACK_OVERFLOW                                    => ("SIGSEGV", true),
        EXCEPTION_INT_DIVIDE_BY_ZERO | EXCEPTION_FLT_DIVIDE_BY_ZERO => ("SIGFPE",  false),
        EXCEPTION_ILLEGAL_INSTRUCTION | EXCEPTION_PRIV_INSTRUCTION  => ("SIGBUS",  false),
        // Not a fault we handle — let other handlers try.
        _ => return EXCEPTION_CONTINUE_SEARCH,
    };

    FAULT_ADDR.store(addr, Ordering::Release);
    FAULT_PC.store(0, Ordering::Release);
    FAULT_IS_STACK_OVERFLOW.store(is_so, Ordering::Release);
    FAULT_PENDING.store(true, Ordering::Release);

    dispatch_fault(fault_name);

    // `dispatch_fault` calls `std::process::exit` — this is unreachable.
    #[allow(unreachable_code)]
    EXCEPTION_CONTINUE_SEARCH
}

#[cfg(windows)]
unsafe fn install_windows_veh() -> Result<(), String> {
    static VEH_INSTALLED: AtomicBool = AtomicBool::new(false);
    if VEH_INSTALLED.swap(true, Ordering::AcqRel) {
        return Ok(()); // Already installed — no-op.
    }

    let handle = AddVectoredExceptionHandler(1, windows_veh_handler);
    if handle.is_null() {
        return Err("AddVectoredExceptionHandler() failed".into());
    }
    Ok(())
}
