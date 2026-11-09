//! Simulated time. an integer nanosecond sort key, never accumulated f64 seconds.
//! nothing in this crate reads the wall clock.
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

pub const NS_PER_SEC: i64 = 1_000_000_000;
pub const NS_PER_MIN: i64 = 60 * NS_PER_SEC;
pub const NS_PER_HOUR: i64 = 60 * NS_PER_MIN;
pub const NS_PER_DAY: i64 = 24 * NS_PER_HOUR;

/// Nanoseconds since the world epoch. NEVER f64.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct SimTime(pub i64);

/// A duration in nanoseconds.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Span(pub i64);

impl Span {
    pub const ZERO: Span = Span(0);
    pub const fn ns(n: i64) -> Span { Span(n) }
    pub const fn secs(s: i64) -> Span { Span(s * NS_PER_SEC) }
    pub const fn mins(m: i64) -> Span { Span(m * NS_PER_MIN) }
    pub const fn hours(h: i64) -> Span { Span(h * NS_PER_HOUR) }
    pub const fn days(d: i64) -> Span { Span(d * NS_PER_DAY) }
    pub fn from_secs_f64(s: f64) -> Span { Span((s * NS_PER_SEC as f64).round() as i64) }
    pub fn from_mins_f64(m: f64) -> Span { Span::from_secs_f64(m * 60.0) }
    pub fn from_hours_f64(h: f64) -> Span { Span::from_secs_f64(h * 3600.0) }
    pub fn from_days_f64(d: f64) -> Span { Span::from_secs_f64(d * 86400.0) }
    pub fn as_secs_f64(self) -> f64 { self.0 as f64 / NS_PER_SEC as f64 }
    pub fn as_mins_f64(self) -> f64 { self.as_secs_f64() / 60.0 }
    pub fn as_hours_f64(self) -> f64 { self.as_secs_f64() / 3600.0 }
    pub fn as_days_f64(self) -> f64 { self.as_secs_f64() / 86400.0 }
    pub fn is_positive(self) -> bool { self.0 > 0 }
    pub fn is_zero(self) -> bool { self.0 == 0 }
    pub fn abs(self) -> Span { Span(self.0.abs()) }
    pub fn min(self, o: Span) -> Span { if o.0 < self.0 { o } else { self } }
    pub fn max(self, o: Span) -> Span { if o.0 > self.0 { o } else { self } }

    /// Parse "7d", "2h30m", "90s", "1.5h", "45m", "3d 4h". A bare number is seconds.
    pub fn parse(s: &str) -> Result<Span, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty span".into());
        }
        let mut total = 0.0f64;
        let mut num = String::new();
        let mut any = false;
        for ch in s.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                num.push(ch);
                continue;
            }
            if ch == ' ' || ch == '_' {
                continue;
            }
            let n: f64 = num.parse().map_err(|_| format!("bad number in span {s:?}"))?;
            num.clear();
            let mult = match ch {
                'd' => 86400.0,
                'h' => 3600.0,
                'm' => 60.0,
                's' => 1.0,
                _ => return Err(format!("bad unit {ch:?} in span {s:?}")),
            };
            total += n * mult;
            any = true;
        }
        if !num.is_empty() {
            let n: f64 = num.parse().map_err(|_| format!("bad number in span {s:?}"))?;
            total += n;
            any = true;
        }
        if !any {
            return Err(format!("no duration in {s:?}"));
        }
        Ok(Span::from_secs_f64(total))
    }
}

impl SimTime {
    pub const ZERO: SimTime = SimTime(0);
    pub fn elapsed_since(self, t0: SimTime) -> Span { Span(self.0 - t0.0) }
    pub fn as_secs_f64(self) -> f64 { self.0 as f64 / NS_PER_SEC as f64 }
    pub fn max(self, o: SimTime) -> SimTime { if o.0 > self.0 { o } else { self } }
    pub fn min(self, o: SimTime) -> SimTime { if o.0 < self.0 { o } else { self } }
    /// Civil (calendar) time for a given epoch. Pure arithmetic; no wall clock.
    pub fn civil(self, epoch: CivilEpoch) -> CivilTime {
        let secs_utc = epoch.unix_secs + self.0.div_euclid(NS_PER_SEC);
        let nanos = self.0.rem_euclid(NS_PER_SEC) as u32;
        let local = secs_utc + epoch.tz_offset_secs as i64;
        let days = local.div_euclid(86400);
        let sod = local.rem_euclid(86400);
        let (year, month, day) = civil_from_days(days);
        CivilTime {
            year,
            month,
            day,
            hour: (sod / 3600) as u32,
            minute: ((sod % 3600) / 60) as u32,
            second: (sod % 60) as u32,
            nanos,
            weekday: (days + 3).rem_euclid(7) as u32,
            day_index: days,
            tod: Span(sod * NS_PER_SEC + nanos as i64),
            unix_secs: secs_utc,
        }
    }
}

impl Add<Span> for SimTime { type Output = SimTime; fn add(self, s: Span) -> SimTime { SimTime(self.0 + s.0) } }
impl Sub<Span> for SimTime { type Output = SimTime; fn sub(self, s: Span) -> SimTime { SimTime(self.0 - s.0) } }
impl Sub<SimTime> for SimTime { type Output = Span; fn sub(self, t: SimTime) -> Span { Span(self.0 - t.0) } }
impl AddAssign<Span> for SimTime { fn add_assign(&mut self, s: Span) { self.0 += s.0; } }
impl Add for Span { type Output = Span; fn add(self, o: Span) -> Span { Span(self.0 + o.0) } }
impl Sub for Span { type Output = Span; fn sub(self, o: Span) -> Span { Span(self.0 - o.0) } }
impl AddAssign for Span { fn add_assign(&mut self, o: Span) { self.0 += o.0; } }
impl SubAssign for Span { fn sub_assign(&mut self, o: Span) { self.0 -= o.0; } }
impl Neg for Span { type Output = Span; fn neg(self) -> Span { Span(-self.0) } }
impl Mul<f64> for Span { type Output = Span; fn mul(self, k: f64) -> Span { Span((self.0 as f64 * k).round() as i64) } }
impl Mul<i64> for Span { type Output = Span; fn mul(self, k: i64) -> Span { Span(self.0 * k) } }

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let neg = self.0 < 0;
        let mut ns = self.0.unsigned_abs();
        let days = ns / NS_PER_DAY as u64;
        ns %= NS_PER_DAY as u64;
        let hours = ns / NS_PER_HOUR as u64;
        ns %= NS_PER_HOUR as u64;
        let mins = ns / NS_PER_MIN as u64;
        ns %= NS_PER_MIN as u64;
        let secs = ns / NS_PER_SEC as u64;
        let sub = ns % NS_PER_SEC as u64;
        if neg {
            write!(f, "-")?;
        }
        if days > 0 {
            write!(f, "{days}d ")?;
        }
        write!(f, "{hours:02}:{mins:02}:{secs:02}")?;
        if sub != 0 {
            write!(f, ".{:03}", sub / 1_000_000)?;
        }
        Ok(())
    }
}
impl fmt::Debug for Span { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self}") } }
impl fmt::Display for SimTime { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "t+{}", Span(self.0)) } }
impl fmt::Debug for SimTime { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self}") } }

/// Where SimTime(0) sits on the civil calendar, and the fixed UTC offset (no DST).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CivilEpoch { pub unix_secs: i64, pub tz_offset_secs: i32 }

impl CivilEpoch {
    pub fn from_local(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32, tz_offset_secs: i32) -> CivilEpoch {
        let days = days_from_civil(y, m, d);
        let local = days * 86400 + (hh * 3600 + mm * 60 + ss) as i64;
        CivilEpoch { unix_secs: local - tz_offset_secs as i64, tz_offset_secs }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CivilTime {
    pub year: i32, pub month: u32, pub day: u32,
    pub hour: u32, pub minute: u32, pub second: u32, pub nanos: u32,
    /// 0 = Monday … 6 = Sunday
    pub weekday: u32,
    /// local days since 1970-01-01
    pub day_index: i64,
    /// time since local midnight
    pub tod: Span,
    pub unix_secs: i64,
}
impl CivilTime {
    pub fn tod_hours(&self) -> f64 { self.tod.as_hours_f64() }
}
impl fmt::Display for CivilTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}

/// Howard Hinnant's algorithm: days since 1970-01-01 → (y, m, d), proleptic Gregorian.
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// (y, m, d) → days since 1970-01-01.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = (if m <= 2 { y - 1 } else { y }) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn civil_roundtrip() {
        for &(y, m, d) in &[(1970, 1, 1), (2000, 2, 29), (2026, 8, 29), (1999, 12, 31), (2100, 3, 1)] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d));
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        let ep = CivilEpoch::from_local(2026, 8, 29, 6, 0, 0, 4 * 3600);
        let c = SimTime::ZERO.civil(ep);
        assert_eq!((c.year, c.month, c.day, c.hour), (2026, 8, 29, 6));
        assert_eq!(c.weekday, 5, "2026-08-29 is a Saturday");
        let c2 = (SimTime::ZERO + Span::hours(20)).civil(ep);
        assert_eq!((c2.day, c2.hour), (30, 2));
    }
    #[test]
    fn span_parse_display() {
        assert_eq!(Span::parse("2h30m").unwrap(), Span::mins(150));
        assert_eq!(Span::parse("7d").unwrap(), Span::days(7));
        assert_eq!(Span::parse("1.5h").unwrap(), Span::mins(90));
        assert_eq!(Span::parse("90").unwrap(), Span::secs(90));
        assert_eq!(format!("{}", Span::mins(150)), "02:30:00");
        assert_eq!(format!("{}", Span::days(1) + Span::secs(1) + Span::ns(5_000_000)), "1d 00:00:01.005");
    }
}
