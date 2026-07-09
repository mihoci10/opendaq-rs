//! Port of the reference openDAQ bindings' high-level `coretypes` test suite
//! (plus the tiny `compile` suite), one `#[test]` per source test.
//!
//! Pure coretypes tests: no `opendaq::Instance` is created, so no
//! `common::instance_lock()` is needed.

mod common;

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use opendaq::{
    BaseObject, BinaryData, BooleanObject, ComplexNumberObject, CoreType, DictObject, Enumeration,
    EnumerationType, Event, EventArgs, EventHandler, FloatObject, FunctionObject, IntegerObject,
    Interface, ListObject, NumberObject, Procedure, PropertyObject, Ratio, RatioObject, SimpleType,
    StringObject, StructBuilder, StructType, TypeManager, Value, VersionInfo,
};

#[test]
fn high_level_coretypes_primitives() -> opendaq::Result<()> {
    let base_object = BaseObject::new()?;
    let wrapped_ratio = RatioObject::new(8, 12)?;
    let ratio = RatioObject::new(6, 9)?;
    let simplified = ratio.simplify()?.expect("simplified ratio");
    let boolean = BooleanObject::bool_object(false)?;
    let complex_number = ComplexNumberObject::new(1.0, 2.0)?;
    let integer = IntegerObject::new(1)?;
    let float_object = FloatObject::new(1.0)?;
    let simple_type = SimpleType::new(CoreType::Int)?;
    let version_info = VersionInfo::new(1, 2, 3)?;
    let binary_data = BinaryData::new(16)?;

    assert!(
        base_object.hash_code().is_ok(),
        "a fresh base object should be usable"
    );
    assert_eq!(
        wrapped_ratio.numerator()?,
        8,
        "ratios should preserve their numerator"
    );
    assert_eq!(
        wrapped_ratio.denominator()?,
        12,
        "ratios should preserve their denominator"
    );
    assert_eq!(ratio.numerator()?, 6);
    assert_eq!(ratio.denominator()?, 9);
    assert_eq!(
        simplified,
        Ratio::new(2, 3),
        "simplification should reduce the ratio"
    );
    assert!(
        !boolean.value()?,
        "boolean wrappers should decode false values"
    );
    assert_eq!(complex_number.real()?, 1.0);
    assert_eq!(complex_number.imaginary()?, 2.0);
    assert_eq!(integer.value()?, 1);
    assert_eq!(float_object.value()?, 1.0);
    // The string round-trip assertions live in high_level_compile_string_object.
    assert!(
        !simple_type.as_raw().is_null(),
        "simple types should create a native type object"
    );
    assert_eq!(version_info.major()?, 1);
    assert_eq!(version_info.minor()?, 2);
    assert_eq!(version_info.patch()?, 3);
    assert_eq!(
        binary_data.size()?,
        16,
        "binary data should preserve its buffer size"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_ratio_boxing() -> opendaq::Result<()> {
    // A native Rust Ratio passed as an argument boxes into a daqRatio, and a
    // daqRatio unboxes back into a native Ratio.
    let list = ListObject::new()?;
    list.push_back(Ratio::new(2, 3))?;
    assert_eq!(
        list.pop_front()?.as_ratio(),
        Some(Ratio::new(2, 3)),
        "a boxed daqRatio should unbox into a native Ratio"
    );

    let list = ListObject::new()?;
    list.push_back(Ratio::new(1, 4))?;
    list.push_back(Ratio::new(3, 8))?;
    let unboxed = list.to_value()?;
    let items = unboxed
        .as_list()
        .expect("a daq list should unbox into a Vec");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_ratio(), Some(Ratio::new(1, 4)));
    assert_eq!(items[1].as_ratio(), Some(Ratio::new(3, 8)));
    Ok(())
}

#[test]
fn high_level_coretypes_complex_boxing() -> opendaq::Result<()> {
    // A native Rust Complex boxes into a daqComplexNumber and unboxes back.
    let list = ListObject::new()?;
    list.push_back(opendaq::Complex::new(1.0, 2.0))?;
    match list.pop_front()? {
        Value::Complex(c) => assert_eq!(c, opendaq::Complex::new(1.0, 2.0)),
        other => panic!("expected a complex value, got {other:?}"),
    }

    let list = ListObject::new()?;
    list.push_back(opendaq::Complex::new(3.0, -4.0))?;
    let unboxed = list.to_value()?;
    let items = unboxed
        .as_list()
        .expect("a daq list should unbox into a Vec");
    assert_eq!(items.len(), 1);
    match &items[0] {
        Value::Complex(c) => assert_eq!(*c, opendaq::Complex::new(3.0, -4.0)),
        other => panic!("expected a complex element, got {other:?}"),
    }
    Ok(())
}

#[test]
fn high_level_coretypes_unbox() -> opendaq::Result<()> {
    // to_value discovers the value's type from the object itself at runtime.
    assert_eq!(IntegerObject::new(42)?.to_value()?.as_i64(), Some(42));
    assert_eq!(FloatObject::new(1.5)?.to_value()?.as_f64(), Some(1.5));
    assert_eq!(
        BooleanObject::bool_object(false)?.to_value()?.as_bool(),
        Some(false)
    );
    assert_eq!(
        RatioObject::new(1, 2)?.to_value()?.as_ratio(),
        Some(Ratio::new(1, 2))
    );

    // A generic base-object holding a boxed scalar unboxes with no cast.  The
    // Lisp test uses a boxed string here, but StringObject::new has a binding
    // bug (see high_level_compile_string_object); an integer-backed generic
    // wrapper exercises the same runtime type discovery.  The string variant
    // is asserted in the ignored test.
    let boxed = IntegerObject::new(7)?.to_base_object();
    assert_eq!(boxed.to_value()?.as_i64(), Some(7));

    // An already-native value is unchanged by the Value round-trip.
    assert_eq!(Value::from(42).as_i64(), Some(42));

    // An object with no natural Rust form: the Lisp UNBOX signals an error;
    // the Rust to_value instead keeps it as a Value::Object wrapper.
    let unboxed = PropertyObject::new()?.to_value()?;
    assert!(
        matches!(unboxed, Value::Object(_)),
        "an object with no natural Rust form should stay a wrapper"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_unbox_containers() -> opendaq::Result<()> {
    // to_value converts daq lists and dicts whole, unboxing each element by
    // its own runtime type; an element with no Rust form stays a wrapper.
    let list = ListObject::new()?;
    list.push_back(1)?;
    list.push_back("two")?;
    let unboxed = list.to_value()?;
    let items = unboxed.as_list().expect("list");
    assert_eq!(
        items.len(),
        2,
        "a daq list should convert element by element"
    );
    assert_eq!(items[0].as_i64(), Some(1));
    assert_eq!(items[1].as_str(), Some("two"));

    assert!(
        ListObject::new()?
            .to_value()?
            .as_list()
            .expect("list")
            .is_empty(),
        "an empty daq list should unbox to an empty Vec"
    );

    let dict = DictObject::new()?;
    dict.set("x", 10)?;
    dict.set("y", "twenty")?;
    let table = dict.to_value()?;
    assert_eq!(
        table.as_dict().expect("dict").len(),
        2,
        "a daq dict should convert whole"
    );
    assert_eq!(table.get("x").and_then(Value::as_i64), Some(10));
    assert_eq!(table.get("y").and_then(Value::as_str), Some("twenty"));

    let outer = ListObject::new()?;
    let inner = ListObject::new()?;
    inner.push_back(7)?;
    outer.push_back(&inner)?;
    let unboxed = outer.to_value()?;
    let outer_items = unboxed.as_list().expect("outer list");
    assert_eq!(outer_items.len(), 1);
    let inner_items = outer_items[0]
        .as_list()
        .expect("unboxing should recurse into nested lists");
    assert_eq!(inner_items.len(), 1);
    assert_eq!(inner_items[0].as_i64(), Some(7));

    let list = ListObject::new()?;
    list.push_back(&PropertyObject::new()?)?;
    let unboxed = list.to_value()?;
    let element = &unboxed.as_list().expect("list")[0];
    assert!(
        element
            .as_object()
            .expect("wrapper element")
            .is_a::<PropertyObject>(),
        "a list element with no Rust form should stay an openDAQ wrapper"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_as_cast_failure() -> opendaq::Result<()> {
    // cast is a real interface query, so casting to an interface the object
    // does not implement must fail -- not hand back a wrapper whose method
    // calls would dispatch through the wrong vtable.
    let boxed = IntegerObject::new(42)?;
    assert!(
        boxed.cast::<StringObject>().is_err(),
        "cast to an unsupported interface should fail with an openDAQ error"
    );
    assert_eq!(
        boxed.value()?,
        42,
        "a failed cast should leave the object intact and usable"
    );

    // INumber sits at a different vtable offset than IInteger, so the queried
    // wrapper must read correctly through its own interface pointer.
    let number = boxed.cast::<NumberObject>()?;
    assert_eq!(
        number.int_value()?,
        42,
        "cast to a secondary interface should yield a working wrapper"
    );

    // The Lisp variants "as with a keyword target" and "as on an already-
    // released object" exercise Lisp-only dynamic typing / manual release;
    // both misuses are impossible by construction in Rust.
    Ok(())
}

#[test]
fn high_level_coretypes_collections() -> opendaq::Result<()> {
    let list = ListObject::new()?;
    list.push_back(1)?;
    list.push_back(2)?;
    list.push_back(3)?;
    assert_eq!(
        list.count()?,
        3,
        "object lists should track the number of boxed elements"
    );
    assert_eq!(
        list.pop_front()?.as_i64(),
        Some(1),
        "pop_front should return the boxed value"
    );
    assert_eq!(
        list.remove_at(1)?.as_i64(),
        Some(3),
        "remove_at should return the boxed value"
    );
    list.clear()?;
    assert_eq!(list.count()?, 0, "object lists should support clear");

    let dict = DictObject::new()?;
    dict.set("key", "value")?;
    assert_eq!(
        dict.count()?,
        1,
        "dictionaries should track inserted entries"
    );
    assert_eq!(
        dict.get("key")?.as_str(),
        Some("value"),
        "dictionaries should return boxed values"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_enumeration_and_structs() -> opendaq::Result<()> {
    let enumerators = DictObject::new()?;
    let field_names = ListObject::new()?;
    let field_type = SimpleType::new(CoreType::Int)?;
    let field_types = ListObject::new()?;
    let type_manager = TypeManager::new()?;
    enumerators.set("One", 1)?;
    enumerators.set("Two", 2)?;
    field_names.push_back("int")?;
    field_types.push_back(&field_type)?;

    let enumeration_type = EnumerationType::with_values("MyEnum", &enumerators)?;
    let enumeration = Enumeration::with_type(&enumeration_type, "Two")?;
    let struct_type = StructType::no_defaults("test", &field_names, &field_types)?;
    type_manager.add_type(&struct_type)?;

    let managed_type = type_manager.type_("test")?.expect("managed type");
    let struct_builder = StructBuilder::new("test", &type_manager)?;
    struct_builder.set("int", 10)?;
    let struct_ = struct_builder.build()?.expect("built struct");
    let field_value = struct_.get("int")?;

    assert_eq!(
        enumeration_type.count()?,
        2,
        "enumeration types should report their enumerators"
    );
    assert_eq!(
        enumeration.int_value()?,
        2,
        "enumeration values should expose their numeric value"
    );
    assert!(
        type_manager.has_type("test")?,
        "type managers should track added struct types"
    );
    assert_eq!(
        managed_type.name()?,
        "test",
        "type managers should resolve added types by name"
    );
    assert_eq!(
        field_value.as_i64(),
        Some(10),
        "structs should preserve builder-assigned values"
    );
    type_manager.remove_type("test")?;
    assert!(
        !type_manager.has_type("test")?,
        "type managers should remove registered types"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_events() -> opendaq::Result<()> {
    let event = Event::new()?;
    let event_args = EventArgs::new(10, "test_event")?;
    let called = Arc::new(AtomicBool::new(false));
    let handler = EventHandler::from_fn({
        let called = called.clone();
        move |_sender, _args| called.store(true, Ordering::SeqCst)
    })?;
    let sender = BaseObject::new()?;

    assert_eq!(
        event.subscriber_count()?,
        0,
        "events should start without subscribers"
    );
    assert_eq!(
        event_args.event_id()?,
        10,
        "event args should expose their event id"
    );
    assert_eq!(
        event_args.event_name()?,
        "test_event",
        "event args should expose their name"
    );
    event.add_handler(&handler)?;
    assert_eq!(
        event.subscriber_count()?,
        1,
        "events should register event handlers"
    );
    handler.handle_event(&sender, &event_args)?;
    assert!(
        called.load(Ordering::SeqCst),
        "event handlers should invoke the supplied callback"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_event_handler_from_function() -> opendaq::Result<()> {
    // A plain Rust closure can be subscribed directly (no manual callback, no
    // ref releasing): sender and args arrive wrapped with references managed.
    let event = Event::new()?;
    let captured = Arc::new(Mutex::new(None::<String>));
    let handler = event.subscribe({
        let captured = captured.clone();
        move |_sender, args| {
            let args = args
                .expect("event args")
                .cast::<EventArgs>()
                .expect("EventArgs cast");
            *captured.lock().unwrap() = Some(args.event_name().expect("event name"));
        }
    })?;
    let sender = BaseObject::new()?;
    let event_args = EventArgs::new(42, "fn_event")?;

    assert_eq!(
        event.subscriber_count()?,
        1,
        "subscribe should register the closure handler"
    );
    handler.handle_event(&sender, &event_args)?;
    assert_eq!(
        captured.lock().unwrap().as_deref(),
        Some("fn_event"),
        "a subscribed closure should run with the wrapped event args"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_event_handler_routing() -> opendaq::Result<()> {
    // Distinct closure handlers must get distinct trampolines that route to
    // their own closures, and a slot freed by unsubscribe must be reusable.
    let event = Event::new()?;
    let a = Arc::new(AtomicI64::new(0));
    let b = Arc::new(AtomicBool::new(false));
    let handler_a = event.subscribe({
        let a = a.clone();
        move |_s, _args| a.store(1, Ordering::SeqCst)
    })?;
    let sender = BaseObject::new()?;
    let event_args = EventArgs::new(1, "e")?;
    event.subscribe({
        let b = b.clone();
        move |_s, _args| b.store(true, Ordering::SeqCst)
    })?;

    assert_eq!(
        event.subscriber_count()?,
        2,
        "both closure handlers should subscribe"
    );
    handler_a.handle_event(&sender, &event_args)?;
    assert_eq!(
        a.load(Ordering::SeqCst),
        1,
        "each closure handler should route to its own closure"
    );
    assert!(
        !b.load(Ordering::SeqCst),
        "the other handler must not be invoked"
    );

    event.unsubscribe(&handler_a)?;
    a.store(0, Ordering::SeqCst);
    let handler_c = event.subscribe({
        let a = a.clone();
        move |_s, _args| a.store(42, Ordering::SeqCst)
    })?;
    handler_c.handle_event(&sender, &event_args)?;
    assert_eq!(
        a.load(Ordering::SeqCst),
        42,
        "a handler subscribed after a removal (reusing a freed slot) should still work"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_procedure_from_lisp_function() -> opendaq::Result<()> {
    // A Rust closure can back an openDAQ Procedure directly: openDAQ invokes
    // it through a trampoline with the params decoded to Rust values.
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let procedure = Procedure::from_fn({
        let seen = seen.clone();
        move |args| {
            seen.lock().unwrap().push(args[0].clone());
            Ok(())
        }
    })?;
    procedure.dispatch_args(&[Value::from(5)])?;
    procedure.dispatch_args(&[Value::from("hello")])?;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].as_i64(),
        Some(5),
        "scalar params should arrive as native values"
    );
    assert_eq!(
        seen[1].as_str(),
        Some("hello"),
        "scalar params should arrive as native values"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_callable_params_conventions() -> opendaq::Result<()> {
    // openDAQ encodes callable params as null / single value / list of
    // values; each must decode to the natural Rust argument list.
    let calls = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let procedure = Procedure::from_fn({
        let calls = calls.clone();
        move |args| {
            calls.lock().unwrap().push(args.to_vec());
            Ok(())
        }
    })?;
    procedure.dispatch(Value::Null)?;
    let params = ListObject::new()?;
    params.push_back(1)?;
    params.push_back("two")?;
    procedure.dispatch(&params)?;

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[0].is_empty(),
        "null params should dispatch no arguments"
    );
    assert_eq!(
        calls[1].len(),
        2,
        "a daq list should dispatch one argument per element"
    );
    assert_eq!(calls[1][0].as_i64(), Some(1));
    assert_eq!(calls[1][1].as_str(), Some("two"));
    Ok(())
}

#[test]
fn high_level_coretypes_function_from_lisp_function() -> opendaq::Result<()> {
    // A closure-backed Function computes its result from the decoded
    // arguments; the result is boxed for the caller and unboxes back.
    let function = FunctionObject::from_fn(|args| {
        Ok(Value::from(
            args[0].as_i64().unwrap_or(0) + args[1].as_i64().unwrap_or(0),
        ))
    })?;
    assert_eq!(
        function.call(&[Value::from(2), Value::from(3)])?.as_i64(),
        Some(5),
        "a closure-backed function should box its computed result for the caller"
    );

    // A Vec result boxes into a daq list.
    let function = FunctionObject::from_fn(|_args| Ok(Value::from(vec![1i64, 2, 3])))?;
    let result = function.call(&[])?;
    let items = result
        .as_list()
        .expect("a Vec result should box into a daq list");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].as_i64(), Some(1));
    assert_eq!(items[1].as_i64(), Some(2));
    assert_eq!(items[2].as_i64(), Some(3));

    // A param with no scalar Rust form arrives wrapped, for the closure to cast.
    let function = FunctionObject::from_fn(|args| {
        let args = args[0]
            .as_object()
            .expect("wrapper param")
            .cast::<EventArgs>()?;
        Ok(Value::from(args.event_id()?))
    })?;
    let event_args = EventArgs::new(7, "x")?;
    assert_eq!(
        function.call(&[Value::from(&event_args)])?.as_i64(),
        Some(7),
        "a non-scalar param should arrive as a wrapper the closure can cast"
    );
    Ok(())
}

#[test]
fn high_level_coretypes_callable_error_propagation() -> opendaq::Result<()> {
    // An error raised inside the closure must not unwind across the C
    // boundary: it is reported as an error code, which the call site
    // surfaces as an opendaq::Error.
    fn boom() -> opendaq::Error {
        // Manufacture a genuine openDAQ error (a failing interface query).
        IntegerObject::new(1)
            .and_then(|i| i.cast::<StringObject>())
            .expect_err("the cast must fail")
    }

    let procedure = Procedure::from_fn(|_args| Err(boom()))?;
    assert!(
        procedure.dispatch_args(&[]).is_err(),
        "a closure error should surface to the caller"
    );

    let function = FunctionObject::from_fn(|_args| Err(boom()))?;
    assert!(
        function.call(&[]).is_err(),
        "a closure error should surface to the caller"
    );

    // A panic (the closest analogue of a signalled Lisp condition) is caught
    // at the boundary and reported as an error too.
    let panicking = Procedure::from_fn(|_args| panic!("boom"))?;
    assert!(
        panicking.dispatch_args(&[]).is_err(),
        "a panic should surface as an error"
    );
    Ok(())
}

// The Lisp test high-level-ratio-automatic-release probes GC finalizers
// (trivial-garbage weak pointers + release hooks).  Rust wrappers release
// their native reference deterministically on Drop, so there is no
// asynchronous finalization to probe; the test is Lisp-only and not ported.

// --- from compile.lisp (high-level-compile-suite) ---

#[test]
fn high_level_compile_string_object() -> opendaq::Result<()> {
    let string_object = StringObject::new("Hello, C bindings!")?;
    assert!(
        string_object.is_a::<StringObject>(),
        "compile coverage should construct a generated wrapper"
    );
    assert_eq!(
        string_object.length()?,
        18,
        "strings should expose their generated length accessor"
    );
    assert_eq!(
        string_object.char_ptr()?,
        "Hello, C bindings!",
        "strings should round-trip their native contents"
    );
    // From high-level-coretypes-primitives / high-level-coretypes-unbox: the
    // boxed-string assertions blocked by the same StringObject::new bug.
    assert_eq!(
        StringObject::new("test")?.char_ptr()?,
        "test",
        "strings should round-trip natively"
    );
    let boxed_string = StringObject::new("hello")?.to_base_object();
    assert_eq!(
        boxed_string.to_value()?.as_str(),
        Some("hello"),
        "a generic base-object holding a string should unbox with no cast"
    );
    // The Lisp test then releases explicitly and checks the raw pointer is
    // cleared; Rust wrappers release deterministically on Drop instead, and
    // expose no explicit release, so those two assertions are Lisp-only.
    Ok(())
}
