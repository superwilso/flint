//! Local wall-clock time to UTC, in this PC's time zone — for logs written by a player that knows
//! the time but not the zone.
//!
//! An Audioscrobbler/1.1 log says which kind of clock wrote it. `#TZ/UTC` means the timestamps are
//! real UTC instants. `#TZ/UNKNOWN` means they are the player's wall clock written down *as if* it
//! were UTC, and the spec leaves the correction to the uploader, using its own time zone. Cinder
//! writes `#TZ/UNKNOWN` on purpose (`player/cinder-ffi/src/scrobble.rs`: the device has no zone
//! setting, so its numbers are local time with a UTC label), and so does `unknown321/scrobbler`.
//! Sending those numbers unconverted puts every play off by the listener's UTC offset — the bug
//! Cinder fixed on its side on 2026-09-06, which only stays fixed if the uploader does this half.
//!
//! The zone and its daylight-saving rules come from the operating system: `localtime_r` on Unix,
//! `SystemTimeToTzSpecificLocalTime` on Windows. No time-zone database is carried here, in keeping
//! with Flint having no dependencies. If the system cannot answer, the offset is taken as zero and
//! the timestamp goes out as written, which is what Flint did before.

/// `local` (a wall-clock time written as if it were UTC) as the UTC instant it names, here.
pub fn local_to_utc(local: i64) -> i64 {
    local_to_utc_with(local, utc_offset_at)
}

/// [`local_to_utc`] with the zone given as a function: seconds east of UTC at a UTC instant.
///
/// Two steps, because the offset depends on the instant being looked for: a first guess with the
/// offset at `local` itself, then the offset at that guess. That lands on the right side of a
/// daylight-saving change whenever the wall-clock time exists. A time that a spring-forward skips
/// gets a nearby instant, and one an autumn change repeats gets one of its two — no player can say
/// which of the two it meant.
pub fn local_to_utc_with(local: i64, offset_at: impl Fn(i64) -> i64) -> i64 {
    let guess = local - offset_at(local);
    local - offset_at(guess)
}

/// Seconds east of UTC in this PC's time zone at the UTC instant `utc`. 0 when the system cannot
/// say.
pub fn utc_offset_at(utc: i64) -> i64 {
    platform::offset(utc).filter(|o| o.abs() <= 18 * 3600).unwrap_or(0)
}

/// Days since 1970-01-01 to `(year, month, day)`, proleptic Gregorian. Howard Hinnant's algorithm.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `(year, month, day)` to days since 1970-01-01 — the inverse of [`civil_from_days`].
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(unix)]
mod platform {
    use std::ffi::{c_char, c_int, c_long};

    /// glibc's, musl's and the BSDs' `struct tm`: nine ints, then the offset and the zone name.
    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }

    extern "C" {
        // `time_t` is a `long` on every Unix Flint is built for.
        fn localtime_r(time: *const c_long, out: *mut Tm) -> *mut Tm;
    }

    pub fn offset(utc: i64) -> Option<i64> {
        let t = c_long::try_from(utc).ok()?;
        let mut tm = Tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        };
        // SAFETY: both pointers are to live locals of the right type; localtime_r writes only `tm`.
        let ok = unsafe { !localtime_r(&t, &mut tm).is_null() };
        // A `long` is an i64 on 64-bit Unix and an i32 on 32-bit, so the conversion is only useless
        // on the first.
        #[allow(clippy::useless_conversion)]
        let offset = i64::from(tm.tm_gmtoff);
        ok.then_some(offset)
    }
}

#[cfg(windows)]
mod platform {
    use super::{civil_from_days, days_from_civil};
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn SystemTimeToTzSpecificLocalTime(zone: *const c_void, utc: *const SystemTime, local: *mut SystemTime) -> i32;
    }

    pub fn offset(utc: i64) -> Option<i64> {
        let days = utc.div_euclid(86_400);
        let secs = utc.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let st = SystemTime {
            year: u16::try_from(year).ok()?,
            month: month as u16,
            day: day as u16,
            hour: (secs / 3600) as u16,
            minute: (secs / 60 % 60) as u16,
            second: (secs % 60) as u16,
            ..SystemTime::default()
        };
        let mut local = SystemTime::default();
        // SAFETY: a null zone means "the current one"; both structs are live and correctly laid out.
        if unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &st, &mut local) } == 0 {
            return None;
        }
        let as_if_utc = days_from_civil(i64::from(local.year), u32::from(local.month), u32::from(local.day)) * 86_400
            + i64::from(local.hour) * 3600
            + i64::from(local.minute) * 60
            + i64::from(local.second);
        Some(as_if_utc - utc)
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    pub fn offset(_utc: i64) -> Option<i64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UK's rule: UTC+1 from 01:00 UTC on the last Sunday of March to 01:00 UTC on the last
    /// Sunday of October, UTC otherwise. Built from the civil-date functions under test, so it is
    /// also a check that they agree with each other.
    fn uk(utc: i64) -> i64 {
        let (year, _, _) = civil_from_days(utc.div_euclid(86_400));
        let last_sunday = |month: u32| {
            let last = days_from_civil(year, month, 31);
            // 1970-01-01 was a Thursday: (days + 4) % 7 is 0 on a Sunday.
            last - (last + 4).rem_euclid(7)
        };
        let start = last_sunday(3) * 86_400 + 3600;
        let end = last_sunday(10) * 86_400 + 3600;
        if (start..end).contains(&utc) {
            3600
        } else {
            0
        }
    }

    fn at(year: i64, month: u32, day: u32, h: i64, m: i64) -> i64 {
        days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60
    }

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(civil_from_days(20_362), (2025, 10, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        for days in (-800_000..800_000).step_by(997) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
    }

    #[test]
    fn a_summer_play_moves_back_an_hour_and_a_winter_one_does_not() {
        // 20:00 on the player's clock in July is 19:00 UTC in London.
        assert_eq!(local_to_utc_with(at(2026, 7, 1, 20, 0), uk), at(2026, 7, 1, 19, 0));
        assert_eq!(local_to_utc_with(at(2026, 1, 15, 20, 0), uk), at(2026, 1, 15, 20, 0));
    }

    #[test]
    fn the_changeover_lands_on_the_right_side() {
        // 2026's changes are on 29 March and 25 October. 00:30 local on 29 March is still GMT;
        // 02:30 is BST. 00:30 local on 25 October is BST; 02:30 is GMT again.
        assert_eq!(local_to_utc_with(at(2026, 3, 29, 0, 30), uk), at(2026, 3, 29, 0, 30));
        assert_eq!(local_to_utc_with(at(2026, 3, 29, 2, 30), uk), at(2026, 3, 29, 1, 30));
        assert_eq!(local_to_utc_with(at(2026, 10, 25, 0, 30), uk), at(2026, 10, 24, 23, 30));
        assert_eq!(local_to_utc_with(at(2026, 10, 25, 2, 30), uk), at(2026, 10, 25, 2, 30));
    }

    #[test]
    fn a_zone_west_of_utc_moves_plays_later() {
        let new_york_winter = |_: i64| -5 * 3600;
        assert_eq!(local_to_utc_with(at(2026, 1, 15, 20, 0), new_york_winter), at(2026, 1, 16, 1, 0));
    }

    /// Whatever zone the machine running the tests is in, the system's answer is a real offset.
    #[test]
    fn the_system_offset_is_a_plausible_one() {
        let o = utc_offset_at(at(2026, 7, 1, 12, 0));
        assert!(o.abs() <= 14 * 3600, "{o}");
        assert_eq!(o % 900, 0, "offsets are whole quarter hours: {o}");
    }
}
