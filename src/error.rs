//! openDAQ error codes mapped to a Rust error type.
//!
//! Every C API call returns a `daqErrCode`; the high bit marks failure.  On
//! failure the descriptive message for the calling thread is fetched through
//! `daqGetErrorInfoMessage` before the next failing call overwrites it.

use std::ffi::c_void;

use crate::sys;

/// Result alias used by every fallible openDAQ call.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A failed openDAQ call: the raw status code, the C function that produced
/// it, and the descriptive message from openDAQ's error info, when available.
#[derive(Debug, Clone, thiserror::Error)]
#[error("openDAQ call {operation} failed with {} (0x{code:08X}){}",
        self.name().unwrap_or("an unknown error"),
        self.message.as_deref().map(|m| format!(": {m}")).unwrap_or_default())]
pub struct Error {
    code: u32,
    operation: &'static str,
    message: Option<String>,
}

impl Error {
    pub(crate) fn new(code: u32, operation: &'static str, message: Option<String>) -> Self {
        Error {
            code,
            operation,
            message,
        }
    }

    /// The raw 32-bit openDAQ status code.
    pub fn code(&self) -> u32 {
        self.code
    }

    /// The upstream symbolic name of the status code (e.g.
    /// `"OPENDAQ_ERR_NOTFOUND"`), when it is a known code.
    pub fn name(&self) -> Option<&'static str> {
        sys::error_code_name(self.code)
    }

    /// The C function whose call failed.
    pub fn operation(&self) -> &'static str {
        self.operation
    }

    /// The descriptive error message openDAQ attached to the failure, if any.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// True when the status code equals the named `OPENDAQ_ERR_*` code.
    pub fn is(&self, code: u32) -> bool {
        self.code == code
    }

    /// True when a reader `read` failed because the reader was invalidated by
    /// a descriptor change it cannot convert; recover by building a
    /// `from_existing` reader with read types matching the new descriptor.
    pub fn is_reader_invalidated(&self) -> bool {
        self.operation == crate::readers::READER_INVALIDATED_OPERATION
    }
}

pub(crate) fn failure_code(code: u32) -> bool {
    code & 0x8000_0000 != 0
}

/// Retrieve the human-readable message for the last error on this thread,
/// using raw calls only (no [`check`] recursion).
fn last_error_message() -> Option<String> {
    let api = sys::api();
    let mut message: *mut sys::daqString = std::ptr::null_mut();
    // Returns the *stored* status code of the thread's last error: a message
    // is present exactly when that code has the failure bit set.
    let stored = unsafe { (api.daqGetErrorInfoMessage)(&mut message) };
    if !failure_code(stored) || message.is_null() {
        return None;
    }
    let mut chars: *const std::ffi::c_char = std::ptr::null();
    let err = unsafe { (api.daqString_getCharPtr)(message, &mut chars) };
    let result = if failure_code(err) || chars.is_null() {
        None
    } else {
        Some(
            unsafe { std::ffi::CStr::from_ptr(chars) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    unsafe { (api.daqBaseObject_releaseRef)(message as *mut c_void) };
    result
}

/// Map a returned status code to `Ok(())` or a populated [`Error`].
pub(crate) fn check(code: u32, operation: &'static str) -> Result<()> {
    if failure_code(code) {
        Err(Error::new(code, operation, last_error_message()))
    } else {
        Ok(())
    }
}

/// Clear any error information stored for the calling thread.
pub fn clear_error_info() {
    unsafe { (sys::api().daqClearErrorInfo)() }
}
