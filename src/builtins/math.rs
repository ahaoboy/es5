//! The Math object (jsmath.c).

use super::{propf, propn};
use crate::object::Class;
use crate::state::{State, R};
use crate::value::JS_DONTENUM;

/// mujs's rounding: round half towards +infinity.
fn js_round(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return x;
    }
    if x > 0.0 && x < 0.5 {
        return 0.0;
    }
    if (-0.5..0.0).contains(&x) {
        return -0.0;
    }
    (x + 0.5).floor()
}

fn math_abs(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.abs())
}

fn math_acos(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.acos())
}

fn math_asin(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.asin())
}

fn math_atan(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.atan())
}

fn math_atan2(st: &mut State) -> R<()> {
    let y = st.tonumber(1)?;
    let x = st.tonumber(2)?;
    st.push_number(y.atan2(x))
}

fn math_ceil(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.ceil())
}

fn math_cos(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.cos())
}

fn math_exp(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.exp())
}

fn math_floor(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.floor())
}

fn math_log(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.ln())
}

fn math_pow(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    let y = st.tonumber(2)?;
    if !y.is_finite() && x.abs() == 1.0 {
        st.push_number(f64::NAN)
    } else {
        st.push_number(x.powf(y))
    }
}

fn math_random(st: &mut State) -> R<()> {
    // Lehmer generator with a=48271 and m=2^31-1 (Park & Miller)
    st.seed = ((st.seed as u64 * 48271) % 0x7fffffff) as u32;
    st.push_number(st.seed as f64 / 0x7fffffff as f64)
}

fn init_random(st: &mut State) {
    // Pick initial seed by scrambling current time with Xorshift
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut seed = (t as u32).wrapping_add(123);
    seed ^= seed << 13;
    seed ^= seed >> 17;
    seed ^= seed << 5;
    st.seed = seed % 0x7fffffff;
}

fn math_round(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(js_round(x))
}

fn math_sin(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.sin())
}

fn math_sqrt(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.sqrt())
}

fn math_tan(st: &mut State) -> R<()> {
    let x = st.tonumber(1)?;
    st.push_number(x.tan())
}

fn math_max(st: &mut State) -> R<()> {
    let n = st.gettop();
    let mut x = f64::NEG_INFINITY;
    for i in 1..n {
        let y = st.tonumber(i)?;
        if y.is_nan() {
            x = y;
            break;
        }
        if x.is_sign_negative() == y.is_sign_negative() {
            x = if x > y { x } else { y };
        } else if x.is_sign_negative() {
            x = y;
        }
    }
    st.push_number(x)
}

fn math_min(st: &mut State) -> R<()> {
    let n = st.gettop();
    let mut x = f64::INFINITY;
    for i in 1..n {
        let y = st.tonumber(i)?;
        if y.is_nan() {
            x = y;
            break;
        }
        if x.is_sign_negative() == y.is_sign_negative() {
            x = if x < y { x } else { y };
        } else if y.is_sign_negative() {
            x = y;
        }
    }
    st.push_number(x)
}

pub fn init(st: &mut State) {
    init_random(st);
    let proto = st.protos.object;
    let m = st.heap.alloc_object(Class::Math, Some(proto));
    st.push_object(m).unwrap();
    {
        propn(st, "E", std::f64::consts::E).unwrap();
        propn(st, "LN10", std::f64::consts::LN_10).unwrap();
        propn(st, "LN2", std::f64::consts::LN_2).unwrap();
        propn(st, "LOG2E", std::f64::consts::LOG2_E).unwrap();
        propn(st, "LOG10E", std::f64::consts::LOG10_E).unwrap();
        propn(st, "PI", std::f64::consts::PI).unwrap();
        propn(st, "SQRT1_2", std::f64::consts::FRAC_1_SQRT_2).unwrap();
        propn(st, "SQRT2", std::f64::consts::SQRT_2).unwrap();

        propf(st, "Math.abs", math_abs, 1).unwrap();
        propf(st, "Math.acos", math_acos, 1).unwrap();
        propf(st, "Math.asin", math_asin, 1).unwrap();
        propf(st, "Math.atan", math_atan, 1).unwrap();
        propf(st, "Math.atan2", math_atan2, 2).unwrap();
        propf(st, "Math.ceil", math_ceil, 1).unwrap();
        propf(st, "Math.cos", math_cos, 1).unwrap();
        propf(st, "Math.exp", math_exp, 1).unwrap();
        propf(st, "Math.floor", math_floor, 1).unwrap();
        propf(st, "Math.log", math_log, 1).unwrap();
        propf(st, "Math.max", math_max, 2).unwrap();
        propf(st, "Math.min", math_min, 2).unwrap();
        propf(st, "Math.pow", math_pow, 2).unwrap();
        propf(st, "Math.random", math_random, 0).unwrap();
        propf(st, "Math.round", math_round, 1).unwrap();
        propf(st, "Math.sin", math_sin, 1).unwrap();
        propf(st, "Math.sqrt", math_sqrt, 1).unwrap();
        propf(st, "Math.tan", math_tan, 1).unwrap();
    }
    st.defglobal("Math", JS_DONTENUM).unwrap();
}
