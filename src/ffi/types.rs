//! `CType`, signatures and platform sizes (INTERNALS 16.4).

use std::rc::Rc;

/// The V2-visible meaning of a C type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CType {
    Void,
    /// C `_Bool` / C++ `bool` / Rust `bool`: 1 byte.
    Bool,
    Int { size: u8, signed: bool },
    F32,
    F64,
    /// `const char*`: a V2 str goes in as a temporary copy; a result is copied, never freed.
    Str,
    /// Compiled exports only: a result copied, then freed by the library (15.4).
    #[allow(dead_code)] // produced by compiled-block signatures (roadmap 2.3)
    OwnedStr,
    /// Any other pointer: an opaque native pointer.
    Ptr,
}

#[allow(dead_code)] // read by the FFI call path (roadmap 2.2)
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub ty: CType,
    pub spelling: Rc<str>,
    pub name: Option<Rc<str>>,
    /// Only for compiled-block string parameters (`std::string`, `&str`, `String`, 15.3).
    pub non_null: bool,
}

/// A function signature read from C declarations.
#[allow(dead_code)] // read by the FFI call path (roadmap 2.2)
#[derive(Clone, Debug, PartialEq)]
pub struct Sig {
    pub name: Rc<str>,
    pub ret: CType,
    pub ret_spelling: Rc<str>,
    pub params: Vec<Param>,
    /// Compiled exports with an error slot (15.4); never for `extern`.
    pub err_slot: bool,
}

/// `long` / `unsigned long`: 4 bytes on Windows (LLP64), 8 on Linux and macOS (LP64).
pub const LONG_SIZE: u8 = if cfg!(windows) { 4 } else { 8 };

/// Plain `char` is unsigned on aarch64 Linux and signed on every other supported target.
pub const CHAR_SIGNED: bool = !(cfg!(target_arch = "aarch64") && cfg!(target_os = "linux"));

/// Pointer-sized integers (`size_t`, `intptr_t`, …): V2 supports 64-bit targets only.
pub const PTR_SIZE: u8 = 8;
