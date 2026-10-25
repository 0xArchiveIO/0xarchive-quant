//! Dates, time zones and exchange hours, in integer arithmetic so every platform agrees and
//! nothing reads the wall clock or the platform's zone data.
//!
//! Zone rules are written out (as the terminal's `market-sessions.mjs` does): New York and
//! Chicago follow the US rule (02:00 local on the second Sunday of March to 02:00 local on
//! the first Sunday of November), London the EU rule (01:00 UTC on the last Sunday of March
//! to 01:00 UTC on the last Sunday of October), Tokyo none.
//!
//! NYSE holidays and early closes come from the exchange's rules (rule 7.2) plus the days
//! it closed by proclamation; a test checks them against the published lists the terminal
//! carries (`market-calendars.mjs`).

pub const MINUTE_MS: i64 = 60_000;
pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 86_400_000;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day number: (year, month 1-12, day 1-31).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// 0 = Monday ... 6 = Sunday.
pub fn weekday(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (days_from_civil(ny, nm, 1) - days_from_civil(y, m, 1)) as u32
}

/// The day number of the `n`th `wd` (0 = Monday) of a month; n = -1 for the last.
pub fn nth_weekday(y: i64, m: u32, wd: u32, n: i32) -> i64 {
    if n > 0 {
        let first = days_from_civil(y, m, 1);
        first + (wd as i64 - weekday(first) as i64).rem_euclid(7) + 7 * (n as i64 - 1)
    } else {
        let last = days_from_civil(y, m, days_in_month(y, m));
        last - (weekday(last) as i64 - wd as i64).rem_euclid(7)
    }
}

/// Easter Sunday's day number (the anonymous Gregorian algorithm).
pub fn easter(y: i64) -> i64 {
    let a = y % 19;
    let b = y / 100;
    let c = y % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    days_from_civil(y, month as u32, day as u32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Utc,
    NewYork,
    Chicago,
    London,
    Tokyo,
}

impl Zone {
    pub fn from_name(name: &str) -> Option<Zone> {
        Some(match name {
            "UTC" | "utc" => Zone::Utc,
            "America/New_York" | "new_york" | "ET" => Zone::NewYork,
            "America/Chicago" | "chicago" | "CT" => Zone::Chicago,
            "Europe/London" | "london" => Zone::London,
            "Asia/Tokyo" | "tokyo" => Zone::Tokyo,
            _ => return None,
        })
    }

    /// Ms the zone is ahead of UTC at instant `t`.
    pub fn offset(self, t: i64) -> i64 {
        let std = match self {
            Zone::Utc | Zone::London => 0,
            Zone::NewYork => -5 * HOUR_MS,
            Zone::Chicago => -6 * HOUR_MS,
            Zone::Tokyo => 9 * HOUR_MS,
        };
        let (y, _, _) = civil_from_days(t.div_euclid(DAY_MS));
        let (start, end) = match self {
            Zone::Utc | Zone::Tokyo => return std,
            Zone::NewYork | Zone::Chicago => (
                nth_weekday(y, 3, 6, 2) * DAY_MS + 2 * HOUR_MS - std,
                nth_weekday(y, 11, 6, 1) * DAY_MS + 2 * HOUR_MS - (std + HOUR_MS),
            ),
            Zone::London => (
                nth_weekday(y, 3, 6, -1) * DAY_MS + HOUR_MS,
                nth_weekday(y, 10, 6, -1) * DAY_MS + HOUR_MS,
            ),
        };
        if t >= start && t < end {
            std + HOUR_MS
        } else {
            std
        }
    }

    /// Local wall-clock ms (as if UTC) at instant `t`.
    pub fn local(self, t: i64) -> i64 {
        t + self.offset(t)
    }

    /// The instant of local wall-clock ms `wall`. Two passes settle the offset on either side
    /// of a clock change; a wall time the change skips or repeats maps to an instant near it
    /// (no session edge sits in such an hour).
    pub fn instant(self, wall: i64) -> i64 {
        let t = wall - self.offset(wall);
        wall - self.offset(t)
    }
}

/// A local date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub days: i64,
    pub year: i64,
    pub month: u32,
    pub day: u32,
    /// 0 = Monday.
    pub weekday: u32,
    /// Ms after local midnight.
    pub ms: i64,
}

impl Local {
    pub fn at(t: i64, zone: Zone) -> Local {
        let wall = zone.local(t);
        let days = wall.div_euclid(DAY_MS);
        let (year, month, day) = civil_from_days(days);
        Local {
            days,
            year,
            month,
            day,
            weekday: weekday(days),
            ms: wall.rem_euclid(DAY_MS),
        }
    }
}

/// Days NYSE closed outside its holiday rules: hurricane Sandy, and national days of
/// mourning for Presidents Bush (2018) and Carter (2025).
const NYSE_CLOSED: &[((i64, u32, u32), &str)] = &[
    ((2012, 10, 29), "Hurricane Sandy"),
    ((2012, 10, 30), "Hurricane Sandy"),
    (
        (2018, 12, 5),
        "National Day of Mourning (George H. W. Bush)",
    ),
    ((2025, 1, 9), "National Day of Mourning (Jimmy Carter)"),
];

/// A fixed-date holiday's observed day: Saturday moves to Friday, Sunday to Monday.
fn observed(d: i64) -> i64 {
    match weekday(d) {
        5 => d - 1,
        6 => d + 1,
        _ => d,
    }
}

/// Why NYSE is closed on day `d` (a New York date), if it is a weekday holiday.
pub fn nyse_holiday(d: i64) -> Option<&'static str> {
    let (y, m, day) = civil_from_days(d);
    if let Some((_, why)) = NYSE_CLOSED.iter().find(|(k, _)| *k == (y, m, day)) {
        return Some(why);
    }
    // New Year's Day on a Saturday is not moved back into December (rule 7.2).
    let ny = days_from_civil(y, 1, 1);
    if weekday(ny) != 5 && observed(ny) == d {
        return Some("New Year's Day");
    }
    let rules: [(i64, &str); 9] = [
        (nth_weekday(y, 1, 0, 3), "Martin Luther King Jr. Day"),
        (nth_weekday(y, 2, 0, 3), "Washington's Birthday"),
        (easter(y) - 2, "Good Friday"),
        (nth_weekday(y, 5, 0, -1), "Memorial Day"),
        (
            if y >= 2022 {
                observed(days_from_civil(y, 6, 19))
            } else {
                i64::MIN
            },
            "Juneteenth",
        ),
        (observed(days_from_civil(y, 7, 4)), "Independence Day"),
        (nth_weekday(y, 9, 0, 1), "Labor Day"),
        (nth_weekday(y, 11, 3, 4), "Thanksgiving Day"),
        (observed(days_from_civil(y, 12, 25)), "Christmas Day"),
    ];
    rules.iter().find(|(r, _)| *r == d).map(|(_, why)| *why)
}

/// Whether NYSE closes at 13:00 on day `d`: July 3 and Christmas Eve when they fall Monday
/// to Thursday, and the day after Thanksgiving.
pub fn nyse_early_close(d: i64) -> bool {
    let (y, m, day) = civil_from_days(d);
    let wd = weekday(d);
    ((m, day) == (7, 3) && wd <= 3)
        || ((m, day) == (12, 24) && wd <= 3)
        || d == nth_weekday(y, 11, 3, 4) + 1
}

/// An exchange's regular hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    /// NYSE, 09:30 to 16:00 New York time (13:00 on early closes), with its holidays.
    Us,
    /// The London Stock Exchange, 08:00 to 16:30 London time. Weekdays; no holiday list.
    Europe,
    /// The Tokyo Stock Exchange, 09:00 to 11:30 and 12:30 to 15:30 Tokyo time. Weekdays; no
    /// holiday list.
    Asia,
}

impl Session {
    pub fn from_name(name: &str) -> Option<Session> {
        Some(match name {
            "us" | "nyse" => Session::Us,
            "europe" | "london" => Session::Europe,
            "asia" | "tokyo" => Session::Asia,
            _ => return None,
        })
    }

    pub fn zone(self) -> Zone {
        match self {
            Session::Us => Zone::NewYork,
            Session::Europe => Zone::London,
            Session::Asia => Zone::Tokyo,
        }
    }

    /// The day's sessions as (open, close) instants; none on a weekend or holiday. `d` is a
    /// date in the exchange's zone.
    pub fn spans(self, d: i64) -> Vec<(i64, i64)> {
        if weekday(d) >= 5 {
            return vec![];
        }
        let m = |h: i64, mi: i64| h * HOUR_MS + mi * MINUTE_MS;
        let local: Vec<(i64, i64)> = match self {
            Session::Us => {
                if nyse_holiday(d).is_some() {
                    return vec![];
                }
                let close = if nyse_early_close(d) {
                    m(13, 0)
                } else {
                    m(16, 0)
                };
                vec![(m(9, 30), close)]
            }
            Session::Europe => vec![(m(8, 0), m(16, 30))],
            Session::Asia => vec![(m(9, 0), m(11, 30)), (m(12, 30), m(15, 30))],
        };
        let z = self.zone();
        local
            .into_iter()
            .map(|(a, b)| (z.instant(d * DAY_MS + a), z.instant(d * DAY_MS + b)))
            .collect()
    }

    /// The session open at instant `t` (open <= t < close), as (open, close).
    pub fn at(self, t: i64) -> Option<(i64, i64)> {
        let d = Local::at(t, self.zone()).days;
        self.spans(d).into_iter().find(|&(a, b)| a <= t && t < b)
    }

    /// The next open after `t`, within 30 days.
    pub fn next_open(self, t: i64) -> Option<i64> {
        let d = Local::at(t, self.zone()).days;
        (d..d + 30)
            .flat_map(|d| self.spans(d))
            .map(|(a, _)| a)
            .find(|&a| a > t)
    }

    /// The last close at or before `t`, within 30 days.
    pub fn last_close(self, t: i64) -> Option<i64> {
        let d = Local::at(t, self.zone()).days;
        (d - 30..=d)
            .rev()
            .flat_map(|d| self.spans(d).into_iter().rev())
            .map(|(_, b)| b)
            .find(|&b| b <= t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> i64 {
        let p: Vec<i64> = s.split('-').map(|x| x.parse().unwrap()).collect();
        days_from_civil(p[0], p[1] as u32, p[2] as u32)
    }

    #[test]
    fn civil_dates_round_trip() {
        for z in -800_000..800_000 {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(day("1970-01-01"), 0);
        assert_eq!(weekday(day("2026-10-07")), 2, "a Wednesday");
        assert_eq!(easter(2026), day("2026-04-05"));
        assert_eq!(easter(2025), day("2025-04-20"));
        assert_eq!(days_in_month(2028, 2), 29);
    }

    #[test]
    fn zones_switch_on_their_rules() {
        let t = |s: &str, h: i64| day(s) * DAY_MS + h * HOUR_MS;
        // 2026: the US switches 8 March and 1 November, the UK 29 March and 25 October.
        assert_eq!(Zone::NewYork.offset(t("2026-03-08", 6)), -5 * HOUR_MS);
        assert_eq!(Zone::NewYork.offset(t("2026-03-08", 7)), -4 * HOUR_MS);
        assert_eq!(Zone::NewYork.offset(t("2026-11-01", 5)), -4 * HOUR_MS);
        assert_eq!(Zone::NewYork.offset(t("2026-11-01", 6)), -5 * HOUR_MS);
        assert_eq!(Zone::London.offset(t("2026-03-29", 0)), 0);
        assert_eq!(Zone::London.offset(t("2026-03-29", 1)), HOUR_MS);
        assert_eq!(Zone::London.offset(t("2026-10-25", 1)), 0);
        assert_eq!(Zone::Tokyo.offset(t("2026-07-01", 0)), 9 * HOUR_MS);
        // 09:30 New York on 9 March 2026 (daylight time) is 13:30 UTC.
        assert_eq!(
            Zone::NewYork.instant(day("2026-03-09") * DAY_MS + 9 * HOUR_MS + 30 * MINUTE_MS),
            t("2026-03-09", 13) + 30 * MINUTE_MS
        );
    }

    #[test]
    fn nyse_matches_the_published_calendars() {
        // NYSE's published holidays, 2024 to 2027 (2025-12 on from the terminal's
        // market-calendars.mjs; earlier from nyse.com's holiday pages).
        let published = [
            "2024-01-01",
            "2024-01-15",
            "2024-02-19",
            "2024-03-29",
            "2024-05-27",
            "2024-06-19",
            "2024-07-04",
            "2024-09-02",
            "2024-11-28",
            "2024-12-25",
            "2025-01-01",
            "2025-01-09",
            "2025-01-20",
            "2025-02-17",
            "2025-04-18",
            "2025-05-26",
            "2025-06-19",
            "2025-07-04",
            "2025-09-01",
            "2025-11-27",
            "2025-12-25",
            "2026-01-01",
            "2026-01-19",
            "2026-02-16",
            "2026-04-03",
            "2026-05-25",
            "2026-06-19",
            "2026-07-03",
            "2026-09-07",
            "2026-11-26",
            "2026-12-25",
            "2027-01-01",
            "2027-01-18",
            "2027-02-15",
            "2027-03-26",
            "2027-05-31",
            "2027-06-18",
            "2027-07-05",
            "2027-09-06",
            "2027-11-25",
            "2027-12-24",
        ];
        let early = [
            "2024-07-03",
            "2024-11-29",
            "2024-12-24",
            "2025-07-03",
            "2025-11-28",
            "2025-12-24",
            "2026-11-27",
            "2026-12-24",
            "2027-11-26",
        ];
        let holidays: Vec<i64> = published.iter().map(|s| day(s)).collect();
        let earlies: Vec<i64> = early.iter().map(|s| day(s)).collect();
        for d in day("2024-01-01")..=day("2027-12-31") {
            if weekday(d) >= 5 {
                continue;
            }
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(
                nyse_holiday(d).is_some(),
                holidays.contains(&d),
                "{y}-{m:02}-{dd:02}"
            );
            assert_eq!(
                nyse_early_close(d) && nyse_holiday(d).is_none(),
                earlies.contains(&d),
                "early close {y}-{m:02}-{dd:02}"
            );
        }
    }

    #[test]
    fn sessions_open_and_close_in_their_zones() {
        let t = |s: &str, h: i64, mi: i64| day(s) * DAY_MS + h * HOUR_MS + mi * MINUTE_MS;
        // Wednesday 7 October 2026: New York is UTC-4, so 13:30 to 20:00 UTC.
        assert_eq!(
            Session::Us.at(t("2026-10-07", 15, 0)),
            Some((t("2026-10-07", 13, 30), t("2026-10-07", 20, 0)))
        );
        assert_eq!(Session::Us.at(t("2026-10-07", 20, 0)), None);
        assert_eq!(Session::Us.at(t("2026-11-26", 15, 0)), None, "Thanksgiving");
        assert_eq!(
            Session::Us.at(t("2026-11-27", 15, 0)).map(|s| s.1),
            Some(t("2026-11-27", 18, 0)),
            "13:00 ET in winter time"
        );
        assert_eq!(
            Session::Us.next_open(t("2026-11-25", 22, 0)),
            Some(t("2026-11-27", 14, 30))
        );
        // Tokyo's lunch break; London in summer time.
        assert!(Session::Asia.at(t("2026-10-07", 2, 29)).is_some());
        assert!(Session::Asia.at(t("2026-10-07", 2, 30)).is_none());
        assert!(Session::Asia.at(t("2026-10-07", 3, 30)).is_some());
        assert_eq!(
            Session::Europe.at(t("2026-10-07", 7, 0)).map(|s| s.0),
            Some(t("2026-10-07", 7, 0))
        );
        assert_eq!(
            Session::Europe.last_close(t("2026-10-10", 12, 0)),
            Some(t("2026-10-09", 15, 30)),
            "Friday's close, from a Saturday"
        );
    }
}
