// Read two linear-domain signals and one explicit-domain signal at the same
// time, and line all three up on a shared wall-clock time axis.
//
//   * The two linear signals are the reference device's analog channels.
//     Their domain is an implicit linear rule (tick = start + n*delta), and we
//     read them together with a `MultiReader`, which hands back one common
//     domain.
//
//   * The explicit-domain signal is the block average of channel 0 produced
//     by a Statistics function block with DomainSignalType = Explicit:
//     instead of a linear rule it emits, per average, the actual tick of the
//     first raw sample in its block.  We read it with a `StreamReader`.
//
// All three ride the same device clock, so every average's explicit tick
// equals one of the channel ticks -- convert both to timestamps and the
// columns line up exactly, one average every BLOCK_SIZE channel samples.

use std::collections::HashMap;
use std::time::Duration;

use opendaq::{Channel, Instance, MultiReader, StreamReader, TickConverter};

const SAMPLE_RATE: f64 = 100.0; // Hz, device-wide
const BLOCK_SIZE: i64 = 10; // channel samples per average
const WINDOW: usize = 30; // channel samples to display
const CHUNK: usize = 5; // samples per read

/// Render nanoseconds since the Unix epoch as a readable UTC time-of-day,
/// e.g. "21:05:28.400000".
fn time_of_day(unix_nanos: i128) -> String {
    let nanos = unix_nanos.rem_euclid(86_400 * 1_000_000_000);
    let seconds = nanos / 1_000_000_000;
    let micros = (nanos % 1_000_000_000) / 1_000;
    format!(
        "{:02}:{:02}:{:02}.{micros:06}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    let device = instance
        .add_device("daqref://device0")?
        .expect("reference device");
    device.set_property_value("GlobalSampleRate", SAMPLE_RATE)?;

    let ch0 = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel 0")
        .cast::<Channel>()?;
    let ch1 = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh1")?
        .expect("reference channel 1")
        .cast::<Channel>()?;
    ch0.set_property_value("Frequency", 5.0)?;
    ch1.set_property_value("Frequency", 10.0)?;
    let ch0_signal = ch0.signals()?[0].clone();
    let ch1_signal = ch1.signals()?[0].clone();

    // Statistics block averaging channel 0 in blocks of BLOCK_SIZE samples.
    // DomainSignalType = 1 (Explicit) makes its output domain an explicit list
    // of ticks -- one tick per average, taken from the block's first raw sample.
    let stats_fb = instance
        .add_function_block("RefFBModuleStatistics")?
        .expect("statistics function block");
    stats_fb.set_property_value("BlockSize", BLOCK_SIZE)?;
    stats_fb.set_property_value("DomainSignalType", 1)?;
    stats_fb.input_ports()?[0].connect(&ch0_signal)?;
    let avg_signal = stats_fb.signals()?[0].clone();

    let multi_reader = MultiReader::<f64, i64>::new(&[ch0_signal, ch1_signal])?;
    let stream_reader = StreamReader::<f64, i64>::new(&avg_signal)?;

    // Let both streams warm up so their queues overlap in time.
    std::thread::sleep(Duration::from_secs(1));

    // Fill the display window with many small reads rather than one big one,
    // interleaving the two readers so they advance together.  Channel samples
    // are collected until the window is full; averages go into a tick->value
    // table and keep being read until they have caught up to the end of the
    // window -- an average only appears once its whole block of channel
    // samples has arrived.
    let mut rows: Vec<(i64, f64, f64)> = Vec::new(); // (tick, v0, v1) per channel sample
    let mut avg_by_tick: HashMap<i64, f64> = HashMap::new();
    let mut newest_avg_tick: i64 = -1;
    for _ in 0..200 {
        let window_full = rows.len() >= WINDOW;
        if window_full && newest_avg_tick >= rows[WINDOW - 1].0 {
            break;
        }
        if !window_full {
            let (values, domain) = multi_reader.read_with_domain(CHUNK, 1000)?;
            for i in 0..domain[0].len() {
                rows.push((domain[0][i], values[0][i], values[1][i]));
            }
        }
        let (averages, ticks) = stream_reader.read_with_domain(CHUNK, 1000)?;
        for (average, tick) in averages.iter().zip(&ticks) {
            avg_by_tick.insert(*tick, *average);
            newest_avg_tick = newest_avg_tick.max(*tick);
        }
    }

    // The multi-reader only knows its common domain once it has read, so build
    // the tick->time converter now.
    let channel_time = TickConverter::from_multi_reader(&multi_reader)?;
    println!(
        "{:<14}{:>14}{:>14}{:>16}",
        "time", "channel 0", "channel 1", "avg(channel 0)"
    );
    for &(tick, v0, v1) in rows.iter().take(WINDOW) {
        let average = avg_by_tick
            .get(&tick)
            .map(|a| format!("{a:.4}"))
            .unwrap_or_default();
        println!(
            "{:<14}{v0:>14.4}{v1:>14.4}{average:>16}",
            time_of_day(channel_time.tick_to_unix_nanos(tick))
        );
    }
    Ok(())
}
