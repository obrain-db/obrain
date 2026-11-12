//! ISO 8601 duration type with separate month, day, and nanosecond components.

use serde::{Deserialize, Serialize};
use std::fmt;

const NANOS_PER_SECOND: i64 = 1_000_000_000;
const NANOS_PER_MINUTE: i64 = 60 * NANOS_PER_SECOND;
const NANOS_PER_HOUR: i64 = 60 * NANOS_PER_MINUTE;

/// An ISO 8601 duration with separate month, day, and nanosecond components.
///
/// The three components are kept separate because months have variable length
/// and cannot be converted to days without a reference date. This means
/// durations are only partially ordered.
///
/// # Examples
///
/// ```
/// use obrain_common::types::Duration;
///
/// let d = Duration::parse("P1Y2M3DT4H5M6S").unwrap();
/// assert_eq!(d.months(), 14); // 1 year + 2 months
/// assert_eq!(d.days(), 3);
/// assert_eq!(d.to_string(), "P1Y2M3DT4H5M6S");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Duration {
    /// Total months (years * 12 + months).
    months: i64,
    /// Days component.
    days: i64,
    /// Sub-day component in nanoseconds.
    nanos: i64,
}

impl Duration {
    /// Creates a duration from months, days, and nanoseconds.
    #[must_use]
    pub const fn new(months: i64, days: i64, nanos: i64) -> Self {
        Self {
            months,
            days,
            nanos,
        }
    }

    /// Clé de tri STRUCTURELLE `(months, days, nanos)` pour l'ordre TOTAL du
    /// comparateur canonique ([`crate::types::value_cmp::comparer_total`]) :
    /// quand l'ordre partiel ne tranche pas (P1M vs P30D), le tri départage
    /// par cette clé — déterministe et documentée, sans prétendre à une
    /// sémantique calendaire.
    #[must_use]
    pub const fn cle_de_tri(&self) -> (i64, i64, i64) {
        (self.months, self.days, self.nanos)
    }

    /// Creates a duration from a number of months.
    #[must_use]
    pub const fn from_months(months: i64) -> Self {
        Self {
            months,
            days: 0,
            nanos: 0,
        }
    }

    /// Creates a duration from a number of days.
    #[must_use]
    pub const fn from_days(days: i64) -> Self {
        Self {
            months: 0,
            days,
            nanos: 0,
        }
    }

    /// Creates a duration from nanoseconds.
    #[must_use]
    pub const fn from_nanos(nanos: i64) -> Self {
        Self {
            months: 0,
            days: 0,
            nanos,
        }
    }

    /// Creates a duration from seconds.
    #[must_use]
    pub const fn from_seconds(secs: i64) -> Self {
        Self {
            months: 0,
            days: 0,
            nanos: secs * NANOS_PER_SECOND,
        }
    }

    /// Returns the months component.
    #[must_use]
    pub const fn months(&self) -> i64 {
        self.months
    }

    /// Returns the days component.
    #[must_use]
    pub const fn days(&self) -> i64 {
        self.days
    }

    /// Returns the nanoseconds component.
    #[must_use]
    pub const fn nanos(&self) -> i64 {
        self.nanos
    }

    /// Returns true if all components are zero.
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.months == 0 && self.days == 0 && self.nanos == 0
    }

    /// Negates all components.
    #[must_use]
    pub const fn neg(self) -> Self {
        Self {
            months: -self.months,
            days: -self.days,
            nanos: -self.nanos,
        }
    }

    /// Adds two durations component-wise.
    #[must_use]
    pub const fn add(self, other: Self) -> Self {
        Self {
            months: self.months + other.months,
            days: self.days + other.days,
            nanos: self.nanos + other.nanos,
        }
    }

    /// Subtracts another duration component-wise.
    #[must_use]
    pub const fn sub(self, other: Self) -> Self {
        Self {
            months: self.months - other.months,
            days: self.days - other.days,
            nanos: self.nanos - other.nanos,
        }
    }

    /// Multiplies all components by a factor.
    #[must_use]
    pub const fn mul(self, factor: i64) -> Self {
        Self {
            months: self.months * factor,
            days: self.days * factor,
            nanos: self.nanos * factor,
        }
    }

    /// Divides all components by a divisor (truncating).
    #[must_use]
    pub const fn div(self, divisor: i64) -> Self {
        Self {
            months: self.months / divisor,
            days: self.days / divisor,
            nanos: self.nanos / divisor,
        }
    }

    /// Parses a duration from ISO 8601 format "OnYnMnDTnHnMnS".
    ///
    /// Examples: "P1Y", "P2M3D", "PT4H5M6S", "P1Y2M3DT4H5M6S", "PT0.5S"
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
            (true, rest)
        } else {
            (false, s)
        };

        let s = s.strip_prefix('P')?;
        let mut months: i64 = 0;
        let mut days: i64 = 0;
        let mut nanos: i64 = 0;
        let mut in_time = false;
        let mut num_start = 0;
        let mut has_content = false;

        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'T' => {
                    in_time = true;
                    i += 1;
                    num_start = i;
                }
                b'Y' if !in_time => {
                    let n: i64 = s[num_start..i].parse().ok()?;
                    months += n * 12;
                    has_content = true;
                    i += 1;
                    num_start = i;
                }
                b'M' if !in_time => {
                    let n: i64 = s[num_start..i].parse().ok()?;
                    months += n;
                    has_content = true;
                    i += 1;
                    num_start = i;
                }
                b'W' if !in_time => {
                    let n: i64 = s[num_start..i].parse().ok()?;
                    days += n * 7;
                    has_content = true;
                    i += 1;
                    num_start = i;
                }
                b'D' if !in_time => {
                    let n: i64 = s[num_start..i].parse().ok()?;
                    days += n;
                    has_content = true;
                    i += 1;
                    num_start = i;
                }
                b'H' if in_time => {
                    let n: i64 = s[num_start..i].parse().ok()?;
                    nanos += n * NANOS_PER_HOUR;
                    has_content = true;
                    i += 1;
                    num_start = i;
                }
                b'M' if in_time => {
                    let n: i64 = s[num_start..i].parse().ok()?;
                    nanos += n * NANOS_PER_MINUTE;
                    has_content = true;
                    i += 1;
                    num_start = i;
                }
                b'S' if in_time => {
                    let text = &s[num_start..i];
                    if let Some(dot_pos) = text.find('.') {
                        let int_part: i64 = text[..dot_pos].parse().ok()?;
                        let frac_str = &text[dot_pos + 1..];
                        let frac_len = frac_str.len().min(9);
                        let frac: i64 = frac_str[..frac_len].parse().ok()?;
                        let scale = 10i64.pow(9 - frac_len as u32);
                        nanos += int_part * NANOS_PER_SECOND + frac * scale;
                    } else {
                        let n: i64 = text.parse().ok()?;
                        nanos += n * NANOS_PER_SECOND;
                    }
                    has_content = true;
                    i += 1;
                    num_start = i;
                }
                _ => {
                    i += 1;
                }
            }
        }

        if !has_content {
            return None;
        }

        let dur = Self {
            months,
            days,
            nanos,
        };
        Some(if negative { dur.neg() } else { dur })
    }
}

// Duration is deliberately NOT Ord (only PartialOrd).
// Comparing "P1M" vs "P30D" is ambiguous without a reference date.
impl PartialOrd for Duration {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // Only comparable when all components are individually comparable
        // and the relationship is consistent across components.
        let m = self.months.cmp(&other.months);
        let d = self.days.cmp(&other.days);
        let n = self.nanos.cmp(&other.nanos);

        if m == d && d == n {
            Some(m)
        } else if (m == std::cmp::Ordering::Equal || m == d)
            && (d == std::cmp::Ordering::Equal || d == n)
            && (m == std::cmp::Ordering::Equal || m == n)
        {
            // All non-equal components agree on direction
            if m != std::cmp::Ordering::Equal {
                Some(m)
            } else if d != std::cmp::Ordering::Equal {
                Some(d)
            } else {
                Some(n)
            }
        } else {
            None // Incomparable
        }
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Duration({})", self)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (neg, months, days, nanos) = if self.months < 0 || self.days < 0 || self.nanos < 0 {
            // If any component is negative, display as negative duration
            if self.months <= 0 && self.days <= 0 && self.nanos <= 0 {
                (true, -self.months, -self.days, -self.nanos)
            } else {
                // Mixed signs: display as-is (unusual but valid)
                (false, self.months, self.days, self.nanos)
            }
        } else {
            (false, self.months, self.days, self.nanos)
        };

        if neg {
            write!(f, "-")?;
        }
        write!(f, "P")?;

        let years = months / 12;
        let m = months % 12;

        if years != 0 {
            write!(f, "{years}Y")?;
        }
        if m != 0 {
            write!(f, "{m}M")?;
        }
        if days != 0 {
            write!(f, "{days}D")?;
        }

        // Time part
        let hours = nanos / NANOS_PER_HOUR;
        let remaining = nanos % NANOS_PER_HOUR;
        let minutes = remaining / NANOS_PER_MINUTE;
        let remaining = remaining % NANOS_PER_MINUTE;
        let secs = remaining / NANOS_PER_SECOND;
        let sub_nanos = remaining % NANOS_PER_SECOND;

        if hours != 0 || minutes != 0 || secs != 0 || sub_nanos != 0 {
            write!(f, "T")?;
            if hours != 0 {
                write!(f, "{hours}H")?;
            }
            if minutes != 0 {
                write!(f, "{minutes}M")?;
            }
            if secs != 0 || sub_nanos != 0 {
                if sub_nanos != 0 {
                    let frac = format!("{:09}", sub_nanos);
                    let trimmed = frac.trim_end_matches('0');
                    write!(f, "{secs}.{trimmed}S")?;
                } else {
                    write!(f, "{secs}S")?;
                }
            }
        }

        // Handle zero duration
        if !neg && years == 0 && m == 0 && days == 0 && nanos == 0 {
            write!(f, "T0S")?;
        }

        Ok(())
    }
}

/// Durées entre deux instants (`duration.between` & co) et accès aux composants.
impl Duration {
    /// Durée calendaire de `from` à `to` : des mois ENTIERS, puis des jours,
    /// puis le reste en nanosecondes — de sorte que `from` plus le résultat
    /// redonne `to`. Sémantique de Neo4j, c'est-à-dire de `java.time`
    /// (`ChronoUnit.MONTHS.between`) :
    ///
    /// - un mois n'est complet que si le JOUR du mois d'arrivée atteint celui
    ///   du départ (puis l'heure, pour un datetime) : du 31 janvier au 28
    ///   février, 0 mois et 28 jours — pas un mois (la fin de mois n'est pas
    ///   ramenée) ;
    /// - dans l'ordre inverse, le résultat est calculé de même, signé : du 28
    ///   février au 31 janvier, 0 mois et -28 jours ; du 31 mars au 29 février
    ///   (2024), -1 mois (31 mars - 1 mois = 29 février : la fin de mois n'est
    ///   ramenée que pour DÉCALER le départ).
    ///
    /// Les jours et le reste se lisent depuis `from` décalé des mois.
    #[must_use]
    pub fn between_timestamps(from: super::Timestamp, to: super::Timestamp) -> Self {
        let (from_date, to_date) = (from.to_date(), to.to_date());
        let (from_time, to_time) = (from.to_time().as_nanos(), to.to_time().as_nanos());
        // `LocalDateTime.until` : la date d'arrivée recule (ou avance) d'un
        // jour quand l'heure d'arrivée n'a pas atteint celle du départ.
        let end_date = if to_date > from_date && to_time < from_time {
            super::Date::from_days(to_date.as_days() - 1)
        } else if to_date < from_date && to_time > from_time {
            super::Date::from_days(to_date.as_days() + 1)
        } else {
            to_date
        };
        let months = months_until(from_date, end_date);
        let shifted = from.add_duration(&Self::from_months(months));
        let rest = to.as_nanos().saturating_sub(shifted.as_nanos());
        Self::new(months, rest / NANOS_PAR_JOUR, rest % NANOS_PAR_JOUR)
    }

    /// Comme [`between_timestamps`](Self::between_timestamps), pour deux dates
    /// (le reste en nanosecondes est toujours nul).
    #[must_use]
    pub fn between_dates(from: super::Date, to: super::Date) -> Self {
        let months = months_until(from, to);
        let shifted = from.add_duration(&Self::from_months(months));
        Self::new(
            months,
            i64::from(to.as_days()) - i64::from(shifted.as_days()),
            0,
        )
    }

    /// Un composant de la durée, à la manière de Neo4j.
    ///
    /// Chaque composant est le TOTAL de son groupe exprimé dans son unité,
    /// tronqué vers zéro : le groupe des mois donne `years`, `quarters`,
    /// `months` ; celui des jours `weeks`, `days` ; celui du reste
    /// `hours`, `minutes`, `seconds`, `milliseconds`, `microseconds`,
    /// `nanoseconds`. Ainsi `P1Y5M` a `months = 17` et `years = 1`, et
    /// `PT1H30M` a `minutes = 90`. Les variantes `…Of…` donnent le reste dans
    /// l'unité englobante (`monthsOfYear`, `minutesOfHour`, …).
    ///
    /// Les mois ne se convertissent pas en jours, ni les jours en heures : un
    /// composant d'un autre groupe n'est pas dérivé (`P1D` a `hours = 0`).
    /// Rend `None` pour un nom inconnu.
    #[must_use]
    pub fn component(&self, name: &str) -> Option<i64> {
        let (m, d, n) = (self.months, self.days, self.nanos);
        Some(match name.to_ascii_lowercase().as_str() {
            "years" => m / 12,
            "quarters" => m / 3,
            "months" => m,
            "quartersofyear" => (m / 3) % 4,
            "monthsofquarter" => m % 3,
            "monthsofyear" => m % 12,
            "weeks" => d / 7,
            "days" => d,
            "daysofweek" => d % 7,
            "hours" => n / 3_600_000_000_000,
            "minutes" => n / 60_000_000_000,
            "seconds" => n / 1_000_000_000,
            "milliseconds" => n / 1_000_000,
            "microseconds" => n / 1_000,
            "nanoseconds" => n,
            "minutesofhour" => (n / 60_000_000_000) % 60,
            "secondsofminute" => (n / 1_000_000_000) % 60,
            "millisecondsofsecond" => (n / 1_000_000) % 1_000,
            "microsecondsofsecond" => (n / 1_000) % 1_000_000,
            "nanosecondsofsecond" => n % 1_000_000_000,
            _ => return None,
        })
    }
}

const NANOS_PAR_JOUR: i64 = 86_400_000_000_000;

/// Mois entiers de `from` à `to` (`LocalDate.until(end, MONTHS)` de Java) :
/// l'écart de mois-calendrier, compté seulement jusqu'au jour du mois de
/// départ — `(mois * 32 + jour)` des deux dates, divisé par 32 en tronquant
/// vers zéro. Signé.
fn months_until(from: super::Date, to: super::Date) -> i64 {
    let packed = |d: super::Date| {
        (i64::from(d.year()) * 12 + i64::from(d.month()) - 1) * 32 + i64::from(d.day())
    };
    (packed(to) - packed(from)) / 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constructors() {
        let d = Duration::new(14, 3, 0);
        assert_eq!(d.months(), 14);
        assert_eq!(d.days(), 3);
        assert_eq!(d.nanos(), 0);

        assert_eq!(Duration::from_months(6).months(), 6);
        assert_eq!(Duration::from_days(10).days(), 10);
        assert_eq!(Duration::from_nanos(1_000_000_000).nanos(), 1_000_000_000);
    }

    #[test]
    fn test_parse_full() {
        let d = Duration::parse("P1Y2M3DT4H5M6S").unwrap();
        assert_eq!(d.months(), 14); // 12 + 2
        assert_eq!(d.days(), 3);
        assert_eq!(
            d.nanos(),
            4 * NANOS_PER_HOUR + 5 * NANOS_PER_MINUTE + 6 * NANOS_PER_SECOND
        );
    }

    #[test]
    fn test_parse_partial() {
        let d = Duration::parse("P1Y").unwrap();
        assert_eq!(d.months(), 12);
        assert_eq!(d.days(), 0);

        let d = Duration::parse("PT30S").unwrap();
        assert_eq!(d.months(), 0);
        assert_eq!(d.nanos(), 30 * NANOS_PER_SECOND);

        let d = Duration::parse("P2W").unwrap();
        assert_eq!(d.days(), 14);
    }

    #[test]
    fn test_parse_fractional_seconds() {
        let d = Duration::parse("PT0.5S").unwrap();
        assert_eq!(d.nanos(), 500_000_000);

        let d = Duration::parse("PT1.123S").unwrap();
        assert_eq!(d.nanos(), 1_123_000_000);
    }

    #[test]
    fn test_parse_negative() {
        let d = Duration::parse("-P1Y").unwrap();
        assert_eq!(d.months(), -12);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(Duration::parse("").is_none());
        assert!(Duration::parse("P").is_none());
        assert!(Duration::parse("not-a-duration").is_none());
    }

    #[test]
    fn test_display_roundtrip() {
        let cases = ["P1Y2M3DT4H5M6S", "P1Y", "P3D", "PT30S", "PT0.5S", "P2W"];
        for case in cases {
            let d = Duration::parse(case).unwrap();
            let reparsed = Duration::parse(&d.to_string()).unwrap();
            assert_eq!(d, reparsed, "roundtrip failed for {case}");
        }
    }

    #[test]
    fn test_arithmetic() {
        let a = Duration::new(1, 2, 3);
        let b = Duration::new(4, 5, 6);
        assert_eq!(a.add(b), Duration::new(5, 7, 9));
        assert_eq!(a.sub(b), Duration::new(-3, -3, -3));
        assert_eq!(a.mul(3), Duration::new(3, 6, 9));
        assert_eq!(a.neg(), Duration::new(-1, -2, -3));
    }

    #[test]
    fn test_partial_ord() {
        let a = Duration::new(1, 2, 3);
        let b = Duration::new(2, 3, 4);
        assert!(a < b);

        // Incomparable: more months but fewer days
        let c = Duration::new(2, 1, 0);
        let d = Duration::new(1, 100, 0);
        assert_eq!(c.partial_cmp(&d), None);
    }

    #[test]
    fn test_zero() {
        let z = Duration::default();
        assert!(z.is_zero());
        assert_eq!(z.to_string(), "PT0S");
    }

    #[test]
    fn between_timestamps_compte_mois_jours_puis_reste() {
        use crate::types::{Date, Time, Timestamp};
        let t = |y, m, d, h, mi, s| {
            Timestamp::from_date_time(
                Date::from_ymd(y, m, d).unwrap(),
                Time::from_hms_nano(h, mi, s, 0).unwrap(),
            )
        };
        // 90 minutes.
        assert_eq!(
            Duration::between_timestamps(t(2024, 1, 1, 0, 0, 0), t(2024, 1, 1, 1, 30, 0)),
            Duration::from_nanos(5_400_000_000_000)
        );
        // 3 mois, 14 jours, 6 h : le composant `days` ne compte pas les mois.
        let d = Duration::between_timestamps(t(2024, 1, 10, 0, 0, 0), t(2024, 4, 24, 6, 0, 0));
        assert_eq!(d, Duration::new(3, 14, 6 * 3_600_000_000_000));
        assert_eq!(d.component("days"), Some(14));
        assert_eq!(d.component("months"), Some(3));
        // Fin de mois : 31 janvier -> 1er mars (2024 bissextile) = 1 mois et 1 jour.
        assert_eq!(
            Duration::between_timestamps(t(2024, 1, 31, 0, 0, 0), t(2024, 3, 1, 0, 0, 0)),
            Duration::new(1, 1, 0)
        );
        // Un jour d'arrivée plus petit retire un mois : 15 janv. -> 10 fév. = 26 jours.
        assert_eq!(
            Duration::between_timestamps(t(2024, 1, 15, 0, 0, 0), t(2024, 2, 10, 0, 0, 0)),
            Duration::new(0, 26, 0)
        );
        // Sens inverse : l'opposé.
        assert_eq!(
            Duration::between_timestamps(t(2024, 4, 24, 6, 0, 0), t(2024, 1, 10, 0, 0, 0)),
            Duration::new(-3, -14, -6 * 3_600_000_000_000)
        );
        assert!(
            Duration::between_timestamps(t(2024, 1, 1, 0, 0, 0), t(2024, 1, 1, 0, 0, 0)).is_zero()
        );
    }

    #[test]
    fn between_dates_et_composants() {
        use crate::types::Date;
        let d = Duration::between_dates(
            Date::from_ymd(2023, 11, 20).unwrap(),
            Date::from_ymd(2025, 2, 5).unwrap(),
        );
        assert_eq!(d, Duration::new(14, 16, 0));
        assert_eq!(d.component("years"), Some(1));
        assert_eq!(d.component("monthsOfYear"), Some(2));
        assert_eq!(d.component("weeks"), Some(2));
        let t = Duration::new(0, 0, 58_390_000_000_001);
        assert_eq!(t.component("hours"), Some(16));
        assert_eq!(t.component("minutes"), Some(973));
        assert_eq!(t.component("seconds"), Some(58_390));
        assert_eq!(t.component("milliseconds"), Some(58_390_000));
        assert_eq!(t.component("minutesOfHour"), Some(13));
        assert_eq!(t.component("inconnu"), None);
    }

    #[test]
    fn fins_de_mois_comme_java_until() {
        use crate::types::Date;
        let d = |y, m, j| Date::from_ymd(y, m, j).unwrap();
        // Le mois n'est complet que si le jour d'arrivée atteint celui du départ.
        assert_eq!(
            Duration::between_dates(d(2026, 1, 31), d(2026, 2, 28)),
            Duration::new(0, 28, 0)
        );
        assert_eq!(
            Duration::between_dates(d(2026, 1, 30), d(2026, 2, 28)),
            Duration::new(0, 29, 0)
        );
        assert_eq!(
            Duration::between_dates(d(2026, 2, 28), d(2026, 1, 31)),
            Duration::new(0, -28, 0)
        );
        assert_eq!(
            Duration::between_dates(d(2024, 3, 31), d(2024, 2, 29)),
            Duration::new(-1, 0, 0)
        );
        assert_eq!(
            Duration::between_dates(d(2024, 1, 31), d(2024, 3, 1)),
            Duration::new(1, 1, 0)
        );
    }
}
