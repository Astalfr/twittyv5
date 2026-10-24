//! Display formatting for times and counters, matching X's conventions.

use chrono::{DateTime, Datelike, TimeZone, Utc};

/// Short relative timestamp: `now`, `42s`, `5m`, `3h`, `Oct 21`, `Oct 21, 2025`.
pub fn relative_time(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - at).num_seconds();
    match secs {
        i64::MIN..=4 => "now".to_owned(),
        5..=59 => format!("{secs}s"),
        60..=3_599 => format!("{}m", secs / 60),
        3_600..=86_399 => format!("{}h", secs / 3_600),
        _ if at.year() == now.year() => at.format("%b %-d").to_string(),
        _ => at.format("%b %-d, %Y").to_string(),
    }
}

/// Full timestamp in `tz`, 24-hour: `2026-10-24 15:45`.
pub fn exact_time<Tz: TimeZone>(at: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    at.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string()
}

/// Compact counter: `999`, `1.2K`, `12K`, `1.2M`. Zero renders as empty.
pub fn compact_count(n: u64) -> String {
    fn scaled(n: u64, unit: u64, suffix: &str) -> String {
        let whole = n / unit;
        if whole >= 10 {
            format!("{whole}{suffix}")
        } else {
            let tenths = (n % unit) * 10 / unit;
            if tenths == 0 {
                format!("{whole}{suffix}")
            } else {
                format!("{whole}.{tenths}{suffix}")
            }
        }
    }
    match n {
        0 => String::new(),
        1..=999 => n.to_string(),
        1_000..=999_999 => scaled(n, 1_000, "K"),
        1_000_000..=999_999_999 => scaled(n, 1_000_000, "M"),
        _ => scaled(n, 1_000_000_000, "B"),
    }
}

/// Full counter with its noun and grouped thousands: `1 Like`, `12,345 Likes`.
pub fn exact_count(n: u64, one: &str, many: &str) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap()
    }

    #[test]
    fn exact_time_is_in_the_given_zone() {
        assert_eq!(exact_time(now(), &Utc), "2026-10-24 12:00");
        let paris = chrono::FixedOffset::east_opt(2 * 3_600).unwrap();
        assert_eq!(exact_time(now(), &paris), "2026-10-24 14:00");
        let late = Utc.with_ymd_and_hms(2026, 1, 5, 23, 7, 0).unwrap();
        assert_eq!(exact_time(late, &paris), "2026-01-06 01:07");
    }

    #[test]
    fn relative_time_buckets() {
        let n = now();
        let cases = [
            (Duration::seconds(-30), "now"), // clock skew: future timestamps
            (Duration::seconds(2), "now"),
            (Duration::seconds(42), "42s"),
            (Duration::minutes(5), "5m"),
            (Duration::minutes(59), "59m"),
            (Duration::hours(3), "3h"),
            (Duration::hours(23), "23h"),
            (Duration::days(3), "Oct 21"),
            (Duration::days(400), "Sep 19, 2025"),
        ];
        for (ago, expected) in cases {
            assert_eq!(relative_time(n - ago, n), expected, "for {ago:?}");
        }
    }

    #[test]
    fn compact_count_matches_x_style() {
        let cases = [
            (0, ""),
            (7, "7"),
            (999, "999"),
            (1_000, "1K"),
            (1_250, "1.2K"),
            (9_999, "9.9K"),
            (12_345, "12K"),
            (999_999, "999K"),
            (1_000_000, "1M"),
            (1_550_000, "1.5M"),
            (42_000_000, "42M"),
            (3_100_000_000, "3.1B"),
        ];
        for (n, expected) in cases {
            assert_eq!(compact_count(n), expected, "for {n}");
        }
    }

    #[test]
    fn exact_count_groups_thousands() {
        let cases = [
            (0, "0 Likes"),
            (1, "1 Like"),
            (999, "999 Likes"),
            (1_000, "1,000 Likes"),
            (12_345, "12,345 Likes"),
            (1_234_567, "1,234,567 Likes"),
            (u64::MAX, "18,446,744,073,709,551,615 Likes"),
        ];
        for (n, expected) in cases {
            assert_eq!(exact_count(n, "Like", "Likes"), expected, "for {n}");
        }
    }
}
