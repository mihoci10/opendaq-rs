// A reader is invalidated when its signal's descriptor changes to a sample
// type the reader cannot convert to its configured read type.  The reader
// needs to be recovered by creating a new one via `StreamReader::from_existing`.

use opendaq::{Channel, Instance, Sample, StreamReader};

/// Read four averages with their domain values and print them (retrying while
/// the stream warms up).
fn show_averages<D: Sample + std::fmt::Display>(
    reader: &StreamReader<f64, D>,
) -> opendaq::Result<()> {
    for _ in 0..50 {
        let (averages, ticks) = reader.read_with_domain(4, 1000)?;
        if !averages.is_empty() {
            for (average, tick) in averages.iter().zip(&ticks) {
                println!("  tick {tick}  ->  avg {average:.3}");
            }
            return Ok(());
        }
    }
    panic!("The statistics stream produced no averages.");
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    instance
        .add_device("daqref://device0")?
        .expect("reference device");
    let channel = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel")
        .cast::<Channel>()?;
    channel.set_property_value("Waveform", 0)?;
    channel.set_property_value("Amplitude", 0.0)?;
    channel.set_property_value("DC", 2.0)?;
    channel.set_property_value("NoiseAmplitude", 0.0)?;

    // Average the channel in blocks of 10 samples.  DomainSignalType starts at
    // 0 = Implicit: the output domain is int64 ticks under a linear rule.
    let statistics_fb = instance
        .add_function_block("RefFBModuleStatistics")?
        .expect("statistics function block");
    statistics_fb.input_ports()?[0].connect(&channel.signals()?[0])?;
    let avg_signal = statistics_fb.signals()?[0].clone();

    // Phase 1: read averages with the domain ticks as doubles -- fine while
    // the domain is int64, which converts to float64.
    let reader = StreamReader::<f64, f64>::new(&avg_signal)?;
    println!("implicit int64 domain, read as float64:");
    show_averages(&reader)?;

    // Phase 2: switch the output domain to 2 = ExplicitRange -- each average's
    // domain value becomes the RangeInt64 tick range of the block it covers,
    // and RangeInt64 has no conversion to float64.  The change reaches the
    // reader as a descriptor-changed event in its packet queue; `read` hands
    // over the samples queued before the change, then hits the event and
    // fails with a reader-invalidated error.
    statistics_fb.set_property_value("DomainSignalType", 2)?;
    let mut recovered = None;
    for _ in 0..50 {
        match reader.read_with_domain(100, 200) {
            Ok(_) => continue,
            Err(err) if err.is_reader_invalidated() => {
                println!("{err}");
                // Recover: a new reader with read types matching the new
                // descriptor, inheriting the invalidated reader's connection
                // and unread packets.
                recovered = Some(StreamReader::<f64, i64>::from_existing(&reader)?);
                break;
            }
            Err(err) => return Err(err),
        }
    }
    let recovered = recovered.expect("expected the domain change to invalidate the reader");

    println!("explicit RangeInt64 domain, read as int64 range starts:");
    show_averages(&recovered)?;
    Ok(())
}
