//! Hand-written [`Instance`] constructors.
//!
//! `Instance` is a builder-backed type; the plain [`Instance::new`] defaults
//! the builder's module path to the directory holding the bundled native
//! modules (reference device, function blocks, streaming), so a fresh
//! instance can immediately add `daqref://` devices -- overriding openDAQ's
//! default of searching the current working directory.

use crate::error::{check, Result};
use crate::generated::{Context, Instance, InstanceBuilder};
use crate::marshal;
use crate::object::Interface;
use crate::sys;

impl Instance {
    /// Create an openDAQ instance with default configuration: a builder whose
    /// module path points at the bundled native modules.
    ///
    /// Calls the openDAQ C function `daqInstance_createInstanceFromBuilder()`.
    pub fn new() -> Result<Instance> {
        let builder = InstanceBuilder::new()?;
        // The native libraries are loaded by now (InstanceBuilder::new), so
        // the directory is always resolvable.
        if let Ok(dir) = sys::native_library_directory() {
            builder.set_module_path(&dir.to_string_lossy())?;
        }
        Instance::from_builder(&builder)
    }

    /// Create an instance from a configured [`InstanceBuilder`].
    ///
    /// Calls the openDAQ C function `daqInstance_createInstanceFromBuilder()`.
    pub fn from_builder(builder: &InstanceBuilder) -> Result<Instance> {
        let op = "daqInstance_createInstanceFromBuilder";
        let mut out: *mut sys::daqInstance = std::ptr::null_mut();
        let code = unsafe {
            (sys::api().daqInstance_createInstanceFromBuilder)(&mut out, builder.as_raw() as *mut _)
        };
        check(code, op)?;
        unsafe { marshal::require_object::<Instance>(out as *mut _, op) }
    }

    /// Create an instance over an existing [`Context`], with the given local
    /// id for the root device (pass `""` for a default).
    ///
    /// Calls the openDAQ C function `daqInstance_createInstance()`.
    pub fn with_context(context: &Context, local_id: &str) -> Result<Instance> {
        let op = "daqInstance_createInstance";
        let __local_id = marshal::make_string(local_id)?;
        let mut out: *mut sys::daqInstance = std::ptr::null_mut();
        let code = unsafe {
            (sys::api().daqInstance_createInstance)(
                &mut out,
                context.as_raw() as *mut _,
                __local_id.as_ptr() as *mut _,
            )
        };
        check(code, op)?;
        unsafe { marshal::require_object::<Instance>(out as *mut _, op) }
    }
}
