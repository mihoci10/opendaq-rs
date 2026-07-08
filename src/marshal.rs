//! Marshaling helpers shared by the generated high-level methods.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
use std::hash::Hash;

use crate::error::{check, Error, Result};
use crate::object::{Interface, Ref};
use crate::sys;
use crate::value::FromDaqOwned;

/// A Rust string as a NUL-terminated C string, for raw `daqConstCharPtr`
/// parameters (which take a plain `const char*`, not an `IString` object).
pub(crate) fn make_c_string(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| {
        Error::new(
            sys::OPENDAQ_ERR_INVALIDPARAMETER,
            "CString conversion",
            Some("string contains NUL".into()),
        )
    })
}

/// Box a Rust string into an owned `daqString` reference.
pub(crate) fn make_string(value: &str) -> Result<Ref> {
    let op = "daqString_createString";
    let c = CString::new(value).map_err(|_| {
        Error::new(
            sys::OPENDAQ_ERR_INVALIDPARAMETER,
            op,
            Some("string contains NUL".into()),
        )
    })?;
    let mut out: *mut sys::daqString = std::ptr::null_mut();
    check(
        unsafe { (sys::api().daqString_createString)(&mut out, c.as_ptr()) },
        op,
    )?;
    unsafe { Ref::from_owned(out as *mut c_void) }
        .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))
}

/// Copy out and release an owned `daqString` reference; null yields `""`.
pub(crate) unsafe fn take_string(ptr: *mut sys::daqString) -> String {
    let Some(r) = Ref::from_owned(ptr as *mut c_void) else {
        return String::new();
    };
    let mut chars: *const c_char = std::ptr::null();
    let code = (sys::api().daqString_getCharPtr)(r.as_ptr() as *mut _, &mut chars);
    if crate::error::failure_code(code) || chars.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(chars)
        .to_string_lossy()
        .into_owned()
}

/// Copy a `daqCharPtr` out-value the caller owns (per the `toString`
/// contract) and free it with `daqFreeMemory`; null yields `""`.
#[allow(dead_code)] // referenced by generated code only when the headers use it
pub(crate) unsafe fn take_char_ptr(ptr: *mut c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let text = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
    (sys::api().daqFreeMemory)(ptr as *mut c_void);
    text
}

/// Copy a `daqConstCharPtr` out-value that borrows storage owned by the
/// object it came from; null yields `""`.
pub(crate) unsafe fn copy_const_char_ptr(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

/// Adopt an owned, correctly-typed interface pointer; null yields `None`.
pub(crate) unsafe fn take_object<T: Interface>(ptr: *mut c_void) -> Option<T> {
    T::from_raw(ptr)
}

/// Adopt an owned interface pointer that must not be null (factory results).
pub(crate) unsafe fn require_object<T: Interface>(ptr: *mut c_void, op: &'static str) -> Result<T> {
    T::from_raw(ptr).ok_or_else(|| {
        Error::new(
            sys::OPENDAQ_ERR_GENERALERROR,
            op,
            Some("returned a null object".into()),
        )
    })
}

/// Convert an owned `daqList` reference into a `Vec`, converting each element
/// per its statically-declared type; a null list is the empty `Vec`.
pub(crate) unsafe fn take_list<T: FromDaqOwned>(
    ptr: *mut sys::daqList,
    op: &'static str,
) -> Result<Vec<T>> {
    let Some(list) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(Vec::new());
    };
    let mut count: usize = 0;
    check(
        (sys::api().daqList_getCount)(list.as_ptr() as *mut _, &mut count),
        op,
    )?;
    let mut items = Vec::with_capacity(count);
    for index in 0..count {
        let mut item: *mut c_void = std::ptr::null_mut();
        check(
            (sys::api().daqList_getItemAt)(list.as_ptr() as *mut _, index, &mut item),
            op,
        )?;
        items.push(T::from_daq_owned(item, op)?);
    }
    Ok(items)
}

/// Convert an owned `daqDict` reference into a `HashMap`; a null dict is
/// empty.
pub(crate) unsafe fn take_dict<K, V>(
    ptr: *mut sys::daqDict,
    op: &'static str,
) -> Result<HashMap<K, V>>
where
    K: FromDaqOwned + Eq + Hash,
    V: FromDaqOwned,
{
    let Some(dict) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(HashMap::new());
    };
    let mut keys: *mut sys::daqList = std::ptr::null_mut();
    check(
        (sys::api().daqDict_getKeyList)(dict.as_ptr() as *mut _, &mut keys),
        op,
    )?;
    let keys = Ref::from_owned(keys as *mut c_void)
        .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
    let mut count: usize = 0;
    check(
        (sys::api().daqList_getCount)(keys.as_ptr() as *mut _, &mut count),
        op,
    )?;
    let mut map = HashMap::with_capacity(count);
    for index in 0..count {
        let mut key: *mut c_void = std::ptr::null_mut();
        check(
            (sys::api().daqList_getItemAt)(keys.as_ptr() as *mut _, index, &mut key),
            op,
        )?;
        let mut value: *mut c_void = std::ptr::null_mut();
        // Fetch the value while we still hold the key pointer; converting the
        // key consumes (releases) it.
        check(
            (sys::api().daqDict_get)(dict.as_ptr() as *mut _, key, &mut value),
            op,
        )?;
        map.insert(K::from_daq_owned(key, op)?, V::from_daq_owned(value, op)?);
    }
    Ok(map)
}

/// Convert an owned `daqDict` reference whose key type has no natural map key
/// form into (key, value) pairs.
#[allow(dead_code)] // referenced by generated code only when the headers use it
pub(crate) unsafe fn take_dict_pairs(
    ptr: *mut sys::daqDict,
    op: &'static str,
) -> Result<Vec<(crate::Value, crate::Value)>> {
    let Some(dict) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(Vec::new());
    };
    match crate::value::unbox_ptr(dict.as_ptr(), op)? {
        crate::Value::Dict(pairs) => Ok(pairs),
        _ => Err(Error::new(
            sys::OPENDAQ_ERR_INVALIDTYPE,
            op,
            Some("expected a dict".into()),
        )),
    }
}

/// Map a decoded C enum out-value onto its Rust enum, erroring on values this
/// binding does not know.
pub(crate) fn enum_out<T>(value: Option<T>, op: &'static str) -> Result<T> {
    value.ok_or_else(|| {
        Error::new(
            sys::OPENDAQ_ERR_INVALIDTYPE,
            op,
            Some("unknown enum value returned".into()),
        )
    })
}

/// Build an owned `daqList` from interface wrappers.
pub(crate) fn list_from_interfaces<T: Interface>(items: &[T]) -> Result<Ref> {
    let op = "daqList_createList";
    let mut out: *mut sys::daqList = std::ptr::null_mut();
    check(unsafe { (sys::api().daqList_createList)(&mut out) }, op)?;
    let list = unsafe { Ref::from_owned(out as *mut c_void) }
        .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
    for item in items {
        check(
            unsafe { (sys::api().daqList_pushBack)(list.as_ptr() as *mut _, item.as_raw()) },
            "daqList_pushBack",
        )?;
    }
    Ok(list)
}

/// Build an owned `daqList` of `daqString`s.
pub(crate) fn list_from_strs(items: &[&str]) -> Result<Ref> {
    let op = "daqList_createList";
    let mut out: *mut sys::daqList = std::ptr::null_mut();
    check(unsafe { (sys::api().daqList_createList)(&mut out) }, op)?;
    let list = unsafe { Ref::from_owned(out as *mut c_void) }
        .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
    for item in items {
        let boxed = make_string(item)?;
        check(
            unsafe { (sys::api().daqList_pushBack)(list.as_ptr() as *mut _, boxed.as_ptr()) },
            "daqList_pushBack",
        )?;
    }
    Ok(list)
}
