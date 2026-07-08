// Read samples together with their domain (time) stamps and print each
// sample against its absolute wall-clock timestamp.
//
// The domain signal delivers plain integer ticks; a `TickConverter` reads the
// domain's origin and tick resolution once and turns every tick into
// nanoseconds since the Unix epoch.

use opendaq::{Channel, Instance, StreamReader, TickConverter};

/// Render nanoseconds since the Unix epoch as e.g. "2026-06-16 21:03:22.060001 UTC".
fn timestamp_to_string(unix_nanos: i128) -> String {
    let micros = unix_nanos.div_euclid(1_000);
    let micro = micros.rem_euclid(1_000_000);
    let secs = micros.div_euclid(1_000_000) as i64;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{micro:06} UTC",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    instance.add_device("daqref://device0")?;

    let channel = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;
    let signal = &channel.signals()?[0];

    let reader = StreamReader::<f64>::new(signal)?;
    let converter = TickConverter::from_signal(signal)?;

    let (values, ticks) = reader.read_with_domain(10, 2000)?;
    println!("Read {} samples:", values.sample_count());
    for (value, tick) in values.iter().zip(&ticks) {
        println!(
            "  {value:.6} @ {}",
            timestamp_to_string(converter.tick_to_unix_nanos(*tick))
        );
    }
    Ok(())
}
