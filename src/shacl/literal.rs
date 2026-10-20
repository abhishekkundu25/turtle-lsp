//! Literal values for SHACL: whether a lexical form is valid for its XSD
//! datatype or `rdf:JSON` (`sh:datatype`, and the `invalid-literal` lint),
//! and the order `sh:minInclusive` & co. and `sh:lessThan` compare by.
//!
//! The lexical spaces are those of XSD 1.1 Part 2 §3.3 and §3.4 and, for
//! `rdf:JSON`, RFC 8259 JSON text (RDF 1.2 Concepts §5.3). Whitespace is
//! not collapsed here: a form with leading or trailing spaces is ill-formed.
//!
//! Comparisons follow SPARQL's `<` for the common cases and are deliberately
//! simple:
//! - numbers (`xsd:integer` and its derived types, `xsd:decimal`,
//!   `xsd:float`, `xsd:double`) compare by value: as 128-bit integers when
//!   both are integers, else as `f64` (so decimals beyond ~15 significant
//!   digits can compare equal when they are not; NaN compares with nothing);
//! - `xsd:dateTime` and `xsd:date` compare when both have a timezone (by the
//!   instant) or neither has (by the local value); a mix is indeterminate, as
//!   in XSD, and so not comparable;
//! - `xsd:string` (and plain) literals compare by code points, `xsd:boolean`
//!   with false < true;
//! - anything else (other datatypes, ill-formed literals, language-tagged
//!   strings, IRIs, blank nodes) is not comparable, which fails the constraint.

use crate::syntax::{RDF, XSD};
use std::cmp::Ordering;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Integer {
        min: Option<i128>,
        max: Option<i128>,
    },
    Decimal,
    Float,
    Boolean,
    DateTime,
    Date,
    Time,
    GYear,
    GYearMonth,
    GMonthDay,
    GDay,
    GMonth,
    /// `xsd:duration` with both parts, `xsd:yearMonthDuration` with only
    /// years and months, `xsd:dayTimeDuration` with only days and time.
    Duration {
        year_month: bool,
        day_time: bool,
    },
    HexBinary,
    Base64Binary,
    Language,
    Json,
    String,
    /// A datatype whose lexical forms are not checked.
    Other,
}

fn kind(datatype: &str) -> Kind {
    let Some(local) = datatype.strip_prefix(XSD) else {
        return match datatype.strip_prefix(RDF) {
            Some("JSON") => Kind::Json,
            _ => Kind::Other,
        };
    };
    let duration = |year_month, day_time| Kind::Duration {
        year_month,
        day_time,
    };
    let int = |min: Option<i128>, max: Option<i128>| Kind::Integer { min, max };
    match local {
        "integer" => int(None, None),
        "nonNegativeInteger" => int(Some(0), None),
        "positiveInteger" => int(Some(1), None),
        "nonPositiveInteger" => int(None, Some(0)),
        "negativeInteger" => int(None, Some(-1)),
        "long" => int(Some(i64::MIN.into()), Some(i64::MAX.into())),
        "int" => int(Some(i32::MIN.into()), Some(i32::MAX.into())),
        "short" => int(Some(i16::MIN.into()), Some(i16::MAX.into())),
        "byte" => int(Some(i8::MIN.into()), Some(i8::MAX.into())),
        "unsignedLong" => int(Some(0), Some(u64::MAX.into())),
        "unsignedInt" => int(Some(0), Some(u32::MAX.into())),
        "unsignedShort" => int(Some(0), Some(u16::MAX.into())),
        "unsignedByte" => int(Some(0), Some(u8::MAX.into())),
        "decimal" => Kind::Decimal,
        "float" | "double" => Kind::Float,
        "boolean" => Kind::Boolean,
        "dateTime" | "dateTimeStamp" => Kind::DateTime,
        "date" => Kind::Date,
        "time" => Kind::Time,
        "gYear" => Kind::GYear,
        "gYearMonth" => Kind::GYearMonth,
        "gMonthDay" => Kind::GMonthDay,
        "gDay" => Kind::GDay,
        "gMonth" => Kind::GMonth,
        "duration" => duration(true, true),
        "yearMonthDuration" => duration(true, false),
        "dayTimeDuration" => duration(false, true),
        "hexBinary" => Kind::HexBinary,
        "base64Binary" => Kind::Base64Binary,
        "language" => Kind::Language,
        "string" => Kind::String,
        _ => Kind::Other,
    }
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn unsign(s: &str) -> &str {
    s.strip_prefix(['+', '-']).unwrap_or(s)
}

fn integer(s: &str) -> Option<i128> {
    if !digits(unsign(s)) {
        return None;
    }
    s.strip_prefix('+').unwrap_or(s).parse().ok()
}

fn decimal(s: &str) -> Option<f64> {
    let u = unsign(s);
    let (a, b) = u.split_once('.').unwrap_or((u, ""));
    let ok = (digits(a) || a.is_empty())
        && (digits(b) || b.is_empty())
        && !(a.is_empty() && b.is_empty());
    ok.then(|| s.parse().ok()).flatten()
}

fn float(s: &str) -> Option<f64> {
    match s {
        "INF" | "+INF" => return Some(f64::INFINITY),
        "-INF" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    let (m, e) = s.split_once(['e', 'E']).unwrap_or((s, ""));
    decimal(m)?;
    if s.len() != m.len() && integer(e).is_none() {
        return None;
    }
    s.parse().ok()
}

fn two(s: &str, at: usize) -> Option<u32> {
    let d = s.get(at..at + 2)?;
    digits(d).then(|| d.parse().ok()).flatten()
}

/// `Z`, `+hh:mm` or `-hh:mm` as minutes east of UTC.
fn timezone(s: &str) -> Option<i64> {
    if s == "Z" {
        return Some(0);
    }
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    if s.len() != 6 || s.as_bytes()[3] != b':' {
        return None;
    }
    let (h, m) = (two(s, 1)?, two(s, 4)?);
    (h <= 14 && m < 60 && (h < 14 || m == 0)).then_some(sign * (h as i64 * 60 + m as i64))
}

/// A year (`-?([1-9][0-9]{3,}|0[0-9]{3})`) at the start of `s`, and the
/// rest of the string.
fn year(s: &str) -> Option<(i64, &str)> {
    let body = s.strip_prefix('-').unwrap_or(s);
    let n = body.bytes().take_while(u8::is_ascii_digit).count();
    if n < 4 || (n > 4 && body.starts_with('0')) {
        return None;
    }
    let year: i64 = body[..n].parse().ok()?;
    Some((if s.starts_with('-') { -year } else { year }, &body[n..]))
}

fn days_in_month(month: u32, leap: bool) -> u32 {
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `[-]YYYY-MM-DD` as (days since 1970-01-01, rest of the string).
fn date_part(s: &str) -> Option<(i64, &str)> {
    let (year, rest) = year(s)?;
    if rest.len() < 6 || rest.as_bytes()[0] != b'-' || rest.as_bytes()[3] != b'-' {
        return None;
    }
    let (month, day) = (two(rest, 1)?, two(rest, 4)?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(month, leap) {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468, &rest[6..]))
}

/// `hh:mm:ss[.fff]` as (nanoseconds since midnight, rest of the string).
fn time_part(s: &str) -> Option<(i128, &str)> {
    if s.len() < 8 || s.as_bytes()[2] != b':' || s.as_bytes()[5] != b':' {
        return None;
    }
    let (h, m, sec) = (two(s, 0)?, two(s, 3)?, two(s, 6)?);
    let mut rest = &s[8..];
    let mut nanos: i128 = 0;
    if let Some(f) = rest.strip_prefix('.') {
        let n = f.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        let frac = &f[..n];
        let padded: String = frac.chars().chain(std::iter::repeat('0')).take(9).collect();
        nanos = padded.parse().ok()?;
        rest = &f[n..];
    }
    let end_of_day = h == 24 && m == 0 && sec == 0 && nanos == 0;
    if !(h < 24 && m < 60 && sec < 60 || end_of_day) {
        return None;
    }
    let secs = (h as i128 * 60 + m as i128) * 60 + sec as i128;
    Some((secs * 1_000_000_000 + nanos, rest))
}

/// A point in time: nanoseconds, and whether a timezone was given (the
/// value is then UTC).
fn instant(k: Kind, s: &str) -> Option<(i128, bool)> {
    const DAY: i128 = 86_400 * 1_000_000_000;
    let (nanos, rest) = match k {
        Kind::DateTime => {
            let (days, rest) = date_part(s)?;
            let (t, rest) = time_part(rest.strip_prefix('T')?)?;
            (days as i128 * DAY + t, rest)
        }
        Kind::Date => {
            let (days, rest) = date_part(s)?;
            (days as i128 * DAY, rest)
        }
        Kind::Time => time_part(s)?,
        _ => return None,
    };
    if rest.is_empty() {
        return Some((nanos, false));
    }
    let tz = timezone(rest)?;
    Some((nanos - tz as i128 * 60 * 1_000_000_000, true))
}

/// Whether `lexical` is in the lexical space of `datatype`. Datatypes this
/// module does not know are taken as valid.
pub fn well_formed(datatype: &str, lexical: &str) -> bool {
    match kind(datatype) {
        Kind::Integer { min, max } => match integer(lexical) {
            Some(n) => min.is_none_or(|m| n >= m) && max.is_none_or(|m| n <= m),
            // Beyond i128 (xsd:integer is unbounded): valid when the side it
            // overflows to has no bound.
            None if digits(unsign(lexical)) => {
                if lexical.starts_with('-') {
                    min.is_none()
                } else {
                    max.is_none()
                }
            }
            None => false,
        },
        Kind::Decimal => decimal(lexical).is_some(),
        Kind::Float => float(lexical).is_some(),
        Kind::Boolean => matches!(lexical, "true" | "false" | "1" | "0"),
        // dateTimeStamp is a dateTime with a required timezone.
        k @ (Kind::DateTime | Kind::Date | Kind::Time) => {
            instant(k, lexical).is_some_and(|(_, tz)| tz || !datatype.ends_with("#dateTimeStamp"))
        }
        Kind::GYear => year(lexical).is_some_and(|(_, tz)| zone(tz)),
        Kind::GYearMonth => year(lexical)
            .and_then(|(_, r)| month(r.strip_prefix('-')?))
            .is_some_and(|(_, tz)| zone(tz)),
        Kind::GMonthDay => lexical
            .strip_prefix("--")
            .and_then(month)
            .and_then(|(m, r)| day(r.strip_prefix('-')?, days_in_month(m, true)))
            .is_some_and(zone),
        Kind::GDay => lexical
            .strip_prefix("---")
            .and_then(|r| day(r, 31))
            .is_some_and(zone),
        Kind::GMonth => lexical
            .strip_prefix("--")
            .and_then(month)
            .is_some_and(|(_, tz)| zone(tz)),
        Kind::Duration {
            year_month,
            day_time,
        } => duration(lexical, year_month, day_time),
        Kind::HexBinary => {
            lexical.len().is_multiple_of(2) && lexical.bytes().all(|b| b.is_ascii_hexdigit())
        }
        Kind::Base64Binary => base64(lexical),
        Kind::Language => language(lexical),
        Kind::Json => {
            let doc = crate::jsonld::json::parse(lexical);
            doc.root.is_some() && doc.errors.is_empty()
        }
        Kind::String | Kind::Other => true,
    }
}

/// Whether `s` is empty or a timezone.
fn zone(s: &str) -> bool {
    s.is_empty() || timezone(s).is_some()
}

/// `MM` at the start of `s`, and the rest.
fn month(s: &str) -> Option<(u32, &str)> {
    two(s, 0)
        .filter(|m| (1..=12).contains(m))
        .map(|m| (m, &s[2..]))
}

/// `DD` up to `max` at the start of `s`, and the rest.
fn day(s: &str, max: u32) -> Option<&str> {
    two(s, 0).filter(|d| (1..=max).contains(d)).map(|_| &s[2..])
}

/// `-?PnYnMnDTnHnMnS`: the components in that order, each optional but at
/// least one, at least one after a `T`, and only seconds with a fraction.
/// `year_month` and `day_time` allow the parts before and after the days.
fn duration(s: &str, year_month: bool, day_time: bool) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let Some(s) = s.strip_prefix('P') else {
        return false;
    };
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let date_designators: &[u8] = match (year_month, day_time) {
        (true, true) => b"YMD",
        (true, false) => b"YM",
        _ => b"D",
    };
    let Some(n) = components(date, date_designators) else {
        return false;
    };
    match time {
        None => n > 0,
        Some(t) => day_time && components(t, b"HMS").is_some_and(|m| m > 0),
    }
}

/// How many `<number><designator>` components `s` is made of, with the
/// designators in the order of `designators`; `None` if it is not.
fn components(mut s: &str, designators: &[u8]) -> Option<usize> {
    let mut next = 0;
    let mut n = 0;
    while !s.is_empty() {
        let mut len = s.bytes().take_while(u8::is_ascii_digit).count();
        let fraction = s[len..].starts_with('.');
        if fraction {
            len += 1 + s[len + 1..].bytes().take_while(u8::is_ascii_digit).count();
        }
        if len == usize::from(fraction) {
            return None;
        }
        let d = *s.as_bytes().get(len)?;
        let i = next + designators[next..].iter().position(|&x| x == d)?;
        if fraction && d != b'S' {
            return None;
        }
        next = i + 1;
        n += 1;
        s = &s[len + 1..];
    }
    Some(n)
}

/// Base64 with single spaces allowed between characters, padded to a
/// multiple of four, the last character before padding one that leaves no
/// stray bits.
fn base64(s: &str) -> bool {
    if s.contains("  ") || s.starts_with(' ') {
        return false;
    }
    let b: Vec<u8> = s.bytes().filter(|&c| c != b' ').collect();
    if !b.len().is_multiple_of(4) {
        return false;
    }
    let pad = b.iter().rev().take_while(|&&c| c == b'=').count();
    let data = &b[..b.len() - pad];
    let alphabet = |c: &u8| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/');
    if pad > 2 || !data.iter().all(alphabet) {
        return false;
    }
    match (pad, data.last()) {
        (1, Some(c)) => b"AEIMQUYcgkosw048".contains(c),
        (2, Some(c)) => b"AQgw".contains(c),
        _ => true,
    }
}

/// `[a-zA-Z]{1,8}(-[a-zA-Z0-9]{1,8})*`
fn language(s: &str) -> bool {
    let part = |p: &str, f: fn(&u8) -> bool| (1..=8).contains(&p.len()) && p.bytes().all(|b| f(&b));
    let mut parts = s.split('-');
    parts
        .next()
        .is_some_and(|p| part(p, u8::is_ascii_alphabetic))
        && parts.all(|p| part(p, u8::is_ascii_alphanumeric))
}

enum Value<'a> {
    Int(i128),
    Num(f64),
    Bool(bool),
    Time(Kind, i128, bool),
    Str(&'a str),
}

fn value<'a>(datatype: &str, lexical: &'a str) -> Option<Value<'a>> {
    let k = kind(datatype);
    if !well_formed(datatype, lexical) {
        return None;
    }
    Some(match k {
        // Huge integers compare as doubles: close enough for ordering.
        Kind::Integer { .. } => match integer(lexical) {
            Some(n) => Value::Int(n),
            None => Value::Num(lexical.parse().ok()?),
        },
        Kind::Decimal => Value::Num(decimal(lexical)?),
        Kind::Float => Value::Num(float(lexical)?),
        Kind::Boolean => Value::Bool(matches!(lexical, "true" | "1")),
        Kind::DateTime | Kind::Date | Kind::Time => {
            let (n, tz) = instant(k, lexical)?;
            Value::Time(k, n, tz)
        }
        Kind::String => Value::Str(lexical),
        _ => return None,
    })
}

/// The order of two literals, `None` when they are not comparable.
pub fn compare(a: (&str, &str), b: (&str, &str)) -> Option<Ordering> {
    use Value::*;
    match (value(a.0, a.1)?, value(b.0, b.1)?) {
        (Int(x), Int(y)) => Some(x.cmp(&y)),
        (Int(x), Num(y)) => (x as f64).partial_cmp(&y),
        (Num(x), Int(y)) => x.partial_cmp(&(y as f64)),
        (Num(x), Num(y)) => x.partial_cmp(&y),
        (Bool(x), Bool(y)) => Some(x.cmp(&y)),
        (Str(x), Str(y)) => Some(x.cmp(y)),
        (Time(k1, x, tz1), Time(k2, y, tz2)) if k1 == k2 && tz1 == tz2 => Some(x.cmp(&y)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(l: &str) -> String {
        format!("{XSD}{l}")
    }

    #[test]
    fn lexical_forms() {
        assert!(well_formed(&x("byte"), "127"));
        assert!(!well_formed(&x("byte"), "300"));
        assert!(!well_formed(&x("byte"), "c"));
        assert!(well_formed(&x("integer"), "+5"));
        let huge = "123456789012345678901234567890123456789012";
        assert!(well_formed(&x("integer"), huge));
        assert!(well_formed(&x("integer"), &format!("-{huge}")));
        assert!(!well_formed(&x("negativeInteger"), huge));
        assert!(!well_formed(&x("int"), huge));
        assert!(!well_formed(&x("integer"), "5.0"));
        assert!(well_formed(&x("decimal"), "-.5"));
        assert!(!well_formed(&x("decimal"), "."));
        assert!(well_formed(&x("double"), "1.5E-3"));
        assert!(well_formed(&x("double"), "INF"));
        assert!(!well_formed(&x("double"), "1e"));
        assert!(well_formed(&x("boolean"), "0"));
        assert!(!well_formed(&x("boolean"), "yes"));
        assert!(well_formed(&x("date"), "2024-02-29"));
        assert!(!well_formed(&x("date"), "2023-02-29"));
        assert!(well_formed(&x("dateTime"), "2024-01-01T10:00:00.5+02:00"));
        assert!(!well_formed(&x("dateTime"), "2024-01-01"));
        assert!(well_formed("http://example.com/dt", "anything"));
    }

    #[test]
    fn ordering() {
        let i = x("integer");
        let d = x("decimal");
        assert_eq!(compare((&i, "2"), (&d, "2.5")), Some(Ordering::Less));
        assert_eq!(compare((&i, "10"), (&i, "9")), Some(Ordering::Greater));
        let dt = x("dateTime");
        assert_eq!(
            compare(
                (&dt, "2024-01-01T12:00:00Z"),
                (&dt, "2024-01-01T13:00:00+02:00")
            ),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare((&dt, "2024-01-01T12:00:00Z"), (&dt, "2024-01-01T12:00:00")),
            None
        );
        assert_eq!(compare((&i, "1"), (&x("string"), "1")), None);
        assert_eq!(
            compare((&x("date"), "2024-01-01"), (&dt, "2024-01-01T00:00:00")),
            None
        );
        assert_eq!(compare((&x("byte"), "300"), (&i, "1")), None);
    }
}
