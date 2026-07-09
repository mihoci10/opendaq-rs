//! High-level runtime tests: instance/API basics, context, logger, device,
//! component, server, signal, and streaming.

mod common;

use opendaq::{sys, Interface, Value};

/// A context with no scheduler, module manager, or authentication provider.
fn make_context() -> opendaq::Context {
    let sink = opendaq::LoggerSink::std_err().expect("std_err sink");
    let logger = opendaq::Logger::new(vec![sink], opendaq::LogLevel::Debug).expect("logger");
    let type_manager = opendaq::TypeManager::new().expect("type manager");
    opendaq::Context::new(
        None,
        &logger,
        &type_manager,
        None,
        None,
        Value::Dict(vec![]),
        Value::Dict(vec![]),
    )
    .expect("context")
}

/// A root component (no parent, no class name).
fn make_root_component(context: &opendaq::Context, local_id: &str) -> opendaq::Component {
    opendaq::Component::new(context, None, local_id, None).expect("component")
}

/// An int64 "vals" descriptor with a volts unit, reused by several tests.
fn make_int64_descriptor() -> opendaq::DataDescriptor {
    let unit_builder = opendaq::UnitBuilder::new().expect("unit builder");
    unit_builder.set_id(-1).expect("set_id");
    unit_builder.set_name("volts").expect("set unit name");
    unit_builder.set_symbol("V").expect("set unit symbol");
    unit_builder
        .set_quantity("voltage")
        .expect("set unit quantity");
    let unit = unit_builder.build().expect("build unit").expect("unit");

    let builder = opendaq::DataDescriptorBuilder::new().expect("data descriptor builder");
    builder
        .set_sample_type(opendaq::SampleType::Int64)
        .expect("set_sample_type");
    builder.set_name("vals").expect("set descriptor name");
    builder.set_unit(&unit).expect("set_unit");
    builder
        .build()
        .expect("build descriptor")
        .expect("descriptor")
}

// ---------------------------------------------------------------------------
// Instance / API basics
// ---------------------------------------------------------------------------

// The runtime type check is subsumed by the static return type.
#[test]
fn opendaq_config_provider() {
    let provider = opendaq::ConfigProvider::env().expect("env config provider");
    assert!(
        !provider.as_raw().is_null(),
        "config provider helpers should return a live native pointer"
    );
}

#[test]
fn opendaq_instance_builder() {
    let _guard = common::instance_lock();
    let module_path = opendaq::native_library_directory()
        .expect("native library directory")
        .to_string_lossy()
        .into_owned();

    let builder = opendaq::InstanceBuilder::new().expect("instance builder");
    builder
        .set_module_path(&module_path)
        .expect("set_module_path");
    builder
        .enable_standard_providers(true)
        .expect("enable_standard_providers");

    let instance = builder
        .build()
        .expect("build failed")
        .expect("no instance built");
    assert_eq!(
        builder.module_path().expect("module_path"),
        module_path,
        "instance builders should preserve their module path"
    );
    assert!(
        instance.root_device().expect("root_device").is_some(),
        "instances built through the builder should expose a root device"
    );
}

#[test]
fn opendaq_instance_make_instance() {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    assert!(
        instance.root_device().expect("root_device").is_some(),
        "instance construction should expose a root device"
    );
    assert!(
        !instance.as_raw().is_null(),
        "instances should hold a live native pointer after construction"
    );
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

// options()/discovery_servers() return HashMaps, so the empty dicts
// round-trip as empty maps.
#[test]
fn context_construction() {
    let context = make_context();
    assert!(
        context.logger().expect("logger").is_some(),
        "contexts should expose their logger wrapper"
    );
    assert!(
        context.type_manager().expect("type_manager").is_some(),
        "contexts should expose their type manager wrapper"
    );
    let options = context.options().expect("options");
    assert!(
        options.is_empty(),
        "the empty options dict should round-trip as an empty map"
    );
    let discovery_servers = context.discovery_servers().expect("discovery_servers");
    assert!(
        discovery_servers.is_empty(),
        "the empty discovery-servers dict should round-trip as an empty map"
    );
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

#[test]
fn logger_construction() {
    let sink = opendaq::LoggerSink::std_out().expect("std_out sink");
    let logger = opendaq::Logger::new(vec![sink], opendaq::LogLevel::Debug).expect("logger");
    assert!(
        !logger.as_raw().is_null(),
        "loggers should hold a native pointer after construction"
    );
    assert_eq!(
        logger.level().expect("level"),
        opendaq::LogLevel::Debug,
        "loggers should expose the configured log level"
    );
}

// ---------------------------------------------------------------------------
// Device
// ---------------------------------------------------------------------------

#[test]
fn device_address_info_builder() {
    let builder = opendaq::AddressInfoBuilder::new().expect("address info builder");
    builder
        .set_connection_string("daqref://device0")
        .expect("set_connection_string");
    builder
        .set_reachability_status(opendaq::AddressReachabilityStatus::Unknown)
        .expect("set_reachability_status");
    builder.set_type("Type").expect("set_type");
    builder.set_address("Address").expect("set_address");

    let address_info = builder
        .build()
        .expect("build failed")
        .expect("no address info built");
    assert_eq!(
        address_info.connection_string().expect("connection_string"),
        "daqref://device0",
        "address-info builders should preserve the connection string"
    );
    assert_eq!(
        address_info.type_().expect("type"),
        "Type",
        "address-info builders should preserve the type field"
    );
    assert_eq!(
        address_info.address().expect("address"),
        "Address",
        "address-info builders should preserve the address field"
    );
}

// The connection-string check is the call succeeding with a String return.
#[test]
fn device_info() {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let root_device = instance
        .root_device()
        .expect("root_device")
        .expect("no root device");
    let info = root_device.info().expect("info").expect("no device info");
    let _connection_string = info
        .connection_string()
        .expect("device info should expose its connection string");
    assert!(
        !info.name().expect("name").is_empty(),
        "device info should expose a non-empty device name"
    );
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

// Exercises component_kind() and is_a::<T>() on a small parent/child tree.
#[test]
fn component_hierarchy() {
    let context = make_context();
    let parent = make_root_component(&context, "parent");
    // A child with no class name.
    let child =
        opendaq::Component::new(&context, Some(&parent), "child", None).expect("child component");

    assert_eq!(
        child.local_id().expect("local_id"),
        "child",
        "components should expose their local identifier"
    );
    assert_eq!(
        child.global_id().expect("global_id"),
        "/parent/child",
        "child components should synthesize the expected global identifier"
    );
    assert_eq!(
        child.component_kind(),
        Some(opendaq::ComponentKind::Component),
        "component_kind should report a plain component as Component"
    );
    assert!(
        child.is_a::<opendaq::Component>(),
        "is_a should confirm a component implements IComponent"
    );
    assert!(
        !child.is_a::<opendaq::Channel>(),
        "is_a should return false (not crash) for an unsupported interface"
    );
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

// The runtime type check is subsumed by the static return type.
#[test]
fn server_type() {
    let default_config = opendaq::PropertyObject::new().expect("property object");
    let server_type = opendaq::ServerType::new(
        "serverType",
        "serverTypeName",
        "serverTypeDescription",
        &default_config,
    )
    .expect("server type");
    assert!(
        !server_type.as_raw().is_null(),
        "server-type wrappers should hold a native pointer after construction"
    );
}

// ---------------------------------------------------------------------------
// Signal
// ---------------------------------------------------------------------------

// The bindings expose no safe Allocator::allocate/free (only the malloc-
// allocator factory), so the allocate/free round-trip goes through
// `opendaq::sys`.
#[test]
fn signal_allocator() {
    let allocator = opendaq::Allocator::malloc().expect("malloc allocator");
    let descriptor = make_int64_descriptor();

    let mut address: *mut std::ffi::c_void = std::ptr::null_mut();
    let code = unsafe {
        (sys::api().daqAllocator_allocate)(
            allocator.as_raw() as *mut _,
            descriptor.as_raw() as *mut _,
            32,
            4,
            &mut address,
        )
    };
    assert_eq!(code, sys::OPENDAQ_SUCCESS, "daqAllocator_allocate failed");
    assert!(
        !address.is_null(),
        "allocators should allocate native sample buffers"
    );
    let code = unsafe { (sys::api().daqAllocator_free)(allocator.as_raw() as *mut _, address) };
    assert_eq!(code, sys::OPENDAQ_SUCCESS, "daqAllocator_free failed");
}

#[test]
fn signal_data_descriptor() {
    let descriptor = make_int64_descriptor();
    assert_eq!(
        descriptor.name().expect("name"),
        "vals",
        "data-descriptor builders should preserve the descriptor name"
    );
    let unit = descriptor.unit().expect("unit").expect("descriptor unit");
    assert_eq!(
        unit.symbol().expect("symbol"),
        "V",
        "data descriptors should expose the configured unit symbol"
    );
    assert_eq!(
        descriptor.sample_type().expect("sample_type"),
        opendaq::SampleType::Int64,
        "data descriptors should preserve the configured sample type"
    );
}

// A root input port (no parent) with gap checking disabled.
#[test]
fn signal_input_port_config() {
    let context = make_context();
    let input_port_config =
        opendaq::InputPortConfig::input_port(&context, None, "daqInputPort", false)
            .expect("input port config");
    assert!(
        !input_port_config
            .gap_checking_enabled()
            .expect("gap_checking_enabled"),
        "input-port-config wrappers should report disabled gap checking as false"
    );
}

#[test]
fn signal_scaling() {
    let parameters = Value::Dict(vec![
        (Value::from("scale"), Value::Int(10)),
        (Value::from("offset"), Value::Int(10)),
    ]);

    let builder = opendaq::ScalingBuilder::new().expect("scaling builder");
    builder
        .set_input_data_type(opendaq::SampleType::Int16)
        .expect("set_input_data_type");
    builder
        .set_output_data_type(opendaq::ScaledSampleType::Float32)
        .expect("set_output_data_type");
    builder
        .set_scaling_type(opendaq::ScalingType::Linear)
        .expect("set_scaling_type");
    builder.set_parameters(parameters).expect("set_parameters");

    let scaling = builder
        .build()
        .expect("build failed")
        .expect("no scaling built");
    assert_eq!(
        scaling.input_sample_type().expect("input_sample_type"),
        opendaq::SampleType::Int16,
        "scaling wrappers should preserve the input sample type"
    );
    assert_eq!(
        scaling.output_sample_type().expect("output_sample_type"),
        opendaq::ScaledSampleType::Float32,
        "scaling wrappers should preserve the output sample type"
    );
    assert_eq!(
        scaling.type_().expect("scaling type"),
        opendaq::ScalingType::Linear,
        "scaling wrappers should preserve the scaling type"
    );

    let scaling_parameters = scaling.parameters().expect("parameters");
    assert_eq!(
        scaling_parameters.get("scale").and_then(|v| v.as_i64()),
        Some(10),
        "scaling parameter dictionaries should preserve boxed numeric values"
    );
    assert_eq!(
        scaling_parameters.get("offset").and_then(|v| v.as_i64()),
        Some(10),
        "scaling parameter dictionaries should preserve boxed numeric values"
    );
}

#[test]
fn signal_config() {
    let context = make_context();
    let descriptor = make_int64_descriptor();
    let signal_config =
        opendaq::SignalConfig::with_descriptor(&context, &descriptor, None, "sig", None)
            .expect("signal config");
    assert!(
        !signal_config.as_raw().is_null(),
        "signal-config wrappers should hold a native pointer after construction"
    );
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

// The runtime type check is subsumed by the static return type.
#[test]
fn streaming_type() {
    let default_config = opendaq::PropertyObject::new().expect("property object");
    let streaming_type = opendaq::StreamingType::new(
        "streamingType",
        "streamingTypeName",
        "streamingTypeDescription",
        "streamingTypePrefix",
        &default_config,
    )
    .expect("streaming type");
    assert_eq!(
        streaming_type
            .connection_string_prefix()
            .expect("connection_string_prefix"),
        "streamingTypePrefix",
        "streaming-type wrappers should expose their connection-string prefix"
    );
}

#[test]
fn streaming_subscription_event_args() {
    let subscription_event_args = opendaq::SubscriptionEventArgs::new(
        "streamingConnectionString",
        opendaq::SubscriptionEventType::Subscribed,
    )
    .expect("subscription event args");
    assert_eq!(
        subscription_event_args
            .streaming_connection_string()
            .expect("streaming_connection_string"),
        "streamingConnectionString",
        "subscription-event-args should expose the connection string"
    );
    assert_eq!(
        subscription_event_args
            .subscription_event_type()
            .expect("subscription_event_type"),
        opendaq::SubscriptionEventType::Subscribed,
        "subscription-event-args should preserve the subscription event type"
    );
}
