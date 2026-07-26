//! JSON.parse and JSON.stringify (json.c).

use super::propf;
use crate::lex::{Lexer, TK_FALSE, TK_NULL, TK_NUMBER, TK_STRING, TK_TRUE};
use crate::object::{Class, Payload};
use crate::state::{State, R};
use crate::value::JS_DONTENUM;

// -- parsing -----------------------------------------------------------------

struct JsonParser {
    lex: Lexer,
    lookahead: i32,
}

impl JsonParser {
    fn next(&mut self, st: &mut State) -> R<()> {
        self.lookahead = self.lex.lexjson(st)?;
        Ok(())
    }

    fn accept(&mut self, st: &mut State, t: i32) -> R<bool> {
        if self.lookahead == t {
            self.next(st)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn expect(&mut self, st: &mut State, t: i32) -> R<()> {
        if !self.accept(st, t)? {
            return st.syntax_error(&format!(
                "JSON: unexpected token: {} (expected {})",
                crate::lex::tokenstring(self.lookahead),
                crate::lex::tokenstring(t)
            ));
        }
        Ok(())
    }

    fn value(&mut self, st: &mut State) -> R<()> {
        match self.lookahead {
            TK_STRING => {
                let s = self.lex.text.clone();
                st.push_string(&s)?;
                self.next(st)?;
            }
            TK_NUMBER => {
                let n = self.lex.number;
                st.push_number(n)?;
                self.next(st)?;
            }
            t if t == '{' as i32 => {
                st.newobject()?;
                self.next(st)?;
                if self.accept(st, '}' as i32)? {
                    return Ok(());
                }
                loop {
                    if self.lookahead != TK_STRING {
                        return st.syntax_error(&format!(
                            "JSON: unexpected token: {} (expected string)",
                            crate::lex::tokenstring(self.lookahead)
                        ));
                    }
                    let key = self.lex.text.clone();
                    st.push_string(&key)?;
                    self.next(st)?;
                    self.expect(st, ':' as i32)?;
                    self.value(st)?;
                    let key2 = st.tostring(-2)?;
                    st.setproperty(-3, &key2)?;
                    st.pop(1);
                    if !self.accept(st, ',' as i32)? {
                        break;
                    }
                }
                self.expect(st, '}' as i32)?;
            }
            t if t == '[' as i32 => {
                st.newarray()?;
                self.next(st)?;
                let mut i = 0;
                if self.accept(st, ']' as i32)? {
                    return Ok(());
                }
                loop {
                    self.value(st)?;
                    st.setindex(-2, i)?;
                    i += 1;
                    if !self.accept(st, ',' as i32)? {
                        break;
                    }
                }
                self.expect(st, ']' as i32)?;
            }
            TK_TRUE => {
                st.push_boolean(true)?;
                self.next(st)?;
            }
            TK_FALSE => {
                st.push_boolean(false)?;
                self.next(st)?;
            }
            TK_NULL => {
                st.push_null()?;
                self.next(st)?;
            }
            _ => {
                return st.syntax_error(&format!(
                    "JSON: unexpected token: {}",
                    crate::lex::tokenstring(self.lookahead)
                ))
            }
        }
        Ok(())
    }
}

/// jsonrevive: revive is in 2; holder is in -1
fn json_revive(st: &mut State, name: &str) -> R<()> {
    st.getproperty(-1, name)?; // get value from holder

    if st.isobject(-1) {
        if st.isarray(-1) {
            let n = st.getlength(-1)?;
            for i in 0..n {
                let key = i.to_string();
                json_revive(st, &key)?;
                if st.isundefined(-1) {
                    st.pop(1);
                    st.delproperty(-1, &key)?;
                } else {
                    st.setproperty(-2, &key)?;
                }
            }
        } else {
            st.pushiterator(-1, true)?;
            loop {
                let key = st.nextiterator(-1)?;
                match key {
                    Some(key) => {
                        st.rot2();
                        json_revive(st, &key)?;
                        if st.isundefined(-1) {
                            st.pop(1);
                            st.delproperty(-1, &key)?;
                        } else {
                            st.setproperty(-2, &key)?;
                        }
                        st.rot2();
                    }
                    None => break,
                }
            }
            st.pop(1);
        }
    }

    st.copy(2)?; // reviver function
    st.copy(-3)?; // holder as this
    st.push_string(name)?; // name
    st.copy(-4)?; // value
    st.call(2)?;
    st.rot2pop1(); // pop old value, leave new value on stack
    Ok(())
}

fn json_parse(st: &mut State) -> R<()> {
    let source = st.tostring(1)?;
    let mut p = JsonParser {
        lex: Lexer::new("JSON", &source),
        lookahead: 0,
    };
    p.next(st)?;

    if st.iscallable(2) {
        st.newobject()?;
        p.value(st)?;
        st.defproperty(-2, "", 0)?;
        json_revive(st, "")
    } else {
        p.value(st)
    }
}

// -- stringify -----------------------------------------------------------------

fn fmtnum(out: &mut String, n: f64) {
    if n.is_nan() || n.is_infinite() {
        out.push_str("null");
    } else if n == 0.0 {
        out.push('0');
    } else {
        out.push_str(&crate::number::number_to_string(n));
    }
}

fn fmtstr(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
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
            c if (c as u32) < 32 || (0xd800..=0xdfff).contains(&(c as u32)) => {
                let v = c as u32;
                out.push('\\');
                out.push('u');
                out.push(HEX[(v >> 12) as usize & 15] as char);
                out.push(HEX[(v >> 8) as usize & 15] as char);
                out.push(HEX[(v >> 4) as usize & 15] as char);
                out.push(HEX[v as usize & 15] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn fmtindent(out: &mut String, gap: &str, level: i32) {
    out.push('\n');
    for _ in 0..level {
        out.push_str(gap);
    }
}

/// replacer/property-list is in stack slot 2
fn filterprop(st: &mut State, key: &str) -> R<bool> {
    if st.isarray(2) {
        let n = st.getlength(2)?;
        for i in 0..n {
            st.getindex(2, i)?;
            let found = if st.isstring(-1) || st.isnumber(-1) || isstringobject(st, -1) || isnumberobject(st, -1) {
                key == st.tostring(-1)?.as_ref() as &str
            } else {
                false
            };
            st.pop(1);
            if found {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    Ok(true)
}

fn isstringobject(st: &State, idx: i32) -> bool {
    match st.stackidx(idx).as_object() {
        Some(o) => st.heap.obj(o).class == Class::String,
        None => false,
    }
}

fn isnumberobject(st: &State, idx: i32) -> bool {
    match st.stackidx(idx).as_object() {
        Some(o) => st.heap.obj(o).class == Class::Number,
        None => false,
    }
}

fn fmtobject(st: &mut State, sb: &mut String, gap: Option<&str>, level: i32) -> R<()> {
    let n = st.gettop() - 1;
    for i in 4..n {
        if st.isobject(i)
            && st.toobject(i)? == st.toobject(-1)? {
                return st.type_error("cyclic object value");
            }
    }

    let mut n = 0;
    sb.push('{');
    st.pushiterator(-1, true)?;
    loop {
        let key = st.nextiterator(-1)?;
        let key = match key {
            Some(k) => k,
            None => break,
        };
        if filterprop(st, &key)? {
            let save = sb.len();
            if n > 0 {
                sb.push(',');
            }
            if let Some(g) = gap {
                fmtindent(sb, g, level + 1);
            }
            fmtstr(sb, &key);
            sb.push(':');
            if gap.is_some() {
                sb.push(' ');
            }
            st.rot2();
            if !fmtvalue(st, sb, &key, gap, level + 1)? {
                sb.truncate(save);
            } else {
                n += 1;
            }
            st.rot2();
        }
    }
    st.pop(1);
    if let Some(g) = gap
        && n > 0 {
            fmtindent(sb, g, level);
        }
    sb.push('}');
    Ok(())
}

fn fmtarray(st: &mut State, sb: &mut String, gap: Option<&str>, level: i32) -> R<()> {
    let n = st.gettop() - 1;
    for i in 4..n {
        if st.isobject(i)
            && st.toobject(i)? == st.toobject(-1)? {
                return st.type_error("cyclic object value");
            }
    }

    sb.push('[');
    let n = st.getlength(-1)?;
    for i in 0..n {
        if i > 0 {
            sb.push(',');
        }
        if let Some(g) = gap {
            fmtindent(sb, g, level + 1);
        }
        let key = i.to_string();
        if !fmtvalue(st, sb, &key, gap, level + 1)? {
            sb.push_str("null");
        }
    }
    if let Some(g) = gap
        && n > 0 {
            fmtindent(sb, g, level);
        }
    sb.push(']');
    Ok(())
}

/// replacer/property-list is in 2; holder is in -1
fn fmtvalue(st: &mut State, sb: &mut String, key: &str, gap: Option<&str>, level: i32) -> R<bool> {
    st.getproperty(-1, key)?;

    if st.isobject(-1)
        && st.hasproperty(-1, "toJSON")? {
            if st.iscallable(-1) {
                st.copy(-2)?;
                st.push_string(key)?;
                st.call(1)?;
                st.rot2pop1();
            } else {
                st.pop(1);
            }
        }

    if st.iscallable(2) {
        st.copy(2)?; // replacer function
        st.copy(-3)?; // holder as this
        st.push_string(key)?; // name
        st.copy(-4)?; // old value
        st.call(2)?;
        st.rot2pop1(); // pop old value, leave new value on stack
    }

    let ok = if st.isobject(-1) && !st.iscallable(-1) {
        let obj = st.toobject(-1)?;
        match st.heap.obj(obj).class {
            Class::Number => {
                let n = match &st.heap.obj(obj).payload {
                    Payload::Number(n) => *n,
                    _ => 0.0,
                };
                fmtnum(sb, n);
                true
            }
            Class::String => {
                let s = match &st.heap.obj(obj).payload {
                    Payload::String(s) => s.string.clone(),
                    _ => st.heap.intern(""),
                };
                fmtstr(sb, &s);
                true
            }
            Class::Boolean => {
                let b = match &st.heap.obj(obj).payload {
                    Payload::Boolean(b) => *b,
                    _ => false,
                };
                sb.push_str(if b { "true" } else { "false" });
                true
            }
            Class::Array => {
                fmtarray(st, sb, gap, level)?;
                true
            }
            _ => {
                fmtobject(st, sb, gap, level)?;
                true
            }
        }
    } else if st.isboolean(-1) {
        sb.push_str(if st.toboolean(-1) { "true" } else { "false" });
        true
    } else if st.isnumber(-1) {
        let n = st.tonumber(-1)?;
        fmtnum(sb, n);
        true
    } else if st.isstring(-1) {
        let s = st.tostring(-1)?;
        fmtstr(sb, &s);
        true
    } else if st.isnull(-1) {
        sb.push_str("null");
        true
    } else {
        st.pop(1);
        return Ok(false);
    };

    let _ = ok;
    st.pop(1);
    Ok(true)
}

fn json_stringify(st: &mut State) -> R<()> {


    let gap: Option<String> = if st.isnumber(3) || isnumberobject(st, 3) {
        let n = st.tointeger(3)?.clamp(0, 10);
        if n > 0 {
            Some(" ".repeat(n as usize))
        } else {
            None
        }
    } else if st.isstring(3) || isstringobject(st, 3) {
        let s = st.tostring(3)?;
        let n = s.len().min(10);
        if n > 0 {
            Some(String::from_utf8_lossy(&s.as_bytes()[..n]).into_owned())
        } else {
            None
        }
    } else {
        None
    };

    st.newobject()?; // wrapper
    st.copy(1)?;
    st.defproperty(-2, "", 0)?;
    let mut sb = String::new();
    if !fmtvalue(st, &mut sb, "", gap.as_deref(), 0)? {
        st.push_undefined()
    } else {
        st.push_string(&sb)?;
        st.rot2pop1();
        Ok(())
    }
}

pub fn init(st: &mut State) {
    let proto = st.protos.object;
    let m = st.heap.alloc_object(Class::Json, Some(proto));
    st.push_object(m).unwrap();
    {
        propf(st, "JSON.parse", json_parse, 2).unwrap();
        propf(st, "JSON.stringify", json_stringify, 3).unwrap();
    }
    st.defglobal("JSON", JS_DONTENUM).unwrap();
}
