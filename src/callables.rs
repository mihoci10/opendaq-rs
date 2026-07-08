//! openDAQ callables ([`Procedure`], [`FunctionObject`]) from Rust closures,
//! and calling them with natural Rust arguments.
//!
//! openDAQ encodes a callable's params as: null for no arguments, the single
//! object for one argument, and a daq list for several.  [`FunctionObject::call`]
//! and [`Procedure::dispatch_args`] apply that encoding for you (with each
//! argument boxed from a [`Value`]), and closure-backed callables receive
//! their arguments decoded the same way.
//!
//! Wherever openDAQ expects a `Procedure` or `Function` -- a `func`/`proc`
//! property's value, a custom search filter, ... -- pass a closure-backed
//! object:
//!
//! ```no_run
//! # fn main() -> opendaq::Result<()> {
//! use opendaq::{FunctionObject, Value};
//!
//! let sum = FunctionObject::from_fn(|args| {
//!     Ok(Value::from(args[0].as_i64().unwrap_or(0) + args[1].as_i64().unwrap_or(0)))
//! })?;
//! # Ok(()) }
//! ```
//!
//! A panic or error inside the closure is reported to the native caller as an
//! openDAQ error rather than unwinding across the C boundary.  openDAQ has no
//! destruction hook for callables, so each closure-backed callable keeps its
//! closure (and its trampoline slot) alive for the rest of the process.

use std::ffi::c_void;
use std::sync::Arc;

use crate::callbacks;
use crate::error::{check, Result};
use crate::generated::{FunctionObject, Procedure};
use crate::marshal;
use crate::object::Interface;
use crate::sys;
use crate::value::{to_daq, Value};

/// Encode `args` as a callable's params object: `None` (null) for no
/// arguments, the single boxed value for one, a daq list for several.
fn encode_params(args: &[Value]) -> Result<Option<crate::object::Ref>> {
    match args {
        [] => Ok(None),
        // A sole Null must still be boxed (to Boolean false): a null params
        // would read as the no-arguments encoding.
        [single] => match single {
            Value::Null => Ok(Some(crate::value::bool_to_ref(false)?)),
            other => to_daq(other),
        },
        many => to_daq(&Value::List(many.to_vec())),
    }
}

impl Procedure {
    /// Wrap a Rust closure as an openDAQ procedure.  openDAQ invokes it --
    /// from Rust or from native code -- with the decoded arguments.
    ///
    /// Calls the openDAQ C function `daqProcedure_createProcedure()`.
    pub fn from_fn(
        procedure: impl Fn(&[Value]) -> Result<()> + Send + Sync + 'static,
    ) -> Result<Procedure> {
        let op = "daqProcedure_createProcedure";
        let (trampoline, _index) = callbacks::allocate_procedure(Arc::new(procedure))?;
        let mut out: *mut sys::daqProcedure = std::ptr::null_mut();
        check(
            unsafe { (sys::api().daqProcedure_createProcedure)(&mut out, trampoline) },
            op,
        )?;
        unsafe { marshal::require_object::<Procedure>(out as *mut _, op) }
    }

    /// Dispatch the procedure with natural Rust arguments (boxed and encoded
    /// per the openDAQ params convention).
    pub fn dispatch_args(&self, args: &[Value]) -> Result<()> {
        let params = encode_params(args)?;
        check(
            unsafe {
                (sys::api().daqProcedure_dispatch)(
                    self.as_raw() as *mut _,
                    crate::value::opt_ref_ptr(&params),
                )
            },
            "daqProcedure_dispatch",
        )
    }
}

impl FunctionObject {
    /// Wrap a Rust closure as an openDAQ function.  openDAQ invokes it --
    /// from Rust or from native code -- with the decoded arguments; the
    /// returned [`Value`] is boxed back for the caller.
    ///
    /// Calls the openDAQ C function `daqFunction_createFunction()`.
    pub fn from_fn(
        function: impl Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
    ) -> Result<FunctionObject> {
        let op = "daqFunction_createFunction";
        let (trampoline, _index) = callbacks::allocate_function(Arc::new(function))?;
        let mut out: *mut sys::daqFunction = std::ptr::null_mut();
        check(
            unsafe { (sys::api().daqFunction_createFunction)(&mut out, trampoline) },
            op,
        )?;
        unsafe { marshal::require_object::<FunctionObject>(out as *mut _, op) }
    }

    /// Call the function with natural Rust arguments and unbox its result.
    ///
    /// Calls the openDAQ C function `daqFunction_call()`.
    pub fn call(&self, args: &[Value]) -> Result<Value> {
        let op = "daqFunction_call";
        let params = encode_params(args)?;
        let mut result: *mut c_void = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqFunction_call)(
                    self.as_raw() as *mut _,
                    crate::value::opt_ref_ptr(&params),
                    &mut result,
                )
            },
            op,
        )?;
        unsafe { crate::value::take_value(result, op) }
    }
}
