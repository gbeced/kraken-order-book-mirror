use chrono::{DateTime, Utc};
use std::time::{SystemTime, UNIX_EPOCH};

/// Nanoseconds since the Unix epoch (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ts(pub i64);

impl Ts {
    /// The current system time. Errors with `timestamp out of range` outside about 1677–2262.
    pub fn now() -> Result<Ts, &'static str> {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_nanos()).ok(),
            Err(e) => i64::try_from(e.duration().as_nanos()).ok().map(|n| -n),
        };
        nanos.map(Ts).ok_or("timestamp out of range")
    }
}

impl TryFrom<DateTime<Utc>> for Ts {
    type Error = &'static str;

    fn try_from(dt: DateTime<Utc>) -> Result<Self, Self::Error> {
        dt.timestamp_nanos_opt()
            .map(Ts)
            .ok_or("timestamp out of range")
    }
}

impl From<Ts> for DateTime<Utc> {
    fn from(ts: Ts) -> Self {
        DateTime::from_timestamp_nanos(ts.0)
    }
}

/// Parses an RFC 3339 timestamp, e.g. `2023-10-06T17:35:55.440295123Z`.
impl TryFrom<&str> for Ts {
    type Error = &'static str;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let dt = DateTime::parse_from_rfc3339(s).map_err(|_| "invalid RFC 3339 timestamp")?;
        Ts::try_from(dt.with_timezone(&Utc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn parse(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn from_datetime_epoch() {
        let dt = Utc.timestamp_opt(0, 0).unwrap();
        assert_eq!(Ts::try_from(dt), Ok(Ts(0)));
    }

    #[test]
    fn from_datetime_known_value() {
        let dt = parse("2023-10-06T17:35:55.440295Z");
        assert_eq!(Ts::try_from(dt), Ok(Ts(1_696_613_755_440_295_000)));
    }

    #[test]
    fn from_datetime_keeps_nanos() {
        let dt = parse("2023-10-06T17:35:55.440295999Z");
        assert_eq!(Ts::try_from(dt), Ok(Ts(1_696_613_755_440_295_999)));
    }

    #[test]
    fn from_datetime_before_epoch() {
        let dt = parse("1969-12-31T23:59:59.999999999Z");
        assert_eq!(Ts::try_from(dt), Ok(Ts(-1)));
    }

    #[test]
    fn from_datetime_out_of_range() {
        for s in ["1600-01-01T00:00:00Z", "2300-01-01T00:00:00Z"] {
            assert_eq!(
                Ts::try_from(parse(s)),
                Err("timestamp out of range"),
                "s={s}"
            );
        }
    }

    #[test]
    fn round_trip() {
        let cases = [
            0,
            1,
            -1,
            1_696_613_755_440_295_999,
            -1_696_613_755_440_295_999,
            i64::MAX,
            i64::MIN,
        ];
        for nanos in cases {
            let dt = DateTime::<Utc>::from(Ts(nanos));
            assert_eq!(Ts::try_from(dt), Ok(Ts(nanos)), "nanos={nanos}");
        }
    }

    #[test]
    fn ordering_matches_chronological() {
        let earlier = parse("2023-10-06T17:35:55.440295001Z");
        let later = parse("2023-10-06T17:35:55.440295002Z");
        assert!(Ts::try_from(earlier).unwrap() < Ts::try_from(later).unwrap());
    }

    #[test]
    fn now_is_close_to_chrono_now() {
        let before = Ts::try_from(Utc::now()).unwrap();
        let now = Ts::now().unwrap();
        let after = Ts::try_from(Utc::now()).unwrap();
        assert!(before <= now && now <= after);
    }

    #[test]
    fn try_from_str_utc() {
        assert_eq!(
            Ts::try_from("2023-10-06T17:35:55.440295123Z"),
            Ok(Ts(1_696_613_755_440_295_123))
        );
    }

    #[test]
    fn try_from_str_matches_from_datetime() {
        let s = "2023-10-06T17:35:55.440295999Z";
        assert_eq!(Ts::try_from(s), Ts::try_from(parse(s)));
    }

    #[test]
    fn try_from_str_with_offset() {
        assert_eq!(
            Ts::try_from("2023-10-06T19:35:55.440295+02:00"),
            Ts::try_from("2023-10-06T17:35:55.440295Z")
        );
    }

    #[test]
    fn try_from_str_without_fraction() {
        assert_eq!(Ts::try_from("1970-01-01T00:00:01Z"), Ok(Ts(1_000_000_000)));
    }

    #[test]
    fn try_from_str_out_of_range() {
        assert_eq!(
            Ts::try_from("2300-01-01T00:00:00Z"),
            Err("timestamp out of range")
        );
    }

    #[test]
    fn try_from_str_invalid() {
        let cases = [
            "",
            "not a timestamp",
            "2023-10-06",
            "2023-10-06T17:35:55",
            "2023-13-06T17:35:55Z",
            "1696613755440295",
        ];
        for s in cases {
            assert_eq!(
                Ts::try_from(s),
                Err("invalid RFC 3339 timestamp"),
                "s={s:?}"
            );
        }
    }
}
