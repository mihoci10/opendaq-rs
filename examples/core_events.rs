// Observing changes to the openDAQ core structure through the core event.
//
// The context's core event fires on every change within the component tree:
// property value changes, component additions and removals, signal
// connections, and more.  This example subscribes a closure, watches it
// report two property writes, then unsubscribes.

use opendaq::{Channel, CoreEventArgs, Instance};

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    instance.add_device("daqref://device0")?;

    let channel = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;

    let core_event = instance
        .context()?
        .expect("context")
        .on_core_event()?
        .expect("core event");

    // The handler may run on openDAQ's own threads.  Its args come in as a
    // generic object; cast to CoreEventArgs for the event name and the
    // per-event-type parameters ("Name" holds the changed property's name).
    let handler = core_event.subscribe(|_sender, args| {
        let Some(args) = args else { return };
        let Ok(event) = args.cast::<CoreEventArgs>() else {
            return;
        };
        let name = event.event_name().unwrap_or_default();
        let parameters = event.parameters().unwrap_or_default();
        let changed = parameters
            .get("Name")
            .map(ToString::to_string)
            .unwrap_or_default();
        println!("  {name}: {changed}");
    })?;

    // While subscribed, each property change is reported by the handler above.
    println!("subscribed:");
    channel.set_property_value("Frequency", 25.0)?;
    channel.set_property_value("Amplitude", 7.5)?;

    // Unsubscribing removes the handler; further changes fire nothing.
    core_event.unsubscribe(&handler)?;
    println!("unsubscribed (no lines expected below):");
    channel.set_property_value("Frequency", 50.0)?;

    Ok(())
}
