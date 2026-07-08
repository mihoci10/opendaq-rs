//! The reference-counted object model shared by every openDAQ wrapper.
//!
//! openDAQ objects are COM-style reference-counted interfaces.  [`Ref`] owns
//! exactly one reference: it releases it on drop and adds one on clone, so a
//! wrapper is never released manually.  Each generated interface type is a
//! `#[repr(transparent)]` chain of newtypes rooted in [`BaseObject`], which
//! both models the single-inheritance interface hierarchy (via `Deref`) and
//! guarantees a `&Device` can be passed where the C ABI expects its parent
//! `daqFolder*` -- base-interface methods occupy the leading virtual-table
//! slots, so the pointer is valid for both.

use std::ffi::c_void;
use std::ptr::NonNull;

use crate::error::{check, Error, Result};
use crate::sys::{self, daqIntfID};

/// An owned reference to a native openDAQ object.
///
/// Releases the reference on drop and adds one on clone.  openDAQ reference
/// counts are atomic and its objects may be touched from its worker threads,
/// so `Ref` is `Send + Sync`.
pub(crate) struct Ref {
    ptr: NonNull<c_void>,
}

unsafe impl Send for Ref {}
unsafe impl Sync for Ref {}

impl Ref {
    /// Adopt an owned reference (does NOT add one).  Returns `None` for null.
    pub(crate) unsafe fn from_owned(ptr: *mut c_void) -> Option<Ref> {
        NonNull::new(ptr).map(|ptr| Ref { ptr })
    }

    /// Wrap a borrowed pointer, adding the reference this `Ref` will own.
    pub(crate) unsafe fn from_borrowed(ptr: *mut c_void) -> Option<Ref> {
        let ptr = NonNull::new(ptr)?;
        (sys::api().daqBaseObject_addRef)(ptr.as_ptr());
        Some(Ref { ptr })
    }

    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.ptr.as_ptr()
    }

    /// Hand the owned reference to the caller without releasing it.
    pub(crate) fn into_raw(self) -> *mut c_void {
        let ptr = self.ptr.as_ptr();
        std::mem::forget(self);
        ptr
    }
}

impl Clone for Ref {
    fn clone(&self) -> Ref {
        unsafe { (sys::api().daqBaseObject_addRef)(self.ptr.as_ptr()) };
        Ref { ptr: self.ptr }
    }
}

impl Drop for Ref {
    fn drop(&mut self) {
        unsafe { (sys::api().daqBaseObject_releaseRef)(self.ptr.as_ptr()) };
    }
}

impl std::fmt::Debug for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:p}", self.ptr.as_ptr())
    }
}

/// The root of the openDAQ interface hierarchy (`IBaseObject`).
///
/// Every generated interface wrapper dereferences to `BaseObject`, so the
/// casting and identity helpers here are available on all of them.
#[repr(transparent)]
#[derive(Clone, Debug)]
pub struct BaseObject(pub(crate) Ref);

/// An openDAQ interface wrapper type.
///
/// Implemented by every generated interface struct (and [`BaseObject`]).
/// The trait is sealed: only the generated wrappers implement it.
///
/// # Safety
///
/// Implementations guarantee that the wrapped pointer really holds the
/// interface the type stands for, and that `from_raw` takes ownership of
/// exactly one reference.
pub unsafe trait Interface: Sized + crate::sealed::Sealed {
    /// The openDAQ interface name, e.g. `"daqDevice"`.
    const NAME: &'static str;

    /// The interface's GUID, or `None` for the few coretypes interfaces the
    /// C API exposes no id for (`IBaseObject`, `IFunction`, `IProcedure`, ...).
    fn interface_id() -> Option<daqIntfID>;

    /// Adopt an *owned* raw interface pointer (takes ownership of one
    /// reference; does not add one).  Returns `None` for null.
    ///
    /// # Safety
    ///
    /// `ptr` must be null or a live openDAQ object pointer that genuinely
    /// holds this interface (its virtual table must match), with one
    /// reference owned by the caller -- use [`BaseObject::cast`] to convert
    /// between interfaces safely.
    unsafe fn from_raw(ptr: *mut c_void) -> Option<Self>;

    /// The raw interface pointer (still owned by this wrapper).
    fn as_raw(&self) -> *mut c_void {
        self.as_base_object().0.as_ptr()
    }

    /// This wrapper viewed as the root `IBaseObject` interface.
    fn as_base_object(&self) -> &BaseObject;

    /// A new owning `BaseObject` handle to the same underlying object.
    fn to_base_object(&self) -> BaseObject {
        BaseObject(self.as_base_object().0.clone())
    }
}

impl crate::sealed::Sealed for BaseObject {}

unsafe impl Interface for BaseObject {
    const NAME: &'static str = "daqBaseObject";

    fn interface_id() -> Option<daqIntfID> {
        None
    }

    unsafe fn from_raw(ptr: *mut c_void) -> Option<Self> {
        Ref::from_owned(ptr).map(BaseObject)
    }

    fn as_base_object(&self) -> &BaseObject {
        self
    }
}

impl BaseObject {
    /// Query this object for interface `T`, returning a wrapper that owns its
    /// own reference.
    ///
    /// This is a genuine `queryInterface`: an object's interfaces live at
    /// different virtual-table offsets, so the pointer for one interface is
    /// not binary-compatible with another and must be queried, not merely
    /// reinterpreted.  Fails with an openDAQ error when the object does not
    /// implement `T` (use [`BaseObject::try_cast`] / [`BaseObject::is_a`] to
    /// probe first).
    ///
    /// For the few interfaces the C API exposes no GUID for (`T::interface_id()`
    /// is `None`), the pointer is reinterpreted unchecked, which is sound only
    /// when the object really lies on `T`'s inheritance chain.
    pub fn cast<T: Interface>(&self) -> Result<T> {
        match T::interface_id() {
            Some(id) => {
                let mut out: *mut c_void = std::ptr::null_mut();
                check(
                    unsafe {
                        (sys::api().daqBaseObject_queryInterface)(self.0.as_ptr(), id, &mut out)
                    },
                    "daqBaseObject_queryInterface",
                )?;
                unsafe { T::from_raw(out) }.ok_or_else(|| {
                    Error::new(
                        sys::OPENDAQ_ERR_NOINTERFACE,
                        "daqBaseObject_queryInterface",
                        None,
                    )
                })
            }
            None => {
                // No GUID to query with: reinterpret unchecked, transferring
                // the reference the clone adds to the new wrapper.
                let cloned = self.0.clone();
                Ok(unsafe { T::from_raw(cloned.into_raw()) }.expect("non-null by construction"))
            }
        }
    }

    /// Like [`BaseObject::cast`], but returns `None` when the object does not
    /// implement `T`.
    pub fn try_cast<T: Interface>(&self) -> Option<T> {
        self.cast().ok()
    }

    /// True when the object implements interface `T` (a `borrowInterface`
    /// probe; adds no reference).  Returns `false` for interfaces without a
    /// GUID in the C API, which cannot be probed.
    pub fn is_a<T: Interface>(&self) -> bool {
        let Some(id) = T::interface_id() else {
            return false;
        };
        let mut out: *mut c_void = std::ptr::null_mut();
        let code =
            unsafe { (sys::api().daqBaseObject_borrowInterface)(self.0.as_ptr(), id, &mut out) };
        !crate::error::failure_code(code) && !out.is_null()
    }

    /// The object's hash code.
    pub fn hash_code(&self) -> Result<usize> {
        let mut out: usize = 0;
        check(
            unsafe { (sys::api().daqBaseObject_getHashCode)(self.0.as_ptr(), &mut out) },
            "daqBaseObject_getHashCode",
        )?;
        Ok(out)
    }

    /// True when openDAQ considers the two objects equal.
    pub fn equals<T: Interface>(&self, other: &T) -> Result<bool> {
        let mut out: u8 = 0;
        check(
            unsafe { (sys::api().daqBaseObject_equals)(self.0.as_ptr(), other.as_raw(), &mut out) },
            "daqBaseObject_equals",
        )?;
        Ok(out != 0)
    }

    /// Identity comparison: both wrappers refer to the same native object.
    /// (Note that pointers to *different interfaces* of one object differ;
    /// this compares the raw interface pointers.)
    pub fn ptr_eq<T: Interface>(&self, other: &T) -> bool {
        self.0.as_ptr() == other.as_raw()
    }

    /// Release the object's internal resources early (openDAQ `dispose`).
    /// The wrapper itself stays alive; most callers never need this.
    pub fn dispose(&self) -> Result<()> {
        check(
            unsafe { (sys::api().daqBaseObject_dispose)(self.0.as_ptr()) },
            "daqBaseObject_dispose",
        )
    }

    /// Create a plain new `IBaseObject`.
    pub fn new() -> Result<BaseObject> {
        let mut out: *mut c_void = std::ptr::null_mut();
        check(
            unsafe { (sys::api().daqBaseObject_create)(&mut out) },
            "daqBaseObject_create",
        )?;
        unsafe { BaseObject::from_raw(out) }
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, "daqBaseObject_create", None))
    }
}

impl std::fmt::Display for BaseObject {
    /// The object's `toString` representation.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut chars: *mut std::ffi::c_char = std::ptr::null_mut();
        let code = unsafe { (sys::api().daqBaseObject_toString)(self.0.as_ptr(), &mut chars) };
        if crate::error::failure_code(code) || chars.is_null() {
            return write!(f, "<openDAQ object {:?}>", self.0);
        }
        let text = unsafe { std::ffi::CStr::from_ptr(chars) }
            .to_string_lossy()
            .into_owned();
        // toString hands out memory the caller must free with daqFreeMemory.
        unsafe { (sys::api().daqFreeMemory)(chars as *mut c_void) };
        f.write_str(&text)
    }
}
