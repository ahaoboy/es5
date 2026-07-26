//! Date constructor and Date.prototype (jsdate.c).
//!
//! Civil date/time math is implemented exactly as in mujs. The local
//! timezone offset is computed once using the same mktime/localtime trick
//! (via the time crate);
use time::UtcOffset;
use super::propf;
use crate::object::{Class, Payload};
use crate::state::{State, R};
use crate::value::JS_DONTENUM;

const HOURS_PER_DAY: f64 = 24.0;
const MINUTES_PER_HOUR: f64 = 60.0;
const SECONDS_PER_MINUTE: f64 = 60.0;
const MS_PER_SECOND: f64 = 1000.0;
const MS_PER_MINUTE: f64 = SECONDS_PER_MINUTE * MS_PER_SECOND;
const MS_PER_HOUR: f64 = MINUTES_PER_HOUR * MS_PER_MINUTE;
const MS_PER_DAY: f64 = HOURS_PER_DAY * MS_PER_HOUR;

fn now() -> f64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (d.as_secs() as f64 * 1000.0 + d.subsec_micros() as f64 / 1000.0).floor()
}
/// LocalTZA: local timezone offset in milliseconds, computed once.
fn local_tza() -> f64 {
    use std::sync::OnceLock;
    static TZA: OnceLock<f64> = OnceLock::new();
    *TZA.get_or_init(compute_tza)
}
fn compute_tza() -> f64 {
    UtcOffset::current_local_offset()
        .expect("failed to determine local UTC offset")
        .whole_seconds() as f64
        * 1000.0
}

fn pmod(x: f64, y: f64) -> f64 {
    let x = x % y;
    if x < 0.0 {
        x + y
    } else {
        x
    }
}

fn day(t: f64) -> i32 {
    (t / MS_PER_DAY).floor() as i32
}

fn time_within_day(t: f64) -> f64 {
    pmod(t, MS_PER_DAY)
}

fn days_in_year(y: i32) -> i32 {
    if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
        366
    } else {
        365
    }
}

fn day_from_year(y: i32) -> i32 {
    365 * (y - 1970) + ((y - 1969) as f64 / 4.0).floor() as i32
        - ((y - 1901) as f64 / 100.0).floor() as i32
        + ((y - 1601) as f64 / 400.0).floor() as i32
}

fn time_from_year(y: i32) -> f64 {
    day_from_year(y) as f64 * MS_PER_DAY
}

fn year_from_time(t: f64) -> i32 {
    let mut y = (t / (MS_PER_DAY * 365.2425)).floor() as i32 + 1970;
    let t2 = time_from_year(y);
    if t2 > t {
        y -= 1;
    } else if t2 + MS_PER_DAY * days_in_year(y) as f64 <= t {
        y += 1;
    }
    y
}

fn in_leap_year(t: f64) -> i32 {
    if days_in_year(year_from_time(t)) == 366 {
        1
    } else {
        0
    }
}

fn day_within_year(t: f64) -> i32 {
    day(t) - day_from_year(year_from_time(t))
}

fn month_from_time(t: f64) -> i32 {
    let day = day_within_year(t);
    let leap = in_leap_year(t);
    if day < 31 {
        return 0;
    }
    if day < 59 + leap {
        return 1;
    }
    if day < 90 + leap {
        return 2;
    }
    if day < 120 + leap {
        return 3;
    }
    if day < 151 + leap {
        return 4;
    }
    if day < 181 + leap {
        return 5;
    }
    if day < 212 + leap {
        return 6;
    }
    if day < 243 + leap {
        return 7;
    }
    if day < 273 + leap {
        return 8;
    }
    if day < 304 + leap {
        return 9;
    }
    if day < 334 + leap {
        return 10;
    }
    11
}

fn date_from_time(t: f64) -> i32 {
    let day = day_within_year(t);
    let leap = in_leap_year(t);
    match month_from_time(t) {
        0 => day + 1,
        1 => day - 30,
        2 => day - 58 - leap,
        3 => day - 89 - leap,
        4 => day - 119 - leap,
        5 => day - 150 - leap,
        6 => day - 180 - leap,
        7 => day - 211 - leap,
        8 => day - 242 - leap,
        9 => day - 272 - leap,
        10 => day - 303 - leap,
        _ => day - 333 - leap,
    }
}

fn week_day(t: f64) -> i32 {
    pmod(day(t) as f64 + 4.0, 7.0) as i32
}

fn local_time(utc: f64) -> f64 {
    utc + local_tza()
}

fn utc(loc: f64) -> f64 {
    loc - local_tza()
}

fn hour_from_time(t: f64) -> i32 {
    pmod((t / MS_PER_HOUR).floor(), HOURS_PER_DAY) as i32
}

fn min_from_time(t: f64) -> i32 {
    pmod((t / MS_PER_MINUTE).floor(), MINUTES_PER_HOUR) as i32
}

fn sec_from_time(t: f64) -> i32 {
    pmod((t / MS_PER_SECOND).floor(), SECONDS_PER_MINUTE) as i32
}

fn ms_from_time(t: f64) -> i32 {
    pmod(t, MS_PER_SECOND) as i32
}

fn make_time(hour: f64, min: f64, sec: f64, ms: f64) -> f64 {
    ((hour * MINUTES_PER_HOUR + min) * SECONDS_PER_MINUTE + sec) * MS_PER_SECOND + ms
}

fn make_day(mut y: f64, m: f64, date: f64) -> f64 {
    const FIRST_DAY_OF_MONTH: [[f64; 12]; 2] = [
        [0.0, 31.0, 59.0, 90.0, 120.0, 151.0, 181.0, 212.0, 243.0, 273.0, 304.0, 334.0],
        [0.0, 31.0, 60.0, 91.0, 121.0, 152.0, 182.0, 213.0, 244.0, 274.0, 305.0, 335.0],
    ];

    y += (m / 12.0).floor();
    let m = pmod(m, 12.0);

    let im = m as i32;
    if !(0..12).contains(&im) {
        return f64::NAN;
    }

    let yd = (time_from_year(y as i32) / MS_PER_DAY).floor();
    let md = FIRST_DAY_OF_MONTH[(days_in_year(y as i32) == 366) as usize][im as usize];

    yd + md + date - 1.0
}

fn make_date(day: f64, time: f64) -> f64 {
    day * MS_PER_DAY + time
}

fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > 8.64e15 {
        return f64::NAN;
    }
    t.trunc()
}

fn toint(s: &str, pos: &mut usize, w: usize) -> Option<i32> {
    let b = s.as_bytes();
    let mut v = 0i32;
    for i in 0..w {
        let c = *b.get(*pos + i)?;
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as i32;
    }
    *pos += w;
    Some(v)
}

/// Parse ISO 8601 formatted date and time:
/// YYYY("-"MM("-"DD)?)?("T"HH":"mm(":"ss("."sss)?)?("Z"|[+-]HH(":"mm)?)?)?
fn parse_date_time(s: &str) -> f64 {
    let mut pos = 0usize;
    let (mut m, mut d, mut h, mut min, mut sec, mut ms) = (1, 1, 0, 0, 0, 0);
    let mut tza = 0.0;

    let y = match toint(s, &mut pos, 4) {
        Some(v) => v,
        None => return f64::NAN,
    };
    if s.as_bytes().get(pos) == Some(&b'-') {
        pos += 1;
        m = match toint(s, &mut pos, 2) {
            Some(v) => v,
            None => return f64::NAN,
        };
        if s.as_bytes().get(pos) == Some(&b'-') {
            pos += 1;
            d = match toint(s, &mut pos, 2) {
                Some(v) => v,
                None => return f64::NAN,
            };
        }
    }

    if s.as_bytes().get(pos) == Some(&b'T') {
        pos += 1;
        h = match toint(s, &mut pos, 2) {
            Some(v) => v,
            None => return f64::NAN,
        };
        if s.as_bytes().get(pos) != Some(&b':') {
            return f64::NAN;
        }
        pos += 1;
        min = match toint(s, &mut pos, 2) {
            Some(v) => v,
            None => return f64::NAN,
        };
        if s.as_bytes().get(pos) == Some(&b':') {
            pos += 1;
            sec = match toint(s, &mut pos, 2) {
                Some(v) => v,
                None => return f64::NAN,
            };
            if s.as_bytes().get(pos) == Some(&b'.') {
                pos += 1;
                ms = match toint(s, &mut pos, 3) {
                    Some(v) => v,
                    None => return f64::NAN,
                };
            }
        }
        match s.as_bytes().get(pos) {
            Some(b'Z') => {
                pos += 1;
                tza = 0.0;
            }
            Some(c) if *c == b'+' || *c == b'-' => {
                let tzs = if *c == b'+' { 1.0 } else { -1.0 };
                pos += 1;
                let tzh = match toint(s, &mut pos, 2) {
                    Some(v) => v,
                    None => return f64::NAN,
                };
                let mut tzm = 0;
                if s.as_bytes().get(pos) == Some(&b':') {
                    pos += 1;
                    tzm = match toint(s, &mut pos, 2) {
                        Some(v) => v,
                        None => return f64::NAN,
                    };
                }
                if tzh > 23 || tzm > 59 {
                    return f64::NAN;
                }
                tza = tzs * (tzh as f64 * MS_PER_HOUR + tzm as f64 * MS_PER_MINUTE);
            }
            _ => {
                tza = local_tza();
            }
        }
    }

    if pos != s.len() {
        return f64::NAN;
    }

    if !(1..=12).contains(&m) {
        return f64::NAN;
    }
    if !(1..=31).contains(&d) {
        return f64::NAN;
    }
    if h > 24 {
        return f64::NAN;
    }
    if min > 59 {
        return f64::NAN;
    }
    if sec > 59 {
        return f64::NAN;
    }
    if ms > 999 {
        return f64::NAN;
    }
    if h == 24 && (min != 0 || sec != 0 || ms != 0) {
        return f64::NAN;
    }

    // TODO: DaylightSavingTA on local times
    let t = make_date(
        make_day(y as f64, (m - 1) as f64, d as f64),
        make_time(h as f64, min as f64, sec as f64, ms as f64),
    );
    t - tza
}

// -- date formatting -----------------------------------------------------------

fn fmtdate(t: f64) -> String {
    if !t.is_finite() {
        return "Invalid Date".to_string();
    }
    let y = year_from_time(t);
    let m = month_from_time(t);
    let d = date_from_time(t);
    format!("{:04}-{:02}-{:02}", y, m + 1, d)
}

fn fmttime(t: f64, tza: f64) -> String {
    if !t.is_finite() {
        return "Invalid Date".to_string();
    }
    let h = hour_from_time(t);
    let m = min_from_time(t);
    let s = sec_from_time(t);
    let ms = ms_from_time(t);
    let tzh = hour_from_time(tza.abs());
    let tzm = min_from_time(tza.abs());
    if tza == 0.0 {
        format!("{:02}:{:02}:{:02}.{:03}Z", h, m, s, ms)
    } else if tza < 0.0 {
        format!("{:02}:{:02}:{:02}.{:03}-{:02}:{:02}", h, m, s, ms, tzh, tzm)
    } else {
        format!("{:02}:{:02}:{:02}.{:03}+{:02}:{:02}", h, m, s, ms, tzh, tzm)
    }
}

fn fmtdatetime(t: f64, tza: f64) -> String {
    if !t.is_finite() {
        return "Invalid Date".to_string();
    }
    format!("{}T{}", fmtdate(t), fmttime(t, tza))
}

// -- Date functions -----------------------------------------------------------

fn js_todate(st: &mut State, idx: i32) -> R<f64> {
    let obj = st.toobject(idx)?;
    match &st.heap.obj(obj).payload {
        Payload::Number(n) if st.heap.obj(obj).class == Class::Date => Ok(*n),
        _ => st.type_error("not a date"),
    }
}

fn js_setdate(st: &mut State, idx: i32, t: f64) -> R<()> {
    let obj = st.toobject(idx)?;
    if st.heap.obj(obj).class != Class::Date {
        return st.type_error("not a date");
    }
    let t = time_clip(t);
    if let Payload::Number(n) = &mut st.heap.obj_mut(obj).payload {
        *n = t;
    }
    st.push_number(t)
}

fn d_parse(st: &mut State) -> R<()> {
    let s = st.tostring(1)?;
    let t = parse_date_time(&s);
    st.push_number(t)
}

fn optnumber(st: &mut State, idx: i32, v: f64) -> R<f64> {
    if st.isdefined(idx) {
        st.tonumber(idx)
    } else {
        Ok(v)
    }
}

fn d_utc(st: &mut State) -> R<()> {
    let mut y = st.tonumber(1)?;
    if y < 100.0 {
        y += 1900.0;
    }
    let m = st.tonumber(2)?;
    let d = optnumber(st, 3, 1.0)?;
    let h = optnumber(st, 4, 0.0)?;
    let min = optnumber(st, 5, 0.0)?;
    let s = optnumber(st, 6, 0.0)?;
    let ms = optnumber(st, 7, 0.0)?;
    let t = make_date(make_day(y, m, d), make_time(h, min, s, ms));
    let t = time_clip(t);
    st.push_number(t)
}

fn d_now(st: &mut State) -> R<()> {
    st.push_number(now())
}

fn jsb_date(st: &mut State) -> R<()> {
    let s = fmtdatetime(local_time(now()), local_tza());
    st.push_string(&s)
}

fn jsb_new_date(st: &mut State) -> R<()> {
    let top = st.gettop();
    let mut t: f64;

    if top == 1 {
        t = now();
    } else if top == 2 {
        st.toprimitive(1, crate::value::Hint::None)?;
        if st.isstring(1) {
            let s = st.tostring(1)?;
            t = parse_date_time(&s);
        } else {
            t = time_clip(st.tonumber(1)?);
        }
    } else {
        let mut y = st.tonumber(1)?;
        if y < 100.0 {
            y += 1900.0;
        }
        let m = st.tonumber(2)?;
        let d = optnumber(st, 3, 1.0)?;
        let h = optnumber(st, 4, 0.0)?;
        let min = optnumber(st, 5, 0.0)?;
        let s = optnumber(st, 6, 0.0)?;
        let ms = optnumber(st, 7, 0.0)?;
        t = make_date(make_day(y, m, d), make_time(h, min, s, ms));
        t = time_clip(utc(t));
    }

    let proto = st.protos.date;
    let obj = st.heap.alloc_object(Class::Date, Some(proto));
    st.heap.obj_mut(obj).payload = Payload::Number(t);
    st.push_object(obj)
}

fn dp_valueof(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    st.push_number(t)
}

fn dp_tostring(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let s = fmtdatetime(local_time(t), local_tza());
    st.push_string(&s)
}

fn dp_todatestring(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let s = fmtdate(local_time(t));
    st.push_string(&s)
}

fn dp_totimestring(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let s = fmttime(local_time(t), local_tza());
    st.push_string(&s)
}

fn dp_toutcstring(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let s = fmtdatetime(t, 0.0);
    st.push_string(&s)
}

fn dp_toisostring(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    if !t.is_finite() {
        return st.range_error("invalid date");
    }
    let s = fmtdatetime(t, 0.0);
    st.push_string(&s)
}

macro_rules! getter {
    ($name:ident, $conv:expr) => {
        fn $name(st: &mut State) -> R<()> {
            let t = js_todate(st, 0)?;
            if t.is_nan() {
                return st.push_number(f64::NAN);
            }
            let f: fn(f64) -> i32 = $conv;
            st.push_number(f(t) as f64)
        }
    };
}

macro_rules! getter_local {
    ($name:ident, $conv:expr) => {
        fn $name(st: &mut State) -> R<()> {
            let t = js_todate(st, 0)?;
            if t.is_nan() {
                return st.push_number(f64::NAN);
            }
            let f: fn(f64) -> i32 = $conv;
            st.push_number(f(local_time(t)) as f64)
        }
    };
}

getter_local!(dp_getfullyear, year_from_time);
getter_local!(dp_getmonth, month_from_time);
getter_local!(dp_getdate, date_from_time);
getter_local!(dp_getday, week_day);
getter_local!(dp_gethours, hour_from_time);
getter_local!(dp_getminutes, min_from_time);
getter_local!(dp_getseconds, sec_from_time);
getter_local!(dp_getmilliseconds, ms_from_time);
getter!(dp_getutcfullyear, year_from_time);
getter!(dp_getutcmonth, month_from_time);
getter!(dp_getutcdate, date_from_time);
getter!(dp_getutcday, week_day);
getter!(dp_getutchours, hour_from_time);
getter!(dp_getutcminutes, min_from_time);
getter!(dp_getutcseconds, sec_from_time);
getter!(dp_getutcmilliseconds, ms_from_time);

fn dp_gettimezoneoffset(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    if t.is_nan() {
        st.push_number(f64::NAN)
    } else {
        st.push_number((t - local_time(t)) / MS_PER_MINUTE)
    }
}

fn dp_settime(st: &mut State) -> R<()> {
    let v = st.tonumber(1)?;
    js_setdate(st, 0, v)
}

macro_rules! setter_ms {
    ($name:ident, $local:expr) => {
        fn $name(st: &mut State) -> R<()> {
            let t0 = js_todate(st, 0)?;
            let t = if $local { local_time(t0) } else { t0 };
            let h = hour_from_time(t) as f64;
            let m = min_from_time(t) as f64;
            let s = sec_from_time(t) as f64;
            let ms = st.tonumber(1)?;
            let v = make_date(day(t) as f64, make_time(h, m, s, ms));
            let v = if $local { utc(v) } else { v };
            js_setdate(st, 0, v)
        }
    };
}

setter_ms!(dp_setmilliseconds, true);
setter_ms!(dp_setutcmilliseconds, false);

fn dp_setseconds(st: &mut State) -> R<()> {
    let t0 = js_todate(st, 0)?;
    let t = local_time(t0);
    let h = hour_from_time(t) as f64;
    let m = min_from_time(t) as f64;
    let s = st.tonumber(1)?;
    let ms = optnumber(st, 2, ms_from_time(t) as f64)?;
    let v = utc(make_date(day(t) as f64, make_time(h, m, s, ms)));
    js_setdate(st, 0, v)
}

fn dp_setutcseconds(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let h = hour_from_time(t) as f64;
    let m = min_from_time(t) as f64;
    let s = st.tonumber(1)?;
    let ms = optnumber(st, 2, ms_from_time(t) as f64)?;
    let v = make_date(day(t) as f64, make_time(h, m, s, ms));
    js_setdate(st, 0, v)
}

fn dp_setminutes(st: &mut State) -> R<()> {
    let t0 = js_todate(st, 0)?;
    let t = local_time(t0);
    let h = hour_from_time(t) as f64;
    let m = st.tonumber(1)?;
    let s = optnumber(st, 2, sec_from_time(t) as f64)?;
    let ms = optnumber(st, 3, ms_from_time(t) as f64)?;
    let v = utc(make_date(day(t) as f64, make_time(h, m, s, ms)));
    js_setdate(st, 0, v)
}

fn dp_setutcminutes(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let h = hour_from_time(t) as f64;
    let m = st.tonumber(1)?;
    let s = optnumber(st, 2, sec_from_time(t) as f64)?;
    let ms = optnumber(st, 3, ms_from_time(t) as f64)?;
    let v = make_date(day(t) as f64, make_time(h, m, s, ms));
    js_setdate(st, 0, v)
}

fn dp_sethours(st: &mut State) -> R<()> {
    let t0 = js_todate(st, 0)?;
    let t = local_time(t0);
    let h = st.tonumber(1)?;
    let m = optnumber(st, 2, min_from_time(t) as f64)?;
    let s = optnumber(st, 3, sec_from_time(t) as f64)?;
    let ms = optnumber(st, 4, ms_from_time(t) as f64)?;
    let v = utc(make_date(day(t) as f64, make_time(h, m, s, ms)));
    js_setdate(st, 0, v)
}

fn dp_setutchours(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let h = st.tonumber(1)?;
    let m = optnumber(st, 2, min_from_time(t) as f64)?;
    let s = optnumber(st, 3, sec_from_time(t) as f64)?;
    let ms = optnumber(st, 4, ms_from_time(t) as f64)?;
    let v = make_date(day(t) as f64, make_time(h, m, s, ms));
    js_setdate(st, 0, v)
}

fn dp_setdate(st: &mut State) -> R<()> {
    let t0 = js_todate(st, 0)?;
    let t = local_time(t0);
    let y = year_from_time(t) as f64;
    let m = month_from_time(t) as f64;
    let d = st.tonumber(1)?;
    let v = utc(make_date(make_day(y, m, d), time_within_day(t)));
    js_setdate(st, 0, v)
}

fn dp_setutcdate(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let y = year_from_time(t) as f64;
    let m = month_from_time(t) as f64;
    let d = st.tonumber(1)?;
    let v = make_date(make_day(y, m, d), time_within_day(t));
    js_setdate(st, 0, v)
}

fn dp_setmonth(st: &mut State) -> R<()> {
    let t0 = js_todate(st, 0)?;
    let t = local_time(t0);
    let y = year_from_time(t) as f64;
    let m = st.tonumber(1)?;
    let d = optnumber(st, 2, date_from_time(t) as f64)?;
    let v = utc(make_date(make_day(y, m, d), time_within_day(t)));
    js_setdate(st, 0, v)
}

fn dp_setutcmonth(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let y = year_from_time(t) as f64;
    let m = st.tonumber(1)?;
    let d = optnumber(st, 2, date_from_time(t) as f64)?;
    let v = make_date(make_day(y, m, d), time_within_day(t));
    js_setdate(st, 0, v)
}

fn dp_setfullyear(st: &mut State) -> R<()> {
    let t0 = js_todate(st, 0)?;
    let t = local_time(t0);
    let y = st.tonumber(1)?;
    let m = optnumber(st, 2, month_from_time(t) as f64)?;
    let d = optnumber(st, 3, date_from_time(t) as f64)?;
    let v = utc(make_date(make_day(y, m, d), time_within_day(t)));
    js_setdate(st, 0, v)
}

fn dp_setutcfullyear(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    let y = st.tonumber(1)?;
    let m = optnumber(st, 2, month_from_time(t) as f64)?;
    let d = optnumber(st, 3, date_from_time(t) as f64)?;
    let v = make_date(make_day(y, m, d), time_within_day(t));
    js_setdate(st, 0, v)
}

// Deprecated methods (for compatibility)

/// Date.prototype.getYear() - Deprecated
/// Returns year - 1900 for years >= 1900, otherwise returns the actual year
fn dp_getyear(st: &mut State) -> R<()> {
    let t = js_todate(st, 0)?;
    if t.is_nan() {
        return st.push_number(f64::NAN);
    }
    let year = year_from_time(local_time(t));
    st.push_number((year - 1900) as f64)
}

/// Date.prototype.setYear(yearValue) - Deprecated
/// If yearValue is between 0-99, adds 1900; otherwise uses the value as-is
fn dp_setyear(st: &mut State) -> R<()> {
    let t0 = js_todate(st, 0)?;
    let t = if t0.is_nan() { 0.0 } else { local_time(t0) };
    let mut y = st.tonumber(1)?;

    // If year is NaN, return NaN
    if y.is_nan() {
        return js_setdate(st, 0, f64::NAN);
    }

    // Handle 2-digit years: 0-99 becomes 1900-1999
    if (0.0..=99.0).contains(&y) {
        y += 1900.0;
    }

    let m = month_from_time(t) as f64;
    let d = date_from_time(t) as f64;
    let v = utc(make_date(make_day(y, m, d), time_within_day(t)));
    js_setdate(st, 0, v)
}

/// Date.prototype.toGMTString() - Deprecated alias for toUTCString
fn dp_togmtstring(st: &mut State) -> R<()> {
    dp_toutcstring(st)
}

fn dp_tojson(st: &mut State) -> R<()> {
    st.copy(0)?;
    st.toprimitive(-1, crate::value::Hint::Number)?;
    if st.isnumber(-1) && !st.tonumber(-1)?.is_finite() {
        return st.push_null();
    }
    st.pop(1);

    st.getproperty(0, "toISOString")?;
    if !st.iscallable(-1) {
        return st.type_error("this.toISOString is not a function");
    }
    st.copy(0)?;
    st.call(0)
}

pub fn init(st: &mut State) {
    let proto = st.protos.date;
    st.push_object(proto).unwrap();
    {
        propf(st, "Date.prototype.valueOf", dp_valueof, 0).unwrap();
        propf(st, "Date.prototype.toString", dp_tostring, 0).unwrap();
        propf(st, "Date.prototype.toDateString", dp_todatestring, 0).unwrap();
        propf(st, "Date.prototype.toTimeString", dp_totimestring, 0).unwrap();
        propf(st, "Date.prototype.toLocaleString", dp_tostring, 0).unwrap();
        propf(st, "Date.prototype.toLocaleDateString", dp_todatestring, 0).unwrap();
        propf(st, "Date.prototype.toLocaleTimeString", dp_totimestring, 0).unwrap();
        propf(st, "Date.prototype.toUTCString", dp_toutcstring, 0).unwrap();

        propf(st, "Date.prototype.getTime", dp_valueof, 0).unwrap();
        propf(st, "Date.prototype.getFullYear", dp_getfullyear, 0).unwrap();
        propf(st, "Date.prototype.getUTCFullYear", dp_getutcfullyear, 0).unwrap();
        propf(st, "Date.prototype.getMonth", dp_getmonth, 0).unwrap();
        propf(st, "Date.prototype.getUTCMonth", dp_getutcmonth, 0).unwrap();
        propf(st, "Date.prototype.getDate", dp_getdate, 0).unwrap();
        propf(st, "Date.prototype.getUTCDate", dp_getutcdate, 0).unwrap();
        propf(st, "Date.prototype.getDay", dp_getday, 0).unwrap();
        propf(st, "Date.prototype.getUTCDay", dp_getutcday, 0).unwrap();
        propf(st, "Date.prototype.getHours", dp_gethours, 0).unwrap();
        propf(st, "Date.prototype.getUTCHours", dp_getutchours, 0).unwrap();
        propf(st, "Date.prototype.getMinutes", dp_getminutes, 0).unwrap();
        propf(st, "Date.prototype.getUTCMinutes", dp_getutcminutes, 0).unwrap();
        propf(st, "Date.prototype.getSeconds", dp_getseconds, 0).unwrap();
        propf(st, "Date.prototype.getUTCSeconds", dp_getutcseconds, 0).unwrap();
        propf(st, "Date.prototype.getMilliseconds", dp_getmilliseconds, 0).unwrap();
        propf(st, "Date.prototype.getUTCMilliseconds", dp_getutcmilliseconds, 0).unwrap();
        propf(st, "Date.prototype.getTimezoneOffset", dp_gettimezoneoffset, 0).unwrap();

        propf(st, "Date.prototype.setTime", dp_settime, 1).unwrap();
        propf(st, "Date.prototype.setMilliseconds", dp_setmilliseconds, 1).unwrap();
        propf(st, "Date.prototype.setUTCMilliseconds", dp_setutcmilliseconds, 1).unwrap();
        propf(st, "Date.prototype.setSeconds", dp_setseconds, 2).unwrap();
        propf(st, "Date.prototype.setUTCSeconds", dp_setutcseconds, 2).unwrap();
        propf(st, "Date.prototype.setMinutes", dp_setminutes, 3).unwrap();
        propf(st, "Date.prototype.setUTCMinutes", dp_setutcminutes, 3).unwrap();
        propf(st, "Date.prototype.setHours", dp_sethours, 4).unwrap();
        propf(st, "Date.prototype.setUTCHours", dp_setutchours, 4).unwrap();
        propf(st, "Date.prototype.setDate", dp_setdate, 1).unwrap();
        propf(st, "Date.prototype.setUTCDate", dp_setutcdate, 1).unwrap();
        propf(st, "Date.prototype.setMonth", dp_setmonth, 2).unwrap();
        propf(st, "Date.prototype.setUTCMonth", dp_setutcmonth, 2).unwrap();
        propf(st, "Date.prototype.setFullYear", dp_setfullyear, 3).unwrap();
        propf(st, "Date.prototype.setUTCFullYear", dp_setutcfullyear, 3).unwrap();

        // Deprecated methods (for compatibility)
        propf(st, "Date.prototype.getYear", dp_getyear, 0).unwrap();
        propf(st, "Date.prototype.setYear", dp_setyear, 1).unwrap();
        propf(st, "Date.prototype.toGMTString", dp_togmtstring, 0).unwrap();

        // ES5
        propf(st, "Date.prototype.toISOString", dp_toisostring, 0).unwrap();
        propf(st, "Date.prototype.toJSON", dp_tojson, 1).unwrap();
    }
    st.newcconstructor(jsb_date, jsb_new_date, "Date", 0).unwrap();
    {
        propf(st, "Date.parse", d_parse, 1).unwrap();
        propf(st, "Date.UTC", d_utc, 7).unwrap();

        // ES5
        propf(st, "Date.now", d_now, 0).unwrap();
    }
    st.defglobal("Date", JS_DONTENUM).unwrap();
}
