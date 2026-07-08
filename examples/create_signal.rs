// Build a signal by hand and read it back with wall-clock timestamps.
//
// The trick for making the domain timestamps line up with the PC clock is NOT
// to move the origin to "now": the origin stays pinned at the Unix epoch, and
// the current time is carried entirely by the integer domain *ticks*.  With a
// tick resolution of 1/1000 s (one tick = one millisecond) a sample's absolute
// time is  origin + tick/1000 s, so a tick equal to the current Unix time in
// milliseconds reads back as the current wall-clock time.  Spacing the ticks
// 200 apart (200 ms) then yields exactly 5 samples per second.

use std::f64::consts::PI;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opendaq::{
    DataDescriptor, DataDescriptorBuilder, DataPacket, DataRule, Instance, Ratio, ReadTimeoutType,
    SampleType, SignalConfig, StreamReader, StreamReaderOptions, TickConverter, Unit,
};

const SAMPLES_PER_SECOND: usize = 5;
const TICK_RESOLUTION: Ratio = Ratio::new(1, 1000); // seconds per tick (1 ms)
const TICKS_PER_SAMPLE: i64 = 200; // 1 / (5 samples/s * 1/1000 s/tick) = 200 ticks = 200 ms

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

/// The current PC time as an integer tick count (milliseconds since the Unix
/// epoch, i.e. since the domain origin).
fn now_ticks() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before 1970")
        .as_millis() as i64
}

/// Send `samples` as one packet whose implicit domain ticks start at `offset`
/// and advance by `TICKS_PER_SAMPLE` per sample.
fn send_chunk(
    signal: &SignalConfig,
    domain_descriptor: &DataDescriptor,
    value_descriptor: &DataDescriptor,
    offset: i64,
    samples: &[f64],
) -> opendaq::Result<()> {
    let count = samples.len();
    let domain_packet = DataPacket::new(domain_descriptor, count, offset)?;
    let packet = DataPacket::with_domain(&domain_packet, value_descriptor, count, 0)?;
    packet.set_data::<f64>(samples)?;
    signal.send_packet(&packet)
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    let context = instance.context()?.expect("instance context");

    let domain_descriptor = {
        let builder = DataDescriptorBuilder::new()?;
        builder.set_sample_type(SampleType::Int64)?;
        builder.set_name("time")?;
        // Origin stays at the epoch; the wall-clock time lives in the ticks.
        builder.set_origin("1970-01-01T00:00:00Z")?;
        builder.set_tick_resolution(TICK_RESOLUTION)?;
        builder.set_unit(&Unit::new(-1, "s", "second", "time")?)?;
        builder.set_rule(&DataRule::linear(TICKS_PER_SAMPLE, 0)?)?;
        builder.build()?.expect("domain descriptor")
    };
    let value_descriptor = {
        let builder = DataDescriptorBuilder::new()?;
        builder.set_sample_type(SampleType::Float64)?;
        builder.set_name("values")?;
        builder.build()?.expect("value descriptor")
    };

    let domain_signal = SignalConfig::signal(&context, None, "time", None)?;
    domain_signal.set_descriptor(&domain_descriptor)?;
    let signal = SignalConfig::signal(&context, None, "values", None)?;
    signal.set_descriptor(&value_descriptor)?;
    signal.set_domain_signal(&domain_signal)?;

    let reader = StreamReader::<f64, i64>::with_options(
        &signal,
        StreamReaderOptions {
            timeout_type: ReadTimeoutType::Any,
            ..Default::default()
        },
    )?;

    // Stream a few seconds of a 5 Hz signal in real time.  Each one-second
    // batch is one packet of 5 samples; the ticks stay contiguous across
    // batches (batch k starts at start_tick + k*1000 ms), so the whole run is
    // an unbroken 5 Hz stream anchored to when the program started.
    let batches = 3;
    let start_tick = now_ticks();
    let to_timestamp = TickConverter::from_signal(&signal)?;
    println!(
        "Streaming {SAMPLES_PER_SECOND} samples/second, starting at {}",
        time_of_day(to_timestamp.tick_to_unix_nanos(start_tick))
    );
    for batch in 0..batches {
        let base_sample = batch * SAMPLES_PER_SECOND;
        let offset = start_tick + base_sample as i64 * TICKS_PER_SAMPLE;
        let samples: Vec<f64> = (0..SAMPLES_PER_SECOND)
            .map(|i| (2.0 * PI * ((base_sample + i) as f64 / 25.0)).sin())
            .collect();
        send_chunk(
            &signal,
            &domain_descriptor,
            &value_descriptor,
            offset,
            &samples,
        )?;

        let (values, ticks) = reader.read_with_domain(100, 1000)?;
        for (value, tick) in values.iter().zip(&ticks) {
            println!(
                "  {}  ->  {value:.4}",
                time_of_day(to_timestamp.tick_to_unix_nanos(*tick))
            );
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}
