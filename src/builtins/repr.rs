//! The repr() debugging helper (jsrepr.c).

use crate::number;
use crate::object::{Class, Payload};
use crate::state::{State, R};

fn reprnum(out: &mut String, n: f64) {
    if n == 0.0 && n.is_sign_negative() {
        out.push_str("-0");
    } else {
        out.push_str(&number::number_to_string(n));
    }
}

fn reprstr(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => {
                let v = c as u32;
                if v < 32 {
                    out.push('\\');
                    out.push('x');
                    out.push(HEX[(v >> 4) as usize & 15] as char);
                    out.push(HEX[v as usize & 15] as char);
                } else if v < 128 {
                    out.push(c);
                } else if v < 0x10000 {
                    out.push('\\');
                    out.push('u');
                    out.push(HEX[(v >> 12) as usize & 15] as char);
                    out.push(HEX[(v >> 8) as usize & 15] as char);
                    out.push(HEX[(v >> 4) as usize & 15] as char);
                    out.push(HEX[v as usize & 15] as char);
                } else {
                    out.push(c);
                }
            }
        }
    }
    out.push('"');
}

fn reprident(st: &mut State, out: &mut String, name: &str) -> R<()> {
    let b = name.as_bytes();
    let mut p = 0;
    if !b.is_empty() && b[0].is_ascii_digit() {
        while p < b.len() && b[p].is_ascii_digit() {
            p += 1;
        }
    } else if !b.is_empty() && (b[0].is_ascii_alphabetic() || b[0] == b'_') {
        while p < b.len() && (b[p].is_ascii_alphanumeric() || b[p] == b'_') {
            p += 1;
        }
    }
    if p > 0 && p == b.len() {
        out.push_str(name);
    } else {
        reprstr(out, name);
    }
    let _ = st;
    Ok(())
}

fn reprobject(st: &mut State, out: &mut String) -> R<()> {
    // cycle detection
    let n = st.gettop() - 1;
    for i in 0..n {
        if st.isobject(i)
            && st.toobject(i)? == st.toobject(-1)? {
                out.push_str("{}");
                return Ok(());
            }
    }

    let mut n = 0;
    out.push('{');
    st.pushiterator(-1, true)?;
    loop {
        let key = st.nextiterator(-1)?;
        let key = match key {
            Some(k) => k,
            None => break,
        };
        if n > 0 {
            out.push_str(", ");
        }
        n += 1;
        reprident(st, out, &key)?;
        out.push_str(": ");
        st.getproperty(-2, &key)?;
        reprvalue(st, out)?;
        st.pop(1);
    }
    st.pop(1);
    out.push('}');
    Ok(())
}

fn reprarray(st: &mut State, out: &mut String) -> R<()> {
    let n = st.gettop() - 1;
    for i in 0..n {
        if st.isobject(i)
            && st.toobject(i)? == st.toobject(-1)? {
                out.push_str("[]");
                return Ok(());
            }
    }

    out.push('[');
    let n = st.getlength(-1)?;
    for i in 0..n {
        if i > 0 {
            out.push_str(", ");
        }
        if st.hasindex(-1, i)? {
            reprvalue(st, out)?;
            st.pop(1);
        }
    }
    out.push(']');
    Ok(())
}

fn reprfun(st: &State, out: &mut String, fun: u32) {
    let f = st.heap.fun(fun);
    out.push_str("function ");
    out.push_str(&f.name);
    out.push('(');
    for i in 0..f.numparams {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&f.vartab[i]);
    }
    out.push_str(") { [byte code] }");
}

fn reprvalue(st: &mut State, out: &mut String) -> R<()> {
    if st.isundefined(-1) {
        out.push_str("undefined");
        return Ok(());
    }
    if st.isnull(-1) {
        out.push_str("null");
        return Ok(());
    }
    if st.isboolean(-1) {
        out.push_str(if st.toboolean(-1) { "true" } else { "false" });
        return Ok(());
    }
    if st.isnumber(-1) {
        let n = st.tonumber(-1)?;
        reprnum(out, n);
        return Ok(());
    }
    if st.isstring(-1) {
        let s = st.tostring(-1)?;
        reprstr(out, &s);
        return Ok(());
    }
    if st.isobject(-1) {
        let obj = st.toobject(-1)?;
        match st.heap.obj(obj).class {
            Class::Array => reprarray(st, out)?,
            Class::Function | Class::Script => {
                let fun = match &st.heap.obj(obj).payload {
                    Payload::Function(fd) => fd.fun,
                    _ => unreachable!(),
                };
                reprfun(st, out, fun);
            }
            Class::CFunction => {
                let name = match &st.heap.obj(obj).payload {
                    Payload::CFunction(cd) => cd.name.clone(),
                    _ => unreachable!(),
                };
                out.push_str("function ");
                out.push_str(&name);
                out.push_str("() { [native code] }");
            }
            Class::Boolean => {
                let b = match &st.heap.obj(obj).payload {
                    Payload::Boolean(b) => *b,
                    _ => false,
                };
                out.push_str("(new Boolean(");
                out.push_str(if b { "true" } else { "false" });
                out.push_str("))");
            }
            Class::Number => {
                let n = match &st.heap.obj(obj).payload {
                    Payload::Number(n) => *n,
                    _ => 0.0,
                };
                out.push_str("(new Number(");
                reprnum(out, n);
                out.push_str("))");
            }
            Class::String => {
                let s = match &st.heap.obj(obj).payload {
                    Payload::String(s) => s.string.clone(),
                    _ => st.heap.intern(""),
                };
                out.push_str("(new String(");
                reprstr(out, &s);
                out.push_str("))");
            }
            Class::Regexp => {
                let (source, flags) = match &st.heap.obj(obj).payload {
                    Payload::Regexp(r) => (r.source.clone(), r.flags),
                    _ => unreachable!(),
                };
                out.push('/');
                out.push_str(&source);
                out.push('/');
                if flags & crate::value::JS_REGEXP_G != 0 {
                    out.push('g');
                }
                if flags & crate::value::JS_REGEXP_I != 0 {
                    out.push('i');
                }
                if flags & crate::value::JS_REGEXP_M != 0 {
                    out.push('m');
                }
            }
            Class::Date => {
                let n = match &st.heap.obj(obj).payload {
                    Payload::Number(n) => *n,
                    _ => 0.0,
                };
                out.push_str("(new Date(");
                out.push_str(&number::number_to_string(n));
                out.push_str("))");
            }
            Class::Error => {
                out.push_str("(new ");
                st.getproperty(-1, "name")?;
                let name = st.tostring(-1)?;
                out.push_str(&name);
                st.pop(1);
                out.push('(');
                if st.hasproperty(-1, "message")? {
                    reprvalue(st, out)?;
                    st.pop(1);
                }
                out.push_str("))");
            }
            Class::Math => out.push_str("Math"),
            Class::Json => out.push_str("JSON"),
            Class::Iterator => out.push_str("[iterator "),
            _ => reprobject(st, out)?,
        }
    }
    Ok(())
}

/// js_repr: push the repr string of the value at idx.
pub fn js_repr(st: &mut State, idx: i32) -> R<()> {
    st.copy(idx)?;

    let savebot = st.bot;
    st.bot = st.top - 1;
    let mut out = String::new();
    let r = reprvalue(st, &mut out);
    st.bot = savebot;

    st.pop(1);
    r?;

    st.push_string(&out)
}

/// js_torepr: replace the value at idx with its repr string.
pub fn js_torepr(st: &mut State, idx: i32) -> R<compact_str::CompactString> {
    js_repr(st, idx)?;
    st.replace(if idx < 0 { idx - 1 } else { idx })?;
    st.tostring(idx)
}

/// js_tryrepr
pub fn js_tryrepr(st: &mut State, idx: i32, error: &str) -> compact_str::CompactString {
    match st.protect_result(|j| js_torepr(j, idx)) {
        Ok(s) => s,
        Err(_) => {
            st.pop(1);
            st.heap.intern(error)
        }
    }
}
