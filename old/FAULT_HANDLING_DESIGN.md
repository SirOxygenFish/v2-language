# Fault Handling Design For V2

This document describes a realistic implementation plan for hardware fault handling in V2 and splits the work by shared runtime behavior, POSIX-specific work, and Windows-specific work.

## Scope

Target APIs:

- `signal.on_fault(name, handler)`
- `signal.set_recovery_point()`
- `signal.recover()`
- `signal.dump_core(path)`
- `signal.dump_json(path)`

Target signals / faults:

- POSIX: `SIGSEGV`, `SIGBUS`, `SIGFPE`, `SIGABRT`
- Windows: access violation, stack overflow, illegal instruction, divide-by-zero, explicit abort mapped into the V2 fault model

Non-goals for the first implementation:

- General “keep running after arbitrary memory corruption” support
- Guaranteeing destructors / defer blocks run when recovering from a hardware fault
- Making fault recovery safe in multithreaded programs

## Shared Runtime Model

### Runtime State

Add a process-global fault runtime with:

- Registered fault handlers keyed by signal kind
- Thread-local current recovery point
- Reentrancy guard to prevent nested fault handler recursion
- Thread-local active-fault snapshot

### V2 Surface Model

- `signal.on_fault(name, handler)` registers one handler per signal per process.
- The handler receives a `FaultInfo` object with normalized fields: `signal`, `address`, `pc`, `thread_id`, `backtrace`, `registers`, `is_stack_overflow`.
- If the handler returns normally, the runtime terminates the process after optional cleanup.
- `signal.recover()` is only legal while a handler is active and only if a recovery point exists on the same thread.

### Safety Contract

- Fault APIs require `unsafe` at the language level.
- Recovery is single-thread scoped.
- Recovery invalidates stack frames between the fault site and the recovery point.
- No promise is made that heap state is consistent after recovery.

## POSIX Plan

### Feasible Mechanism

- Install handlers with `sigaction` and `SA_SIGINFO`.
- Extract fault metadata from `siginfo_t` and `ucontext_t`.
- Use an alternate signal stack via `sigaltstack` for stack-overflow survivability.
- Use thread-local `sigjmp_buf` storage for recovery points.

### POSIX Fault Mapping

- `SIGSEGV` -> invalid memory access / access violation
- `SIGBUS` -> alignment / physical address fault
- `SIGFPE` -> arithmetic trap
- `SIGABRT` -> explicit abort/assertion termination

### POSIX Recovery Flow

1. Fault arrives on a signal handler.
2. Handler snapshots registers, PC, address, and thread ID.
3. Runtime resolves symbols for the current backtrace.
4. V2 handler is invoked in a restricted fault context.
5. If handler calls `signal.recover()`, perform `siglongjmp` to the most recent same-thread recovery point.
6. Otherwise terminate with a non-zero exit code.

### POSIX Risks

- Most runtime operations are not async-signal-safe.
- Allocating memory, formatting strings, or locking inside the raw signal handler is dangerous.
- The implementation should do only minimal capture inside the signal handler and defer richer formatting until a safer post-capture path when possible.

## Windows Plan

### Feasible Mechanism

- Use vectored exception handling or structured exception handling as the low-level trap surface.
- Capture data from `EXCEPTION_POINTERS`, `EXCEPTION_RECORD`, and `CONTEXT`.
- Use `MiniDumpWriteDump` for native crash dump generation.
- Model recovery points with thread-local exception context bookkeeping.

### Windows Fault Mapping

- `EXCEPTION_ACCESS_VIOLATION` -> `SIGSEGV` equivalent
- `EXCEPTION_STACK_OVERFLOW` -> stack overflow with `is_stack_overflow = true`
- `EXCEPTION_INT_DIVIDE_BY_ZERO` / `EXCEPTION_FLT_DIVIDE_BY_ZERO` -> `SIGFPE` equivalent
- `RaiseAbort` / CRT abort path -> `SIGABRT` equivalent

### Windows Recovery Flow

1. Exception filter receives `EXCEPTION_POINTERS`.
2. Runtime normalizes the exception into `FaultInfo`.
3. Registered V2 fault handler runs on the same thread.
4. If recovery is requested and a valid recovery point exists, restore execution via the saved continuation model.
5. Otherwise emit dump/report and terminate.

### Windows Risks

- SEH and Rust unwinding rules do not mix freely.
- Stack-overflow handling requires extra care because the normal stack may be exhausted.
- Recovery should be limited to interpreter mode first; native codegen recovery should remain disabled until proven stable.

## Recommended Delivery Order

1. Implement registration, normalized `FaultInfo`, and termination-only fault callbacks.
2. Add crash dump generation (`dump_json` first, native minidump/core later).
3. Add same-thread recovery points for interpreter mode only.
4. Gate native-code recovery behind a separate experimental flag.
5. Add conformance tests for registration, metadata capture, and termination behavior.

## What Is Reasonable To Ship First

Reasonable first milestone:

- Register handlers
- Capture fault metadata
- Invoke V2 callback
- Write JSON crash reports
- Terminate reliably

Not reasonable as an initial guarantee:

- Continuing safely after arbitrary segmentation faults
- Recovering across threads
- Running full user cleanup logic from a corrupted process state
