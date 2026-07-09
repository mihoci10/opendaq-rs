//! High-level tests for core objects: properties, property objects and
//! builders, coercers/validators, permissions, users, and eval values.
//!
//! Pure coreobjects tests: no `opendaq::Instance` is created, so no
//! `common::instance_lock()` is needed.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use opendaq::{
    ArgumentInfo, AuthenticationProvider, CallableInfo, Coercer, CoreType, DataRule, DataRuleType,
    DictObject, EndUpdateEventArgs, EvalValue, FunctionObject, ListObject, Property,
    PropertyBuilder, PropertyEventType, PropertyObject, PropertyObjectClassBuilder,
    PropertyValueEventArgs, Ratio, User, Validator, Value,
};

#[test]
fn high_level_coreobjects_argument_and_callable_info() -> opendaq::Result<()> {
    let argument_info = ArgumentInfo::new("test_argument", CoreType::Int)?;
    let arguments = ListObject::new()?;
    arguments.push_back(&argument_info)?;
    let callable_info = CallableInfo::new(&arguments, CoreType::Int, true)?;

    assert_eq!(
        argument_info.name()?,
        "test_argument",
        "argument info should expose its name"
    );
    assert_eq!(
        argument_info.type_()?,
        CoreType::Int,
        "argument info should expose its core type"
    );
    assert_eq!(
        callable_info.arguments()?.len(),
        1,
        "callable info should preserve its arguments"
    );
    assert_eq!(
        callable_info.return_type()?,
        CoreType::Int,
        "callable info should expose its return type"
    );
    assert!(
        callable_info.is_const()?,
        "callable info should preserve the const flag"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_function_property_from_closure() -> opendaq::Result<()> {
    // End to end: a FUNC property whose value is a closure-backed Function
    // reads back as a callable wrapper, and a call round-trips
    // Rust -> openDAQ -> Rust (args boxed by the caller, decoded by the
    // trampoline; the result boxed by the closure side, unboxed by the caller).
    let arguments = ListObject::new()?;
    arguments.push_back(&ArgumentInfo::new("a", CoreType::Int)?)?;
    arguments.push_back(&ArgumentInfo::new("b", CoreType::Int)?)?;
    let object = PropertyObject::new()?;
    let callable_info = CallableInfo::new(&arguments, CoreType::Int, false)?;
    object.add_property(&Property::function("Sum", &callable_info, true)?)?;

    let sum_fn = FunctionObject::from_fn(|args| {
        Ok(Value::from(
            args[0].as_i64().unwrap_or(0) + args[1].as_i64().unwrap_or(0),
        ))
    })?;
    object.set_property_value("Sum", &sum_fn)?;

    let sum = object
        .property_value("Sum")?
        .into_object()
        .expect("a FUNC property should read back as an object wrapper")
        .cast::<FunctionObject>()?;
    assert_eq!(
        sum.call(&[Value::from(2), Value::from(3)])?.as_i64(),
        Some(5),
        "calling it should round-trip the arguments and result through openDAQ"
    );
    assert_eq!(
        sum.call(&[Value::from(40), Value::from(2)])?.as_i64(),
        Some(42),
        "the round-tripping function should be reusable across calls"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_authentication_provider() -> opendaq::Result<()> {
    let groups = ListObject::new()?;
    groups.push_back("guest")?;
    let user = User::new("test_user", "test_hash", &groups)?;
    let users = ListObject::new()?;
    users.push_back(&user)?;

    let authentication_provider = AuthenticationProvider::static_(true, &users)?;
    let anonymous_user = authentication_provider
        .authenticate_anonymous()?
        .expect("anonymous user");
    let authenticated_user = authentication_provider
        .authenticate("test_user", "test_hash")?
        .expect("authenticated user");
    let found_user = authentication_provider
        .find_user("test_user")?
        .expect("found user");
    let user_groups = user.groups()?;

    assert_eq!(
        user.username()?,
        "test_user",
        "users should expose their username"
    );
    // openDAQ adds the implicit "everyone" group alongside "guest".
    assert!(
        user_groups.iter().any(|g| g == "guest"),
        "users should expose their groups as native strings (got {user_groups:?})"
    );
    assert!(
        anonymous_user.username().is_ok(),
        "providers should synthesize a working anonymous user when enabled"
    );
    assert_eq!(
        authenticated_user.username()?,
        "test_user",
        "providers should authenticate known users"
    );
    assert_eq!(
        found_user.username()?,
        "test_user",
        "providers should resolve users by name"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_property_builders() -> opendaq::Result<()> {
    let property = Property::int("test_property", 10, true)?;
    let property_builder = PropertyBuilder::int("test_property", 10)?;
    property_builder.set_visible(true)?;

    let built_property = property_builder.build()?.expect("built property");
    let built_default = built_property.default_value()?;
    let property_object = PropertyObject::new()?;
    let property_class_builder = PropertyObjectClassBuilder::new("test_property_class")?;
    property_object.add_property(&property)?;
    property_class_builder.add_property(&property)?;

    let property_object_property = property_object
        .property("test_property")?
        .expect("property");
    let property_default = property_object_property.default_value()?;
    let property_class = property_class_builder.build()?.expect("property class");
    let class_property = property_class
        .property("test_property")?
        .expect("class property");

    assert_eq!(
        property.name()?,
        "test_property",
        "factories should preserve the property name"
    );
    assert_eq!(
        built_default.as_i64(),
        Some(10),
        "builders should preserve their default value"
    );
    assert!(
        built_property.visible()?,
        "built properties should expose their visibility"
    );
    assert!(
        property_object.has_property("test_property")?,
        "property objects should contain added properties"
    );
    assert_eq!(
        property_default.as_i64(),
        Some(10),
        "property objects should expose the property's default value"
    );
    assert!(
        property_class.has_property("test_property")?,
        "class builders should contain added properties"
    );
    assert_eq!(
        class_property.name()?,
        "test_property",
        "classes should expose their class property by name"
    );
    property_object.remove_property("test_property")?;
    assert!(
        !property_object.has_property("test_property")?,
        "property objects should remove properties"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_factory_proxies_plain_values() -> opendaq::Result<()> {
    let int_property = Property::int("test_property", 10, true)?;
    let float_property = Property::float("test_float", 1.5, true)?;
    let bool_property = Property::bool("test_bool", false, true)?;
    let linear_rule = DataRule::linear(2, 0)?;

    assert!(
        int_property.is_a::<Property>(),
        "an int property should implement the Property interface"
    );
    assert_eq!(
        int_property.default_value()?.as_i64(),
        Some(10),
        "Property::int should box a plain integer default value"
    );
    assert_eq!(
        float_property.default_value()?.as_f64(),
        Some(1.5),
        "Property::float should box a plain float default value"
    );
    assert_eq!(
        bool_property.default_value()?.as_bool(),
        Some(false),
        "Property::bool should box a plain boolean default value"
    );
    assert_eq!(
        linear_rule.type_()?,
        DataRuleType::Linear,
        "DataRule::linear should accept plain numeric arguments and build a data rule"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_property_value_unboxes_scalars() -> opendaq::Result<()> {
    // property_value converts a scalar property to its native Rust value, so
    // the caller needs no cast/unbox; a non-scalar (here an object property)
    // has no single value to unbox and is handed back as the raw wrapper.
    let property_object = PropertyObject::new()?;
    property_object.add_property(&Property::int("anint", 10, true)?)?;
    property_object.add_property(&Property::float("afloat", 1.5, true)?)?;
    property_object.add_property(&Property::string("astring", "hi", true)?)?;
    property_object.add_property(&Property::bool("abool", true, true)?)?;
    property_object.add_property(&Property::ratio("aratio", Ratio::new(1, 2), true)?)?;
    property_object.add_property(&Property::object("anobject", &PropertyObject::new()?)?)?;

    assert_eq!(
        property_object.property_value("anint")?.as_i64(),
        Some(10),
        "a scalar INT property should come back as a native integer"
    );
    assert_eq!(
        property_object.property_value("afloat")?.as_f64(),
        Some(1.5),
        "a scalar FLOAT property should come back as a native float"
    );
    assert_eq!(
        property_object.property_value("astring")?.as_str(),
        Some("hi"),
        "a scalar STRING property should come back as a native string"
    );
    assert_eq!(
        property_object.property_value("abool")?.as_bool(),
        Some(true),
        "a scalar BOOL property should come back as a native boolean"
    );
    assert_eq!(
        property_object.property_value("aratio")?.as_ratio(),
        Some(Ratio::new(1, 2)),
        "a scalar RATIO property should come back as a native ratio"
    );
    let object_value = property_object.property_value("anobject")?;
    assert!(
        object_value
            .as_object()
            .expect("wrapper value")
            .is_a::<PropertyObject>(),
        "an OBJECT property has no scalar value to unbox, so it stays a daq wrapper"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_eval_coercer_validator() -> opendaq::Result<()> {
    let property_object = PropertyObject::new()?;
    let property = Property::int("test_property", 10, true)?;
    let coercer = Coercer::new("value + 2")?;
    let validator = Validator::new("value > 5")?;
    property_object.add_property(&property)?;

    let eval_value = EvalValue::new("%test_property")?;
    property_object.add_property(&Property::reference("ref_property", &eval_value)?)?;

    let reference_property = property_object.property_value("ref_property")?;
    let coerced_value = coercer.coerce(&property_object, 10)?;

    assert_eq!(
        reference_property.as_i64(),
        Some(10),
        "eval-value references should resolve through property_value, unboxed to a native value"
    );
    assert_eq!(
        coercer.eval()?,
        "value + 2",
        "coercers should expose their configured expression"
    );
    assert_eq!(
        coerced_value.as_i64(),
        Some(12),
        "coercers should transform the value"
    );
    assert!(
        validator.validate(&property_object, 10).is_ok(),
        "validators should accept values that satisfy the expression"
    );
    assert!(
        validator.validate(&property_object, 5).is_err(),
        "validators should fail with an openDAQ error for invalid values"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_property_value_event_args() -> opendaq::Result<()> {
    let property = Property::int("test_property", 10, true)?;
    let event_args =
        PropertyValueEventArgs::new(&property, 30, 20, PropertyEventType::Update, false)?;
    let event_value = event_args.value()?;
    let event_old_value = event_args.old_value()?;

    assert_eq!(
        event_args.property()?.expect("property").name()?,
        "test_property",
        "property-value event args should expose their property"
    );
    assert_eq!(
        event_value.as_i64(),
        Some(30),
        "event args should expose the new value"
    );
    assert_eq!(
        event_old_value.as_i64(),
        Some(20),
        "event args should expose the previous value"
    );
    assert_eq!(
        event_args.property_event_type()?,
        PropertyEventType::Update,
        "event args should preserve the event type"
    );
    assert!(
        !event_args.is_updating()?,
        "event args should decode false updating flags"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_unified_optional_generic() -> opendaq::Result<()> {
    // `property` is exposed as two distinct inherent methods -- a zero-arg
    // form on property-value-event-args and an extra-arg form on
    // property-object -- so calling either with the wrong arity is a compile
    // error, not a runtime one.
    let property = Property::int("test_property", 10, true)?;
    let property_object = PropertyObject::new()?;
    let event_args =
        PropertyValueEventArgs::new(&property, 30, 20, PropertyEventType::Update, false)?;
    property_object.add_property(&property)?;

    assert_eq!(
        property_object
            .property("test_property")?
            .expect("property")
            .name()?,
        "test_property",
        "PropertyObject::property should accept the property-name argument"
    );
    assert_eq!(
        event_args.property()?.expect("property").name()?,
        "test_property",
        "PropertyValueEventArgs::property should work with no extra argument"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_selection_property() -> opendaq::Result<()> {
    let labels = ListObject::new()?;
    labels.push_back("Low")?;
    labels.push_back("Mid")?;
    labels.push_back("High")?;

    let property = Property::selection("Gain", &labels, 1, true)?;
    let choices = property.selection_values()?;
    let builder = PropertyBuilder::selection("Gain", &labels, 0)?;
    let builder_choices = builder.selection_values()?;
    let property_object = PropertyObject::new()?;

    let expect_labels = |value: &Value, what: &str| {
        let items = value
            .as_list()
            .unwrap_or_else(|| panic!("{what} should be a list"));
        let items: Vec<&str> = items.iter().map(|v| v.as_str().expect("label")).collect();
        assert_eq!(
            items,
            ["Low", "Mid", "High"],
            "{what} should read back in order"
        );
    };
    expect_labels(&choices, "a Selection property's choices");
    expect_labels(&builder_choices, "a Selection property builder's choices");

    property_object.add_property(&property)?;
    assert_eq!(
        property_object.property_value("Gain")?.as_i64(),
        Some(1),
        "a Selection property's value should be the default index, unboxed to an integer"
    );
    property_object.set_property_value("Gain", 2)?;
    assert_eq!(
        property_object.property_value("Gain")?.as_i64(),
        Some(2),
        "setting a Selection property's index should round-trip"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_sparse_selection_property() -> opendaq::Result<()> {
    let labels = DictObject::new()?;
    labels.set(1, "Control")?;
    labels.set(3, "ViewOnly")?;
    labels.set(7, "Other")?;

    let property = Property::sparse_selection("Mode", &labels, 3, true)?;
    let choices = property.selection_values()?;
    let property_object = PropertyObject::new()?;

    let pairs = choices
        .as_dict()
        .expect("sparse selection choices should unbox to a dict");
    assert_eq!(
        pairs.len(),
        3,
        "a sparse Selection property should read all of its choices back"
    );
    let label_of = |key: i64| {
        pairs
            .iter()
            .find(|(k, _)| k.as_i64() == Some(key))
            .and_then(|(_, v)| v.as_str())
    };
    assert_eq!(
        label_of(1),
        Some("Control"),
        "key 1 should map to its label"
    );
    assert_eq!(
        label_of(3),
        Some("ViewOnly"),
        "key 3 should map to its label"
    );
    assert_eq!(label_of(7), Some("Other"), "key 7 should map to its label");

    property_object.add_property(&property)?;
    assert_eq!(
        property_object.property_value("Mode")?.as_i64(),
        Some(3),
        "a sparse Selection property's value should be the default key, unboxed to an integer"
    );
    Ok(())
}

#[test]
fn high_level_coreobjects_end_update_event() -> opendaq::Result<()> {
    let property_object = PropertyObject::new()?;
    let event = property_object.on_end_update()?.expect("end-update event");
    let update_ended = Arc::new(AtomicBool::new(false));
    event.subscribe({
        let update_ended = update_ended.clone();
        move |_sender, args| {
            // Read the updated-properties list off the typed event args
            // before flagging completion.
            let args = args
                .expect("event args")
                .cast::<EndUpdateEventArgs>()
                .expect("EndUpdateEventArgs cast");
            args.properties().expect("updated properties");
            update_ended.store(true, Ordering::SeqCst);
        }
    })?;

    property_object.begin_update()?;
    property_object.end_update()?;
    assert!(
        update_ended.load(Ordering::SeqCst),
        "property objects should emit the end-update event"
    );
    Ok(())
}
