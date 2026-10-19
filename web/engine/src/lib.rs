//! turtle-lsp in the browser: the engine of the RDF Toolbox (web/app).
//!
//! The page sends a JSON request, `{"op": ..., "files": [{"name", "text"}]}`,
//! and reads back `{"ok": ...}` or `{"error": "..."}`. For `fix_all` a file
//! may also say `"context": true`: it informs the fixes but is not fixed or
//! returned. The interface is four
//! plain functions over the module's memory (no generated glue):
//! `tl_alloc(len)` reserves room for the request, `tl_call(ptr, len)` runs it
//! (taking ownership of that room) and returns the answer's length, and
//! `tl_result()` points at the answer, valid until the next call.
//! `tl_free(ptr, len)` gives back room reserved but not passed to `tl_call`.
//!
//! The page supplies three imports: `env.random_fill(ptr, len)`, which fills
//! memory with random bytes (crypto.getRandomValues), `env.now_ms()`, the
//! time in milliseconds since 1970 (Date.now()), as the module has no clock
//! of its own, and `env.report_panic(ptr, len)`, which receives a panic's
//! message (UTF-8) just before the module aborts.

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
    #[cfg(target_arch = "wasm32")]
    std::panic::set_hook(Box::new(|info| {
        let message = info.to_string();
        unsafe { report_panic(message.as_ptr(), message.len()) };
    }));
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
    fn report_panic(ptr: *const u8, len: usize);
    fn now_ms() -> f64;
}

/// The current time for SPARQL's `NOW()`, from the page.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
fn custom_ox_now() -> oxsdatatypes::Duration {
    let seconds = oxsdatatypes::Double::from(unsafe { now_ms() } / 1000.0);
    oxsdatatypes::DayTimeDuration::new(oxsdatatypes::Decimal::try_from(seconds).unwrap_or_default())
        .into()
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
