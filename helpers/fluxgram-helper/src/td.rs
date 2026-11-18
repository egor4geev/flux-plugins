//! TDLib's JSON interface (`td/telegram/td_json_client.h`), linked in statically by build.rs.

use std::ffi::{CStr, CString, c_char, c_int};

unsafe extern "C" {
    fn td_create_client_id() -> c_int;
    fn td_send(client_id: c_int, request: *const c_char);
    fn td_receive(timeout: f64) -> *const c_char;
    fn td_execute(request: *const c_char) -> *const c_char;
}

/// A new TDLib client; it starts with the first request sent to it.
pub fn create_client() -> i32 {
    unsafe { td_create_client_id() }
}

/// Sends a request (a JSON object) to a client: TDLib answers through [`receive`]. Any thread.
pub fn send(client: i32, request: &str) {
    // serde_json escapes NUL in strings: a JSON text has none.
    let Ok(request) = CString::new(request) else {
        return;
    };
    unsafe { td_send(client, request.as_ptr()) }
}

/// The next answer or update of any client, waiting up to `timeout` seconds; 0 — only what's
/// ready. One thread only.
pub fn receive(timeout: f64) -> Option<String> {
    // TDLib's string lives until this thread's next call: copied at once.
    unsafe { owned(td_receive(timeout)) }
}

/// A synchronous request (`getTextEntities`, `setLogStream`…): TDLib's answer.
pub fn execute(request: &str) -> Option<String> {
    let request = CString::new(request).ok()?;
    unsafe { owned(td_execute(request.as_ptr())) }
}

unsafe fn owned(text: *const c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// libc++'s handler of a failed check in its code (TDLib's libraries call it). macOS's libc++
/// has it since 13.3 only, and TDLib's libraries are built for macOS 15: the program's own
/// handler — libc++ lets a program define it — keeps it running on macOS 12, Flux's minimum.
/// The format's arguments aren't read: the format names the check.
#[cfg(target_os = "macos")]
#[unsafe(export_name = "_ZNSt3__122__libcpp_verbose_abortEPKcz")]
extern "C" fn libcpp_verbose_abort(format: *const c_char) -> ! {
    if !format.is_null() {
        let format = unsafe { CStr::from_ptr(format) }.to_string_lossy();
        eprintln!("fluxgram-helper: libc++: {}", format.trim_end());
    }
    std::process::abort()
}
