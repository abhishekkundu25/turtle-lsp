//! turtle-lsp in the browser: the engine of the RDF Toolbox (web/app).
//!
//! The page sends a JSON request, `{"op": ..., "files": [{"name", "text"}]}`,
//! and reads back `{"ok": ...}` or `{"error": "..."}`. The interface is four
//! plain functions over the module's memory (no generated glue):
//! `tl_alloc(len)` reserves room for the request, `tl_call(ptr, len)` runs it
//! (taking ownership of that room) and returns the answer's length, and
//! `tl_result()` points at the answer, valid until the next call.
//! `tl_free(ptr, len)` gives back room reserved but not passed to `tl_call`.
//!
//! The page supplies one import, `env.random_fill(ptr, len)`, which fills
//! memory with random bytes (crypto.getRandomValues).

mod api;

use std::cell::RefCell;

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[unsafe(no_mangle)]
pub extern "C" fn tl_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// # Safety
/// `ptr` and `len` come from one `tl_alloc` not yet freed or passed on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_free(ptr: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
}

/// # Safety
/// `ptr` and `len` come from one `tl_alloc`, filled with `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_call(ptr: *mut u8, len: usize) -> usize {
    let request = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let answer = match std::str::from_utf8(&request) {
        Ok(text) => api::handle(text),
        Err(_) => api::error("the request is not UTF-8"),
    };
    drop(request);
    RESULT.with(|r| {
        let mut r = r.borrow_mut();
        r.clear();
        r.shrink_to(answer.len().max(1 << 16));
        r.extend_from_slice(answer.as_bytes());
        r.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn tl_result() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn random_fill(ptr: *mut u8, len: usize);
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    unsafe { random_fill(dest, len) };
    Ok(())
}
