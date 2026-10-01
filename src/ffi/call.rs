//! The libffi call path (INTERNALS 16.5-16.7).

#[cfg(test)]
mod tests {
    use libffi::middle::{arg, Cif, CodePtr, Type};

    extern "C" fn mixed(a: i64, b: f64, c: i32) -> f64 {
        a as f64 + b + c as f64
    }

    /// The vendored libffi builds and links on this toolchain, and a mixed int/float
    /// signature crosses the platform ABI correctly.
    #[test]
    fn libffi_links_and_calls() {
        let cif = Cif::new([Type::i64(), Type::f64(), Type::i32()], Type::f64());
        let (a, b, c) = (2i64, 0.5f64, 40i32);
        let r: f64 = unsafe { cif.call(CodePtr(mixed as *mut _), &[arg(&a), arg(&b), arg(&c)]) };
        assert_eq!(r, 42.5);
    }
}
