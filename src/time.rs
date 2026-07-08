//! Converting domain ticks to wall-clock time.
//!
//! A domain signal reports plain integer ticks.  Turning a tick into absolute
//! time needs two pieces of metadata: the domain origin (an ISO-8601 epoch
//! string) and the tick resolution (seconds per tick, a ratio), so that
//! `absolute time = origin + tick * resolution`.  [`TickConverter`] reads the
//! metadata once and converts any number of ticks into [`SystemTime`]s.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::generated::{DataDescriptor, Signal};
use crate::readers::{MultiReader, Sample};
use crate::sys;
use crate::value::Ratio;

/// Converts integer domain ticks into absolute [`SystemTime`]s.  Reads the
/// origin and tick resolution once at construction, so build it once per
/// domain and reuse it for every tick.
#[derive(Debug, Clone, Copy)]
pub struct TickConverter {
    origin_unix_ns: i128,
    resolution: Ratio,
}

impl TickConverter {
    /// Build a converter from a domain [`DataDescriptor`]'s origin and tick
    /// resolution (an unset origin counts as the Unix epoch; an unset
    /// resolution as 1 second per tick).
    pub fn from_descriptor(descriptor: &DataDescriptor) -> Result<TickConverter> {
        let origin = descriptor.origin()?;
        let resolution = descriptor.tick_resolution()?.unwrap_or(Ratio::new(1, 1));
        Self::from_parts(&origin, resolution)
    }

    /// Build a converter for a value signal from its domain signal's
    /// descriptor.
    pub fn from_signal(signal: &Signal) -> Result<TickConverter> {
        let op = "TickConverter::from_signal";
        let domain = signal.domain_signal()?.ok_or_else(|| {
            Error::new(
                sys::OPENDAQ_ERR_NOTASSIGNED,
                op,
                Some("signal has no domain signal".into()),
            )
        })?;
        let descriptor = domain.descriptor()?.ok_or_else(|| {
            Error::new(
                sys::OPENDAQ_ERR_NOTASSIGNED,
                op,
                Some("domain signal has no descriptor".into()),
            )
        })?;
        Self::from_descriptor(&descriptor)
    }

    /// Build a converter from a [`MultiReader`]'s common domain.
    pub fn from_multi_reader<V: Sample, D: Sample>(
        reader: &MultiReader<V, D>,
    ) -> Result<TickConverter> {
        let origin = reader.origin()?;
        let resolution = reader.tick_resolution()?.unwrap_or(Ratio::new(1, 1));
        Self::from_parts(&origin, resolution)
    }

    /// Build a converter from a raw ISO-8601 `origin` string (empty counts as
    /// the Unix epoch) and a tick `resolution` in seconds per tick.
    pub fn from_parts(origin: &str, resolution: Ratio) -> Result<TickConverter> {
        let origin_unix_ns = if origin.trim().is_empty() {
            0
        } else {
            parse_iso8601_unix_ns(origin.trim()).ok_or_else(|| {
                Error::new(
                    sys::OPENDAQ_ERR_CONVERSIONFAILED,
                    "TickConverter",
                    Some(format!(
                        "cannot parse domain origin {origin:?} as an ISO-8601 timestamp"
                    )),
                )
            })?
        };
        Ok(TickConverter {
            origin_unix_ns,
            resolution,
        })
    }

    /// The absolute time of `tick` as nanoseconds since the Unix epoch.
    pub fn tick_to_unix_nanos(&self, tick: i64) -> i128 {
        let numerator = self.resolution.numerator as i128 * 1_000_000_000;
        let denominator = self.resolution.denominator.max(1) as i128;
        self.origin_unix_ns + (tick as i128 * numerator) / denominator
    }

    /// The absolute time of `tick`.
    pub fn tick_to_system_time(&self, tick: i64) -> SystemTime {
        let nanos = self.tick_to_unix_nanos(tick);
        if nanos >= 0 {
            UNIX_EPOCH + Duration::from_nanos(nanos as u64)
        } else {
            UNIX_EPOCH - Duration::from_nanos(nanos.unsigned_abs() as u64)
        }
    }
}

/// Days from the civil epoch 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let d = day as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Parse an ISO-8601 timestamp (`YYYY-MM-DD[Thh:mm[:ss[.fff...]]][Z|±hh[:mm]]`)
/// into nanoseconds since the Unix epoch.  Hand-rolled to keep the crate free
/// of a date-time dependency; anything unparseable yields `None`.
fn parse_iso8601_unix_ns(text: &str) -> Option<i128> {
    let digits = |s: &str| -> Option<i64> { s.parse().ok() };

    let (date_part, rest) = match text.find(['T', 't', ' ']) {
        Some(index) => (&text[..index], &text[index + 1..]),
        None => (text, ""),
    };
    let mut date_iter = date_part.splitn(3, '-');
    let year = digits(date_iter.next()?)?;
    let month: u32 = date_iter.next()?.parse().ok()?;
    let day: u32 = date_iter.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (time_part, offset_ns) = split_utc_offset(rest)?;
    let mut seconds_of_day: i64 = 0;
    let mut fraction_ns: i64 = 0;
    if !time_part.is_empty() {
        let mut time_iter = time_part.splitn(3, ':');
        let hour = digits(time_iter.next()?)?;
        let minute = digits(time_iter.next().unwrap_or("0"))?;
        let second_text = time_iter.next().unwrap_or("0");
        let (whole, fraction) = match second_text.split_once('.') {
            Some((w, f)) => (w, f),
            None => (second_text, ""),
        };
        let second = digits(whole)?;
        if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..=60).contains(&second) {
            return None;
        }
        if !fraction.is_empty() {
            let trimmed: String = fraction.chars().take(9).collect();
            if !trimmed.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            fraction_ns = trimmed.parse::<i64>().ok()? * 10_i64.pow(9 - trimmed.len() as u32);
        }
        seconds_of_day = hour * 3600 + minute * 60 + second;
    }

    let days = days_from_civil(year, month, day);
    Some(
        (days as i128) * 86_400_000_000_000
            + (seconds_of_day as i128) * 1_000_000_000
            + fraction_ns as i128
            - offset_ns,
    )
}

/// Split a time-with-offset string into the local time part and the UTC
/// offset in nanoseconds (`Z`, `+hh:mm`, `+hhmm`, `+hh`, or none).
fn split_utc_offset(time: &str) -> Option<(&str, i128)> {
    if time.is_empty() {
        return Some((time, 0));
    }
    if let Some(stripped) = time.strip_suffix(['Z', 'z']) {
        return Some((stripped, 0));
    }
    if let Some(index) = time.rfind(['+', '-']) {
        // A '-' inside the time part cannot occur, so any sign starts an offset.
        let (local, offset) = (&time[..index], &time[index..]);
        let sign: i128 = if offset.starts_with('-') { -1 } else { 1 };
        let body = &offset[1..];
        let (hours, minutes) = match body.split_once(':') {
            Some((h, m)) => (h.parse::<i128>().ok()?, m.parse::<i128>().ok()?),
            None if body.len() == 4 => (body[..2].parse().ok()?, body[2..].parse().ok()?),
            None => (body.parse::<i128>().ok()?, 0),
        };
        return Some((local, sign * (hours * 3600 + minutes * 60) * 1_000_000_000));
    }
    Some((time, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_epoch() {
        assert_eq!(parse_iso8601_unix_ns("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601_unix_ns("1970-01-01"), Some(0));
    }

    #[test]
    fn parses_fraction_and_offset() {
        assert_eq!(
            parse_iso8601_unix_ns("1970-01-01T00:00:01.5Z"),
            Some(1_500_000_000)
        );
        assert_eq!(parse_iso8601_unix_ns("1970-01-01T01:00:00+01:00"), Some(0),);
        assert_eq!(parse_iso8601_unix_ns("1969-12-31T23:00:00-01:00"), Some(0));
    }

    #[test]
    fn parses_modern_date() {
        // 2024-01-01T00:00:00Z = 1704067200 seconds.
        assert_eq!(
            parse_iso8601_unix_ns("2024-01-01T00:00:00Z"),
            Some(1_704_067_200_000_000_000)
        );
    }

    #[test]
    fn converts_ticks() {
        let converter =
            TickConverter::from_parts("1970-01-01T00:00:00Z", Ratio::new(1, 1000)).unwrap();
        assert_eq!(converter.tick_to_unix_nanos(1500), 1_500_000_000);
    }
}
