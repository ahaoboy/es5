//! Locale-independent string <-> double conversions.
//!
//! This module replaces jsdtoa.c (grisu2, js_strtod, js_fmtexp) with the
//! correctly-rounded parsers and shortest-round-trip formatters provided by
//! the Rust standard library. Only the ECMAScript-specific formatting rules
//! (jsV_numbertostring) and the digit-table strtol are implemented by hand.

/// A tiny stack buffer implementing `fmt::Write`, so `write!` can format
/// into fixed storage without allocating.
struct FixBuf {
    buf: [u8; 40],
    len: usize,
}

impl FixBuf {
    fn new() -> FixBuf {
        FixBuf { buf: [0; 40], len: 0 }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl std::fmt::Write for FixBuf {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let b = s.as_bytes();
        if self.len + b.len() > self.buf.len() {
            return Err(std::fmt::Error);
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
        Ok(())
    }
}

/// Append an exponent like `sprintf(p, "e%+d", e)` (js_fmtexp). No allocation.
fn push_fmtexp(out: &mut String, e: i32) {
    out.push('e');
    if e < 0 {
        out.push('-');
        push_u32(out, e.unsigned_abs());
    } else {
        out.push('+');
        push_u32(out, e as u32);
    }
}

/// Append the decimal digits of a u32 without allocating.
fn push_u32(out: &mut String, mut v: u32) {
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    // all bytes written are ASCII digits
    out.push_str(std::str::from_utf8(&buf[i..]).unwrap());
}

/// ToString() on a number, following the ECMA-262 rules as implemented by
/// jsV_numbertostring in jsvalue.c. The shortest round-trip digits are
/// obtained from Rust's `{:e}` formatter instead of grisu2.
pub fn number_to_string(f: f64) -> String {
    let mut out = String::with_capacity(24);
    write_number(&mut out, f);
    out
}

/// Append ToString(number) to `out` (allocation-free apart from growing
/// `out` itself).
fn write_number(out: &mut String, f: f64) {
    if f == 0.0 {
        out.push('0');
        return;
    }
    if f.is_nan() {
        out.push_str("NaN");
        return;
    }
    if f.is_infinite() {
        out.push_str(if f < 0.0 { "-Infinity" } else { "Infinity" });
        return;
    }

    // Fast case for 32-bit integers exactly representable as doubles.
    if (-2147483648.0..=2147483647.0).contains(&f) {
        let i = f as i32;
        if i as f64 == f {
            if i < 0 {
                out.push('-');
                push_u32(out, (i as i64).unsigned_abs() as u32);
            } else {
                push_u32(out, i as u32);
            }
            return;
        }
    }

    // Obtain the shortest digits and decimal exponent from Rust's
    // LowerExp formatter, e.g. "1.2345e3" -> digits="12345", point=4.
    // A fixed 32-byte buffer holds any f64 in scientific notation
    // ("-d.ddddddddddddddddde-308" is well under 32 bytes), so this needs
    // no heap allocation at all.
    let mut fb = FixBuf::new();
    {
        use std::fmt::Write as _;
        write!(&mut fb, "{:e}", f.abs()).expect("buffer is large enough");
    }
    let sci = fb.as_bytes();
    let epos = sci.iter().position(|&c| c == b'e').expect("LowerExp has 'e'");
    let exp10: i32 = std::str::from_utf8(&sci[epos + 1..])
        .ok()
        .and_then(|s| s.parse().ok())
        .expect("valid exponent");
    // collect the significand digits, skipping the '.'
    let mut digits = [0u8; 24];
    let mut ndigits = 0usize;
    for &c in &sci[..epos] {
        if c != b'.' {
            digits[ndigits] = c;
            ndigits += 1;
        }
    }
    // value = 0.d1d2...dn * 10^point (grisu2: point = ndigits + K)
    let point = exp10 + 1;

    if f.is_sign_negative() {
        out.push('-');
    }

    if !(-5..=21).contains(&point) {
        // Scientific notation: d.ddde+-X
        out.push(digits[0] as char);
        if ndigits > 1 {
            out.push('.');
            for &d in &digits[1..ndigits] {
                out.push(d as char);
            }
        }
        push_fmtexp(out, point - 1);
    } else if point <= 0 {
        // Small fraction: 0.000ddd
        out.push_str("0.");
        for _ in 0..-point {
            out.push('0');
        }
        for &d in &digits[..ndigits] {
            out.push(d as char);
        }
    } else {
        // Plain decimal with the point inside or after the digits.
        let mut point = point;
        let mut i = 0;
        let mut nd = ndigits as i32;
        while nd > 0 {
            out.push(digits[i] as char);
            i += 1;
            nd -= 1;
            point -= 1;
            if point == 0 && nd > 0 {
                out.push('.');
            }
        }
        while point > 0 {
            out.push('0');
            point -= 1;
        }
    }
}

/// Parse as many decimal digits (in the given radix) as possible,
/// accumulating in a double exactly like js_strtol in jsvalue.c.
/// Returns (value, bytes consumed).
pub fn strtol(s: &str, base: u32) -> (f64, usize) {
    fn digit_value(c: u8) -> u32 {
        match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 10,
            b'A'..=b'Z' => (c - b'A') as u32 + 10,
            _ => 80,
        }
    }
    let bytes = s.as_bytes();
    let mut x = 0.0f64;
    let mut i = 0;
    while i < bytes.len() {
        let d = digit_value(bytes[i]);
        if d >= base {
            break;
        }
        x = x * base as f64 + d as f64;
        i += 1;
    }
    (x, i)
}

/// Scan a floating point literal with the exact grammar used by
/// js_stringtofloat: [+-]? digits ('.' digits?)? ([eE][+-]?digits)?
/// Returns (value, bytes consumed) or (0.0, 0) on failure.
pub fn string_to_float(s: &str) -> (f64, usize) {
    let b = s.as_bytes();
    let mut e = 0;
    if e < b.len() && (b[e] == b'+' || b[e] == b'-') {
        e += 1;
    }
    while e < b.len() && b[e].is_ascii_digit() {
        e += 1;
    }
    if e < b.len() && b[e] == b'.' {
        e += 1;
    }
    while e < b.len() && b[e].is_ascii_digit() {
        e += 1;
    }
    if e < b.len() && (b[e] == b'e' || b[e] == b'E') {
        e += 1;
        if e < b.len() && (b[e] == b'+' || b[e] == b'-') {
            e += 1;
        }
        while e < b.len() && b[e].is_ascii_digit() {
            e += 1;
        }
    }
    if e == 0 || (e == 1 && (b[0] == b'+' || b[0] == b'-')) {
        return (0.0, 0);
    }
    let text = &s[..e];
    // The scanner validated the syntax, so Rust's parser must accept it.
    match text.parse::<f64>() {
        Ok(v) => (v, e),
        Err(_) => (0.0, 0),
    }
}

/// Portable strtod replacement: parse the longest valid double prefix.
/// Used by the lexer for number literals; the caller has already validated
/// the syntax, so this is just a thin wrapper over the std parser.
pub fn strtod(s: &str) -> f64 {
    let (v, n) = string_to_float(s);
    let _ = n;
    v
}

/// ToNumber() on a string (jsV_stringtonumber).
pub fn string_to_number(s: &str) -> f64 {
    let s = s.trim_start_matches(crate::utf::is_js_white_or_newline);
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))
        && !rest.is_empty() {
            let (v, n) = strtol(rest, 16);
            let tail = &rest[n..];
            if tail.trim_start_matches(crate::utf::is_js_white_or_newline).is_empty() {
                return v;
            }
            return f64::NAN;
        }
    let (n, len) = if let Some(_rest) = s.strip_prefix("Infinity") {
        (f64::INFINITY, 8)
    } else if let Some(_rest) = s.strip_prefix("+Infinity") {
        (f64::INFINITY, 9)
    } else if let Some(_rest) = s.strip_prefix("-Infinity") {
        (f64::NEG_INFINITY, 9)
    } else {
        let (v, n) = string_to_float(s);
        (v, n)
    };
    let tail = &s[len..];
    if !tail.trim_start_matches(crate::utf::is_js_white_or_newline).is_empty() {
        return f64::NAN;
    }
    n
}

/// C fmod() for doubles (Rust's % operator on f64 has the same semantics).
#[inline]
pub fn fmod(x: f64, y: f64) -> f64 {
    x % y
}

/// ToInteger() on a number: truncate toward zero, clamp to i32 range.
pub fn number_to_integer(n: f64) -> i32 {
    if n == 0.0 || n.is_nan() {
        return 0;
    }
    let n = n.trunc();
    if n < i32::MIN as f64 {
        return i32::MIN;
    }
    if n > i32::MAX as f64 {
        return i32::MAX;
    }
    n as i32
}

/// ECMAScript ToInt32.
pub fn number_to_int32(n: f64) -> i32 {
    const TWO32: f64 = 4294967296.0;
    const TWO31: f64 = 2147483648.0;
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    let n = n % TWO32;
    let n = if n >= 0.0 { n.floor() } else { n.ceil() + TWO32 };
    if n >= TWO31 {
        (n - TWO32) as i64 as i32
    } else {
        n as i64 as i32
    }
}

/// ECMAScript ToUint32.
pub fn number_to_uint32(n: f64) -> u32 {
    number_to_int32(n) as u32
}

/// ECMAScript ToInt16.
pub fn number_to_int16(n: f64) -> i16 {
    number_to_int32(n) as i16
}

/// ECMAScript ToUint16.
pub fn number_to_uint16(n: f64) -> u16 {
    number_to_int32(n) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_number_to_string() {
        assert_eq!(number_to_string(0.0), "0");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(1.0), "1");
        assert_eq!(number_to_string(-1.5), "-1.5");
        assert_eq!(number_to_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1e-7), "1e-7");
        assert_eq!(number_to_string(1e-6), "0.000001");
        assert_eq!(number_to_string(123456789012345680000.0), "123456789012345680000");
        assert_eq!(number_to_string(f64::NAN), "NaN");
        assert_eq!(number_to_string(f64::INFINITY), "Infinity");
        assert_eq!(number_to_string(5e-324), "5e-324");
        assert_eq!(number_to_string(1.7976931348623157e308), "1.7976931348623157e+308");
    }

    #[test]
    fn test_string_to_number() {
        assert_eq!(string_to_number("42"), 42.0);
        assert_eq!(string_to_number("  42  "), 42.0);
        assert_eq!(string_to_number("0x1f"), 31.0);
        assert_eq!(string_to_number("Infinity"), f64::INFINITY);
        assert!(string_to_number("abc").is_nan());
        assert_eq!(string_to_number(""), 0.0); // mujs maps "" to 0
        assert_eq!(string_to_number(" \t\n"), 0.0);
        assert_eq!(string_to_number("5."), 5.0);
        assert_eq!(string_to_number(".5e2"), 50.0);
    }
}
