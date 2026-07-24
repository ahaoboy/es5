//! Number constructor and Number.prototype (jsnumber.c).

use super::{propf, propn};
use crate::number as num;
use crate::object::{Class, Payload};
use crate::state::{State, R};
use crate::value::JS_DONTENUM;

fn jsb_new_number(st: &mut State) -> R<()> {
    let v = if st.gettop() > 1 { st.tonumber(1)? } else { 0.0 };
    st.newnumber(v)
}

fn jsb_number(st: &mut State) -> R<()> {
    let v = if st.gettop() > 1 { st.tonumber(1)? } else { 0.0 };
    st.push_number(v)
}

fn self_number(st: &mut State) -> R<f64> {
    let obj = st.toobject(0)?;
    match &st.heap.obj(obj).payload {
        Payload::Number(n) if st.heap.obj(obj).class == Class::Number => Ok(*n),
        _ => st.type_error("not a number"),
    }
}

fn np_valueof(st: &mut State) -> R<()> {
    let x = self_number(st)?;
    st.push_number(x)
}

fn np_tostring(st: &mut State) -> R<()> {
    let radix = if st.isundefined(1) { 10 } else { st.tointeger(1)? };
    let x = self_number(st)?;

    if radix == 10 {
        let s = num::number_to_string(x);
        return st.push_string(&s);
    }
    if !(2..=36).contains(&radix) {
        return st.range_error("invalid radix");
    }

    // lame number to string conversion for any radix from 2 to 36
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let radix = radix as u32;
    let sign = x < 0.0;
    let mut number = x;

    if number == 0.0 {
        return st.push_string("0");
    }
    if number.is_nan() {
        return st.push_string("NaN");
    }
    if number.is_infinite() {
        return st.push_string(if sign { "-Infinity" } else { "Infinity" });
    }

    if sign {
        number = -number;
    }

    let limit = (1u64 << 52) as f64;
    let rf = radix as f64;

    // fit as many digits as we want in an int
    let mut exp = 0i32;
    while number * rf.powi(exp) > limit {
        exp -= 1;
    }
    while number * rf.powi(exp + 1) < limit {
        exp += 1;
    }
    let mut u = (number * rf.powi(exp) + 0.5) as u64;

    // trim trailing zeros
    while u > 0 && u.is_multiple_of(radix as u64) {
        u /= radix as u64;
        exp -= 1;
    }

    // serialize digits
    let mut buf = Vec::new();
    while u > 0 {
        buf.push(DIGITS[(u % radix as u64) as usize]);
        u /= radix as u64;
    }
    let ndigits = buf.len() as i32;
    let mut point = ndigits - exp;

    let mut out = String::new();
    if sign {
        out.push('-');
    }

    if point <= 0 {
        out.push('0');
        out.push('.');
        while point < 0 {
            out.push('0');
            point += 1;
        }
        for d in buf.iter().rev() {
            out.push(*d as char);
        }
    } else {
        let mut i = ndigits;
        while i > 0 {
            i -= 1;
            out.push(buf[i as usize] as char);
            point -= 1;
            if point == 0 && i > 0 {
                out.push('.');
            }
        }
        while point > 0 {
            out.push('0');
            point -= 1;
        }
    }

    st.push_string(&out)
}

/// Customized ToString() on a number with printf-style precision.
fn numtostr(st: &mut State, spec: NumFmt, w: i32, n: f64) -> R<()> {
    let s = match spec {
        NumFmt::Fixed => format!("{:.*}", w as usize, n),
        NumFmt::Exponential => format_scientific(w as usize, n),
        NumFmt::Precision => format_precision(w as usize, n),
    };
    st.push_string(&s)
}

enum NumFmt {
    Fixed,
    Exponential,
    Precision,
}

/// Emulates sprintf("%.*e", w, n) then rewrites the exponent as e%+d.
fn format_scientific(w: usize, n: f64) -> String {
    // Rust's {:.*e} yields e.g. "1.23e4"; sprintf gives "1.23e+04".
    let s = format!("{:.*e}", w, n);
    let (mant, exp) = s.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    format!("{}e{}{}", mant, if exp < 0 { "-" } else { "+" }, exp.abs())
}

/// Emulates sprintf("%.*g", w, n).
fn format_precision(w: usize, n: f64) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    let exp = n.abs().log10().floor() as i32;
    if exp < -4 || exp >= w as i32 {
        // scientific, with w-1 digits after the point, trailing zeros removed
        let s = format!("{:.*e}", w.saturating_sub(1), n);
        let (mant, e) = s.split_once('e').unwrap();
        let expv: i32 = e.parse().unwrap();
        let mant = trim_trailing_zeros(mant);
        format!("{}e{}{}", mant, if expv < 0 { "-" } else { "+" }, expv.abs())
    } else {
        let prec = (w as i32 - exp - 1).max(0) as usize;
        let s = format!("{:.*}", prec, n);
        trim_trailing_zeros(&s).to_string()
    }
}

fn trim_trailing_zeros(s: &str) -> &str {
    if s.contains('.') {
        let t = s.trim_end_matches('0');
        t.trim_end_matches('.')
    } else {
        s
    }
}

fn np_tofixed(st: &mut State) -> R<()> {
    let width = st.tointeger(1)?;
    let x = self_number(st)?;
    if width < 0 {
        return st.range_error(&format!("precision {} out of range", width));
    }
    if width > 20 {
        return st.range_error(&format!("precision {} out of range", width));
    }
    if x.is_nan() || x.is_infinite() || x <= -1e21 || x >= 1e21 {
        let s = num::number_to_string(x);
        st.push_string(&s)
    } else {
        numtostr(st, NumFmt::Fixed, width, x)
    }
}

fn np_toexponential(st: &mut State) -> R<()> {
    let width = st.tointeger(1)?;
    let x = self_number(st)?;
    if width < 0 {
        return st.range_error(&format!("precision {} out of range", width));
    }
    if width > 20 {
        return st.range_error(&format!("precision {} out of range", width));
    }
    if x.is_nan() || x.is_infinite() {
        let s = num::number_to_string(x);
        st.push_string(&s)
    } else {
        numtostr(st, NumFmt::Exponential, width, x)
    }
}

fn np_toprecision(st: &mut State) -> R<()> {
    let width = st.tointeger(1)?;
    let x = self_number(st)?;
    if width < 1 {
        return st.range_error(&format!("precision {} out of range", width));
    }
    if width > 21 {
        return st.range_error(&format!("precision {} out of range", width));
    }
    if x.is_nan() || x.is_infinite() {
        let s = num::number_to_string(x);
        st.push_string(&s)
    } else {
        numtostr(st, NumFmt::Precision, width, x)
    }
}

pub fn init(st: &mut State) {
    let proto = st.protos.number;
    st.push_object(proto).unwrap();
    {
        propf(st, "Number.prototype.valueOf", np_valueof, 0).unwrap();
        propf(st, "Number.prototype.toString", np_tostring, 1).unwrap();
        propf(st, "Number.prototype.toLocaleString", np_tostring, 0).unwrap();
        propf(st, "Number.prototype.toFixed", np_tofixed, 1).unwrap();
        propf(st, "Number.prototype.toExponential", np_toexponential, 1).unwrap();
        propf(st, "Number.prototype.toPrecision", np_toprecision, 1).unwrap();
    }
    st.newcconstructor(jsb_number, jsb_new_number, "Number", 1).unwrap();
    {
        propn(st, "MAX_VALUE", 1.7976931348623157e+308).unwrap();
        propn(st, "MIN_VALUE", 5e-324).unwrap();
        propn(st, "NaN", f64::NAN).unwrap();
        propn(st, "NEGATIVE_INFINITY", f64::NEG_INFINITY).unwrap();
        propn(st, "POSITIVE_INFINITY", f64::INFINITY).unwrap();

        // ES6: Number.parseInt / Number.parseFloat (same as global)
        propf(st, "Number.parseInt", crate::builtins::jsb_parse_int, 2).unwrap();
        propf(st, "Number.parseFloat", crate::builtins::jsb_parse_float, 1).unwrap();
    }
    st.defglobal("Number", JS_DONTENUM).unwrap();
}
