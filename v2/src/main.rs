#![allow(dead_code, unused_variables, unreachable_patterns, clippy::clone_double_ref)]
mod token;
mod lexer;
mod ast;
mod parser;
mod value;
mod regex_engine;
mod environment;
mod fault;
mod interpreter;
mod bytecode;
mod compiler;
mod vm;
mod serialize;
mod native;
mod hashing;
mod bigint;
mod decimal;
mod pkg;

use std::env;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use interpreter::{Interpreter, RuntimeSafetyOptions};
use lexer::Lexer;
use parser::Parser;

fn run(interp: &mut Interpreter, source: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse()?;
    let result = interp.exec(&program)?;
    match &result {
        value::Value::Null => {}
        _ => println!("{}", result),
    }
    Ok(())
}

fn run_file(path: &str, test_mode: bool, safety: RuntimeSafetyOptions) -> Result<(), String> {
    let source = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read file '{}': {}", path, e))?;
    let mut interp = Interpreter::with_safety(safety);
    interp.test_mode = test_mode;
    run(&mut interp, &source)
}

fn compile_file(path: &str, out_path: Option<&str>, disasm: bool, run_after: bool, safety: RuntimeSafetyOptions) -> Result<(), String> {
    let source = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read file '{}': {}", path, e))?;
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse()?;

    let output = compiler::compile_program(&program)?;

    if disasm {
        println!("=== Disassembly ===");
        output.main.chunk.disassemble("<main>");
        for (name, func) in &output.compiled_funcs {
            func.chunk.disassemble(name);
        }
        return Ok(());
    }

    // Write compiled bytecode to file
    let dest = match out_path {
        Some(o) => o.to_string(),
        None => {
            let p = Path::new(path);
            p.with_extension("v2c").to_string_lossy().to_string()
        }
    };
    let bytes = serialize::serialize(&output);
    fs::write(&dest, &bytes)
        .map_err(|e| format!("Cannot write bytecode file '{}': {}", dest, e))?;
    println!("Compiled {} -> {} ({} bytes)", path, dest, bytes.len());

    if run_after {
        run_bytecode(output, safety)?;
    }
    Ok(())
}

fn run_bytecode(output: compiler::CompileOutput, safety: RuntimeSafetyOptions) -> Result<(), String> {
    let mut virtual_machine = vm::VM::with_safety(safety);
    let result = virtual_machine.run(output)?;
    match &result {
        value::Value::Null => {}
        _ => println!("{}", result),
    }
    Ok(())
}

fn load_and_run_bytecode(path: &str, safety: RuntimeSafetyOptions) -> Result<(), String> {
    let data = fs::read(path)
        .map_err(|e| format!("Cannot read bytecode file '{}': {}", path, e))?;
    let output = serialize::deserialize(&data)?;
    run_bytecode(output, safety)
}

/// 8-byte magic trailer appended after bytecode in self-contained exe
const EXE_TRAILER_MAGIC: &[u8; 8] = b"V2BUNDLE";

/// Check if the current executable has embedded bytecode appended.
fn try_run_embedded() -> Option<Result<(), String>> {
    let exe_path = env::current_exe().ok()?;
    let mut file = fs::File::open(&exe_path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len < 16 {
        return None;
    }
    // Read the last 16 bytes: 8 bytes trailer magic + 8 bytes bytecode length (little-endian u64)
    file.seek(SeekFrom::End(-16)).ok()?;
    let mut footer = [0u8; 16];
    file.read_exact(&mut footer).ok()?;
    let magic = &footer[8..16];
    if magic != EXE_TRAILER_MAGIC {
        return None;
    }
    let bc_len = u64::from_le_bytes([
        footer[0], footer[1], footer[2], footer[3],
        footer[4], footer[5], footer[6], footer[7],
    ]) as usize;
    // Read the bytecode
    let bc_start = file_len as usize - 16 - bc_len;
    file.seek(SeekFrom::Start(bc_start as u64)).ok()?;
    let mut bc_data = vec![0u8; bc_len];
    file.read_exact(&mut bc_data).ok()?;
    let output = match serialize::deserialize(&bc_data) {
        Ok(o) => o,
        Err(e) => return Some(Err(e)),
    };
    Some(run_bytecode(output, RuntimeSafetyOptions::default()))
}

/// Bundle bytecode into a self-contained executable.
fn bundle_exe(source_path: &str, out_path: Option<&str>) -> Result<(), String> {
    // Compile
    let source = fs::read_to_string(source_path)
        .map_err(|e| format!("Cannot read file '{}': {}", source_path, e))?;
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse()?;
    let output = compiler::compile_program(&program)?;
    let bytecode = serialize::serialize(&output);

    // Read the v2 runtime exe
    let runtime_exe = env::current_exe()
        .map_err(|e| format!("Cannot locate runtime executable: {}", e))?;
    let mut exe_data = fs::read(&runtime_exe)
        .map_err(|e| format!("Cannot read runtime executable: {}", e))?;

    // Append: bytecode + bytecode_len(u64 LE) + trailer magic
    let bc_len = bytecode.len() as u64;
    exe_data.extend_from_slice(&bytecode);
    exe_data.extend_from_slice(&bc_len.to_le_bytes());
    exe_data.extend_from_slice(EXE_TRAILER_MAGIC);

    // Determine output path
    let dest = match out_path {
        Some(o) => o.to_string(),
        None => {
            let p = Path::new(source_path);
            let stem = p.file_stem().unwrap_or_default().to_string_lossy();
            if cfg!(windows) {
                format!("{}.exe", stem)
            } else {
                stem.to_string()
            }
        }
    };

    fs::write(&dest, &exe_data)
        .map_err(|e| format!("Cannot write executable '{}': {}", dest, e))?;
    println!("Bundled {} -> {} ({:.1} KB)", source_path, dest, exe_data.len() as f64 / 1024.0);
    Ok(())
}

/// Compile to native machine code.
fn compile_native(source_path: &str, out_path: Option<&str>) -> Result<(), String> {
    let source = fs::read_to_string(source_path)
        .map_err(|e| format!("Cannot read file '{}': {}", source_path, e))?;
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse()?;

    let dest = match out_path {
        Some(o) => o.to_string(),
        None => {
            let p = Path::new(source_path);
            let stem = p.file_stem().unwrap_or_default().to_string_lossy();
            if cfg!(windows) {
                format!("{}.exe", stem)
            } else {
                stem.to_string()
            }
        }
    };

    native::compile_to_native(&program, &dest)?;
    let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    println!("Compiled {} -> {} ({:.1} KB, native)", source_path, dest, size as f64 / 1024.0);
    Ok(())
}

fn run_file_debug(path: &str, safety: RuntimeSafetyOptions) -> Result<(), String> {
    let source = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read file '{}': {}", path, e))?;
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize()?;

    println!("=== Tokens ===");
    for tok in &tokens {
        println!("  {:?}", tok);
    }

    let mut parser = Parser::new(tokens);
    let program = parser.parse()?;

    println!("\n=== AST ===");
    for stmt in &program.stmts {
        println!("  {:?}", stmt);
    }

    println!("\n=== Execution ===");
    let mut interp = Interpreter::with_safety(safety);
    let result = interp.exec(&program)?;
    match &result {
        value::Value::Null => {}
        _ => println!("{}", result),
    }
    Ok(())
}

fn find_doc_file(name: &str) -> Option<String> {
    // Look next to the executable (handles target/debug/ layout)
    if let Ok(exe) = env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        // Walk up from exe directory (e.g., v2/target/debug/ -> v2/ -> project root)
        for _ in 0..4 {
            if let Some(ref d) = dir {
                let path = d.join(name);
                if path.exists() {
                    return Some(path.to_string_lossy().to_string());
                }
                dir = d.parent().map(|p| p.to_path_buf());
            }
        }
    }
    // Look in current directory
    let path = Path::new(name);
    if path.exists() {
        return Some(path.to_string_lossy().to_string());
    }
    None
}

fn generate_docs_html(title: &str, markdown_content: &str) -> String {
    let escaped = markdown_content
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let template = include_str!("docs_template.html");
    template
        .replace("___V2_DOC_TITLE___", title)
        .replace("___V2_DOC_CONTENT___", &escaped)
}

fn open_in_browser(path: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", path])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(path)
            .spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(path)
            .spawn();
    }
}

fn open_docs_in_browser(doc_name: &str) {
    let path = match find_doc_file(doc_name) {
        Some(p) => p,
        None => {
            eprintln!("Cannot find '{}'. Looked next to the v2 executable and in the current directory.", doc_name);
            std::process::exit(1);
        }
    };

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Cannot read '{}': {}", path, e);
            std::process::exit(1);
        }
    };

    let display_title = if doc_name == "DOCS.md" {
        "Language Documentation"
    } else if doc_name == "INTERNALS.md" {
        "Internals Documentation"
    } else if doc_name == "PACKAGES.md" {
        "Packages & Package Manager"
    } else {
        doc_name
    };

    let html = generate_docs_html(display_title, &content);

    let tmp_dir = env::temp_dir();
    let html_filename = format!("v2_{}.html", doc_name.replace('.', "_").to_lowercase());
    let html_path = tmp_dir.join(&html_filename);

    if let Err(e) = fs::write(&html_path, &html) {
        eprintln!("Cannot write temporary HTML file: {}", e);
        std::process::exit(1);
    }

    let url = html_path.to_string_lossy().replace('\\', "/");
    let url = if url.starts_with('/') {
        format!("file://{}", url)
    } else {
        format!("file:///{}", url)
    };

    println!("Opening {} in your browser...", doc_name);
    open_in_browser(&url);
}

fn print_help() {
    println!("V2 Language v0.1.0\n");
    println!("Usage: v2 [options] [file.v2]");
    println!("       v2 <command> [args]\n");
    println!("Package & project commands:");
    println!("  init [--lib]              Create a v2.toml (and src scaffold)");
    println!("  add <name> [--path P|--git URL]   Add and install a dependency (name@ver to pin)");
    println!("  remove <name>            Remove a dependency");
    println!("  install [--frozen]       Install all dependencies into v2_modules/");
    println!("  update                   Re-fetch dependencies");
    println!("  list                     List declared/installed dependencies");
    println!("  run | test | build       Run/test/build the project entry from v2.toml");
    println!("  (Set $V2_REGISTRY to a git index base URL to resolve `name = \"version\"` deps.)\n");
    println!("Options:");
    println!("  (none)                    Start interactive REPL");
    println!("  file.v2                   Run a V2 program");
    println!("  -c, --compile             Compile to bytecode");
    println!("  -r, --run                 Run bytecode after compilation");
    println!("  -i, --interpret           Force interpreter mode");
    println!("  -d, --debug               Debug mode (show tokens, AST, then execute)");
    println!("  -O, --optimize            Enable bytecode optimizations (default)");
    println!("  -O0                       Disable optimizations");
    println!("  -S, --disasm              Show bytecode disassembly");
    println!("  -o <file>                 Custom output filename");
    println!("  -V, --verbose             Verbose diagnostic output");
    println!("  -v, --version             Print version");
    println!("  -h, --help                Print this help");
    println!("  -D, --docs                Open language documentation in your browser");
    println!("  -I, --internals           Open internals documentation in your browser");
    println!("  -P, --packages            Open the packages/package-manager guide in your browser");
    println!("  --target <t>              Compile target: native, wasm, bytecode, exe");
    println!("  --arch <a>                Target architecture: x86_64, arm64, wasm32");
    println!("  --os <os>                 Target OS: linux, windows, macos, android, ios, none");
    println!("  --wasm-cap <caps>         WASM host capabilities (comma-separated)");
    println!("  --incremental             Enable incremental compilation");
    println!("  --cache-dir <dir>         Incremental compilation cache directory");
    println!("  --async-workers <n>       Async scheduler worker threads (default: 1)");
    println!("  --build-script <file>     Override build script path");
    println!("  --embed-engines           Force managed embedded runtime resolution");
    println!("  --strict-unsafe           Reject unchecked pointer operations");
    println!("  --sanitizer <s>           Enable sanitizer: address, ub, thread, leak");
    println!("  --lsp                     Start Language Server on stdio");
    println!("  --lsp-port <n>            Start Language Server on TCP port");
    println!("  --step-debug              Launch step debugger");
    println!("  --break <file:line>       Set breakpoint for --step-debug");
    println!("  --test                    Run all test blocks");
    println!("  --test --tag <t>          Run tests with a given tag");
    println!("  --test --skip-tag <t>     Skip tests with a given tag");
    println!("  --test --update-snapshots Regenerate snapshot baselines");
    println!("  --bench                   Run all bench blocks");
    println!("  --overflow <mode>         Integer overflow: wrap, saturate, panic (default)");
    println!("  --no-tco                  Disable tail-call optimization");
    println!("  --warn <w>                Enable specific warning category");
    println!("  --no-warn <w>             Suppress specific warning category");
    println!("  --doc                     Generate documentation from doc comments");
    println!("  --doc --out <dir>         Custom output for generated docs");
    println!("  --doc --format <f>        Doc format: html (default), markdown");
    println!("  --profile                 Profile program and print summary");
    println!("  --profile --flame         Generate flamegraph SVG");
    println!("  --coverage                Run with code coverage");
    println!("  --coverage --out <dir>    Coverage report directory");
    println!("  --coverage --format <f>   Coverage format: text, html, lcov, json");
    println!("  --dump-cfg <file>         Export control-flow graph as .dot file");
}

fn repl() {
    println!("V2 Language v0.1.0");
    println!("Type 'exit' or Ctrl+C to quit.\n");
    let mut interp = Interpreter::new();
    loop {
        print!("v2> ");
        io::stdout().flush().ok();
        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed == "exit" || trimmed == "quit" {
                    break;
                }
                if trimmed.is_empty() {
                    continue;
                }
                match run(&mut interp, trimmed) {
                    Ok(()) => {}
                    Err(e) => eprintln!("{}", e),
                }
            }
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    // Generous native stack: the tree-walking interpreter recurses on the Rust
    // stack for deep user recursion and macro expansion, so give it room.
    // (Reserved, not committed — the interpreter's recursion_limit keeps
    // real usage well inside this.)
    let builder = std::thread::Builder::new().stack_size(512 * 1024 * 1024);
    let handler = builder.spawn(move || {
        // Check for embedded bytecode first (self-contained exe mode)
        if args.len() == 1 {
            if let Some(result) = try_run_embedded() {
                if let Err(e) = result {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
                return;
            }
        }

        // Package-manager subcommands (`v2 init/add/install/...`) run before the
        // flag parser, since they take their own positional arguments.
        if let Some(sub) = args.get(1) {
            if pkg::is_subcommand(sub) {
                if let Err(e) = pkg::dispatch(&args) {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
                return;
            }
        }

        // Project commands (`v2 run/test/build/check`) resolve the entry file
        // from v2.toml, then fall through to normal execution.
        let mut args = args;
        if let Some(sub) = args.get(1).cloned() {
            if matches!(sub.as_str(), "run" | "test" | "build" | "check") {
                match pkg::project_entry() {
                    Some(entry) => {
                        // Replace the subcommand with the resolved entry file,
                        // adding --test for the test command.
                        args.remove(1);
                        if sub == "test" {
                            args.insert(1, "--test".to_string());
                        }
                        args.insert(if sub == "test" { 2 } else { 1 }, entry);
                    }
                    None => {
                        eprintln!("error: no {} with a [project].entry found for `v2 {}`", pkg::MANIFEST, sub);
                        std::process::exit(1);
                    }
                }
            }
        }

        // ── Parse all CLI flags ──────────────────────────────────────────
        let mut test_mode = false;
        let mut compile_mode = false;
        let mut run_mode = false;
        let mut disasm_mode = false;
        let mut debug_mode = false;
        let mut verbose_mode = false;
        let mut show_version = false;
        let mut show_help = false;
        let mut show_docs = false;
        let mut show_internals = false;
        let mut show_packages = false;
        let mut bench_mode = false;
        let mut doc_gen = false;
        let mut profile_mode = false;
        let mut profile_flame = false;
        let mut coverage_mode = false;
        let mut incremental = false;
        let mut step_debug = false;
        let mut lsp_mode = false;
        let mut strict_unsafe = false;
        let mut embed_engines = false;
        let mut no_tco = false;
        let mut test_update_snapshots = false;

        let mut out_path: Option<String> = None;
        let mut target: Option<String> = None;
        let mut arch: Option<String> = None;
        let mut target_os: Option<String> = None;
        let mut wasm_cap: Option<String> = None;
        let mut cache_dir: Option<String> = None;
        let mut async_workers: Option<String> = None;
        let mut build_script: Option<String> = None;
        let mut sanitizer: Option<String> = None;
        let mut lsp_port: Option<String> = None;
        let mut overflow_mode: Option<String> = None;
        let mut dump_cfg: Option<String> = None;
        let mut doc_out: Option<String> = None;
        let mut doc_format: Option<String> = None;
        let mut coverage_out: Option<String> = None;
        let mut coverage_format: Option<String> = None;
        let mut test_tag: Option<String> = None;
        let mut test_skip_tag: Option<String> = None;
        let mut breakpoints: Vec<String> = Vec::new();
        let mut warn_flags: Vec<String> = Vec::new();
        let mut no_warn_flags: Vec<String> = Vec::new();
        let mut file_args: Vec<String> = Vec::new();

        let mut i = 1;
        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                // Boolean flags
                "--test"              => test_mode = true,
                "--compile" | "-c"    => compile_mode = true,
                "--run" | "-r"        => run_mode = true,
                "--disasm" | "-S"     => disasm_mode = true,
                "--debug" | "-d"      => debug_mode = true,
                "--verbose" | "-V"    => verbose_mode = true,
                "--optimize" | "-O"   => {} // default, no-op
                "-O0"                 => {} // disable optimizations (accepted but no-op for interpreter)
                "--version" | "-v"    => show_version = true,
                "--help" | "-h"       => show_help = true,
                "--docs" | "-D"       => show_docs = true,
                "--internals" | "-I"  => show_internals = true,
                "--packages" | "-P"   => show_packages = true,
                "--interpret" | "-i"  => {} // default mode, no-op
                "--bench"             => bench_mode = true,
                "--doc"               => doc_gen = true,
                "--profile"           => profile_mode = true,
                "--flame"             => profile_flame = true,
                "--coverage"          => coverage_mode = true,
                "--incremental"       => incremental = true,
                "--step-debug"        => step_debug = true,
                "--lsp"               => lsp_mode = true,
                "--strict-unsafe"     => strict_unsafe = true,
                "--embed-engines"     => embed_engines = true,
                "--no-tco"            => no_tco = true,
                "--update-snapshots"  => test_update_snapshots = true,
                // Flags with a value argument
                "-o"               => { i += 1; out_path = args.get(i).cloned(); }
                "--target"         => { i += 1; target = args.get(i).cloned(); }
                "--arch"           => { i += 1; arch = args.get(i).cloned(); }
                "--os"             => { i += 1; target_os = args.get(i).cloned(); }
                "--wasm-cap"       => { i += 1; wasm_cap = args.get(i).cloned(); }
                "--cache-dir"      => { i += 1; cache_dir = args.get(i).cloned(); }
                "--async-workers"  => { i += 1; async_workers = args.get(i).cloned(); }
                "--build-script"   => { i += 1; build_script = args.get(i).cloned(); }
                "--sanitizer"      => { i += 1; sanitizer = args.get(i).cloned(); }
                "--lsp-port"       => { i += 1; lsp_port = args.get(i).cloned(); }
                "--overflow"       => { i += 1; overflow_mode = args.get(i).cloned(); }
                "--dump-cfg"       => { i += 1; dump_cfg = args.get(i).cloned(); }
                "--tag"            => { i += 1; test_tag = args.get(i).cloned(); }
                "--skip-tag"       => { i += 1; test_skip_tag = args.get(i).cloned(); }
                "--break"          => { i += 1; if let Some(b) = args.get(i) { breakpoints.push(b.clone()); } }
                "--warn"           => { i += 1; if let Some(w) = args.get(i) { warn_flags.push(w.clone()); } }
                "--no-warn"        => { i += 1; if let Some(w) = args.get(i) { no_warn_flags.push(w.clone()); } }
                "--out"            => {
                    i += 1;
                    let v = args.get(i).cloned();
                    if doc_gen { doc_out = v; }
                    else if coverage_mode { coverage_out = v; }
                }
                "--format"         => {
                    i += 1;
                    let v = args.get(i).cloned();
                    if doc_gen { doc_format = v; }
                    else if coverage_mode { coverage_format = v; }
                }
                // Positional / file arguments
                _ => {
                    if arg.starts_with('-') {
                        eprintln!("Unknown flag: {}", arg);
                        eprintln!("Run 'v2 --help' for usage information.");
                        std::process::exit(1);
                    }
                    file_args.push(arg.clone());
                }
            }
            i += 1;
        }

        let safety = RuntimeSafetyOptions {
            strict_unsafe,
            sanitizer: sanitizer.clone(),
        };

        // ── Handle immediate flags (no file needed) ─────────────────────
        if show_version {
            println!("V2 Language v0.1.0");
            return;
        }

        if show_help {
            print_help();
            return;
        }

        if show_docs {
            open_docs_in_browser("DOCS.md");
            return;
        }

        if show_internals {
            open_docs_in_browser("INTERNALS.md");
            return;
        }

        if show_packages {
            open_docs_in_browser("PACKAGES.md");
            return;
        }

        if lsp_mode {
            eprintln!("LSP server is not yet implemented.");
            std::process::exit(1);
        }

        // ── File-based operations ───────────────────────────────────────
        if let Some(path) = file_args.first() {
            if path.ends_with(".v2c") {
                if let Err(e) = load_and_run_bytecode(path, safety.clone()) {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            } else if debug_mode {
                if let Err(e) = run_file_debug(path, safety.clone()) {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            } else if compile_mode || disasm_mode {
                let result = match target.as_deref() {
                    Some("exe") => bundle_exe(path, out_path.as_deref()),
                    Some("native") => compile_native(path, out_path.as_deref()),
                    Some(t) => Err(format!("Unknown target '{}'. Supported: exe, native, bytecode, wasm", t)),
                    None => compile_file(path, out_path.as_deref(), disasm_mode, run_mode, safety.clone()),
                };
                if let Err(e) = result {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            } else {
                if let Err(e) = run_file(path, test_mode, safety.clone()) {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            }
        } else if doc_gen || bench_mode || profile_mode || coverage_mode || step_debug {
            // These flags require a file
            eprintln!("No input file specified. These flags require a .v2 file.");
            eprintln!("Run 'v2 --help' for usage information.");
            std::process::exit(1);
        } else {
            repl();
        }
    }).unwrap();
    handler.join().unwrap();
}
