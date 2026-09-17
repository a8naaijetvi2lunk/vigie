use std::time::{SystemTime, UNIX_EPOCH};

/// Convertit un horodatage ISO 8601 UTC en epoch secondes.
///
/// Ne lit que "YYYY-MM-DDTHH:MM:SS" et ignore la suite (fraction, "Z", "+00:00") :
/// Codex et l'API Claude produisent tous deux de l'UTC.
pub fn parse_iso8601_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    let second: i64 = s.get(17..19)?.parse().ok()?;

    let days = days_from_civil(year, month, day);
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

/// Algorithme de Howard Hinnant (days_from_civil) : nombre de jours écoulés
/// depuis l'epoch Unix (1970-01-01) pour une date calendaire donnée.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Instant courant en epoch secondes.
pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Instant courant en epoch millisecondes (pour comparaison avec `expiresAt`).
pub fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_style_timestamp_with_z_suffix() {
        assert_eq!(
            parse_iso8601_utc("2026-07-17T06:03:24.764Z"),
            Some(1784268204)
        );
    }

    #[test]
    fn parses_claude_style_timestamp_with_explicit_offset() {
        // Même instant que le test ci-dessus, formaté avec un offset explicite.
        assert_eq!(
            parse_iso8601_utc("2026-07-17T06:03:24.407877+00:00"),
            Some(1784268204)
        );
    }
}
