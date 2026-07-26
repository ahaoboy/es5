//! RegExp constructor and RegExp.prototype (jsregexp.c).
//!
//! The engine itself is provided by the `regress` crate (see regexp.rs in
//! the crate root), which replaces the hand-written engine in regexp.c.

use super::propf;
use crate::object::{Class, ObjRef, Payload, RegexpData};
use crate::regexp::Regexp;
use crate::state::{State, R};
use crate::utf;
use crate::value::{JS_DONTENUM, JS_REGEXP_G, JS_REGEXP_I, JS_REGEXP_M};
use compact_str::CompactString;

/// Escape slashes in a pattern for the source property.
fn escape_regexp(st: &mut State, pattern: &str) -> CompactString {
    if !pattern.contains('/') {
        return st.heap.intern(pattern);
    }
    let mut out = String::with_capacity(pattern.len() + 4);
    for c in pattern.chars() {
        if c == '/' {
            out.push('\\');
        }
        out.push(c);
    }
    st.heap.intern(&out)
}

/// js_newregexpx
pub fn new_regexp_x(st: &mut State, pattern: &str, flags: u32, is_clone: bool) -> R<()> {
    let prog = match Regexp::compile(pattern, flags) {
        Ok(p) => p,
        Err(e) => return st.syntax_error(&format!("regular expression: {}", e)),
    };
    let source = if is_clone {
        st.heap.intern(pattern)
    } else {
        escape_regexp(st, pattern)
    };
    let obj = st.heap.alloc_object(Class::Regexp, Some(st.protos.regexp));
    st.heap.obj_mut(obj).payload = Payload::Regexp(RegexpData {
        prog,
        source,
        flags,
        last: 0,
    });
    st.push_object(obj)
}

/// js_newregexp (used by OP_NEWREGEXP and String methods)
pub fn new_regexp(st: &mut State, pattern: &str, flags: u32) -> R<()> {
    new_regexp_x(st, pattern, flags, false)
}

/// Convert the stored lastIndex (UTF-16 units) into a byte offset for the
/// search; returns None when the index is out of range (ES5: reset to 0
/// and return null).
fn last_to_byte(text: &str, last: i64) -> Option<usize> {
    if last < 0 || last as usize > utf::utflen(text) {
        return None;
    }
    Some(utf::utf16_idx_to_byte(text, last as usize).unwrap_or(text.len()))
}

/// js_RegExp_prototype_exec: run the match and build the result array.
pub fn regexp_prototype_exec(st: &mut State, re: ObjRef, text: &str) -> R<()> {
    let flags = match &st.heap.obj(re).payload {
        Payload::Regexp(r) => r.flags,
        _ => return st.type_error("not a regexp"),
    };

    let mut start = 0usize;
    if flags & JS_REGEXP_G != 0 {
        let last = match &st.heap.obj(re).payload {
            Payload::Regexp(r) => r.last,
            _ => unreachable!(),
        };
        match last_to_byte(text, last) {
            Some(b) => start = b,
            None => {
                if let Payload::Regexp(r) = &mut st.heap.obj_mut(re).payload {
                    r.last = 0;
                }
                return st.push_null();
            }
        }
    }

    let m = {
        let prog = match &st.heap.obj(re).payload {
            Payload::Regexp(r) => &r.prog,
            _ => unreachable!(),
        };
        prog.exec(text, start)
    };

    if let Some(sub) = m {
        st.newarray()?;
        st.push_string(text)?;
        st.setproperty(-2, "input")?;
        let idx = utf::byte_to_utf16_idx(text, sub[0].unwrap().0);
        st.push_number(idx as f64)?;
        st.setproperty(-2, "index")?;
        for (i, g) in sub.iter().enumerate() {
            match g {
                Some((s, e)) => st.push_string(&text[*s..*e])?,
                None => st.push_literal("")?,
            }
            st.setindex(-2, i as i32)?;
        }
        if flags & JS_REGEXP_G != 0 {
            let last = utf::byte_to_utf16_idx(text, sub[0].unwrap().1) as i64;
            if let Payload::Regexp(r) = &mut st.heap.obj_mut(re).payload {
                r.last = last;
            }
        }
        return Ok(());
    }

    if flags & JS_REGEXP_G != 0
        && let Payload::Regexp(r) = &mut st.heap.obj_mut(re).payload {
            r.last = 0;
        }
    st.push_null()
}

fn rp_test(st: &mut State) -> R<()> {
    let re = toregexp(st, 0)?;
    let text = st.tostring(1)?;

    let flags = match &st.heap.obj(re).payload {
        Payload::Regexp(r) => r.flags,
        _ => unreachable!(),
    };

    let mut start = 0usize;
    if flags & JS_REGEXP_G != 0 {
        let last = match &st.heap.obj(re).payload {
            Payload::Regexp(r) => r.last,
            _ => unreachable!(),
        };
        match last_to_byte(&text, last) {
            Some(b) => start = b,
            None => {
                if let Payload::Regexp(r) = &mut st.heap.obj_mut(re).payload {
                    r.last = 0;
                }
                return st.push_boolean(false);
            }
        }
    }

    let m = {
        let prog = match &st.heap.obj(re).payload {
            Payload::Regexp(r) => &r.prog,
            _ => unreachable!(),
        };
        prog.exec(&text, start)
    };
    if let Some(sub) = m {
        if flags & JS_REGEXP_G != 0 {
            let newlast = utf::byte_to_utf16_idx(&text, sub[0].unwrap().1) as i64;
            if let Payload::Regexp(r) = &mut st.heap.obj_mut(re).payload {
                r.last = newlast;
            }
        }
        return st.push_boolean(true);
    }

    if flags & JS_REGEXP_G != 0
        && let Payload::Regexp(r) = &mut st.heap.obj_mut(re).payload {
            r.last = 0;
        }
    st.push_boolean(false)
}

fn jsb_new_regexp(st: &mut State) -> R<()> {
    let pattern: CompactString;
    let mut flags = 0u32;
    let mut is_clone = false;

    if st.isregexp(1) {
        if st.isdefined(2) {
            return st.type_error("cannot supply flags when creating one RegExp from another");
        }
        let old = toregexp(st, 1)?;
        let r = match &st.heap.obj(old).payload {
            Payload::Regexp(r) => r,
            _ => unreachable!(),
        };
        pattern = r.source.clone();
        flags = r.flags;
        is_clone = true;
    } else if st.isundefined(1) {
        pattern = st.heap.intern("(?:)");
    } else {
        pattern = st.tostring(1)?;
    }

    let pattern = if pattern.is_empty() {
        st.heap.intern("(?:)")
    } else {
        pattern
    };

    if st.isdefined(2) {
        let s = st.tostring(2)?;
        let (mut g, mut i, mut m) = (0, 0, 0);
        for c in s.chars() {
            match c {
                'g' => g += 1,
                'i' => i += 1,
                'm' => m += 1,
                _ => {
                    return st.syntax_error(&format!("invalid regular expression flag: '{}'", c))
                }
            }
        }
        if g > 1 {
            return st.syntax_error("invalid regular expression flag: 'g'");
        }
        if i > 1 {
            return st.syntax_error("invalid regular expression flag: 'i'");
        }
        if m > 1 {
            return st.syntax_error("invalid regular expression flag: 'm'");
        }
        if g > 0 {
            flags |= JS_REGEXP_G;
        }
        if i > 0 {
            flags |= JS_REGEXP_I;
        }
        if m > 0 {
            flags |= JS_REGEXP_M;
        }
    }

    new_regexp_x(st, &pattern, flags, is_clone)
}

fn jsb_regexp(st: &mut State) -> R<()> {
    // ES5.1 15.10.3.1: calling RegExp with a regexp argument returns it
    if st.isregexp(1) {
        return st.copy(1);
    }
    jsb_new_regexp(st)
}

fn rp_tostring(st: &mut State) -> R<()> {
    let re = toregexp(st, 0)?;
    let (source, flags) = match &st.heap.obj(re).payload {
        Payload::Regexp(r) => (r.source.clone(), r.flags),
        _ => unreachable!(),
    };
    let mut out = String::with_capacity(source.len() + 6);
    out.push('/');
    out.push_str(&source);
    out.push('/');
    if flags & JS_REGEXP_G != 0 {
        out.push('g');
    }
    if flags & JS_REGEXP_I != 0 {
        out.push('i');
    }
    if flags & JS_REGEXP_M != 0 {
        out.push('m');
    }
    st.push_string(&out)
}

fn rp_exec(st: &mut State) -> R<()> {
    let re = toregexp(st, 0)?;
    let text = st.tostring(1)?;
    regexp_prototype_exec(st, re, &text)
}

fn toregexp(st: &mut State, idx: i32) -> R<ObjRef> {
    match st.stackidx(idx).as_object() {
        Some(o) if st.heap.obj(o).class == Class::Regexp => Ok(o),
        _ => st.type_error("not a regexp"),
    }
}

pub fn init(st: &mut State) {
    let proto = st.protos.regexp;
    st.push_object(proto).unwrap();
    {
        propf(st, "RegExp.prototype.toString", rp_tostring, 0).unwrap();
        propf(st, "RegExp.prototype.test", rp_test, 0).unwrap();
        propf(st, "RegExp.prototype.exec", rp_exec, 0).unwrap();
    }
    st.newcconstructor(jsb_regexp, jsb_new_regexp, "RegExp", 1).unwrap();
    st.defglobal("RegExp", JS_DONTENUM).unwrap();
}
