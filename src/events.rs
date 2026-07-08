//! Subscribing Rust closures to openDAQ events.
//!
//! Reference counting is handled for you: the C event handler add-refs the
//! sender and args before invoking the callback, and the [`BaseObject`]
//! wrappers handed to the closure release exactly those references.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::callbacks;
use crate::error::{check, Result};
use crate::generated::{Event, EventHandler};
use crate::marshal;
use crate::object::{BaseObject, Interface};
use crate::sys;

/// Maps a closure-backed handler (by raw pointer) to its trampoline slot, so
/// [`Event::unsubscribe`] can return the slot to the pool.
fn handler_slots() -> &'static Mutex<HashMap<usize, usize>> {
    static SLOTS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

impl EventHandler {
    /// Wrap a Rust closure as an openDAQ event handler.  The closure is
    /// called with the (wrapped) sender and event args each time the event
    /// fires -- possibly from openDAQ's own worker threads.  Cast the args
    /// with [`BaseObject::cast`] for typed event args (e.g. to
    /// [`crate::CoreEventArgs`]).
    ///
    /// Calls the openDAQ C function `daqEventHandler_createEventHandler()`.
    pub fn from_fn(
        handler: impl Fn(Option<BaseObject>, Option<BaseObject>) + Send + Sync + 'static,
    ) -> Result<EventHandler> {
        let op = "daqEventHandler_createEventHandler";
        let (trampoline, index) = callbacks::allocate_event(Arc::new(handler))?;
        let mut out: *mut sys::daqEventHandler = std::ptr::null_mut();
        let code = unsafe { (sys::api().daqEventHandler_createEventHandler)(&mut out, trampoline) };
        if let Err(e) = check(code, op) {
            callbacks::free_event(index);
            return Err(e);
        }
        let handler = unsafe { marshal::require_object::<EventHandler>(out as *mut _, op) }
            .inspect_err(|_| callbacks::free_event(index))?;
        handler_slots()
            .lock()
            .unwrap()
            .insert(handler.as_raw() as usize, index);
        Ok(handler)
    }
}

impl Event {
    /// Subscribe a Rust closure to this event, returning the created
    /// [`EventHandler`]; pass it to [`Event::unsubscribe`] to unsubscribe.
    pub fn subscribe(
        &self,
        handler: impl Fn(Option<BaseObject>, Option<BaseObject>) + Send + Sync + 'static,
    ) -> Result<EventHandler> {
        let handler = EventHandler::from_fn(handler)?;
        self.add_handler(&handler)?;
        Ok(handler)
    }

    /// Unsubscribe a handler and, if it was closure-backed, return its
    /// callback slot to the pool for reuse.
    pub fn unsubscribe(&self, handler: &EventHandler) -> Result<()> {
        self.remove_handler(handler)?;
        if let Some(index) = handler_slots()
            .lock()
            .unwrap()
            .remove(&(handler.as_raw() as usize))
        {
            callbacks::free_event(index);
        }
        Ok(())
    }
}
