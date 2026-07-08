//! Boxing and unboxing between native Rust values and openDAQ core objects.
//!
//! [`Value`] is the Rust view of "any openDAQ value": scalars unbox to native
//! Rust types, daq lists and dicts convert recursively, and anything without a
//! natural Rust form (a device, a property object, ...) stays a wrapped
//! [`BaseObject`] to be [`BaseObject::cast`] to the interface you expect.  The
//! two directions are inverses, mirroring the boxing table of the other
//! openDAQ bindings.

use std::collections::HashMap;
use std::ffi::c_void;

use crate::error::{check, failure_code, Error, Result};
use crate::object::{BaseObject, Interface, Ref};
use crate::sys::{self, daqIntfID};

/// A rational number (openDAQ `IRatio`), e.g. a domain tick resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ratio {
    pub numerator: i64,
    pub denominator: i64,
}

impl Ratio {
    pub const fn new(numerator: i64, denominator: i64) -> Ratio {
        Ratio {
            numerator,
            denominator,
        }
    }

    pub fn as_f64(&self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }
}

impl std::fmt::Display for Ratio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

/// A complex number (openDAQ `IComplexNumber`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub const fn new(re: f64, im: f64) -> Complex {
        Complex { re, im }
    }
}

impl std::fmt::Display for Complex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{:+}i", self.re, self.im)
    }
}

/// Any openDAQ value in its natural Rust form.
///
/// Scalars, lists, and dicts convert to native Rust data; an object with no
/// natural Rust form stays an [`Object`](Value::Object) wrapper.  `Value` is
/// what generic `daqBaseObject` parameters accept (via `impl Into<Value>`)
/// and what generic results (property values, event parameters, dict
/// elements, ...) unbox to.
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Ratio(Ratio),
    Complex(Complex),
    List(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Object(BaseObject),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The value as a float; an `Int` or `Ratio` converts too.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            Value::Ratio(r) => Some(r.as_f64()),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_ratio(&self) -> Option<Ratio> {
        match self {
            Value::Ratio(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Dict(pairs) => Some(pairs),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BaseObject> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// The wrapped object, or an error for a non-object value.
    pub fn into_object(self) -> Result<BaseObject> {
        match self {
            Value::Object(o) => Ok(o),
            other => Err(wrong_kind("an openDAQ object", &other)),
        }
    }

    /// Look up `key` in a `Dict` value (string keys).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(pairs) => pairs
                .iter()
                .find(|(k, _)| k.as_str() == Some(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    fn kind_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "a bool",
            Value::Int(_) => "an integer",
            Value::Float(_) => "a float",
            Value::Str(_) => "a string",
            Value::Ratio(_) => "a ratio",
            Value::Complex(_) => "a complex number",
            Value::List(_) => "a list",
            Value::Dict(_) => "a dict",
            Value::Object(_) => "an openDAQ object",
        }
    }
}

fn wrong_kind(expected: &str, got: &Value) -> Error {
    Error::new(
        sys::OPENDAQ_ERR_INVALIDTYPE,
        "Value conversion",
        Some(format!("expected {expected}, got {}", got.kind_name())),
    )
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Str(s) => write!(f, "{s}"),
            Value::Ratio(r) => write!(f, "{r}"),
            Value::Complex(c) => write!(f, "{c}"),
            Value::List(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Value::Dict(pairs) => {
                write!(f, "{{")?;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Value::Object(o) => std::fmt::Display::fmt(o, f),
        }
    }
}

macro_rules! value_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Value { Value::Int(v as i64) }
        }
    )*};
}
value_from_int!(i8, i16, i32, i64, u8, u16, u32, usize);

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Float(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Value {
        Value::Float(v as f64)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Str(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Str(v)
    }
}
impl From<Ratio> for Value {
    fn from(v: Ratio) -> Value {
        Value::Ratio(v)
    }
}
impl From<Complex> for Value {
    fn from(v: Complex) -> Value {
        Value::Complex(v)
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Value {
        Value::List(items.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Value> + Clone> From<&[T]> for Value {
    fn from(items: &[T]) -> Value {
        Value::List(items.iter().cloned().map(Into::into).collect())
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Value {
        v.map_or(Value::Null, Into::into)
    }
}
impl From<HashMap<String, Value>> for Value {
    fn from(map: HashMap<String, Value>) -> Value {
        Value::Dict(map.into_iter().map(|(k, v)| (Value::Str(k), v)).collect())
    }
}
impl From<BaseObject> for Value {
    fn from(o: BaseObject) -> Value {
        Value::Object(o)
    }
}
impl From<&BaseObject> for Value {
    fn from(o: &BaseObject) -> Value {
        Value::Object(o.clone())
    }
}

impl TryFrom<Value> for bool {
    type Error = Error;
    fn try_from(v: Value) -> Result<bool> {
        v.as_bool().ok_or_else(|| wrong_kind("a bool", &v))
    }
}
impl TryFrom<Value> for i64 {
    type Error = Error;
    fn try_from(v: Value) -> Result<i64> {
        v.as_i64().ok_or_else(|| wrong_kind("an integer", &v))
    }
}
impl TryFrom<Value> for f64 {
    type Error = Error;
    fn try_from(v: Value) -> Result<f64> {
        v.as_f64().ok_or_else(|| wrong_kind("a float", &v))
    }
}
impl TryFrom<Value> for String {
    type Error = Error;
    fn try_from(v: Value) -> Result<String> {
        match v {
            Value::Str(s) => Ok(s),
            other => Err(wrong_kind("a string", &other)),
        }
    }
}
impl TryFrom<Value> for Ratio {
    type Error = Error;
    fn try_from(v: Value) -> Result<Ratio> {
        v.as_ratio().ok_or_else(|| wrong_kind("a ratio", &v))
    }
}
impl TryFrom<Value> for Vec<Value> {
    type Error = Error;
    fn try_from(v: Value) -> Result<Vec<Value>> {
        match v {
            Value::List(items) => Ok(items),
            other => Err(wrong_kind("a list", &other)),
        }
    }
}

// ---------------------------------------------------------------------------
// Interface ids of the unboxable core interfaces
// ---------------------------------------------------------------------------

fn iface_id(getter: unsafe extern "C" fn(*mut daqIntfID)) -> daqIntfID {
    let mut id = daqIntfID {
        Data1: 0,
        Data2: 0,
        Data3: 0,
        Data4: 0,
    };
    unsafe { getter(&mut id) };
    id
}

/// Borrow the given interface of the object at `ptr` (no reference added), or
/// `None` when the object does not implement it.
unsafe fn borrow_iface(ptr: *mut c_void, id: daqIntfID) -> Option<*mut c_void> {
    let mut out: *mut c_void = std::ptr::null_mut();
    let code = (sys::api().daqBaseObject_borrowInterface)(ptr, id, &mut out);
    if failure_code(code) || out.is_null() {
        None
    } else {
        Some(out)
    }
}

/// Query the given interface, returning an owned reference.
fn query_iface(ptr: *mut c_void, id: daqIntfID, op: &'static str) -> Result<Ref> {
    let mut out: *mut c_void = std::ptr::null_mut();
    check(
        unsafe { (sys::api().daqBaseObject_queryInterface)(ptr, id, &mut out) },
        op,
    )?;
    unsafe { Ref::from_owned(out) }
        .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_NOINTERFACE, op, None))
}

// ---------------------------------------------------------------------------
// Boxing: Rust -> openDAQ
// ---------------------------------------------------------------------------

pub(crate) fn int_to_ref(value: i64) -> Result<Ref> {
    let mut out: *mut sys::daqInteger = std::ptr::null_mut();
    check(
        unsafe { (sys::api().daqInteger_createInteger)(&mut out, value) },
        "daqInteger_createInteger",
    )?;
    unsafe { Ref::from_owned(out as *mut c_void) }.ok_or_else(|| {
        Error::new(
            sys::OPENDAQ_ERR_GENERALERROR,
            "daqInteger_createInteger",
            None,
        )
    })
}

pub(crate) fn float_to_ref(value: f64) -> Result<Ref> {
    let mut out: *mut sys::daqFloatObject = std::ptr::null_mut();
    check(
        unsafe { (sys::api().daqFloatObject_createFloatObject)(&mut out, value) },
        "daqFloatObject_createFloatObject",
    )?;
    unsafe { Ref::from_owned(out as *mut c_void) }.ok_or_else(|| {
        Error::new(
            sys::OPENDAQ_ERR_GENERALERROR,
            "daqFloatObject_createFloatObject",
            None,
        )
    })
}

pub(crate) fn bool_to_ref(value: bool) -> Result<Ref> {
    let mut out: *mut sys::daqBoolean = std::ptr::null_mut();
    check(
        unsafe { (sys::api().daqBoolean_createBoolObject)(&mut out, u8::from(value)) },
        "daqBoolean_createBoolObject",
    )?;
    unsafe { Ref::from_owned(out as *mut c_void) }.ok_or_else(|| {
        Error::new(
            sys::OPENDAQ_ERR_GENERALERROR,
            "daqBoolean_createBoolObject",
            None,
        )
    })
}

pub(crate) fn ratio_to_ref(value: Ratio) -> Result<Ref> {
    let mut out: *mut sys::daqRatio = std::ptr::null_mut();
    check(
        unsafe { (sys::api().daqRatio_createRatio)(&mut out, value.numerator, value.denominator) },
        "daqRatio_createRatio",
    )?;
    unsafe { Ref::from_owned(out as *mut c_void) }
        .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, "daqRatio_createRatio", None))
}

pub(crate) fn complex_to_ref(value: Complex) -> Result<Ref> {
    let mut out: *mut sys::daqComplexNumber = std::ptr::null_mut();
    check(
        unsafe { (sys::api().daqComplexNumber_createComplexNumber)(&mut out, value.re, value.im) },
        "daqComplexNumber_createComplexNumber",
    )?;
    unsafe { Ref::from_owned(out as *mut c_void) }.ok_or_else(|| {
        Error::new(
            sys::OPENDAQ_ERR_GENERALERROR,
            "daqComplexNumber_createComplexNumber",
            None,
        )
    })
}

/// Box a [`Value`] into an owned openDAQ object reference; `Null` maps to
/// `None` (a null pointer argument).
pub(crate) fn to_daq(value: &Value) -> Result<Option<Ref>> {
    Ok(Some(match value {
        Value::Null => return Ok(None),
        Value::Bool(b) => bool_to_ref(*b)?,
        Value::Int(i) => int_to_ref(*i)?,
        Value::Float(f) => float_to_ref(*f)?,
        Value::Str(s) => crate::marshal::make_string(s)?,
        Value::Ratio(r) => ratio_to_ref(*r)?,
        Value::Complex(c) => complex_to_ref(*c)?,
        Value::List(items) => {
            let mut out: *mut sys::daqList = std::ptr::null_mut();
            check(
                unsafe { (sys::api().daqList_createList)(&mut out) },
                "daqList_createList",
            )?;
            let list = unsafe { Ref::from_owned(out as *mut c_void) }.ok_or_else(|| {
                Error::new(sys::OPENDAQ_ERR_GENERALERROR, "daqList_createList", None)
            })?;
            for item in items {
                let boxed = to_daq(item)?;
                check(
                    unsafe {
                        (sys::api().daqList_pushBack)(list.as_ptr() as *mut _, opt_ref_ptr(&boxed))
                    },
                    "daqList_pushBack",
                )?;
            }
            list
        }
        Value::Dict(pairs) => {
            let mut out: *mut sys::daqDict = std::ptr::null_mut();
            check(
                unsafe { (sys::api().daqDict_createDict)(&mut out) },
                "daqDict_createDict",
            )?;
            let dict = unsafe { Ref::from_owned(out as *mut c_void) }.ok_or_else(|| {
                Error::new(sys::OPENDAQ_ERR_GENERALERROR, "daqDict_createDict", None)
            })?;
            for (key, val) in pairs {
                let boxed_key = to_daq(key)?;
                let boxed_val = to_daq(val)?;
                check(
                    unsafe {
                        (sys::api().daqDict_set)(
                            dict.as_ptr() as *mut _,
                            opt_ref_ptr(&boxed_key),
                            opt_ref_ptr(&boxed_val),
                        )
                    },
                    "daqDict_set",
                )?;
            }
            dict
        }
        Value::Object(o) => o.0.clone(),
    }))
}

pub(crate) fn opt_ref_ptr(r: &Option<Ref>) -> *mut c_void {
    r.as_ref().map_or(std::ptr::null_mut(), |r| r.as_ptr())
}

/// Box a [`Value`] into a genuine `INumber` reference.  openDAQ's C ABI needs
/// a real INumber pointer where a Number is expected -- a raw IInteger or
/// IFloat pointer is not interface-compatible -- so the boxed scalar is
/// queried for INumber.  Only integers and floats (and objects implementing
/// INumber) qualify; `Null` maps to `None` (a null pointer argument).
pub(crate) fn to_daq_number(value: &Value) -> Result<Option<Ref>> {
    let op = "INumber conversion";
    let number_id = iface_id(sys::api().daqNumber_getInterfaceId);
    let boxed = match value {
        Value::Null => return Ok(None),
        Value::Int(i) => int_to_ref(*i)?,
        Value::Float(f) => float_to_ref(*f)?,
        Value::Object(o) => o.0.clone(),
        other => return Err(wrong_kind("a number", other)),
    };
    query_iface(boxed.as_ptr(), number_id, op).map(Some)
}

// ---------------------------------------------------------------------------
// Unboxing: openDAQ -> Rust
// ---------------------------------------------------------------------------

unsafe fn read_string_iface(iface: *mut c_void) -> Result<String> {
    let mut chars: *const std::ffi::c_char = std::ptr::null();
    check(
        (sys::api().daqString_getCharPtr)(iface as *mut sys::daqString, &mut chars),
        "daqString_getCharPtr",
    )?;
    if chars.is_null() {
        return Ok(String::new());
    }
    Ok(std::ffi::CStr::from_ptr(chars)
        .to_string_lossy()
        .into_owned())
}

unsafe fn unbox_list_iface(iface: *mut c_void, op: &'static str) -> Result<Vec<Value>> {
    let mut count: usize = 0;
    check(
        (sys::api().daqList_getCount)(iface as *mut sys::daqList, &mut count),
        op,
    )?;
    let mut items = Vec::with_capacity(count);
    for index in 0..count {
        let mut item: *mut c_void = std::ptr::null_mut();
        check(
            (sys::api().daqList_getItemAt)(iface as *mut sys::daqList, index, &mut item),
            op,
        )?;
        items.push(take_value(item, op)?);
    }
    Ok(items)
}

unsafe fn unbox_dict_iface(iface: *mut c_void, op: &'static str) -> Result<Vec<(Value, Value)>> {
    let mut keys: *mut sys::daqList = std::ptr::null_mut();
    check(
        (sys::api().daqDict_getKeyList)(iface as *mut sys::daqDict, &mut keys),
        op,
    )?;
    let keys_ref = Ref::from_owned(keys as *mut c_void)
        .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
    let mut count: usize = 0;
    check(
        (sys::api().daqList_getCount)(keys_ref.as_ptr() as *mut _, &mut count),
        op,
    )?;
    let mut pairs = Vec::with_capacity(count);
    for index in 0..count {
        let mut key: *mut c_void = std::ptr::null_mut();
        check(
            (sys::api().daqList_getItemAt)(keys_ref.as_ptr() as *mut _, index, &mut key),
            op,
        )?;
        let key_ref = Ref::from_owned(key);
        let mut val: *mut c_void = std::ptr::null_mut();
        check(
            (sys::api().daqDict_get)(
                iface as *mut sys::daqDict,
                key_ref
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |r| r.as_ptr()),
                &mut val,
            ),
            op,
        )?;
        let key_value = match &key_ref {
            Some(r) => unbox_ptr(r.as_ptr(), op)?,
            None => Value::Null,
        };
        pairs.push((key_value, take_value(val, op)?));
    }
    Ok(pairs)
}

/// The natural Rust form of the object at `ptr` (borrowed; no ownership is
/// taken): a boxed scalar yields its native value, a daq list/dict converts
/// recursively, anything else becomes an owning [`Value::Object`] wrapper.
pub(crate) unsafe fn unbox_ptr(ptr: *mut c_void, op: &'static str) -> Result<Value> {
    let api = sys::api();
    if let Some(iface) = borrow_iface(ptr, iface_id(api.daqString_getInterfaceId)) {
        return Ok(Value::Str(read_string_iface(iface)?));
    }
    if let Some(iface) = borrow_iface(ptr, iface_id(api.daqInteger_getInterfaceId)) {
        let mut v: i64 = 0;
        check(
            (api.daqInteger_getValue)(iface as *mut sys::daqInteger, &mut v),
            op,
        )?;
        return Ok(Value::Int(v));
    }
    if let Some(iface) = borrow_iface(ptr, iface_id(api.daqFloatObject_getInterfaceId)) {
        let mut v: f64 = 0.0;
        check(
            (api.daqFloatObject_getValue)(iface as *mut sys::daqFloatObject, &mut v),
            op,
        )?;
        return Ok(Value::Float(v));
    }
    if let Some(iface) = borrow_iface(ptr, iface_id(api.daqBoolean_getInterfaceId)) {
        let mut v: u8 = 0;
        check(
            (api.daqBoolean_getValue)(iface as *mut sys::daqBoolean, &mut v),
            op,
        )?;
        return Ok(Value::Bool(v != 0));
    }
    if let Some(iface) = borrow_iface(ptr, iface_id(api.daqRatio_getInterfaceId)) {
        let mut numerator: i64 = 0;
        let mut denominator: i64 = 1;
        check(
            (api.daqRatio_getNumerator)(iface as *mut sys::daqRatio, &mut numerator),
            op,
        )?;
        check(
            (api.daqRatio_getDenominator)(iface as *mut sys::daqRatio, &mut denominator),
            op,
        )?;
        return Ok(Value::Ratio(Ratio::new(numerator, denominator)));
    }
    if let Some(iface) = borrow_iface(ptr, iface_id(api.daqComplexNumber_getInterfaceId)) {
        let mut re: f64 = 0.0;
        let mut im: f64 = 0.0;
        check(
            (api.daqComplexNumber_getReal)(iface as *mut sys::daqComplexNumber, &mut re),
            op,
        )?;
        check(
            (api.daqComplexNumber_getImaginary)(iface as *mut sys::daqComplexNumber, &mut im),
            op,
        )?;
        return Ok(Value::Complex(Complex::new(re, im)));
    }
    if let Some(iface) = borrow_iface(ptr, iface_id(api.daqList_getInterfaceId)) {
        return Ok(Value::List(unbox_list_iface(iface, op)?));
    }
    if let Some(iface) = borrow_iface(ptr, iface_id(api.daqDict_getInterfaceId)) {
        return Ok(Value::Dict(unbox_dict_iface(iface, op)?));
    }
    let object = BaseObject(
        Ref::from_borrowed(ptr)
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_ARGUMENT_NULL, op, None))?,
    );
    Ok(Value::Object(object))
}

/// Unbox an *owned* reference (releasing it) into a [`Value`]; null is
/// [`Value::Null`].
pub(crate) unsafe fn take_value(ptr: *mut c_void, op: &'static str) -> Result<Value> {
    match Ref::from_owned(ptr) {
        None => Ok(Value::Null),
        Some(r) => unbox_ptr(r.as_ptr(), op),
    }
}

/// Read and release an owned `daqRatio*` out-value.
pub(crate) unsafe fn take_ratio(
    ptr: *mut sys::daqRatio,
    op: &'static str,
) -> Result<Option<Ratio>> {
    let Some(r) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(None);
    };
    let mut numerator: i64 = 0;
    let mut denominator: i64 = 1;
    check(
        (sys::api().daqRatio_getNumerator)(r.as_ptr() as *mut _, &mut numerator),
        op,
    )?;
    check(
        (sys::api().daqRatio_getDenominator)(r.as_ptr() as *mut _, &mut denominator),
        op,
    )?;
    Ok(Some(Ratio::new(numerator, denominator)))
}

/// Read and release an owned `daqComplexNumber*` out-value.
#[allow(dead_code)] // referenced by generated code only when the headers use it
pub(crate) unsafe fn take_complex(
    ptr: *mut sys::daqComplexNumber,
    op: &'static str,
) -> Result<Option<Complex>> {
    let Some(r) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(None);
    };
    let mut re: f64 = 0.0;
    let mut im: f64 = 0.0;
    check(
        (sys::api().daqComplexNumber_getReal)(r.as_ptr() as *mut _, &mut re),
        op,
    )?;
    check(
        (sys::api().daqComplexNumber_getImaginary)(r.as_ptr() as *mut _, &mut im),
        op,
    )?;
    Ok(Some(Complex::new(re, im)))
}

pub(crate) unsafe fn take_boxed_int(
    ptr: *mut sys::daqInteger,
    op: &'static str,
) -> Result<Option<i64>> {
    let Some(r) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(None);
    };
    let mut v: i64 = 0;
    check(
        (sys::api().daqInteger_getValue)(r.as_ptr() as *mut _, &mut v),
        op,
    )?;
    Ok(Some(v))
}

#[allow(dead_code)] // referenced by generated code only when the headers use it
pub(crate) unsafe fn take_boxed_float(
    ptr: *mut sys::daqFloatObject,
    op: &'static str,
) -> Result<Option<f64>> {
    let Some(r) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(None);
    };
    let mut v: f64 = 0.0;
    check(
        (sys::api().daqFloatObject_getValue)(r.as_ptr() as *mut _, &mut v),
        op,
    )?;
    Ok(Some(v))
}

pub(crate) unsafe fn take_boxed_bool(
    ptr: *mut sys::daqBoolean,
    op: &'static str,
) -> Result<Option<bool>> {
    let Some(r) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(None);
    };
    let mut v: u8 = 0;
    check(
        (sys::api().daqBoolean_getValue)(r.as_ptr() as *mut _, &mut v),
        op,
    )?;
    Ok(Some(v != 0))
}

/// Read and release an owned `daqNumber*` out-value as a float.
pub(crate) unsafe fn take_number(
    ptr: *mut sys::daqNumber,
    op: &'static str,
) -> Result<Option<f64>> {
    let Some(r) = Ref::from_owned(ptr as *mut c_void) else {
        return Ok(None);
    };
    let mut v: f64 = 0.0;
    check(
        (sys::api().daqNumber_getFloatValue)(r.as_ptr() as *mut _, &mut v),
        op,
    )?;
    Ok(Some(v))
}

// ---------------------------------------------------------------------------
// Typed element conversion (list / dict elements)
// ---------------------------------------------------------------------------

/// Conversion of an *owned* daq object reference into a typed Rust value,
/// used for list elements and dict keys/values whose type is statically
/// declared in the C headers.  Implemented for the scalar Rust types, for
/// [`Value`], and (by the generator) for every interface wrapper.
pub(crate) trait FromDaqOwned: Sized {
    unsafe fn from_daq_owned(ptr: *mut c_void, op: &'static str) -> Result<Self>;
}

impl FromDaqOwned for Value {
    unsafe fn from_daq_owned(ptr: *mut c_void, op: &'static str) -> Result<Value> {
        take_value(ptr, op)
    }
}

impl FromDaqOwned for String {
    unsafe fn from_daq_owned(ptr: *mut c_void, op: &'static str) -> Result<String> {
        let Some(r) = Ref::from_owned(ptr) else {
            return Ok(String::new());
        };
        let id = iface_id(sys::api().daqString_getInterfaceId);
        let iface = borrow_iface(r.as_ptr(), id).ok_or_else(|| {
            Error::new(
                sys::OPENDAQ_ERR_INVALIDTYPE,
                op,
                Some("element is not a string".into()),
            )
        })?;
        read_string_iface(iface)
    }
}

impl FromDaqOwned for i64 {
    unsafe fn from_daq_owned(ptr: *mut c_void, op: &'static str) -> Result<i64> {
        match take_value(ptr, op)? {
            Value::Int(v) => Ok(v),
            other => Err(wrong_kind("an integer", &other)),
        }
    }
}

impl FromDaqOwned for f64 {
    unsafe fn from_daq_owned(ptr: *mut c_void, op: &'static str) -> Result<f64> {
        let v = take_value(ptr, op)?;
        v.as_f64().ok_or_else(|| wrong_kind("a float", &v))
    }
}

impl FromDaqOwned for bool {
    unsafe fn from_daq_owned(ptr: *mut c_void, op: &'static str) -> Result<bool> {
        match take_value(ptr, op)? {
            Value::Bool(v) => Ok(v),
            other => Err(wrong_kind("a bool", &other)),
        }
    }
}

/// Convert an owned generic object reference into interface `T` (the interface
/// query adds its own reference; the original is released).
pub(crate) unsafe fn cast_owned<T: Interface>(ptr: *mut c_void, op: &'static str) -> Result<T> {
    let base = BaseObject::from_raw(ptr).ok_or_else(|| {
        Error::new(
            sys::OPENDAQ_ERR_ARGUMENT_NULL,
            op,
            Some("null element".into()),
        )
    })?;
    base.cast::<T>()
}

impl BaseObject {
    /// The natural Rust value of this object: a boxed scalar yields its
    /// native value, a daq list a `Vec`, a daq dict key/value pairs -- with
    /// elements converted recursively -- and an object with no natural Rust
    /// form (a device, a property object, ...) comes back as
    /// [`Value::Object`], for the caller to [`BaseObject::cast`].
    pub fn to_value(&self) -> Result<Value> {
        unsafe { unbox_ptr(self.as_raw(), "BaseObject::to_value") }
    }
}
