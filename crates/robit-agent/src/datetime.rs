//! Date and time formatting helpers.

use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};

const DATE_FORMAT: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]");
const ISO8601_FORMAT: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");

/// Get today's date in local time as a string (YYYY-MM-DD).
///
/// Uses the system's local UTC offset (UTC fallback when it cannot be
/// determined — same policy as robit-ai's log rotation), so the prompt's
/// `{date}` and the daily memory filenames (`memory-YYYY-MM-DD.md`) roll at
/// local midnight instead of UTC midnight.
pub fn current_date() -> String {
    date_with_offset(
        OffsetDateTime::now_utc(),
        robit_ai::logging::local_utc_offset(),
    )
}

/// Format `now` as YYYY-MM-DD at the given offset. Split out for testability.
fn date_with_offset(now: OffsetDateTime, offset: UtcOffset) -> String {
    now.to_offset(offset)
        .format(DATE_FORMAT)
        .expect("date format should be valid")
}

/// Get the current UTC timestamp as an ISO 8601-like string.
///
/// Deliberately UTC: these values are persisted as DB timestamps
/// (created_at / updated_at) and must stay comparable with existing rows.
pub fn current_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(ISO8601_FORMAT)
        .expect("timestamp format should be valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn offset(hours: i8) -> UtcOffset {
        UtcOffset::from_hms(hours, 0, 0).unwrap()
    }

    #[test]
    fn date_with_offset_crosses_day_forward() {
        // 2026-09-30 23:00 UTC is already 2026-10-01 in UTC+8
        let now = datetime!(2026-09-30 23:00:00 UTC);
        assert_eq!(date_with_offset(now, UtcOffset::UTC), "2026-09-30");
        assert_eq!(date_with_offset(now, offset(8)), "2026-10-01");
    }

    #[test]
    fn date_with_offset_crosses_day_backward() {
        // 2026-10-01 00:30 UTC is still 2026-09-30 in UTC-1
        let now = datetime!(2026-10-01 00:30:00 UTC);
        assert_eq!(date_with_offset(now, UtcOffset::UTC), "2026-10-01");
        assert_eq!(date_with_offset(now, offset(-1)), "2026-09-30");
    }

    #[test]
    fn current_date_has_expected_shape() {
        let date = current_date();
        assert_eq!(date.len(), 10);
        let bytes = date.as_bytes();
        assert!(bytes[..4].iter().all(u8::is_ascii_digit));
        assert_eq!(bytes[4], b'-');
        assert!(bytes[5..7].iter().all(u8::is_ascii_digit));
        assert_eq!(bytes[7], b'-');
        assert!(bytes[8..].iter().all(u8::is_ascii_digit));
    }
}
