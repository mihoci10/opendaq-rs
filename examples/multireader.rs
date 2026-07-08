// Read two channels at once with a multi reader, which aligns the signals on
// a common clock so each row of values shares one timestamp.

use opendaq::{Channel, Instance, MultiReader, TickConverter};

/// Render nanoseconds since the Unix epoch as a readable UTC time of day,
/// e.g. "21:05:28.401836".
fn time_of_day(unix_nanos: i128) -> String {
    let micros = unix_nanos.div_euclid(1_000);
    let micro = micros.rem_euclid(1_000_000);
    let s = micros.div_euclid(1_000_000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}.{micro:06}",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

fn find_channel(instance: &Instance, path: &str) -> opendaq::Result<Channel> {
    instance
        .find_component(path)?
        .unwrap_or_else(|| panic!("channel {path} not found"))
        .cast::<Channel>()
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    instance.add_device("daqref://device0")?;

    let channels = [
        find_channel(&instance, "Dev/RefDev0/IO/AI/RefCh0")?,
        find_channel(&instance, "Dev/RefDev0/IO/AI/RefCh1")?,
    ];
    channels[0].set_property_value("Frequency", 0.5)?;
    channels[1].set_property_value("Frequency", 2.0)?;

    let signals = [
        channels[0].signals()?[0].clone(),
        channels[1].signals()?[0].clone(),
    ];
    let reader = MultiReader::<f64>::new(&signals)?;

    // The first reads can return nothing while the reader synchronises the
    // signals onto the common domain; retry until rows arrive.
    for _ in 0..10 {
        let (values, domain) = reader.read_with_domain(8, 1000)?;
        if domain[0].is_empty() {
            continue;
        }
        let converter = TickConverter::from_multi_reader(&reader)?;
        print!("{:<16}", "timestamp");
        for index in 0..values.len() {
            print!("{:>14}", format!("signal {index}"));
        }
        println!();
        for (row, tick) in domain[0].iter().enumerate() {
            print!("{:<16}", time_of_day(converter.tick_to_unix_nanos(*tick)));
            for signal_values in &values {
                print!("{:>14.6}", signal_values[row]);
            }
            println!();
        }
        return Ok(());
    }
    println!("Multi reader did not synchronise in time.");
    Ok(())
}
