//! Timestamp rendering for the `@rumu` read views.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use rumu_core::Timestamp as Millis;

/// `YYYY-MM-DDTHH:MM:SS±HH:MM` in the system time zone — the display form
/// of timestamps in `@rumu:/sessions` and `@rumu:/tasks` output.
///
/// The numeric offset keeps the string unambiguous and lets the model line
/// these times up with what `date`/`git log` report on the same machine.
/// An unreadable system zone falls back to UTC.
pub(super) fn iso_local(ms: Millis) -> String {
    render(ms, TimeZone::try_system().unwrap_or(TimeZone::UTC))
}

fn render(ms: Millis, tz: TimeZone) -> String {
    Timestamp::from_millisecond(ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned_datetime(tz)
        .strftime("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::render;
    use jiff::tz::{Offset, TimeZone};

    #[test]
    fn renders_whole_seconds_with_the_numeric_offset() {
        let shanghai = TimeZone::fixed(Offset::from_hours(8).unwrap());
        assert_eq!(render(0, shanghai.clone()), "1970-01-01T08:00:00+08:00");
        assert_eq!(render(1, shanghai), "1970-01-01T08:00:00+08:00"); // sub-second drops
    }
}
