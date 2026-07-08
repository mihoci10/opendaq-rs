//! Routing openDAQ's C callbacks to Rust closures.
//!
//! The C callback types (`daqEventCall`, `daqProcCall`, `daqFuncCall`) carry
//! no user-data pointer, so each distinct Rust closure handed to openDAQ
//! needs its own native entry point.  A fixed pool of generated trampolines
//! (see `sys::EVENT_TRAMPOLINES` and friends) routes each invocation, via the
//! slot index baked into the trampoline, to the closure registered here.
//!
//! openDAQ may invoke callbacks from its own worker threads, so closures must
//! be `Send + Sync` and the registries are lock-protected (the lock is
//! released before the closure runs, so a callback may re-enter openDAQ).  A
//! panic inside a closure is caught at the boundary and reported to the
//! native caller as `OPENDAQ_ERR_GENERALERROR`; unwinding across the C frames
//! would be undefined behavior.
//!
//! Lifetime: openDAQ offers no destruction hook for a callable, so a
//! closure-backed [`crate::Procedure`] / [`crate::FunctionObject`] pins its
//! slot for the life of the process.  Event-handler slots are returned to the
//! pool by [`crate::Event::unsubscribe`].

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::object::BaseObject;
use crate::sys;
use crate::value::Value;

pub(crate) type EventClosure = dyn Fn(Option<BaseObject>, Option<BaseObject>) + Send + Sync;
pub(crate) type ProcedureClosure = dyn Fn(&[Value]) -> Result<()> + Send + Sync;
pub(crate) type FunctionClosure = dyn Fn(&[Value]) -> Result<Value> + Send + Sync;

struct Pool<T: ?Sized> {
    slots: Mutex<Vec<Option<Arc<T>>>>,
}

impl<T: ?Sized> Pool<T> {
    const fn new() -> Self {
        Pool {
            slots: Mutex::new(Vec::new()),
        }
    }

    /// Reserve a slot for `closure`, returning its index.
    fn allocate(&self, closure: Arc<T>, kind: &'static str) -> Result<usize> {
        let mut slots = self.slots.lock().unwrap();
        if slots.is_empty() {
            slots.resize_with(sys::TRAMPOLINE_POOL_SIZE, || None);
        }
        let index = slots.iter().position(Option::is_none).ok_or_else(|| {
            crate::Error::new(
                sys::OPENDAQ_ERR_OUTOFRANGE,
                "callback allocation",
                Some(format!(
                    "all {} {kind} callback slots are in use; openDAQ callables cannot be \
                     destroyed, so each closure pins its slot for the life of the process",
                    sys::TRAMPOLINE_POOL_SIZE
                )),
            )
        })?;
        slots[index] = Some(closure);
        Ok(index)
    }

    fn free(&self, index: usize) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(slot) = slots.get_mut(index) {
            *slot = None;
        }
    }

    /// The closure at `index`; cloned out so the registry lock is not held
    /// while the closure runs.
    fn get(&self, index: usize) -> Option<Arc<T>> {
        self.slots.lock().unwrap().get(index).and_then(Clone::clone)
    }
}

static EVENTS: Pool<EventClosure> = Pool::new();
static PROCEDURES: Pool<ProcedureClosure> = Pool::new();
static FUNCTIONS: Pool<FunctionClosure> = Pool::new();

pub(crate) fn allocate_event(closure: Arc<EventClosure>) -> Result<(sys::daqEventCall, usize)> {
    let index = EVENTS.allocate(closure, "event")?;
    Ok((sys::EVENT_TRAMPOLINES[index], index))
}

pub(crate) fn free_event(index: usize) {
    EVENTS.free(index);
}

pub(crate) fn allocate_procedure(
    closure: Arc<ProcedureClosure>,
) -> Result<(sys::daqProcCall, usize)> {
    let index = PROCEDURES.allocate(closure, "procedure")?;
    Ok((sys::PROCEDURE_TRAMPOLINES[index], index))
}

pub(crate) fn allocate_function(
    closure: Arc<FunctionClosure>,
) -> Result<(sys::daqFuncCall, usize)> {
    let index = FUNCTIONS.allocate(closure, "function")?;
    Ok((sys::FUNCTION_TRAMPOLINES[index], index))
}

/// Decode a callable's `params` argument (borrowed; ownership stays with the
/// caller) into the argument list, inverting the encoding convention: null
/// means no arguments, a daq list one argument per element, anything else the
/// single argument.
unsafe fn decode_params(params: *mut c_void) -> Result<Vec<Value>> {
    if params.is_null() {
        return Ok(Vec::new());
    }
    match crate::value::unbox_ptr(params, "callback params")? {
        Value::List(items) => Ok(items),
        other => Ok(vec![other]),
    }
}

pub(crate) fn dispatch_event(index: usize, sender: *mut c_void, args: *mut c_void) {
    let Some(closure) = EVENTS.get(index) else {
        return;
    };
    // The C event handler add-refs sender and args before invoking us, so the
    // callback owns one reference to each; the wrappers release them.
    let sender = unsafe { <BaseObject as crate::Interface>::from_raw(sender) };
    let args = unsafe { <BaseObject as crate::Interface>::from_raw(args) };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| closure(sender, args)));
}

pub(crate) fn dispatch_procedure(index: usize, params: *mut c_void) -> u32 {
    let Some(closure) = PROCEDURES.get(index) else {
        return sys::OPENDAQ_ERR_NOTASSIGNED;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        closure(&unsafe { decode_params(params) }?)
    }));
    match result {
        Ok(Ok(())) => sys::OPENDAQ_SUCCESS,
        Ok(Err(e)) => e.code(),
        Err(_) => sys::OPENDAQ_ERR_GENERALERROR,
    }
}

pub(crate) fn dispatch_function(
    index: usize,
    params: *mut c_void,
    result: *mut *mut c_void,
) -> u32 {
    let Some(closure) = FUNCTIONS.get(index) else {
        return sys::OPENDAQ_ERR_NOTASSIGNED;
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value> {
        closure(&unsafe { decode_params(params) }?)
    }));
    match outcome {
        Ok(Ok(value)) => {
            // The caller receives an owned reference; Null is boxed to a
            // Boolean false, since a callable result cannot be a null object.
            let boxed = match value {
                Value::Null => crate::value::bool_to_ref(false),
                other => crate::value::to_daq(&other).map(|r| r.expect("non-null by construction")),
            };
            match boxed {
                Ok(r) => {
                    if !result.is_null() {
                        unsafe { *result = r.into_raw() };
                    }
                    sys::OPENDAQ_SUCCESS
                }
                Err(e) => e.code(),
            }
        }
        Ok(Err(e)) => e.code(),
        Err(_) => sys::OPENDAQ_ERR_GENERALERROR,
    }
}
