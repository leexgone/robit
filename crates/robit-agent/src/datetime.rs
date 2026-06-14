//! Date and time formatting helpers.

use time::macros::format_description;
use time::OffsetDateTime;

const DATE_FORMAT: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]");
const ISO8601_FORMAT: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");

/// Get today's UTC date as a string (YYYY-MM-DD).
pub fn current_date() -> String {
    OffsetDateTime::now_utc()
        .format(DATE_FORMAT)
        .expect("date format should be valid")
}

/// Get the current UTC timestamp as an ISO 8601-like string.
pub fn current_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(ISO8601_FORMAT)
        .expect("timestamp format should be valid")
}
