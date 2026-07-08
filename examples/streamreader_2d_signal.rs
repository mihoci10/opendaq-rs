// Read a dimensioned ("2-D") signal: feed a sine wave into the reference FFT
// function block and read amplitude spectra, where every sample is a whole
// row of frequency bins.

use opendaq::{Channel, Instance, StreamReader};

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    instance.add_device("daqref://device0")?;

    let channel = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;
    channel.set_property_value("Waveform", 0)?; // 0 = Sine
    channel.set_property_value("Frequency", 125.0)?;
    channel.set_property_value("Amplitude", 5.0)?;
    channel.set_property_value("NoiseAmplitude", 0.1)?;

    let fft = instance
        .add_function_block("RefFBModuleFFT")?
        .expect("FFT function block not found");
    fft.set_property_value("BlockSize", 16)?;
    fft.input_ports()?[0].connect(&channel.signals()?[0])?;
    let signal = &fft.signals()?[0];

    // Wait for the block to publish its output descriptor, then read the
    // frequency axis off the value descriptor's single dimension.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let descriptor = signal
        .descriptor()?
        .expect("output descriptor not published");
    let dimension = &descriptor.dimensions()?[0];
    let axis: Vec<f64> = dimension
        .labels()?
        .iter()
        .map(|label| label.as_f64().expect("numeric frequency label"))
        .collect();

    // Read 5 samples.  Each sample is a full spectrum, so `read` returns a
    // (samples x bins) matrix; retry until 5 rows have arrived (the first
    // reads may come back short while the stream warms up).
    let reader = StreamReader::<f64>::new(signal)?;
    let mut spectra = reader.read(5, 1000)?;
    for _ in 0..50 {
        if spectra.sample_count() == 5 {
            break;
        }
        spectra = reader.read(5, 1000)?;
    }

    // Print the axis down the rows and one column of amplitudes per sample.
    // The 125 Hz tone dominates a single bin (~5, our amplitude) while noise
    // fills the rest with small values.  The reference block labels its bins
    // one step (31.25 Hz) below the true bin centre, so the tone lands in the
    // 93.75 Hz row.
    println!(
        "{} spectrum, {} bins, {}\n",
        dimension.name()?,
        axis.len(),
        dimension.unit()?.expect("dimension unit").symbol()?
    );
    print!("{:>12}", "freq (Hz)");
    for sample in 1..=spectra.sample_count() {
        print!("{:>14}", format!("sample {sample}"));
    }
    println!();
    let rows: Vec<&[f64]> = spectra.rows().collect();
    for (bin, frequency) in axis.iter().enumerate() {
        print!("{frequency:>12.2}");
        for row in &rows {
            print!("{:>14.4}", row[bin]);
        }
        println!();
    }
    Ok(())
}
