//! Safe Rust bindings for [openDAQ](https://opendaq.com/), the open-source
//! data acquisition SDK.
//!
//! The bindings sit on openDAQ's flat C API (the `copendaq` library), loaded
//! dynamically at runtime: building this crate needs no openDAQ SDK, headers,
//! or C toolchain.  Prebuilt native libraries for x64 Linux, x64 Windows, and
//! ARM macOS are downloaded automatically on first use (only for your
//! platform, ~15 MB) and cached per user; see the crate README for the search
//! order and the environment variables that control it.
//!
//! # Quickstart
//!
//! ```no_run
//! use opendaq::{Instance, StreamReader};
//!
//! fn main() -> opendaq::Result<()> {
//!     let instance = Instance::new()?;
//!
//!     for device in instance.available_devices()? {
//!         println!(" - {}", device.connection_string()?);
//!     }
//!
//!     let device = instance.add_device("daqref://device0")?.expect("device");
//!     let channel = instance
//!         .find_component("Dev/RefDev0/IO/AI/RefCh0")?
//!         .expect("reference channel not found")
//!         .cast::<opendaq::Channel>()?;
//!     let signal = &channel.signals()?[0];
//!     let reader = StreamReader::<f64>::new(signal)?;
//!
//!     device.set_property_value("GlobalSampleRate", 100)?;
//!     channel.set_property_value("Frequency", 0.5)?;
//!
//!     let samples = reader.read(100, 2000)?;
//!     println!("{samples:?}");
//!     Ok(())
//! }
//! ```

mod callables;
mod callbacks;
mod data;
mod error;
mod events;
// Generated code keeps the generator's formatting so regeneration diffs stay
// readable.
#[rustfmt::skip]
mod generated;
mod instance;
mod loader;
mod marshal;
mod native_manifest;
mod object;
mod properties;
mod readers;
mod time;
mod value;

pub mod sys;

pub(crate) mod sealed {
    /// Prevents outside implementations of [`crate::Interface`] and
    /// [`crate::Sample`].
    pub trait Sealed {}
}

pub use error::{clear_error_info, Error, Result};
pub use generated::*;
pub use loader::{install_native_libraries, LoadError};
pub use object::{BaseObject, Interface};
pub use properties::{BatchedPropertyUpdate, ComponentKind};
pub use readers::{
    BlockReader, MultiReader, PacketReader, Sample, Samples, StreamReader, StreamReaderOptions,
    TailReader,
};
pub use sys::daqIntfID as IntfID;
pub use time::TickConverter;
pub use value::{Complex, Ratio, Value};

use std::path::{Path, PathBuf};

/// Load the openDAQ native libraries now (locating or downloading them
/// first), instead of implicitly on first use.
pub fn init() -> std::result::Result<(), LoadError> {
    sys::initialize().map(|_| ())
}

/// Load the openDAQ native libraries from an explicit directory.
pub fn init_from(directory: impl AsRef<Path>) -> std::result::Result<(), LoadError> {
    sys::initialize_from(directory.as_ref()).map(|_| ())
}

/// The directory holding the native openDAQ libraries (and the bundled device
/// / function-block modules), triggering the download when necessary.
pub fn native_library_directory() -> std::result::Result<PathBuf, LoadError> {
    sys::native_library_directory()
}

/// Print a diagnostic report about where the native libraries were searched
/// for and whether they load.
pub fn healthcheck() {
    println!("openDAQ healthcheck");
    println!("  platform: {:?}", loader::platform_directory_name());
    for (name, var) in [
        ("native dir override", loader::NATIVE_DIR_ENV_VAR),
        ("downloads disabled", loader::NO_DOWNLOAD_ENV_VAR),
        ("archive mirror", loader::ARCHIVE_URL_ENV_VAR),
        ("modules path", loader::MODULES_PATH_ENV_VAR),
    ] {
        if let Ok(value) = std::env::var(var) {
            println!("  {name} ({var}): {value}");
        }
    }
    println!(
        "  pinned binaries: {} (openDAQ {} @ {})",
        native_manifest::TAG,
        native_manifest::OPENDAQ_REF,
        &native_manifest::OPENDAQ_SHA[..12.min(native_manifest::OPENDAQ_SHA.len())]
    );
    match sys::initialize() {
        Ok(_) => {
            println!("  status: loaded");
            if let Ok(dir) = sys::native_library_directory() {
                println!("  native directory: {}", dir.display());
                for file in loader::NATIVE_LIBRARY_FILE_NAMES {
                    let path = dir.join(file);
                    let state = if path.is_file() { "ok" } else { "MISSING" };
                    println!("  library {file}: {state}");
                }
            }
        }
        Err(e) => {
            println!("  status: FAILED");
            println!("  error: {e}");
        }
    }
}
