//! Trigger model: `once` / `interval` / standard 5-field `cron`.
//!
//! Structured JSON instead of raw RRULE strings (RFC 拍板点 A): parameters
//! carry the semantics, the validation surface stays small, and LLM argument
//! generation is more reliable. Cron expressions evaluate in the task's
//! timezone (IANA via `chrono-tz`, or host local when unset) and convert back
//! to UTC for storage.

use chrono::{DateTime, TimeZone, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// The trigger of a scheduled task. Persisted as JSON in `scheduled_tasks.schedule`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScheduleSpec {
    /// One-shot at an absolute instant. `at` must be RFC3339 with an explicit
    /// offset (`Z` or `±HH:MM`) and in the future at creation time.
    Once { at: String },
    /// Fixed interval. `anchor` (RFC3339, optional) defaults to the task's
    /// creation time; fires at anchor + k·every_secs.
    Interval {
        every_secs: u64,
        #[serde(default)]
        anchor: Option<String>,
    },
    /// Standard 5-field cron: `分 时 日 月 周` (e.g. `0 9 * * 1-5`).
    Cron { expr: String },
}

/// Task timezone: host local when the config string is empty, else IANA.
#[derive(Debug, Clone)]
pub enum TaskTz {
    Local,
    Named(chrono_tz::Tz),
}

/// Resolve a timezone name; empty = host local.
pub fn resolve_tz(name: &str) -> Result<TaskTz, String> {
    if name.trim().is_empty() {
        return Ok(TaskTz::Local);
    }
    name.trim()
        .parse::<chrono_tz::Tz>()
        .map(TaskTz::Named)
        .map_err(|_| format!("Unknown IANA timezone: '{}'", name))
}

impl ScheduleSpec {
    /// Validate at creation/update time. `now` is the current instant;
    /// `min_interval_secs` gates interval granularity and cron effective gap.
    pub fn validate(&self, now: DateTime<Utc>, min_interval_secs: u64) -> Result<(), String> {
        match self {
            Self::Once { at } => {
                let at = parse_offset(at)?;
                if at <= now {
                    return Err(format!(
                        "once.at '{}' is in the past; pick a future instant",
                        at
                    ));
                }
                Ok(())
            }
            Self::Interval { every_secs, anchor } => {
                if *every_secs < min_interval_secs.max(1) {
                    return Err(format!(
                        "interval.every_secs {} is below the minimum {}s",
                        every_secs, min_interval_secs
                    ));
                }
                if let Some(a) = anchor {
                    parse_offset(a)?;
                }
                Ok(())
            }
            Self::Cron { expr } => {
                let schedule = parse_cron(expr)?;
                // Effective-gap guard: the first two upcoming occurrences must
                // be at least min_interval apart (rejects e.g. every-second
                // patterns smuggled through a permissive parser).
                let tz = chrono_tz::UTC;
                let local_now = now.with_timezone(&tz);
                let mut it = schedule.after(&local_now);
                let first = it.next().ok_or_else(|| {
                    format!("cron '{}' has no upcoming occurrence (impossible date?)", expr)
                })?;
                if let Some(second) = it.next() {
                    let gap = (second - first).num_seconds();
                    if gap < min_interval_secs.max(1) as i64 {
                        return Err(format!(
                            "cron '{}' fires every {}s, below the minimum {}s",
                            expr, gap, min_interval_secs
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    /// Next occurrence strictly after `after`. `anchor_default` (the task's
    /// creation time) backs interval specs without an explicit anchor.
    /// Returns `None` when the trigger is exhausted (a spent `once`).
    pub fn next_after(
        &self,
        after: DateTime<Utc>,
        tz: &TaskTz,
        anchor_default: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        match self {
            Self::Once { at } => {
                let at = parse_offset(at).ok()?;
                if at > after {
                    Some(at)
                } else {
                    None
                }
            }
            Self::Interval { every_secs, anchor } => {
                let every = *every_secs.max(&1);
                let anchor = anchor
                    .as_deref()
                    .and_then(|a| parse_offset(a).ok())
                    .unwrap_or(anchor_default);
                if anchor > after {
                    return Some(anchor);
                }
                let diff = (after - anchor).num_seconds();
                let k = diff / every as i64 + 1;
                anchor.checked_add_signed(TimeDelta::seconds(k * every as i64))
            }
            Self::Cron { expr } => {
                let schedule = parse_cron(expr).ok()?;
                match tz {
                    TaskTz::Local => next_cron(&schedule, after, &chrono::Local),
                    TaskTz::Named(tz) => next_cron(&schedule, after, tz),
                }
            }
        }
    }
}

/// Parse an RFC3339 instant that must carry an explicit offset.
fn parse_offset(s: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("Invalid RFC3339 timestamp '{}' (offset required): {}", s, e))
}

/// Normalize a 5-field cron to the 7-field form the `cron` crate expects
/// (sec min hour dom mon dow year) and parse it.
///
/// Weekday convention adapter: standard cron uses 0/7=Sunday, 1=Monday …
/// 6=Saturday, while the `cron` crate (0.15) uses 1=Sunday … 7=Saturday
/// (both numeric and named). The dow field is therefore translated:
/// numeric `n` → `(n % 7) + 1`; names resolve to the same weekdays and are
/// normalized to numbers first. Wrap-around ranges (`5-1` = Fri…Mon) are
/// split into two crate ranges; wrap-around ranges with a step ≠ 1 are
/// rejected (modular step enumeration cannot be expressed as two ranges —
/// use a list instead).
fn parse_cron(expr: &str) -> Result<cron::Schedule, String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!(
            "cron must have exactly 5 fields (分 时 日 月 周), got {}: '{}'",
            fields.len(),
            expr
        ));
    }
    let dow = translate_dow(fields[4])?;
    let normalized = format!("0 {} {} {} {} {} *", fields[0], fields[1], fields[2], fields[3], dow);
    normalized
        .parse::<cron::Schedule>()
        .map_err(|e| format!("Invalid cron '{}': {}", expr, e))
}

/// Standard weekday name → standard ordinal (0=Sunday … 6=Saturday).
fn std_weekday_name(s: &str) -> Option<u32> {
    match s.to_ascii_lowercase().as_str() {
        "sun" | "sunday" => Some(0),
        "mon" | "monday" => Some(1),
        "tue" | "tuesday" => Some(2),
        "wed" | "wednesday" => Some(3),
        "thu" | "thursday" => Some(4),
        "fri" | "friday" => Some(5),
        "sat" | "saturday" => Some(6),
        _ => None,
    }
}

/// One dow endpoint (number or name) → crate ordinal (1=Sunday … 7=Saturday).
fn dow_endpoint(s: &str) -> Result<u32, String> {
    let std = if let Ok(n) = s.parse::<u32>() {
        if n > 7 {
            return Err(format!("weekday '{}' out of range 0-7", s));
        }
        n % 7
    } else {
        std_weekday_name(s).ok_or_else(|| format!("unknown weekday '{}'", s))?
    };
    Ok(std + 1)
}

/// Translate the standard dow field to the crate convention.
fn translate_dow(field: &str) -> Result<String, String> {
    let mut out: Vec<String> = Vec::new();
    for item in field.split(',') {
        let (base, step) = match item.split_once('/') {
            Some((b, s)) => {
                let n: u32 = s
                    .parse()
                    .map_err(|_| format!("invalid step in weekday item '{}'", item))?;
                if n == 0 {
                    return Err(format!("zero step in weekday item '{}'", item));
                }
                (b, Some(n))
            }
            None => (item, None),
        };
        if base == "*" {
            // */n over std 0-6 maps exactly to */n over crate 1-7 (+1 shift).
            out.push(match step {
                Some(n) => format!("*/{}", n),
                None => "*".to_string(),
            });
            continue;
        }
        if let Some((a, b)) = base.split_once('-') {
            let a = dow_endpoint(a)?;
            let b = dow_endpoint(b)?;
            let step_s = step.map(|n| format!("/{}", n)).unwrap_or_default();
            if a <= b {
                out.push(format!("{}-{}{}", a, b, step_s));
            } else if step.unwrap_or(1) == 1 {
                // Wrap-around: split into two ranges.
                out.push(format!("{}-7", a));
                out.push(format!("1-{}", b));
            } else {
                return Err(format!(
                    "wrap-around weekday range with step ('{}') is not supported; use a list",
                    item
                ));
            }
            continue;
        }
        let a = dow_endpoint(base)?;
        out.push(match step {
            Some(n) => format!("{}/{}", a, n),
            None => a.to_string(),
        });
    }
    Ok(out.join(","))
}

/// Evaluate the next cron occurrence in `tz`, returned as UTC.
fn next_cron<Tz>(schedule: &cron::Schedule, after: DateTime<Utc>, tz: &Tz) -> Option<DateTime<Utc>>
where
    Tz: TimeZone,
    Tz::Offset: Copy,
{
    let local_after = after.with_timezone(tz);
    schedule
        .after(&local_after)
        .next()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn once_future_and_past() {
        let now = utc(2026, 11, 24, 9, 0);
        let spec = ScheduleSpec::Once {
            at: "2026-11-25T09:00:00+08:00".into(),
        };
        assert!(spec.validate(now, 60).is_ok());
        let next = spec.next_after(now, &TaskTz::Local, now).unwrap();
        assert_eq!(next, utc(2026, 11, 25, 1, 0));
        // Spent once → None.
        assert!(spec.next_after(next, &TaskTz::Local, now).is_none());
        // Past at creation → validation error.
        let past = ScheduleSpec::Once {
            at: "2026-11-23T09:00:00Z".into(),
        };
        assert!(past.validate(now, 60).is_err());
        // Offset-less timestamp rejected.
        let naive = ScheduleSpec::Once {
            at: "2026-11-25T09:00:00".into(),
        };
        assert!(naive.validate(now, 60).is_err());
    }

    #[test]
    fn interval_grid_from_anchor() {
        let now = utc(2026, 11, 24, 9, 0);
        let spec = ScheduleSpec::Interval {
            every_secs: 3600,
            anchor: Some("2026-11-24T08:30:00Z".into()),
        };
        assert!(spec.validate(now, 60).is_ok());
        // 09:00 → next grid point 09:30.
        assert_eq!(
            spec.next_after(now, &TaskTz::Local, now).unwrap(),
            utc(2026, 11, 24, 9, 30)
        );
        // Exactly on a grid point → strictly after, next one.
        let on_grid = utc(2026, 11, 24, 9, 30);
        assert_eq!(
            spec.next_after(on_grid, &TaskTz::Local, now).unwrap(),
            utc(2026, 11, 24, 10, 30)
        );
        // Before anchor → anchor itself.
        let early = utc(2026, 11, 24, 7, 0);
        assert_eq!(
            spec.next_after(early, &TaskTz::Local, now).unwrap(),
            utc(2026, 11, 24, 8, 30)
        );
    }

    #[test]
    fn interval_defaults_to_created_at() {
        let created = utc(2026, 11, 24, 9, 0);
        let spec = ScheduleSpec::Interval {
            every_secs: 60,
            anchor: None,
        };
        assert_eq!(
            spec.next_after(created, &TaskTz::Local, created).unwrap(),
            utc(2026, 11, 24, 9, 1)
        );
    }

    #[test]
    fn interval_below_minimum_rejected() {
        let now = utc(2026, 11, 24, 9, 0);
        let spec = ScheduleSpec::Interval {
            every_secs: 5,
            anchor: None,
        };
        assert!(spec.validate(now, 60).is_err());
    }

    #[test]
    fn cron_weekday_morning_in_timezone() {
        let now = utc(2026, 11, 24, 9, 0); // Tuesday 17:00 +08:00
        let spec = ScheduleSpec::Cron {
            expr: "0 9 * * 1-5".into(),
        };
        assert!(spec.validate(now, 60).is_ok());
        let tz = resolve_tz("Asia/Shanghai").unwrap();
        // Next weekday 09:00 +08:00 = Wed 01:00 UTC.
        assert_eq!(
            spec.next_after(now, &tz, now).unwrap(),
            utc(2026, 11, 25, 1, 0)
        );
    }

    #[test]
    fn cron_skips_weekend() {
        // Friday 2026-11-27 10:00 +08:00 → next Mon 09:00.
        let now = utc(2026, 11, 27, 2, 0);
        let spec = ScheduleSpec::Cron {
            expr: "0 9 * * 1-5".into(),
        };
        let tz = resolve_tz("Asia/Shanghai").unwrap();
        assert_eq!(
            spec.next_after(now, &tz, now).unwrap(),
            utc(2026, 11, 30, 1, 0)
        );
    }

    #[test]
    fn cron_every_minute_gap_ok_every_second_rejected() {
        let now = utc(2026, 11, 24, 9, 0);
        let minute = ScheduleSpec::Cron {
            expr: "* * * * *".into(),
        };
        assert!(minute.validate(now, 60).is_ok());
        // 6-field input is rejected (5-field contract).
        let six = ScheduleSpec::Cron {
            expr: "*/10 * * * * *".into(),
        };
        assert!(six.validate(now, 60).is_err());
        // Impossible date → no occurrence → error.
        let impossible = ScheduleSpec::Cron {
            expr: "0 0 30 2 *".into(),
        };
        assert!(impossible.validate(now, 60).is_err());
    }

    #[test]
    fn cron_invalid_expr_rejected() {
        let now = utc(2026, 11, 24, 9, 0);
        let bad = ScheduleSpec::Cron {
            expr: "99 * * * *".into(),
        };
        assert!(bad.validate(now, 60).is_err());
    }

    #[test]
    fn dow_translation_names_lists_and_wrap() {
        // Names mean the same weekdays as standard cron.
        assert_eq!(translate_dow("MON-FRI").unwrap(), "2-6");
        assert_eq!(translate_dow("sun").unwrap(), "1");
        assert_eq!(translate_dow("7").unwrap(), "1"); // 7 = Sunday too
        assert_eq!(translate_dow("0,6").unwrap(), "1,7"); // Sun,Sat
        assert_eq!(translate_dow("1-5/2").unwrap(), "2-6/2"); // Mon,Wed,Fri
        // Wrap-around range splits into two crate ranges.
        assert_eq!(translate_dow("5-1").unwrap(), "6-7,1-2"); // Fri…Mon
        // Wrap + step ≠ 1 is rejected with guidance.
        assert!(translate_dow("5-1/2").is_err());
        assert!(translate_dow("8").is_err());
        assert!(translate_dow("funday").is_err());
    }

    #[test]
    fn cron_named_weekdays_skip_weekend() {
        // Friday 2026-11-27 10:00 +08:00 → next MON 09:00.
        let now = utc(2026, 11, 27, 2, 0);
        let spec = ScheduleSpec::Cron {
            expr: "0 9 * * MON-FRI".into(),
        };
        let tz = resolve_tz("Asia/Shanghai").unwrap();
        assert_eq!(
            spec.next_after(now, &tz, now).unwrap(),
            utc(2026, 11, 30, 1, 0)
        );
    }

    #[test]
    fn cron_wrap_range_fires_on_sunday() {
        // dow 5-1 = Fri,Sat,Sun,Mon. After Wed 2026-11-25 → Fri 27.
        let now = utc(2026, 11, 25, 2, 0);
        let spec = ScheduleSpec::Cron {
            expr: "0 9 * * 5-1".into(),
        };
        let tz = resolve_tz("Asia/Shanghai").unwrap();
        assert_eq!(
            spec.next_after(now, &tz, now).unwrap(),
            utc(2026, 11, 27, 1, 0)
        );
        // After Fri 10:00 → Sat 28 (wrap range includes the weekend).
        let fri = utc(2026, 11, 27, 2, 0);
        assert_eq!(
            spec.next_after(fri, &tz, fri).unwrap(),
            utc(2026, 11, 28, 1, 0)
        );
    }

    #[test]
    fn tz_resolution() {
        assert!(matches!(resolve_tz("").unwrap(), TaskTz::Local));
        assert!(matches!(resolve_tz("Asia/Shanghai").unwrap(), TaskTz::Named(_)));
        assert!(resolve_tz("Mars/Olympus").is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let spec = ScheduleSpec::Cron {
            expr: "0 9 * * 1-5".into(),
        };
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(json["kind"], "cron");
        let back: ScheduleSpec = serde_json::from_value(json).unwrap();
        assert_eq!(back, spec);
    }
}
