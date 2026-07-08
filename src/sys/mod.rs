//! Low-level FFI surface of the openDAQ flat C API.
//!
//! The C API (built as the `copendaq` shared library, plus the core libraries
//! it re-exports pieces of) is loaded dynamically at runtime, so this crate
//! compiles without any openDAQ SDK present.  The machine-generated part --
//! opaque interface types, enums, error codes, the [`Api`] function-pointer
//! table, and the callback trampolines -- is emitted by
//! `tools/generate_bindings.py` from the C headers.
//!
//! Everything here is `unsafe` plumbing; end users should stay on the safe
//! wrappers in the crate root.

#![allow(non_camel_case_types, non_snake_case, clippy::missing_safety_doc)]

// Generated code keeps the generator's formatting so regeneration diffs stay
// readable.
#[rustfmt::skip]
mod generated;

pub use generated::*;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::loader::{self, LoadError};

/// The 16-byte openDAQ interface GUID, passed by value across the C ABI.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct daqIntfID {
    pub Data1: u32,
    pub Data2: u16,
    pub Data3: u16,
    pub Data4: u64,
}

struct Loaded {
    api: &'static Api,
    directory: PathBuf,
}

static LOADED: OnceLock<Result<Loaded, LoadError>> = OnceLock::new();

fn load(directory_override: Option<&Path>) -> Result<Loaded, LoadError> {
    let directory = match directory_override {
        Some(dir) => dir.to_path_buf(),
        None => loader::native_library_directory()?,
    };
    let loaded = loader::load_from(&directory)?;
    let api = Api::resolve(&loaded.libraries)?;
    // The libraries stay loaded for the life of the process; openDAQ is not
    // designed to be unloaded, and the Api table borrows their code.
    std::mem::forget(loaded.libraries);
    Ok(Loaded {
        api: Box::leak(Box::new(api)),
        directory,
    })
}

/// Load the native libraries (locating or downloading them first) if they are
/// not loaded yet.  Every entry point calls this implicitly; call it directly
/// to surface load problems as a `Result` instead of a panic.
pub fn initialize() -> Result<&'static Api, LoadError> {
    match LOADED.get_or_init(|| load(None)) {
        Ok(loaded) => Ok(loaded.api),
        Err(e) => Err(e.clone()),
    }
}

/// Like [`initialize`], but load from an explicit directory.  Fails when the
/// libraries were already loaded from a different directory: they cannot be
/// reloaded within one process.
pub fn initialize_from(directory: &Path) -> Result<&'static Api, LoadError> {
    let result = LOADED.get_or_init(|| load(Some(directory)));
    match result {
        Ok(loaded) => {
            if loaded.directory != directory {
                return Err(LoadError::LibraryLoadFailed {
                    library: directory.to_path_buf(),
                    reason: format!(
                        "openDAQ native libraries are already loaded from {} and cannot be \
                         reloaded in the same process",
                        loaded.directory.display()
                    ),
                });
            }
            Ok(loaded.api)
        }
        Err(e) => Err(e.clone()),
    }
}

/// The resolved C API table.
///
/// # Panics
///
/// Panics with a descriptive message when the native libraries cannot be
/// located, downloaded, or loaded.  Call [`initialize`] first to handle that
/// case gracefully.
pub fn api() -> &'static Api {
    match initialize() {
        Ok(api) => api,
        Err(e) => panic!("failed to load the openDAQ native libraries: {e}"),
    }
}

/// The directory the native libraries were (or would be) loaded from.
pub fn native_library_directory() -> Result<PathBuf, LoadError> {
    match LOADED.get_or_init(|| load(None)) {
        Ok(loaded) => Ok(loaded.directory.clone()),
        Err(e) => Err(e.clone()),
    }
}
