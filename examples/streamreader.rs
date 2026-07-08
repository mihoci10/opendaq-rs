// Read samples from a reference-device channel with a stream reader.
//
// The reader keeps an internal position, so consecutive reads return
// consecutive stretches of the signal.

use opendaq::{Channel, Instance, StreamReader};

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    instance.add_device("daqref://device0")?;

    let channel = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;
    let signal = &channel.signals()?[0];

    let reader = StreamReader::<f64>::new(signal)?;
    println!("some samples: {:?}", reader.read(100, 1000)?.values());
    println!("and more samples: {:?}", reader.read(100, 1000)?.values());
    println!("and more still: {:?}", reader.read(100, 1000)?.values());
    Ok(())
}
