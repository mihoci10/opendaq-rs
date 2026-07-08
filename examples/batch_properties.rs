// Stage property writes on two channels and apply them all at once with a
// batched update.  While the batch is open the writes are only staged: reads
// still return the old values, and nothing takes effect until the commit.

use opendaq::{BatchedPropertyUpdate, Channel, Instance};

fn show_settings(label: &str, channel: &Channel) -> opendaq::Result<()> {
    println!(
        "{label} {}:  Amplitude={}  Frequency={}  Waveform={}",
        channel.name()?,
        channel.property_value("Amplitude")?,
        channel.property_value("Frequency")?,
        channel.property_value("Waveform")?
    );
    Ok(())
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    let device = instance.add_device("daqref://device0")?.expect("device");
    let channels = device.channels()?;
    let (ch0, ch1) = (&channels[0], &channels[1]);

    show_settings("Before:          ", ch0)?;
    show_settings("Before:          ", ch1)?;

    let batch = BatchedPropertyUpdate::new(&[ch0, ch1])?;
    ch0.set_property_value("Amplitude", 2.5)?;
    ch0.set_property_value("Frequency", 25.0)?;
    ch0.set_property_value("Waveform", 1)?;
    ch1.set_property_value("Amplitude", 4.0)?;
    ch1.set_property_value("Frequency", 50.0)?;
    ch1.set_property_value("Waveform", 2)?;
    // Still inside the batch: the writes above are staged but not yet applied.
    show_settings("During (staged): ", ch0)?;
    show_settings("During (staged): ", ch1)?;
    batch.commit()?;

    show_settings("After the batch: ", ch0)?;
    show_settings("After the batch: ", ch1)?;
    Ok(())
}
