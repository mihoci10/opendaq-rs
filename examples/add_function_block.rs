// Adds a statistics function block, connects a reference channel's signal to
// its input port, and reads back the averaged output signals.

use opendaq::{Channel, Instance, InstanceBuilder, MultiReader, Signal};

/// The signal with the given local id.  Match on local id, not name: a
/// signal's name is a mutable display label, while its local id is the stable
/// identifier within its parent.
fn signal_by_local_id(signals: &[Signal], local_id: &str) -> opendaq::Result<Option<Signal>> {
    for signal in signals {
        if signal.local_id()? == local_id {
            return Ok(Some(signal.clone()));
        }
    }
    Ok(None)
}

fn main() -> opendaq::Result<()> {
    // Instance::new() already loads the bundled modules; build from an
    // InstanceBuilder when you want to control the module search path.
    let builder = InstanceBuilder::new()?;
    // The bundled modules (reference device, function blocks, streaming):
    let bundled = opendaq::native_library_directory().expect("native libraries");
    builder.set_module_path(&bundled.to_string_lossy())?;
    // Your own modules folder:
    // builder.add_module_path("/path/to/your/modules")?;
    let instance = Instance::from_builder(&builder)?;
    instance.add_device("daqref://device0")?;

    let channel = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;
    channel.set_property_value("Amplitude", 5.0)?;
    channel.set_property_value("DC", 1.0)?;

    let statistics = instance
        .add_function_block("RefFBModuleStatistics")?
        .expect("statistics function block");
    statistics.set_property_value("BlockSize", 100)?;

    let status = statistics.status_container()?.expect("status container");
    println!(
        "Before connect: {} ({})",
        status.status("ComponentStatus")?.expect("status").value()?,
        status.status_message("ComponentStatus")?
    );

    let port = &statistics.input_ports()?[0];
    let signal = &channel.signals()?[0];
    port.connect(signal)?;

    println!(
        "After connect:  {} ({})",
        status.status("ComponentStatus")?.expect("status").value()?,
        status.status_message("ComponentStatus")?
    );

    let outputs = statistics.signals()?;
    let avg = signal_by_local_id(&outputs, "avg")?.expect("avg signal");
    let rms = signal_by_local_id(&outputs, "rms")?.expect("rms signal");
    let reader = MultiReader::<f64>::new(&[avg, rms])?;

    // The first reads may return nothing while the block waits for a complete
    // input descriptor (the multi reader by default doesn't skip events), so
    // retry until samples arrive.
    for _attempt in 0..20 {
        let values = reader.read(5, 1000)?;
        if !values[0].is_empty() {
            println!("{:>10}{:>12}", "avg", "rms");
            for (a, r) in values[0].iter().zip(&values[1]) {
                println!("{a:>10.4}{r:>12.4}");
            }
            return Ok(());
        }
    }
    println!("Statistics block produced no samples in time.");
    Ok(())
}
