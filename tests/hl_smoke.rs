//! Port of the reference openDAQ bindings' high-level smoke suite: broad
//! smoke coverage of the generated wrappers and simulator access.
//!
//! Deviations from the Lisp suite:
//! - Element-type assertions (double-float vectors, signed-byte 64 domains)
//!   are compile-time facts here: the readers are statically typed as
//!   `StreamReader<f64, i64>` etc., so those `is` forms have no runtime port.
//! - `high-level-callable-argument-boxing` drives `%box-callable-argument`,
//!   an internal of the Lisp bindings; the equivalent Rust mechanism
//!   (boxing `Value` dicts/lists across a callable boundary) is exercised
//!   instead.
//! - The complex-float64 packet round-trip of `high-level-data-packet-write`
//!   is omitted: the Rust `Sample` trait covers scalar numeric types only,
//!   so complex packets have no typed accessor to write through.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use opendaq::{
    BlockReader, Channel, ComponentKind, DataDescriptorBuilder, DataPacket, DataRule, Folder,
    FunctionObject, MultiReader, Procedure, ReadTimeoutType, SampleType, SignalConfig,
    StreamReader, StreamReaderOptions, TickConverter, Value,
};

#[test]
fn high_level_simulator_reads() -> opendaq::Result<()> {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let root_device = instance.root_device()?.expect("no root device");
    let device = root_device
        .add_device("daqref://device0")?
        .expect("no device added");
    let channel = device
        .find_component("IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;
    let signals = channel.signals_recursive()?;
    assert!(
        !signals.is_empty(),
        "high-level signal discovery should find at least one signal on the channel"
    );
    let signal = &signals[0];

    // No priming read: the first read must skip the reader's initial
    // descriptor-change event on its own and still return samples.
    let reader = StreamReader::<f64>::new(signal)?;
    let samples = reader.read(100, 2000)?;
    assert_eq!(
        samples.sample_count(),
        100,
        "stream reader read should return the requested number of samples"
    );
    assert!(
        samples.iter().all(|v| v.is_finite()),
        "stream reader read should return numeric elements"
    );

    let reader2 = StreamReader::<f64>::new(signal)?;
    let (values, domain) = reader2.read_with_domain(10, 2000)?;
    assert_eq!(
        values.sample_count(),
        10,
        "read_with_domain should return the requested number of values"
    );
    assert_eq!(
        values.sample_count(),
        domain.len(),
        "read_with_domain value and domain arrays must have equal length"
    );

    // TickConverter is the Rust counterpart of domain-time-converter /
    // domain-tick->timestamp.
    let converter = TickConverter::from_signal(signal)?;
    let timestamps: Vec<_> = domain
        .iter()
        .map(|&t| converter.tick_to_system_time(t))
        .collect();
    assert_eq!(
        timestamps.len(),
        domain.len(),
        "the tick converter should map each tick to a timestamp"
    );
    assert!(
        timestamps.windows(2).all(|w| w[0] <= w[1]),
        "successive domain ticks should map to non-decreasing timestamps"
    );
    let converter2 = TickConverter::from_signal(signal)?;
    assert_eq!(
        timestamps[0],
        converter2.tick_to_system_time(domain[0]),
        "independently built converters should agree on the first tick"
    );
    Ok(())
}

#[test]
fn high_level_stream_reader_skips_domain_only_events() -> opendaq::Result<()> {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let root_device = instance.root_device()?.expect("no root device");
    let device = root_device
        .add_device("daqref://device0")?
        .expect("no device added");
    let channel = device
        .find_component("IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;
    let signal = channel
        .signals()?
        .into_iter()
        .next()
        .expect("no signal on the channel");
    let reader = StreamReader::<f64>::new(&signal)?;

    device.set_property_value("GlobalSampleRate", 100)?;
    channel.set_property_value("Frequency", 0.5)?;
    assert_eq!(
        reader.read(100, 2000)?.sample_count(),
        100,
        "first read must skip the initial and the sample-rate-change events and still return samples"
    );

    device.set_property_value("GlobalSampleRate", 200)?;
    let mut total = 0;
    for _ in 0..20 {
        if total >= 100 {
            break;
        }
        total += reader.read(100, 2000)?.sample_count();
    }
    assert!(
        total >= 100,
        "reads after a mid-stream sample-rate change must keep returning samples, \
         not wedge on the domain-only event"
    );
    Ok(())
}

#[test]
fn high_level_component_type_detection() -> opendaq::Result<()> {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let root_device = instance.root_device()?.expect("no root device");
    let device = root_device
        .add_device("daqref://device0")?
        .expect("no device added");
    let channel_component = device
        .find_component("IO/AI/RefCh0")?
        .expect("reference channel not found");
    let channel = channel_component.cast::<Channel>()?;
    let signal = channel
        .signals_recursive()?
        .into_iter()
        .next()
        .expect("no signal on the channel");

    assert_eq!(
        device.component_kind(),
        Some(ComponentKind::Device),
        "component_kind should identify the reference device as Device"
    );
    assert_eq!(
        channel_component.component_kind(),
        Some(ComponentKind::Channel),
        "component_kind should identify a reference channel as Channel"
    );
    assert_eq!(
        signal.component_kind(),
        Some(ComponentKind::Signal),
        "component_kind should identify a channel's signal as Signal"
    );
    assert!(
        channel_component.is_a::<Folder>(),
        "a channel should support IFolder (a channel is a function block)"
    );
    assert!(
        !signal.is_a::<Folder>(),
        "a signal should not support IFolder (the failure path must not crash)"
    );
    Ok(())
}

#[test]
fn high_level_multi_reader() -> opendaq::Result<()> {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let root_device = instance.root_device()?.expect("no root device");
    let device = root_device
        .add_device("daqref://device0")?
        .expect("no device added");
    let ch0 = device
        .find_component("IO/AI/RefCh0")?
        .expect("no RefCh0")
        .cast::<Channel>()?;
    let ch1 = device
        .find_component("IO/AI/RefCh1")?
        .expect("no RefCh1")
        .cast::<Channel>()?;
    let signals = vec![
        ch0.signals()?
            .into_iter()
            .next()
            .expect("no signal on RefCh0"),
        ch1.signals()?
            .into_iter()
            .next()
            .expect("no signal on RefCh1"),
    ];
    let reader = MultiReader::<f64>::new(&signals)?;

    // The first reads only synchronise the streams; loop until aligned data.
    for _ in 0..30 {
        let (values, domain) = reader.read_with_domain(10, 1000)?;
        if domain[0].is_empty() {
            continue;
        }
        assert_eq!(
            values.len(),
            2,
            "multi reader should return one value vector per signal"
        );
        assert_eq!(
            domain.len(),
            2,
            "multi reader should return one domain vector per signal"
        );
        assert_eq!(
            values[0].len(),
            values[1].len(),
            "all per-signal value vectors should share the same length"
        );
        assert_eq!(
            domain[0], domain[1],
            "synchronised signals should share identical domain ticks"
        );
        assert_eq!(
            reader.read(5, 1000)?.len(),
            2,
            "multi reader read should also return one vector per signal"
        );
        return Ok(());
    }
    panic!("multi reader did not synchronise within the attempt budget");
}

#[test]
fn high_level_block_reader() -> opendaq::Result<()> {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let channel = common::make_ref_channel(&instance);
    let signals = channel.signals_recursive()?;
    let signal = signals.first().expect("no signal on the channel");
    let reader = BlockReader::<f64>::new(signal, 10)?;

    let blocks = reader.read(5, 2000)?;
    assert_eq!(
        blocks.sample_count(),
        5,
        "block reader read should return the requested number of blocks"
    );
    assert_eq!(
        blocks.width(),
        10,
        "block reader row width should equal the block size"
    );
    assert_eq!(
        blocks.rows().count(),
        5,
        "the flat samples should chunk into one row per block"
    );
    Ok(())
}

#[test]
fn high_level_stream_reader_2d_signal() -> opendaq::Result<()> {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let channel = common::make_ref_channel(&instance);
    // The FFT block emits a dimensioned (2-D) signal: each sample is a whole
    // amplitude spectrum, one value per frequency bin.
    let fft = instance
        .add_function_block("RefFBModuleFFT")?
        .expect("no FFT function block");
    fft.set_property_value("BlockSize", 16)?;
    fft.input_ports()?
        .first()
        .expect("no FFT input port")
        .connect(&channel.signals()?[0])?;
    let signal = fft
        .signals()?
        .into_iter()
        .next()
        .expect("no FFT output signal");

    // Wait for the block to publish its output descriptor before reading, so
    // the reader knows each sample is a spectrum (not a scalar).
    let mut descriptor = None;
    for _ in 0..50 {
        descriptor = signal.descriptor()?;
        if descriptor.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let descriptor = descriptor.expect("the FFT block should publish an output descriptor");
    let dimensions = descriptor.dimensions()?;
    assert_eq!(
        dimensions.len(),
        1,
        "the FFT signal should carry a single dimension (the frequency axis)"
    );
    assert_eq!(
        dimensions[0].size()?,
        16,
        "the dimension size should match the configured FFT block size"
    );

    let reader = StreamReader::<f64>::new(&signal)?;
    // The first reads may be short while the stream warms up; loop until 5
    // whole spectra arrive as a (samples x bins) matrix.
    let mut complete = false;
    for _ in 0..50 {
        let spectra = reader.read(5, 1000)?;
        if spectra.sample_count() == 5 {
            assert_eq!(
                spectra.width(),
                16,
                "each row should hold one value per frequency bin"
            );
            assert_eq!(
                spectra.values().len(),
                5 * 16,
                "a 2-D signal read should return sample-count * width values"
            );
            complete = true;
            break;
        }
    }
    assert!(
        complete,
        "the FFT stream did not produce 5 spectra within the attempt budget"
    );

    // read_with_domain pairs the (samples x bins) matrix with one tick per sample.
    let mut complete = false;
    for _ in 0..50 {
        let (spectra, domain) = reader.read_with_domain(3, 1000)?;
        if spectra.sample_count() == 3 {
            assert_eq!(
                spectra.width(),
                16,
                "read_with_domain rows should hold one value per frequency bin"
            );
            assert_eq!(
                domain.len(),
                3,
                "read_with_domain should return one domain tick per sample"
            );
            complete = true;
            break;
        }
    }
    assert!(
        complete,
        "the FFT stream did not produce 3 spectra with domain within the attempt budget"
    );
    Ok(())
}

#[test]
fn high_level_data_packet_buffers() -> opendaq::Result<()> {
    // Pure core objects: no instance, so no instance lock is needed.
    let builder = DataDescriptorBuilder::new()?;
    builder.set_sample_type(SampleType::Float64)?;
    let descriptor = builder.build()?.expect("no descriptor built");
    let packet = DataPacket::new(&descriptor, 8, 0)?;

    let values = packet.data::<f64>()?;
    assert_eq!(
        values.len(),
        8,
        "data-packet data length should match the sample count"
    );
    let raw = packet.raw_data()?;
    assert_eq!(
        raw.len(),
        64,
        "data-packet raw data size should be sample-count * element-size bytes"
    );
    Ok(())
}

fn make_packet(sample_type: SampleType, count: usize) -> opendaq::Result<DataPacket> {
    let builder = DataDescriptorBuilder::new()?;
    builder.set_sample_type(sample_type)?;
    let descriptor = builder.build()?.expect("no descriptor built");
    DataPacket::new(&descriptor, count, 0)
}

#[test]
fn high_level_data_packet_write() -> opendaq::Result<()> {
    // Float64 round-trip.
    let packet = make_packet(SampleType::Float64, 4)?;
    packet.set_data(&[1.5f64, 2.5, 3.5, 4.5])?;
    assert_eq!(
        packet.data::<f64>()?,
        vec![1.5, 2.5, 3.5, 4.5],
        "set_data should round-trip float samples"
    );

    // Int32 round-trip.  (The Lisp test also coerces reals with rounding;
    // Rust's typed API takes i32 directly, so there is nothing to round.)
    let packet = make_packet(SampleType::Int32, 3)?;
    packet.set_data(&[1i32, 3, -3])?;
    assert_eq!(
        packet.data::<i32>()?,
        vec![1, 3, -3],
        "set_data should round-trip integer samples"
    );
    // A mismatched element type errors rather than corrupting the buffer.
    assert!(
        packet.set_data(&[1.0f64]).is_err(),
        "writing f64 samples into an int32 packet should fail"
    );

    // Complex float64 round-trip: omitted (see the module docs) -- `Sample`
    // covers scalar numeric types only.

    // A sample type that is not a flat numeric buffer errors rather than
    // corrupting (whether at packet creation or at the typed access).
    let result = make_packet(SampleType::Struct, 1).and_then(|packet| packet.data::<f64>());
    assert!(
        result.is_err(),
        "reading a struct packet through a numeric type should fail"
    );
    Ok(())
}

#[test]
fn high_level_create_signal_and_read() -> opendaq::Result<()> {
    // Build a signal by hand, push packets into it, and read them back with a
    // StreamReader.  The domain uses an implicit linear rule; the rule's
    // delta/start and the packets' offsets are plain integers -- the
    // `impl Into<Value>` parameters box them through the INumber coercion,
    // which openDAQ's DataRule and DataPacket factories require.
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let context = instance.context()?.expect("no context");

    let domain_descriptor = {
        let builder = DataDescriptorBuilder::new()?;
        builder.set_sample_type(SampleType::Int64)?;
        builder.set_name("time")?;
        builder.set_rule(&DataRule::linear(1, 0)?)?;
        builder.build()?.expect("no domain descriptor built")
    };
    let value_descriptor = {
        let builder = DataDescriptorBuilder::new()?;
        builder.set_sample_type(SampleType::Float64)?;
        builder.set_name("values")?;
        builder.build()?.expect("no value descriptor built")
    };
    let domain_signal = SignalConfig::signal(&context, None, "time", None)?;
    let signal = SignalConfig::signal(&context, None, "values", None)?;
    domain_signal.set_descriptor(&domain_descriptor)?;
    signal.set_descriptor(&value_descriptor)?;
    signal.set_domain_signal(&domain_signal)?;

    let reader = StreamReader::<f64>::with_options(
        &signal,
        StreamReaderOptions {
            timeout_type: ReadTimeoutType::Any,
            ..Default::default()
        },
    )?;

    let send = |offset: i64, samples: &[f64]| -> opendaq::Result<()> {
        let domain_packet = DataPacket::new(&domain_descriptor, samples.len(), offset)?;
        let packet = DataPacket::with_domain(&domain_packet, &value_descriptor, samples.len(), 0)?;
        packet.set_data(samples)?;
        signal.send_packet(&packet)
    };
    send(0, &[1.0, 2.0, 3.0, 4.0])?;
    send(4, &[5.0, 6.0, 7.0, 8.0])?;
    send(8, &[9.0, 10.0])?;

    let (values, ticks) = reader.read_with_domain(100, 1000)?;
    assert_eq!(
        values.sample_count(),
        10,
        "reading a hand-built signal should return every sent sample"
    );
    assert_eq!(
        values.values(),
        &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0],
        "the read values should match the samples written into the packets"
    );
    assert_eq!(
        ticks,
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        "the implicit linear domain should yield contiguous ticks across packets"
    );
    Ok(())
}

#[test]
fn high_level_callable_properties() -> opendaq::Result<()> {
    let _guard = common::instance_lock();
    let instance = common::make_test_instance();
    let root_device = instance.root_device()?.expect("no root device");
    let device = root_device
        .add_device("daqref://device0")?
        .expect("no device added");
    let channel = device
        .find_component("IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;

    // FUNC property: property_value returns the callable as an object; cast
    // to FunctionObject, then call boxes the args and unboxes the result.
    let sum = device
        .property_value("Protected.Sum")?
        .into_object()?
        .cast::<FunctionObject>()?;
    assert_eq!(
        sum.call(&[Value::from(7), Value::from(5)])?.as_i64(),
        Some(12),
        "calling a func property should box the args, invoke it, and unbox the result"
    );
    assert_eq!(
        sum.call(&[Value::from(40), Value::from(2)])?.as_i64(),
        Some(42),
        "the function object should be reusable across calls"
    );
    assert!(
        sum.call(&[Value::from(1)]).is_err(),
        "calling a func property with the wrong number of arguments should fail"
    );

    // FUNC property taking a LIST argument: a Value::List boxes into a daq
    // list and is passed as the callable's params.
    let sum_list = device
        .property_value("Protected.SumList")?
        .into_object()?
        .cast::<FunctionObject>()?;
    assert_eq!(
        sum_list.call(&[Value::from(vec![1i64, 2, 3, 4])])?.as_i64(),
        Some(10),
        "a list argument should be boxed into a daq list and summed"
    );

    // Scalar property: unboxes to its native value, not a callable.
    let number_of_channels = device.property_value("NumberOfChannels")?;
    assert!(
        matches!(number_of_channels, Value::Int(_)),
        "property_value of a scalar property should return its native value directly, \
         got {number_of_channels:?}"
    );

    // PROC property with no arguments: dispatched for its side effect.
    // Unlike the Lisp bindings -- whose closure wrapper checks arity against
    // the property's CallableInfo before dispatching -- the Rust bindings
    // pass arguments straight through, so surplus-argument handling is the
    // callee's business.  The declared arity stays checkable via the
    // property metadata, which is what the Lisp wrapper consulted.
    let reset = channel
        .property_value("ResetCounter")?
        .into_object()?
        .cast::<Procedure>()?;
    reset.dispatch_args(&[])?;
    let reset_arity = channel
        .property("ResetCounter")?
        .expect("ResetCounter property")
        .callable_info()?
        .expect("callable info")
        .arguments()?
        .len();
    assert_eq!(reset_arity, 0, "ResetCounter should declare zero arguments");

    // FUNC property with a single argument: the bare-value param encoding.
    let get_and_set = channel
        .property_value("GetAndSetCounter")?
        .into_object()?
        .cast::<FunctionObject>()?;
    assert!(
        matches!(get_and_set.call(&[Value::from(0)])?, Value::Int(_)),
        "a single-argument func property should encode its lone arg and unbox the int result"
    );
    Ok(())
}

#[test]
fn high_level_callable_argument_boxing() -> opendaq::Result<()> {
    // The Lisp test drives %box-callable-argument, an internal of the Lisp
    // bindings.  The Rust bindings box arguments through `Value`, so exercise
    // that mechanism end-to-end across an openDAQ callable boundary instead.
    let echo = FunctionObject::from_fn(|args| Ok(args.first().cloned().unwrap_or(Value::Null)))?;

    // DICT argument: a Rust HashMap boxes into a daq dict and round-trips.
    let mut table = HashMap::new();
    table.insert("x".to_string(), Value::from(10));
    table.insert("y".to_string(), Value::from(20));
    let round_trip = echo.call(&[Value::from(table)])?;
    let pairs = round_trip
        .as_dict()
        .expect("a dict argument should round-trip as a dict");
    assert_eq!(
        pairs.len(),
        2,
        "the boxed dict should preserve the entry count"
    );
    assert_eq!(
        round_trip.get("x").and_then(Value::as_i64),
        Some(10),
        "the boxed dict should preserve its string->int entries"
    );
    assert_eq!(
        round_trip.get("y").and_then(Value::as_i64),
        Some(20),
        "the boxed dict should preserve its string->int entries"
    );

    // An empty list boxes to an empty daq list, which decodes as zero
    // arguments (openDAQ's params encoding uses a list for several args).
    let count = FunctionObject::from_fn(|args| Ok(Value::from(args.len())))?;
    assert_eq!(
        count.call(&[Value::List(Vec::new())])?.as_i64(),
        Some(0),
        "an empty list params should decode to no arguments"
    );

    // Several scalar arguments box element-wise into the params list.
    let sum = FunctionObject::from_fn(|args| {
        Ok(Value::from(
            args.iter().filter_map(Value::as_i64).sum::<i64>(),
        ))
    })?;
    assert_eq!(
        sum.call(&[
            Value::from(1),
            Value::from(2),
            Value::from(3),
            Value::from(4)
        ])?
        .as_i64(),
        Some(10),
        "multiple arguments should be boxed into a daq list and decoded element-wise"
    );
    Ok(())
}

#[test]
fn high_level_autoload_healthcheck() {
    // The Lisp test inspects (daq:healthcheck)'s plist; the Rust equivalents
    // are init() succeeding (status :loaded, no autoload error) and the
    // resolved native directory existing.
    opendaq::init().expect("the native library should load");
    let directory = opendaq::native_library_directory()
        .expect("the native library directory should be resolved");
    assert!(
        directory.is_dir(),
        "the discovered native library directory should exist: {}",
        directory.display()
    );
}
