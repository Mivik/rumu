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
    fn fixed_offset() {
        let shanghai = TimeZone::fixed(Offset::from_hours(8).unwrap());
        assert_eq!(render(0, shanghai.clone()), "1970-01-01T08:00:00+08:00");
        assert_eq!(render(1, shanghai.clone()), "1970-01-01T08:00:00+08:00"); // sub-second drops
        assert_eq!(render(86_399_000, shanghai), "1970-01-02T07:59:59+08:00");
        assert_eq!(
            render(1_763_709_600_000, TimeZone::UTC),
            "2025-11-21T07:20:00+00:00"
        );
        assert_eq!(
            render(951_782_400_000, TimeZone::UTC),
            "2000-02-29T00:00:00+00:00"
        ); // leap day
        assert_eq!(render(-1, TimeZone::UTC), "1969-12-31T23:59:59+00:00"); // pre-epoch
    }

    #[test]
    fn dst_transition() {
        // New York springs forward on 2026-03-08 at 02:00 EST → 03:00 EDT.
        let ny = TimeZone::get("America/New_York").unwrap();
        assert_eq!(
            render(1_772_953_199_000, ny.clone()),
            "2026-03-08T01:59:59-05:00"
        );
        assert_eq!(render(1_772_953_200_000, ny), "2026-03-08T03:00:00-04:00");
    }
}
