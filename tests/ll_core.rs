//! Low-level coverage of the raw FFI layer (`opendaq::sys`): coretypes,
//! coreobjects, and errors.  Each mechanism is exercised at least once through
//! the `Api` function-pointer table; it is not exhaustive.
//!
//! `opendaq::Error` is only ever constructed by the bindings, so the
//! error-reporting test triggers a real failure and checks its report contents
//! rather than building an error by hand.

mod common;

use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use opendaq::sys;

fn api() -> &'static sys::Api {
    sys::api()
}

fn failed(code: u32) -> bool {
    code & 0x8000_0000 != 0
}

#[track_caller]
fn ok(code: u32, what: &str) {
    assert!(!failed(code), "{what} failed with error code 0x{code:08X}");
}

/// RAII owner of one reference to a raw openDAQ object.
struct Obj(*mut c_void);

impl Obj {
    #[track_caller]
    fn new(ptr: *mut c_void, what: &str) -> Obj {
        assert!(!ptr.is_null(), "{what} returned a null object");
        Obj(ptr)
    }

    /// The pointer as the generic `daqBaseObject` (`c_void`).
    fn ptr(&self) -> *mut c_void {
        self.0
    }

    /// The pointer as a concrete interface type (openDAQ interface pointers
    /// share their leading virtual-table slots, so the reinterpretation the C
    /// tests perform is valid here too).
    fn cast<T>(&self) -> *mut T {
        self.0 as *mut T
    }
}

impl Drop for Obj {
    fn drop(&mut self) {
        unsafe { (api().daqBaseObject_releaseRef)(self.0) };
    }
}

fn make_string(text: &str) -> Obj {
    let c = CString::new(text).unwrap();
    let mut out: *mut sys::daqString = ptr::null_mut();
    ok(
        unsafe { (api().daqString_createString)(&mut out, c.as_ptr()) },
        "daqString_createString",
    );
    Obj::new(out as *mut c_void, "daqString_createString")
}

fn string_value(string: &Obj) -> String {
    let mut chars: *const c_char = ptr::null();
    ok(
        unsafe { (api().daqString_getCharPtr)(string.cast(), &mut chars) },
        "daqString_getCharPtr",
    );
    assert!(
        !chars.is_null(),
        "daqString_getCharPtr returned a null character pointer"
    );
    unsafe { CStr::from_ptr(chars) }
        .to_string_lossy()
        .into_owned()
}

fn make_int(value: i64) -> Obj {
    let mut out: *mut sys::daqInteger = ptr::null_mut();
    ok(
        unsafe { (api().daqInteger_createInteger)(&mut out, value) },
        "daqInteger_createInteger",
    );
    Obj::new(out as *mut c_void, "daqInteger_createInteger")
}

fn int_value(integer: &Obj) -> i64 {
    let mut value: i64 = 0;
    ok(
        unsafe { (api().daqInteger_getValue)(integer.cast(), &mut value) },
        "daqInteger_getValue",
    );
    value
}

fn make_bool(value: bool) -> Obj {
    let mut out: *mut sys::daqBoolean = ptr::null_mut();
    ok(
        unsafe { (api().daqBoolean_createBoolean)(&mut out, u8::from(value)) },
        "daqBoolean_createBoolean",
    );
    Obj::new(out as *mut c_void, "daqBoolean_createBoolean")
}

fn make_list() -> Obj {
    let mut out: *mut sys::daqList = ptr::null_mut();
    ok(
        unsafe { (api().daqList_createList)(&mut out) },
        "daqList_createList",
    );
    Obj::new(out as *mut c_void, "daqList_createList")
}

fn list_push_back(list: &Obj, item: &Obj) {
    ok(
        unsafe { (api().daqList_pushBack)(list.cast(), item.ptr()) },
        "daqList_pushBack",
    );
}

fn list_count(list: &Obj) -> usize {
    let mut count: usize = 0;
    ok(
        unsafe { (api().daqList_getCount)(list.cast(), &mut count) },
        "daqList_getCount",
    );
    count
}

fn make_dict() -> Obj {
    let mut out: *mut sys::daqDict = ptr::null_mut();
    ok(
        unsafe { (api().daqDict_createDict)(&mut out) },
        "daqDict_createDict",
    );
    Obj::new(out as *mut c_void, "daqDict_createDict")
}

fn dict_set(dict: &Obj, key: &Obj, value: &Obj) {
    ok(
        unsafe { (api().daqDict_set)(dict.cast(), key.ptr(), value.ptr()) },
        "daqDict_set",
    );
}

fn dict_count(dict: &Obj) -> usize {
    let mut count: usize = 0;
    ok(
        unsafe { (api().daqDict_getCount)(dict.cast(), &mut count) },
        "daqDict_getCount",
    );
    count
}

fn base_object_equals(a: &Obj, b: &Obj) -> bool {
    let mut equal: u8 = 0;
    ok(
        unsafe { (api().daqBaseObject_equals)(a.ptr(), b.ptr(), &mut equal) },
        "daqBaseObject_equals",
    );
    equal != 0
}

fn make_int_property(name: &Obj, default_value: &Obj, visible: &Obj) -> Obj {
    let mut out: *mut sys::daqProperty = ptr::null_mut();
    ok(
        unsafe {
            (api().daqProperty_createIntProperty)(
                &mut out,
                name.cast(),
                default_value.cast(),
                visible.cast(),
            )
        },
        "daqProperty_createIntProperty",
    );
    Obj::new(out as *mut c_void, "daqProperty_createIntProperty")
}

fn make_property_object() -> Obj {
    let mut out: *mut sys::daqPropertyObject = ptr::null_mut();
    ok(
        unsafe { (api().daqPropertyObject_createPropertyObject)(&mut out) },
        "daqPropertyObject_createPropertyObject",
    );
    Obj::new(out as *mut c_void, "daqPropertyObject_createPropertyObject")
}

// ---------------------------------------------------------------------------
// coretypes
// ---------------------------------------------------------------------------

#[test]
fn coretypes_base_object() {
    let mut obj: *mut sys::daqBaseObject = ptr::null_mut();
    ok(
        unsafe { (api().daqBaseObject_create)(&mut obj) },
        "daqBaseObject_create",
    );
    assert!(
        !obj.is_null(),
        "coretypes/BaseObject returned a null object"
    );
    let refcount = unsafe { (api().daqBaseObject_releaseRef)(obj) };
    assert_eq!(
        refcount, 0,
        "coretypes/BaseObject release refcount mismatch"
    );
}

#[test]
fn coretypes_binary_data() {
    let mut out: *mut sys::daqBinaryData = ptr::null_mut();
    ok(
        unsafe { (api().daqBinaryData_createBinaryData)(&mut out, 10) },
        "daqBinaryData_createBinaryData",
    );
    let binary_data = Obj::new(out as *mut c_void, "daqBinaryData_createBinaryData");
    let mut address: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqBinaryData_getAddress)(binary_data.cast(), &mut address) },
        "daqBinaryData_getAddress",
    );
    assert!(
        !address.is_null(),
        "coretypes/BinaryData returned a null data pointer"
    );
    let mut size: usize = 0;
    ok(
        unsafe { (api().daqBinaryData_getSize)(binary_data.cast(), &mut size) },
        "daqBinaryData_getSize",
    );
    assert_eq!(size, 10, "coretypes/BinaryData size mismatch");
}

#[test]
fn coretypes_boolean() {
    let boolean = make_bool(false);
    let mut value: u8 = 1;
    ok(
        unsafe { (api().daqBoolean_getValue)(boolean.cast(), &mut value) },
        "daqBoolean_getValue",
    );
    assert_eq!(value, 0, "coretypes/Boolean value mismatch");
}

#[test]
fn coretypes_complex_number() {
    let mut out: *mut sys::daqComplexNumber = ptr::null_mut();
    ok(
        unsafe { (api().daqComplexNumber_createComplexNumber)(&mut out, 1.0, 2.0) },
        "daqComplexNumber_createComplexNumber",
    );
    let complex = Obj::new(out as *mut c_void, "daqComplexNumber_createComplexNumber");
    let mut real: f64 = 0.0;
    ok(
        unsafe { (api().daqComplexNumber_getReal)(complex.cast(), &mut real) },
        "daqComplexNumber_getReal",
    );
    assert_eq!(real, 1.0, "coretypes/ComplexNumber real mismatch");
    let mut imaginary: f64 = 0.0;
    ok(
        unsafe { (api().daqComplexNumber_getImaginary)(complex.cast(), &mut imaginary) },
        "daqComplexNumber_getImaginary",
    );
    assert_eq!(imaginary, 2.0, "coretypes/ComplexNumber imaginary mismatch");
}

#[test]
fn coretypes_dictobject() {
    let key = make_string("key");
    let value = make_string("value");
    let dict = make_dict();
    dict_set(&dict, &key, &value);
    assert_eq!(dict_count(&dict), 1, "coretypes/DictObject count mismatch");
    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqDict_get)(dict.cast(), key.ptr(), &mut out) },
        "daqDict_get",
    );
    let value_copy = Obj::new(out, "daqDict_get");
    assert_eq!(
        string_value(&value_copy),
        "value",
        "coretypes/DictObject value mismatch"
    );
}

#[test]
fn coretypes_enumerations() {
    let enumerators = make_dict();
    let enum_one = make_int(1);
    let enum_two = make_int(2);
    let enum_one_name = make_string("One");
    let enum_two_name = make_string("Two");
    let enum_type_name = make_string("MyEnum");
    dict_set(&enumerators, &enum_one_name, &enum_one);
    dict_set(&enumerators, &enum_two_name, &enum_two);

    let mut out: *mut sys::daqEnumerationType = ptr::null_mut();
    ok(
        unsafe {
            (api().daqEnumerationType_createEnumerationTypeWithValues)(
                &mut out,
                enum_type_name.cast(),
                enumerators.cast(),
            )
        },
        "daqEnumerationType_createEnumerationTypeWithValues",
    );
    let enum_type = Obj::new(
        out as *mut c_void,
        "daqEnumerationType_createEnumerationTypeWithValues",
    );
    let mut count: usize = 0;
    ok(
        unsafe { (api().daqEnumerationType_getCount)(enum_type.cast(), &mut count) },
        "daqEnumerationType_getCount",
    );
    assert_eq!(count, 2, "coretypes/Enumerations count mismatch");

    let mut out: *mut sys::daqEnumeration = ptr::null_mut();
    ok(
        unsafe {
            (api().daqEnumeration_createEnumerationWithType)(
                &mut out,
                enum_type.cast(),
                enum_two_name.cast(),
            )
        },
        "daqEnumeration_createEnumerationWithType",
    );
    let enum_value = Obj::new(
        out as *mut c_void,
        "daqEnumeration_createEnumerationWithType",
    );
    let mut int_value: i64 = 0;
    ok(
        unsafe { (api().daqEnumeration_getIntValue)(enum_value.cast(), &mut int_value) },
        "daqEnumeration_getIntValue",
    );
    assert_eq!(int_value, 2, "coretypes/Enumerations value mismatch");
}

#[test]
fn coretypes_event() {
    let mut out: *mut sys::daqEvent = ptr::null_mut();
    ok(
        unsafe { (api().daqEvent_createEvent)(&mut out) },
        "daqEvent_createEvent",
    );
    let event = Obj::new(out as *mut c_void, "daqEvent_createEvent");
    let mut count: usize = 42;
    ok(
        unsafe { (api().daqEvent_getSubscriberCount)(event.cast(), &mut count) },
        "daqEvent_getSubscriberCount",
    );
    assert_eq!(count, 0, "coretypes/Event subscriber count mismatch");
}

#[test]
fn coretypes_event_args() {
    let event_name = make_string("test_event");
    let mut out: *mut sys::daqEventArgs = ptr::null_mut();
    ok(
        unsafe { (api().daqEventArgs_createEventArgs)(&mut out, 10, event_name.cast()) },
        "daqEventArgs_createEventArgs",
    );
    let event_args = Obj::new(out as *mut c_void, "daqEventArgs_createEventArgs");
    let mut id: i64 = 0;
    ok(
        unsafe { (api().daqEventArgs_getEventId)(event_args.cast(), &mut id) },
        "daqEventArgs_getEventId",
    );
    assert_eq!(id, 10, "coretypes/EventArgs id mismatch");
    let mut name: *mut sys::daqString = ptr::null_mut();
    ok(
        unsafe { (api().daqEventArgs_getEventName)(event_args.cast(), &mut name) },
        "daqEventArgs_getEventName",
    );
    let name = Obj::new(name as *mut c_void, "daqEventArgs_getEventName");
    assert_eq!(
        string_value(&name),
        "test_event",
        "coretypes/EventArgs name mismatch"
    );
}

static EVENT_CALLED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_event(sender: *mut sys::daqBaseObject, args: *mut sys::daqBaseObject) {
    EVENT_CALLED.store(true, Ordering::SeqCst);
    // The handler receives ownership of both references and releases them.
    (api().daqBaseObject_releaseRef)(sender);
    (api().daqBaseObject_releaseRef)(args);
}

#[test]
fn coretypes_event_handler() {
    EVENT_CALLED.store(false, Ordering::SeqCst);
    let mut sender: *mut sys::daqBaseObject = ptr::null_mut();
    ok(
        unsafe { (api().daqBaseObject_create)(&mut sender) },
        "daqBaseObject_create",
    );
    let sender = Obj::new(sender, "daqBaseObject_create");
    let mut args: *mut sys::daqBaseObject = ptr::null_mut();
    ok(
        unsafe { (api().daqBaseObject_create)(&mut args) },
        "daqBaseObject_create",
    );
    let args = Obj::new(args, "daqBaseObject_create");

    let mut out: *mut sys::daqEventHandler = ptr::null_mut();
    ok(
        unsafe { (api().daqEventHandler_createEventHandler)(&mut out, on_event) },
        "daqEventHandler_createEventHandler",
    );
    let handler = Obj::new(out as *mut c_void, "daqEventHandler_createEventHandler");
    ok(
        unsafe {
            (api().daqEventHandler_handleEvent)(
                handler.cast(),
                sender.ptr(),
                args.ptr() as *mut sys::daqEventArgs,
            )
        },
        "daqEventHandler_handleEvent",
    );
    assert!(
        EVENT_CALLED.load(Ordering::SeqCst),
        "coretypes/EventHandler callback was not invoked"
    );
}

#[test]
fn coretypes_float() {
    let mut out: *mut sys::daqFloatObject = ptr::null_mut();
    ok(
        unsafe { (api().daqFloatObject_createFloat)(&mut out, 1.0) },
        "daqFloatObject_createFloat",
    );
    let float_object = Obj::new(out as *mut c_void, "daqFloatObject_createFloat");
    let mut value: f64 = 0.0;
    ok(
        unsafe { (api().daqFloatObject_getValue)(float_object.cast(), &mut value) },
        "daqFloatObject_getValue",
    );
    assert_eq!(value, 1.0, "coretypes/Float value mismatch");
}

static FUNCTION_CALLED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn func_call(
    _params: *mut sys::daqBaseObject,
    _result: *mut *mut sys::daqBaseObject,
) -> sys::daqErrCode {
    FUNCTION_CALLED.store(true, Ordering::SeqCst);
    0
}

#[test]
fn coretypes_function() {
    FUNCTION_CALLED.store(false, Ordering::SeqCst);
    let mut out: *mut sys::daqFunction = ptr::null_mut();
    ok(
        unsafe { (api().daqFunction_createFunction)(&mut out, func_call) },
        "daqFunction_createFunction",
    );
    let function = Obj::new(out as *mut c_void, "daqFunction_createFunction");
    let mut params: *mut sys::daqBaseObject = ptr::null_mut();
    ok(
        unsafe { (api().daqBaseObject_create)(&mut params) },
        "daqBaseObject_create",
    );
    let params = Obj::new(params, "daqBaseObject_create");
    let mut result: *mut sys::daqBaseObject = ptr::null_mut();
    ok(
        unsafe { (api().daqFunction_call)(function.cast(), params.ptr(), &mut result) },
        "daqFunction_call",
    );
    if !result.is_null() {
        unsafe { (api().daqBaseObject_releaseRef)(result) };
    }
    assert!(
        FUNCTION_CALLED.load(Ordering::SeqCst),
        "coretypes/Function callback was not invoked"
    );
}

#[test]
fn coretypes_integer() {
    let integer = make_int(1);
    assert_eq!(int_value(&integer), 1, "coretypes/Integer value mismatch");
}

#[test]
fn coretypes_listobject() {
    let list = make_list();
    for value in [1, 2, 3] {
        let item = make_int(value);
        list_push_back(&list, &item);
    }
    assert_eq!(list_count(&list), 3, "coretypes/ListObject count mismatch");

    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqList_popFront)(list.cast(), &mut out) },
        "daqList_popFront",
    );
    let front = Obj::new(out, "daqList_popFront");
    assert_eq!(
        int_value(&front),
        1,
        "coretypes/ListObject pop-front mismatch"
    );

    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqList_removeAt)(list.cast(), 1, &mut out) },
        "daqList_removeAt",
    );
    let removed = Obj::new(out, "daqList_removeAt");
    assert_eq!(
        int_value(&removed),
        3,
        "coretypes/ListObject remove-at mismatch"
    );

    ok(
        unsafe { (api().daqList_clear)(list.cast()) },
        "daqList_clear",
    );
    assert_eq!(list_count(&list), 0, "coretypes/ListObject clear mismatch");
}

static PROCEDURE_CALLED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn proc_call(_params: *mut sys::daqBaseObject) -> sys::daqErrCode {
    PROCEDURE_CALLED.store(true, Ordering::SeqCst);
    0
}

#[test]
fn coretypes_procedure() {
    PROCEDURE_CALLED.store(false, Ordering::SeqCst);
    let mut out: *mut sys::daqProcedure = ptr::null_mut();
    ok(
        unsafe { (api().daqProcedure_createProcedure)(&mut out, proc_call) },
        "daqProcedure_createProcedure",
    );
    let procedure = Obj::new(out as *mut c_void, "daqProcedure_createProcedure");
    ok(
        unsafe { (api().daqProcedure_dispatch)(procedure.cast(), ptr::null_mut()) },
        "daqProcedure_dispatch",
    );
    assert!(
        PROCEDURE_CALLED.load(Ordering::SeqCst),
        "coretypes/Procedure callback was not invoked"
    );
}

#[test]
fn coretypes_ratio() {
    let mut out: *mut sys::daqRatio = ptr::null_mut();
    ok(
        unsafe { (api().daqRatio_createRatio)(&mut out, 1, 2) },
        "daqRatio_createRatio",
    );
    let ratio = Obj::new(out as *mut c_void, "daqRatio_createRatio");
    let mut numerator: i64 = 0;
    ok(
        unsafe { (api().daqRatio_getNumerator)(ratio.cast(), &mut numerator) },
        "daqRatio_getNumerator",
    );
    assert_eq!(numerator, 1, "coretypes/Ratio numerator mismatch");
    let mut denominator: i64 = 0;
    ok(
        unsafe { (api().daqRatio_getDenominator)(ratio.cast(), &mut denominator) },
        "daqRatio_getDenominator",
    );
    assert_eq!(denominator, 2, "coretypes/Ratio denominator mismatch");
}

fn make_simple_type(core_type: sys::CoreType) -> Obj {
    let mut out: *mut sys::daqSimpleType = ptr::null_mut();
    ok(
        unsafe { (api().daqSimpleType_createSimpleType)(&mut out, core_type as u32) },
        "daqSimpleType_createSimpleType",
    );
    Obj::new(out as *mut c_void, "daqSimpleType_createSimpleType")
}

#[test]
fn coretypes_simple_type() {
    let simple_type = make_simple_type(sys::CoreType::Bool);
    assert!(
        !simple_type.ptr().is_null(),
        "coretypes/SimpleType returned a null object"
    );
}

#[test]
fn coretypes_stringobject() {
    let string_object = make_string("Hello");
    assert_eq!(
        string_value(&string_object),
        "Hello",
        "coretypes/StringObject value mismatch"
    );
    let mut length: usize = 0;
    ok(
        unsafe { (api().daqString_getLength)(string_object.cast(), &mut length) },
        "daqString_getLength",
    );
    assert_eq!(length, 5, "coretypes/StringObject length mismatch");
}

/// Builds the one-int-field struct type shared by the struct and type-manager
/// tests, plus its name string and a fresh type manager it is registered in.
fn make_registered_struct_type() -> (Obj, Obj, Obj) {
    let field_names = make_list();
    let field_types = make_list();
    let field_name = make_string("int");
    let field_simple_type = make_simple_type(sys::CoreType::Int);
    list_push_back(&field_types, &field_simple_type);
    list_push_back(&field_names, &field_name);
    let struct_type_name = make_string("test");

    let mut out: *mut sys::daqStructType = ptr::null_mut();
    ok(
        unsafe {
            (api().daqStructType_createStructTypeNoDefaults)(
                &mut out,
                struct_type_name.cast(),
                field_names.cast(),
                field_types.cast(),
            )
        },
        "daqStructType_createStructTypeNoDefaults",
    );
    let struct_type = Obj::new(
        out as *mut c_void,
        "daqStructType_createStructTypeNoDefaults",
    );

    let mut out: *mut sys::daqTypeManager = ptr::null_mut();
    ok(
        unsafe { (api().daqTypeManager_createTypeManager)(&mut out) },
        "daqTypeManager_createTypeManager",
    );
    let type_manager = Obj::new(out as *mut c_void, "daqTypeManager_createTypeManager");
    ok(
        unsafe { (api().daqTypeManager_addType)(type_manager.cast(), struct_type.cast()) },
        "daqTypeManager_addType",
    );
    (struct_type_name, struct_type, type_manager)
}

#[test]
fn coretypes_struct() {
    let (struct_type_name, _struct_type, type_manager) = make_registered_struct_type();
    let field_name = make_string("int");
    let field_value = make_int(10);

    let mut out: *mut sys::daqStructBuilder = ptr::null_mut();
    ok(
        unsafe {
            (api().daqStructBuilder_createStructBuilder)(
                &mut out,
                struct_type_name.cast(),
                type_manager.cast(),
            )
        },
        "daqStructBuilder_createStructBuilder",
    );
    let struct_builder = Obj::new(out as *mut c_void, "daqStructBuilder_createStructBuilder");
    ok(
        unsafe {
            (api().daqStructBuilder_set)(
                struct_builder.cast(),
                field_name.cast(),
                field_value.ptr(),
            )
        },
        "daqStructBuilder_set",
    );
    let mut out: *mut sys::daqStruct = ptr::null_mut();
    ok(
        unsafe { (api().daqStructBuilder_build)(struct_builder.cast(), &mut out) },
        "daqStructBuilder_build",
    );
    let struct_object = Obj::new(out as *mut c_void, "daqStructBuilder_build");

    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqStruct_get)(struct_object.cast(), field_name.cast(), &mut out) },
        "daqStruct_get",
    );
    let field_value_copy = Obj::new(out, "daqStruct_get");
    assert_eq!(
        int_value(&field_value_copy),
        10,
        "coretypes/Struct field mismatch"
    );
}

#[test]
fn coretypes_type_manager() {
    let (struct_type_name, _struct_type, type_manager) = make_registered_struct_type();
    ok(
        unsafe { (api().daqTypeManager_removeType)(type_manager.cast(), struct_type_name.cast()) },
        "daqTypeManager_removeType",
    );
}

#[test]
fn coretypes_version_info() {
    let mut out: *mut sys::daqVersionInfo = ptr::null_mut();
    ok(
        unsafe { (api().daqVersionInfo_createVersionInfo)(&mut out, 1, 2, 3) },
        "daqVersionInfo_createVersionInfo",
    );
    let version_info = Obj::new(out as *mut c_void, "daqVersionInfo_createVersionInfo");
    let mut major: usize = 0;
    ok(
        unsafe { (api().daqVersionInfo_getMajor)(version_info.cast(), &mut major) },
        "daqVersionInfo_getMajor",
    );
    assert_eq!(major, 1, "coretypes/VersionInfo major mismatch");
    let mut minor: usize = 0;
    ok(
        unsafe { (api().daqVersionInfo_getMinor)(version_info.cast(), &mut minor) },
        "daqVersionInfo_getMinor",
    );
    assert_eq!(minor, 2, "coretypes/VersionInfo minor mismatch");
    let mut patch: usize = 0;
    ok(
        unsafe { (api().daqVersionInfo_getPatch)(version_info.cast(), &mut patch) },
        "daqVersionInfo_getPatch",
    );
    assert_eq!(patch, 3, "coretypes/VersionInfo patch mismatch");
}

// ---------------------------------------------------------------------------
// coreobjects
// ---------------------------------------------------------------------------

fn make_argument_info(name: &Obj, core_type: sys::CoreType) -> Obj {
    let mut out: *mut sys::daqArgumentInfo = ptr::null_mut();
    ok(
        unsafe {
            (api().daqArgumentInfo_createArgumentInfo)(&mut out, name.cast(), core_type as u32)
        },
        "daqArgumentInfo_createArgumentInfo",
    );
    Obj::new(out as *mut c_void, "daqArgumentInfo_createArgumentInfo")
}

#[test]
fn coreobjects_argument_info() {
    let name = make_string("test_argument");
    let arg_info = make_argument_info(&name, sys::CoreType::Int);
    let mut out: *mut sys::daqString = ptr::null_mut();
    ok(
        unsafe { (api().daqArgumentInfo_getName)(arg_info.cast(), &mut out) },
        "daqArgumentInfo_getName",
    );
    let name_out = Obj::new(out as *mut c_void, "daqArgumentInfo_getName");
    assert_eq!(
        string_value(&name_out),
        "test_argument",
        "coreobjects/ArgumentInfo name mismatch"
    );
    let mut type_out: u32 = u32::MAX;
    ok(
        unsafe { (api().daqArgumentInfo_getType)(arg_info.cast(), &mut type_out) },
        "daqArgumentInfo_getType",
    );
    assert_eq!(
        sys::CoreType::from_raw(type_out),
        Some(sys::CoreType::Int),
        "coreobjects/ArgumentInfo type mismatch"
    );
}

fn make_user(username: &Obj, password_hash: &Obj) -> Obj {
    let groups = make_list();
    let mut out: *mut sys::daqUser = ptr::null_mut();
    ok(
        unsafe {
            (api().daqUser_createUser)(
                &mut out,
                username.cast(),
                password_hash.cast(),
                groups.cast(),
            )
        },
        "daqUser_createUser",
    );
    Obj::new(out as *mut c_void, "daqUser_createUser")
}

#[test]
fn coreobjects_authentication_provider() {
    let username = make_string("test_user");
    let password_hash = make_string("test_hash");
    let user = make_user(&username, &password_hash);
    let user_list = make_list();
    list_push_back(&user_list, &user);

    let mut out: *mut sys::daqAuthenticationProvider = ptr::null_mut();
    ok(
        unsafe {
            (api().daqAuthenticationProvider_createStaticAuthenticationProvider)(
                &mut out,
                1,
                user_list.cast(),
            )
        },
        "daqAuthenticationProvider_createStaticAuthenticationProvider",
    );
    let provider = Obj::new(
        out as *mut c_void,
        "daqAuthenticationProvider_createStaticAuthenticationProvider",
    );

    let mut user_out: *mut sys::daqUser = ptr::null_mut();
    ok(
        unsafe {
            (api().daqAuthenticationProvider_authenticateAnonymous)(provider.cast(), &mut user_out)
        },
        "daqAuthenticationProvider_authenticateAnonymous",
    );
    drop(Obj::new(user_out as *mut c_void, "authenticate-anonymous"));

    let mut user_out: *mut sys::daqUser = ptr::null_mut();
    ok(
        unsafe {
            (api().daqAuthenticationProvider_authenticate)(
                provider.cast(),
                username.cast(),
                password_hash.cast(),
                &mut user_out,
            )
        },
        "daqAuthenticationProvider_authenticate",
    );
    drop(Obj::new(user_out as *mut c_void, "authenticate"));

    let mut user_out: *mut sys::daqUser = ptr::null_mut();
    ok(
        unsafe {
            (api().daqAuthenticationProvider_findUser)(
                provider.cast(),
                username.cast(),
                &mut user_out,
            )
        },
        "daqAuthenticationProvider_findUser",
    );
    drop(Obj::new(user_out as *mut c_void, "find-user"));
}

#[test]
fn coreobjects_callable_info() {
    let argument_info_list = make_list();
    let name = make_string("test_argument");
    let arg_info = make_argument_info(&name, sys::CoreType::Int);
    list_push_back(&argument_info_list, &arg_info);

    let mut out: *mut sys::daqCallableInfo = ptr::null_mut();
    ok(
        unsafe {
            (api().daqCallableInfo_createCallableInfo)(
                &mut out,
                argument_info_list.cast(),
                sys::CoreType::Int as u32,
                1,
            )
        },
        "daqCallableInfo_createCallableInfo",
    );
    let callable_info = Obj::new(out as *mut c_void, "daqCallableInfo_createCallableInfo");

    let mut const_flag: u8 = 0;
    ok(
        unsafe { (api().daqCallableInfo_isConst)(callable_info.cast(), &mut const_flag) },
        "daqCallableInfo_isConst",
    );
    assert_eq!(
        const_flag, 1,
        "coreobjects/CallableInfo const flag mismatch"
    );
    let mut return_type: u32 = u32::MAX;
    ok(
        unsafe { (api().daqCallableInfo_getReturnType)(callable_info.cast(), &mut return_type) },
        "daqCallableInfo_getReturnType",
    );
    assert_eq!(
        sys::CoreType::from_raw(return_type),
        Some(sys::CoreType::Int),
        "coreobjects/CallableInfo return type mismatch"
    );
    let mut arguments: *mut sys::daqList = ptr::null_mut();
    ok(
        unsafe { (api().daqCallableInfo_getArguments)(callable_info.cast(), &mut arguments) },
        "daqCallableInfo_getArguments",
    );
    let arguments = Obj::new(arguments as *mut c_void, "daqCallableInfo_getArguments");
    assert_eq!(
        list_count(&arguments),
        1,
        "coreobjects/CallableInfo arguments mismatch"
    );
}

#[test]
fn coreobjects_coercer() {
    let eval_str = make_string("value + 2");
    let mut out: *mut sys::daqCoercer = ptr::null_mut();
    ok(
        unsafe { (api().daqCoercer_createCoercer)(&mut out, eval_str.cast()) },
        "daqCoercer_createCoercer",
    );
    let coercer = Obj::new(out as *mut c_void, "daqCoercer_createCoercer");
    let value = make_int(10);
    let mut coerced: *mut c_void = ptr::null_mut();
    ok(
        unsafe {
            (api().daqCoercer_coerce)(coercer.cast(), ptr::null_mut(), value.ptr(), &mut coerced)
        },
        "daqCoercer_coerce",
    );
    let coerced_value = Obj::new(coerced, "daqCoercer_coerce");
    assert_eq!(
        int_value(&coerced_value),
        12,
        "coreobjects/Coercer value mismatch"
    );
}

static END_UPDATE_CALLED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_property_object_update_end(
    sender: *mut sys::daqBaseObject,
    args: *mut sys::daqBaseObject,
) {
    let mut properties: *mut sys::daqList = ptr::null_mut();
    let code = (api().daqEndUpdateEventArgs_getProperties)(
        args as *mut sys::daqEndUpdateEventArgs,
        &mut properties,
    );
    if !failed(code) && !properties.is_null() {
        let mut count: usize = usize::MAX;
        if !failed((api().daqList_getCount)(properties, &mut count)) && count == 0 {
            END_UPDATE_CALLED.store(true, Ordering::SeqCst);
        }
        (api().daqBaseObject_releaseRef)(properties as *mut c_void);
    }
    (api().daqBaseObject_releaseRef)(sender);
    (api().daqBaseObject_releaseRef)(args);
}

#[test]
fn coreobjects_end_update_event_args() {
    END_UPDATE_CALLED.store(false, Ordering::SeqCst);
    let prop_obj = make_property_object();
    let mut out: *mut sys::daqEvent = ptr::null_mut();
    ok(
        unsafe { (api().daqPropertyObject_getOnEndUpdate)(prop_obj.cast(), &mut out) },
        "daqPropertyObject_getOnEndUpdate",
    );
    let event = Obj::new(out as *mut c_void, "daqPropertyObject_getOnEndUpdate");
    let mut out: *mut sys::daqEventHandler = ptr::null_mut();
    ok(
        unsafe {
            (api().daqEventHandler_createEventHandler)(&mut out, on_property_object_update_end)
        },
        "daqEventHandler_createEventHandler",
    );
    let handler = Obj::new(out as *mut c_void, "daqEventHandler_createEventHandler");
    ok(
        unsafe { (api().daqEvent_addHandler)(event.cast(), handler.cast()) },
        "daqEvent_addHandler",
    );
    ok(
        unsafe { (api().daqPropertyObject_beginUpdate)(prop_obj.cast()) },
        "daqPropertyObject_beginUpdate",
    );
    ok(
        unsafe { (api().daqPropertyObject_endUpdate)(prop_obj.cast()) },
        "daqPropertyObject_endUpdate",
    );
    assert!(
        END_UPDATE_CALLED.load(Ordering::SeqCst),
        "coreobjects/EndUpdateEventArgs callback was not invoked"
    );
}

#[test]
fn coreobjects_eval_value() {
    let prop_obj = make_property_object();
    let name = make_string("test_property");
    let default_value = make_int(10);
    let visible = make_bool(true);
    let prop = make_int_property(&name, &default_value, &visible);
    ok(
        unsafe { (api().daqPropertyObject_addProperty)(prop_obj.cast(), prop.cast()) },
        "daqPropertyObject_addProperty",
    );

    let ref_name = make_string("ref_property");
    let eval_str = make_string("%test_property");
    let mut out: *mut sys::daqEvalValue = ptr::null_mut();
    ok(
        unsafe { (api().daqEvalValue_createEvalValue)(&mut out, eval_str.cast()) },
        "daqEvalValue_createEvalValue",
    );
    let eval_value = Obj::new(out as *mut c_void, "daqEvalValue_createEvalValue");
    let mut out: *mut sys::daqProperty = ptr::null_mut();
    ok(
        unsafe {
            (api().daqProperty_createReferenceProperty)(
                &mut out,
                ref_name.cast(),
                eval_value.cast(),
            )
        },
        "daqProperty_createReferenceProperty",
    );
    let ref_prop = Obj::new(out as *mut c_void, "daqProperty_createReferenceProperty");
    ok(
        unsafe { (api().daqPropertyObject_addProperty)(prop_obj.cast(), ref_prop.cast()) },
        "daqPropertyObject_addProperty",
    );

    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe {
            (api().daqPropertyObject_getPropertyValue)(prop_obj.cast(), ref_name.cast(), &mut out)
        },
        "daqPropertyObject_getPropertyValue",
    );
    let value = Obj::new(out, "daqPropertyObject_getPropertyValue");
    assert_eq!(
        int_value(&value),
        10,
        "coreobjects/EvalValue value mismatch"
    );
}

#[test]
fn coreobjects_property() {
    let name = make_string("test_property");
    let default_value = make_int(10);
    let visible = make_bool(true);
    let prop = make_int_property(&name, &default_value, &visible);

    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqProperty_getDefaultValue)(prop.cast(), &mut out) },
        "daqProperty_getDefaultValue",
    );
    let default_out = Obj::new(out, "daqProperty_getDefaultValue");
    assert_eq!(
        int_value(&default_out),
        10,
        "coreobjects/Property default value mismatch"
    );

    let mut out: *mut sys::daqString = ptr::null_mut();
    ok(
        unsafe { (api().daqProperty_getName)(prop.cast(), &mut out) },
        "daqProperty_getName",
    );
    let name_out = Obj::new(out as *mut c_void, "daqProperty_getName");
    assert_eq!(
        string_value(&name_out),
        "test_property",
        "coreobjects/Property name mismatch"
    );

    let mut visible_out: u8 = 0;
    ok(
        unsafe { (api().daqProperty_getVisible)(prop.cast(), &mut visible_out) },
        "daqProperty_getVisible",
    );
    assert_eq!(visible_out, 1, "coreobjects/Property visible mismatch");
}

#[test]
fn coreobjects_property_builder() {
    let name = make_string("test_property");
    let default_value = make_int(10);
    let visible = make_bool(true);

    let mut out: *mut sys::daqPropertyBuilder = ptr::null_mut();
    ok(
        unsafe {
            (api().daqPropertyBuilder_createIntPropertyBuilder)(
                &mut out,
                name.cast(),
                default_value.cast(),
            )
        },
        "daqPropertyBuilder_createIntPropertyBuilder",
    );
    let prop_builder = Obj::new(
        out as *mut c_void,
        "daqPropertyBuilder_createIntPropertyBuilder",
    );
    ok(
        unsafe { (api().daqPropertyBuilder_setVisible)(prop_builder.cast(), visible.cast()) },
        "daqPropertyBuilder_setVisible",
    );
    let mut out: *mut sys::daqProperty = ptr::null_mut();
    ok(
        unsafe { (api().daqPropertyBuilder_build)(prop_builder.cast(), &mut out) },
        "daqPropertyBuilder_build",
    );
    let property = Obj::new(out as *mut c_void, "daqPropertyBuilder_build");

    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqProperty_getDefaultValue)(property.cast(), &mut out) },
        "daqProperty_getDefaultValue",
    );
    let default_out = Obj::new(out, "daqProperty_getDefaultValue");
    assert_eq!(
        int_value(&default_out),
        10,
        "coreobjects/PropertyBuilder default value mismatch"
    );

    let mut out: *mut sys::daqString = ptr::null_mut();
    ok(
        unsafe { (api().daqProperty_getName)(property.cast(), &mut out) },
        "daqProperty_getName",
    );
    let name_out = Obj::new(out as *mut c_void, "daqProperty_getName");
    assert_eq!(
        string_value(&name_out),
        "test_property",
        "coreobjects/PropertyBuilder name mismatch"
    );

    let mut visible_out: u8 = 0;
    ok(
        unsafe { (api().daqProperty_getVisible)(property.cast(), &mut visible_out) },
        "daqProperty_getVisible",
    );
    assert_eq!(
        visible_out, 1,
        "coreobjects/PropertyBuilder visible mismatch"
    );
}

#[test]
fn coreobjects_property_object() {
    let prop_obj = make_property_object();
    let name = make_string("test_property");
    let default_value = make_int(10);
    let visible = make_bool(true);
    let prop = make_int_property(&name, &default_value, &visible);
    ok(
        unsafe { (api().daqPropertyObject_addProperty)(prop_obj.cast(), prop.cast()) },
        "daqPropertyObject_addProperty",
    );

    let mut out: *mut sys::daqProperty = ptr::null_mut();
    ok(
        unsafe { (api().daqPropertyObject_getProperty)(prop_obj.cast(), name.cast(), &mut out) },
        "daqPropertyObject_getProperty",
    );
    let prop_out = Obj::new(out as *mut c_void, "daqPropertyObject_getProperty");
    assert!(
        base_object_equals(&prop, &prop_out),
        "coreobjects/PropertyObject property mismatch"
    );

    let mut has: u8 = 0;
    ok(
        unsafe { (api().daqPropertyObject_hasProperty)(prop_obj.cast(), name.cast(), &mut has) },
        "daqPropertyObject_hasProperty",
    );
    assert_eq!(
        has, 1,
        "coreobjects/PropertyObject expected property before removal"
    );

    ok(
        unsafe { (api().daqPropertyObject_removeProperty)(prop_obj.cast(), name.cast()) },
        "daqPropertyObject_removeProperty",
    );
    let mut has: u8 = 1;
    ok(
        unsafe { (api().daqPropertyObject_hasProperty)(prop_obj.cast(), name.cast(), &mut has) },
        "daqPropertyObject_hasProperty",
    );
    assert_eq!(
        has, 0,
        "coreobjects/PropertyObject expected property removal"
    );
}

#[test]
fn coreobjects_property_object_class() {
    let name = make_string("test_property_class");
    let mut out: *mut sys::daqPropertyObjectClassBuilder = ptr::null_mut();
    ok(
        unsafe {
            (api().daqPropertyObjectClassBuilder_createPropertyObjectClassBuilder)(
                &mut out,
                name.cast(),
            )
        },
        "daqPropertyObjectClassBuilder_createPropertyObjectClassBuilder",
    );
    let builder = Obj::new(
        out as *mut c_void,
        "daqPropertyObjectClassBuilder_createPropertyObjectClassBuilder",
    );

    let prop_name = make_string("test_property");
    let default_value = make_int(10);
    let visible = make_bool(true);
    let prop = make_int_property(&prop_name, &default_value, &visible);
    ok(
        unsafe { (api().daqPropertyObjectClassBuilder_addProperty)(builder.cast(), prop.cast()) },
        "daqPropertyObjectClassBuilder_addProperty",
    );

    let mut out: *mut sys::daqPropertyObjectClass = ptr::null_mut();
    ok(
        unsafe { (api().daqPropertyObjectClassBuilder_build)(builder.cast(), &mut out) },
        "daqPropertyObjectClassBuilder_build",
    );
    let prop_obj_class = Obj::new(out as *mut c_void, "daqPropertyObjectClassBuilder_build");

    let mut out: *mut sys::daqProperty = ptr::null_mut();
    ok(
        unsafe {
            (api().daqPropertyObjectClass_getProperty)(
                prop_obj_class.cast(),
                prop_name.cast(),
                &mut out,
            )
        },
        "daqPropertyObjectClass_getProperty",
    );
    let prop_out = Obj::new(out as *mut c_void, "daqPropertyObjectClass_getProperty");
    assert!(
        base_object_equals(&prop, &prop_out),
        "coreobjects/PropertyObjectClass property mismatch"
    );
}

#[test]
fn coreobjects_property_value_event_args() {
    let name = make_string("test_property");
    let default_value = make_int(10);
    let visible = make_bool(true);
    let prop = make_int_property(&name, &default_value, &visible);
    let value1 = make_int(20);
    let value2 = make_int(30);

    let mut out: *mut sys::daqPropertyValueEventArgs = ptr::null_mut();
    ok(
        unsafe {
            (api().daqPropertyValueEventArgs_createPropertyValueEventArgs)(
                &mut out,
                prop.cast(),
                value2.ptr(),
                value1.ptr(),
                sys::PropertyEventType::Update as u32,
                0,
            )
        },
        "daqPropertyValueEventArgs_createPropertyValueEventArgs",
    );
    let event_args = Obj::new(
        out as *mut c_void,
        "daqPropertyValueEventArgs_createPropertyValueEventArgs",
    );

    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqPropertyValueEventArgs_getValue)(event_args.cast(), &mut out) },
        "daqPropertyValueEventArgs_getValue",
    );
    let value_out = Obj::new(out, "daqPropertyValueEventArgs_getValue");
    assert!(
        base_object_equals(&value_out, &value2),
        "coreobjects/PropertyValueEventArgs value mismatch"
    );

    let mut out: *mut c_void = ptr::null_mut();
    ok(
        unsafe { (api().daqPropertyValueEventArgs_getOldValue)(event_args.cast(), &mut out) },
        "daqPropertyValueEventArgs_getOldValue",
    );
    let old_value_out = Obj::new(out, "daqPropertyValueEventArgs_getOldValue");
    assert!(
        base_object_equals(&old_value_out, &value1),
        "coreobjects/PropertyValueEventArgs old value mismatch"
    );
}

#[test]
fn coreobjects_unit() {
    let name = make_string("test_unit");
    let symbol = make_string("tu");
    let mut out: *mut sys::daqUnitBuilder = ptr::null_mut();
    ok(
        unsafe { (api().daqUnitBuilder_createUnitBuilder)(&mut out) },
        "daqUnitBuilder_createUnitBuilder",
    );
    let unit_builder = Obj::new(out as *mut c_void, "daqUnitBuilder_createUnitBuilder");
    ok(
        unsafe { (api().daqUnitBuilder_setName)(unit_builder.cast(), name.cast()) },
        "daqUnitBuilder_setName",
    );
    ok(
        unsafe { (api().daqUnitBuilder_setSymbol)(unit_builder.cast(), symbol.cast()) },
        "daqUnitBuilder_setSymbol",
    );
    let mut out: *mut sys::daqUnit = ptr::null_mut();
    ok(
        unsafe { (api().daqUnitBuilder_build)(unit_builder.cast(), &mut out) },
        "daqUnitBuilder_build",
    );
    let unit = Obj::new(out as *mut c_void, "daqUnitBuilder_build");

    let mut out: *mut sys::daqString = ptr::null_mut();
    ok(
        unsafe { (api().daqUnit_getName)(unit.cast(), &mut out) },
        "daqUnit_getName",
    );
    let name_out = Obj::new(out as *mut c_void, "daqUnit_getName");
    assert_eq!(
        string_value(&name_out),
        "test_unit",
        "coreobjects/Unit name mismatch"
    );

    let mut out: *mut sys::daqString = ptr::null_mut();
    ok(
        unsafe { (api().daqUnit_getSymbol)(unit.cast(), &mut out) },
        "daqUnit_getSymbol",
    );
    let symbol_out = Obj::new(out as *mut c_void, "daqUnit_getSymbol");
    assert_eq!(
        string_value(&symbol_out),
        "tu",
        "coreobjects/Unit symbol mismatch"
    );
}

#[test]
fn coreobjects_user() {
    let username = make_string("test_user");
    let password_hash = make_string("test_hash");
    let user = make_user(&username, &password_hash);
    let mut out: *mut sys::daqString = ptr::null_mut();
    ok(
        unsafe { (api().daqUser_getUsername)(user.cast(), &mut out) },
        "daqUser_getUsername",
    );
    let username_out = Obj::new(out as *mut c_void, "daqUser_getUsername");
    assert_eq!(
        string_value(&username_out),
        "test_user",
        "coreobjects/User username mismatch"
    );
}

#[test]
fn coreobjects_validator() {
    let eval_str = make_string("value > 5");
    let mut out: *mut sys::daqValidator = ptr::null_mut();
    ok(
        unsafe { (api().daqValidator_createValidator)(&mut out, eval_str.cast()) },
        "daqValidator_createValidator",
    );
    let validator = Obj::new(out as *mut c_void, "daqValidator_createValidator");

    let value = make_int(10);
    ok(
        unsafe { (api().daqValidator_validate)(validator.cast(), ptr::null_mut(), value.ptr()) },
        "daqValidator_validate",
    );

    let invalid_value = make_int(3);
    let code = unsafe {
        (api().daqValidator_validate)(validator.cast(), ptr::null_mut(), invalid_value.ptr())
    };
    assert!(
        failed(code),
        "coreobjects/Validator expected a validation error for the invalid value"
    );

    // At the FFI level, the failing call stores per-thread error info
    // retrievable through daqGetErrorInfoMessage.
    let mut message: *mut sys::daqString = ptr::null_mut();
    let stored = unsafe { (api().daqGetErrorInfoMessage)(&mut message) };
    if !failed(stored) && !message.is_null() {
        let message = Obj::new(message as *mut c_void, "daqGetErrorInfoMessage");
        assert!(
            !string_value(&message).is_empty(),
            "the stored validation error message should not be empty"
        );
    }
    unsafe { (api().daqClearErrorInfo)() };
}

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

#[test]
fn opendaq_error_report_includes_code_name_and_message() {
    // The code-to-name table is directly checkable here...
    assert_eq!(
        sys::error_code_name(0x8000_000A),
        Some("OPENDAQ_ERR_ALREADYEXISTS"),
        "the error code table should name 0x8000000A"
    );
    // ...and opendaq::Error is only constructed by the bindings, so trigger a
    // real failure and make the same assertions about its report.
    let object = opendaq::PropertyObject::new().expect("creating a property object should succeed");
    let err = object
        .property_value("no_such_property")
        .expect_err("looking up a missing property should fail");
    assert!(
        failed(err.code()),
        "the error should carry a failure status code"
    );
    let report = err.to_string();
    let name = err
        .name()
        .expect("the failure code should have a symbolic OPENDAQ_ERR_* name");
    assert!(
        report.contains(name),
        "the error report should include the code name, not 'an unknown error': {report}"
    );
    let message = err
        .message()
        .expect("the error should carry a descriptive message");
    assert!(!message.is_empty(), "the error message should not be empty");
    assert!(
        report.contains(message),
        "the error report should include the descriptive message: {report}"
    );
    opendaq::clear_error_info();
}

#[test]
fn duplicating_device_reports_readable_error() {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let root_device = instance
        .root_device()
        .expect("root_device failed")
        .expect("no root device");
    root_device
        .add_device("daqref://device1")
        .expect("adding the device the first time should succeed")
        .expect("no device added");
    let err = root_device
        .add_device("daqref://device1")
        .expect_err("adding the same device twice should fail");
    assert!(
        err.name().is_some(),
        "the duplicate-device code should map to a known OPENDAQ_ERR_* name"
    );
    let message = err.message().expect("the error should have a message");
    assert!(!message.is_empty(), "the error message should not be empty");
    opendaq::clear_error_info();
}
