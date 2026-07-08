// Connect to a remote device as a *view-only* client: the connection lets you
// browse components and read signals, but the server refuses configuration
// writes.  The client type is picked in the add-device config, under the
// "General" child object's "ClientType" selection property.
//
// Point OPENDAQ_DEVICE at a running openDAQ device (e.g. the reference device
// simulator); it defaults to `daq.nd://127.0.0.1`, the openDAQ native
// configuration protocol on the local machine.

use opendaq::{CoreType, Device, Instance, PropertyObject, Value};

/// Name of `object`'s first visible, non-read-only, non-callable property, or
/// `None`.  Used to probe whether the connection actually refuses writes.
fn first_writable_property_name(object: &PropertyObject) -> opendaq::Result<Option<String>> {
    for property in object.visible_properties()? {
        if property.read_only()? {
            continue;
        }
        if matches!(property.value_type()?, CoreType::Func | CoreType::Proc) {
            continue;
        }
        return Ok(Some(property.name()?));
    }
    Ok(None)
}

fn probe_view_only_device(device: &Device) -> opendaq::Result<()> {
    println!("Connected to {} as a view-only client.", device.name()?);
    println!("Visible signals: {}", device.signals_recursive()?.len());
    match first_writable_property_name(device)? {
        Some(name) => {
            // Write the property's current value back to itself: harmless if
            // it were allowed, but a view-only connection must reject it.
            let value = device.property_value(&name)?;
            match device.set_property_value(&name, value) {
                Ok(()) => println!("Unexpected: writing {name:?} was allowed."),
                Err(err) => {
                    println!("Write to {name:?} refused, as expected for view-only: {err}")
                }
            }
        }
        None => println!("Device exposes no writable property to probe."),
    }
    Ok(())
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;

    let config = instance
        .create_default_add_device_config()?
        .expect("default add-device config");

    // The "General" child object holds the protocol-independent options,
    // ClientType among them; its selection values name the available modes.
    let general = config
        .property_value("General")?
        .into_object()?
        .cast::<PropertyObject>()?;
    let client_type = general
        .property("ClientType")?
        .expect("ClientType property");
    println!("Available ClientType options:");
    if let Value::Dict(mut options) = client_type.selection_values()? {
        options.sort_by_key(|(key, _)| key.as_i64());
        for (key, name) in options {
            println!("  {key} = {name}");
        }
    }

    config.set_property_value("General.ClientType", 2)?; // 2 = view-only
    let connection_string =
        std::env::var("OPENDAQ_DEVICE").unwrap_or_else(|_| "daq.nd://127.0.0.1".into());

    match instance.add_device_with(&connection_string, Some(&config)) {
        Ok(Some(device)) => probe_view_only_device(&device)?,
        Ok(None) => println!("Could not connect to {connection_string}: no device returned."),
        Err(err) => {
            println!("Could not connect to {connection_string}: {err}");
            println!("Start an openDAQ device/simulator there, or set OPENDAQ_DEVICE.");
        }
    }
    Ok(())
}
