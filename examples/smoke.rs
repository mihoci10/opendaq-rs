// Development smoke test exercising every hand-written subsystem end-to-end.

use opendaq::{
    BatchedPropertyUpdate, Channel, FunctionObject, Instance, StreamReader, TickConverter, Value,
};

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;

    println!("Available devices:");
    for info in instance.available_devices()? {
        println!(" - {}: {}", info.connection_string()?, info.name()?);
    }

    let device = instance.add_device("daqref://device0")?.expect("device");
    println!("Added device: {}", device.name()?);

    let channel = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;
    let signal = &channel.signals()?[0];

    // Batched property updates.
    {
        let batch = BatchedPropertyUpdate::new(&[&device, &channel])?;
        device.set_property_value("GlobalSampleRate", 1000)?;
        channel.set_property_value("Frequency", 50.0)?;
        batch.commit()?;
    }
    println!(
        "GlobalSampleRate = {}",
        device.property_value("GlobalSampleRate")?
    );

    // Stream reading with domain + tick conversion.
    let reader = StreamReader::<f64>::new(signal)?;
    let (samples, ticks) = reader.read_with_domain(100, 2000)?;
    println!(
        "read {} samples, {} ticks",
        samples.sample_count(),
        ticks.len()
    );
    let converter = TickConverter::from_signal(signal)?;
    if let Some(first) = ticks.first() {
        println!(
            "first tick {} -> {:?}",
            first,
            converter.tick_to_system_time(*first)
        );
    }

    // Callables: a Rust closure as an openDAQ function, called through openDAQ.
    let sum = FunctionObject::from_fn(|args| {
        let a = args[0].as_i64().unwrap_or(0);
        let b = args[1].as_i64().unwrap_or(0);
        Ok(Value::from(a + b))
    })?;
    let result = sum.call(&[Value::from(20), Value::from(22)])?;
    println!("sum(20, 22) = {result}");

    // Events: subscribe to property changes on the channel.
    let events = channel
        .on_property_value_write("Amplitude")?
        .expect("event");
    let handler = events.subscribe(|_sender, args| {
        if let Some(args) = args {
            println!("  event fired: {args}");
        }
    })?;
    channel.set_property_value("Amplitude", 2.5)?;
    events.unsubscribe(&handler)?;
    channel.set_property_value("Amplitude", 1.0)?;
    println!("Amplitude = {}", channel.property_value("Amplitude")?);

    Ok(())
}
