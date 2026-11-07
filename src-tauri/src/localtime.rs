//! Local wall-clock time, which std does not have.
//!
//! Catch-up URLs are asked for in the provider's local time, and the
//! recording service names its files and draws its clock track in this
//! machine's. Only the offset from UTC comes from the platform; the calendar
//! arithmetic is here, in Rust, where it can be tested.

/// A wall-clock reading, `offset` seconds east of UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Civil {
    /// `unix` read `offset` seconds east of UTC. Pure: the offset is the
    /// caller's, so tests need no time zone.
    pub fn at(unix: i64, offset: i64) -> Self {
        let t = unix + offset;
        let (year, month, day) = civil_from_days(t.div_euclid(86_400));
        let s = t.rem_euclid(86_400) as u32;
        Self {
            year,
            month,
            day,
            hour: s / 3600,
            minute: (s % 3600) / 60,
            second: s % 60,
        }
    }

    /// `unix` in this machine's time zone.
    pub fn local(unix: i64) -> Self {
        Self::at(unix, utc_offset(unix))
    }
}

/// Days since the Unix epoch to a civil date. Howard Hinnant's `civil_from_days`.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A civil date to days since the Unix epoch: the inverse of
/// [`civil_from_days`] (Howard Hinnant's `days_from_civil`).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (m, d) = (i64::from(month), i64::from(day));
    let y = if m <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds east of UTC at `unix`, from the C library.
///
/// std has no local time, so this is the C library's `localtime_r`, which
/// every Unix has: declared here and linked like any libc function rather
/// than looked up at run time, which the static zapper-server for Linux
/// (musl, no `dlopen`) cannot do. Only the offset is taken; the calendar
/// arithmetic stays in Rust where it can be tested.
#[cfg(unix)]
pub fn utc_offset(unix: i64) -> i64 {
    use std::ffi::{c_char, c_int, c_long};

    /// glibc's `struct tm`: nine ints, then `tm_gmtoff` and `tm_zone`. musl
    /// and the BSDs, macOS among them, lay it out the same way.
    #[repr(C)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        mday: c_int,
        mon: c_int,
        year: c_int,
        wday: c_int,
        yday: c_int,
        isdst: c_int,
        gmtoff: c_long,
        zone: *const c_char,
    }

    extern "C" {
        fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
    }
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    let t = unix;
    if unsafe { localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    tm.gmtoff as i64
}

/// Seconds east of UTC at `unix`, from Windows' own time-zone rules.
///
/// There is no `tm_gmtoff` here. Instead the instant is converted to local
/// time with `SystemTimeToTzSpecificLocalTime`, which applies the daylight
/// rule in force *at that instant* rather than now (a catch-up programme from
/// before the clocks changed must be asked for in the offset it aired in),
/// and the two wall-clock readings are subtracted.
#[cfg(windows)]
pub fn utc_offset(unix: i64) -> i64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };

    /// 100ns ticks between 1601-01-01 (FILETIME's epoch) and 1970-01-01.
    const EPOCH_DELTA: i64 = 116_444_736_000_000_000;
    const TICKS_PER_SEC: i64 = 10_000_000;

    let ticks = unix
        .saturating_mul(TICKS_PER_SEC)
        .saturating_add(EPOCH_DELTA);
    let utc_ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut local_ft = FILETIME::default();
    let converted = unsafe {
        FileTimeToSystemTime(&utc_ft, &mut utc).is_ok()
            && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok()
            && SystemTimeToFileTime(&local, &mut local_ft).is_ok()
    };
    if !converted {
        tracing::warn!("could not read the local time zone; local times will read as UTC");
        return 0;
    }
    let local_ticks =
        (i64::from(local_ft.dwHighDateTime) << 32) | i64::from(local_ft.dwLowDateTime);
    (local_ticks - ticks) / TICKS_PER_SEC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_carries_the_offset() {
        // 2026-11-03 20:00:00 UTC.
        let t = 1_793_736_000;
        assert_eq!(
            Civil::at(t, 0),
            Civil {
                year: 2026,
                month: 11,
                day: 3,
                hour: 20,
                minute: 0,
                second: 0
            }
        );
        // Five hours west crosses no date; eleven east crosses one.
        assert_eq!(Civil::at(t, -5 * 3600).hour, 15);
        let east = Civil::at(t, 11 * 3600);
        assert_eq!((east.day, east.hour), (4, 7));
    }

    #[test]
    fn days_from_civil_inverts_civil_from_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        for days in [-800_000, -1, 0, 59, 19_782, 20_764, 400_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
    }
}
