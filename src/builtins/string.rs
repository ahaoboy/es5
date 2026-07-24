//! String constructor and String.prototype (jsstring.c).

use super::propf;
use crate::builtins::make_es6_iterator;
use crate::object::{Class, Payload};
use crate::state::{JS_STRLIMIT, R, State};
use crate::utf;
use crate::value::{JS_DONTENUM, JS_REGEXP_G, Value};
use std::rc::Rc;

/// checkstring: ToString, but null/undefined is a TypeError.
fn checkstring(st: &mut State, idx: i32) -> R<Rc<str>> {
    if !st.iscoercible(idx) {
        return st.type_error("string function called on null or undefined");
    }
    st.tostring(idx)
}

fn jsb_new_string(st: &mut State) -> R<()> {
    if st.gettop() > 1 {
        let s = st.tostring(1)?;
        st.newstring(&s)
    } else {
        st.newstring("")
    }
}

fn jsb_string(st: &mut State) -> R<()> {
    if st.gettop() > 1 {
        let s = st.tostring(1)?;
        st.push_string_rc(s)
    } else {
        st.push_string("")
    }
}

fn self_string(st: &mut State) -> R<Rc<str>> {
    let obj = st.toobject(0)?;
    match &st.heap.obj(obj).payload {
        Payload::String(s) if st.heap.obj(obj).class == Class::String => Ok(s.string.clone()),
        _ => st.type_error("not a string"),
    }
}

fn sp_tostring(st: &mut State) -> R<()> {
    let s = self_string(st)?;
    st.push_string_rc(s)
}

fn sp_charat(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let pos = st.tointeger(1)?;
    match utf::runeat(&s, pos.max(0) as usize) {
        Some(r) => {
            let mut out = String::new();
            utf::push_rune(&mut out, r);
            st.push_string(&out)
        }
        None => st.push_literal(""),
    }
}

fn sp_charcodeat(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let pos = st.tointeger(1)?;
    match utf::runeat(&s, pos.max(0) as usize) {
        Some(r) => st.push_number(r as f64),
        None => st.push_number(f64::NAN),
    }
}

fn sp_concat(st: &mut State) -> R<()> {
    let top = st.gettop();
    if top == 1 {
        return Ok(());
    }
    let first = checkstring(st, 0)?;
    // Pre-allocate: sum the lengths of all arg strings.
    let mut total: usize = first.len();
    for i in 1..top {
        // tostring may mutate the stack slot for numbers, so do it eagerly.
        let s = st.tostring(i)?;
        total = total.saturating_add(s.len());
    }
    if total > crate::state::JS_STRLIMIT {
        return st.range_error("invalid string length");
    }
    let mut out = String::with_capacity(total);
    out.push_str(&first);
    for i in 1..top {
        let s = st.tostring(i)?;
        out.push_str(&s);
    }
    st.push_string(&out)
}

fn sp_indexof(st: &mut State) -> R<()> {
    let haystack = checkstring(st, 0)?;
    let needle = st.tostring(1)?;
    let pos = st.tointeger(2)?;

    let from = pos.max(0) as usize;
    // Empty needle: return from (clamped).
    if needle.is_empty() {
        let out = from.min(utf::utflen(&haystack));
        return st.push_number(out as f64);
    }
    // Convert UTF-16 offset to byte offset; past-the-end means not found.
    let start_byte = match utf::utf16_idx_to_byte(&haystack, from) {
        Some(b) => b,
        None => return st.push_number(-1.0),
    };
    // O(n+m) search.
    match haystack[start_byte..].find(needle.as_ref()) {
        Some(offset) => {
            let k = utf::byte_to_utf16_idx(&haystack, start_byte + offset);
            st.push_number(k as f64)
        }
        None => st.push_number(-1.0),
    }
}

fn sp_lastindexof(st: &mut State) -> R<()> {
    let haystack = checkstring(st, 0)?;
    let needle = st.tostring(1)?;
    let len = utf::utflen(&haystack);
    let pos = if st.isdefined(2) {
        st.tointeger(2)?
    } else {
        len as i32
    };

    let to = pos.max(0).min(len as i32) as usize;
    // Empty needle: return to.
    if needle.is_empty() {
        return st.push_number(to as f64);
    }
    // Convert UTF-16 offset to byte offset.
    let end_byte = match utf::utf16_idx_to_byte(&haystack, to) {
        Some(b) => b,
        None => return st.push_number(-1.0),
    };
    // O(n+m) reverse search.
    match haystack[..end_byte].rfind(needle.as_ref()) {
        Some(offset) => {
            let k = utf::byte_to_utf16_idx(&haystack, offset);
            st.push_number(k as f64)
        }
        None => st.push_number(-1.0),
    }
}

fn sp_localecompare(st: &mut State) -> R<()> {
    let a = checkstring(st, 0)?;
    let b = st.tostring(1)?;
    let c = a.as_bytes().cmp(b.as_bytes());
    st.push_number(c as i32 as f64)
}

fn sp_slice(st: &mut State) -> R<()> {
    let str_ = checkstring(st, 0)?;
    let len = utf::utflen(&str_) as i32;
    let mut s = st.tointeger(1)?;
    let mut e = if st.isdefined(2) {
        st.tointeger(2)?
    } else {
        len
    };

    s = if s < 0 { s + len } else { s };
    e = if e < 0 { e + len } else { e };

    s = s.clamp(0, len);
    e = e.clamp(0, len);

    if s < e {
        let out = utf::substring_utf16(&str_, s as usize, (e - s) as usize);
        st.push_string(&out)
    } else if s > e {
        let out = utf::substring_utf16(&str_, e as usize, (s - e) as usize);
        st.push_string(&out)
    } else {
        st.push_literal("")
    }
}

fn sp_substr(st: &mut State) -> R<()> {
    let str_ = checkstring(st, 0)?;
    let len = utf::utflen(&str_) as i32;

    let mut s = st.tointeger(1)?;

    if s < 0 {
        s = (len + s).max(0);
    }

    if s >= len {
        return st.push_literal("");
    }

    let l = if st.isdefined(2) {
        st.tointeger(2)?
    } else {
        len - s
    };

    if l <= 0 {
        return st.push_literal("");
    }

    let count = l.min(len - s);

    let out = utf::substring_utf16(&str_, s as usize, count as usize);

    st.push_string(&out)
}

fn sp_substring(st: &mut State) -> R<()> {
    let str_ = checkstring(st, 0)?;
    let len = utf::utflen(&str_) as i32;
    let mut s = st.tointeger(1)?;
    let mut e = if st.isdefined(2) {
        st.tointeger(2)?
    } else {
        len
    };

    s = s.clamp(0, len);
    e = e.clamp(0, len);

    if s < e {
        let out = utf::substring_utf16(&str_, s as usize, (e - s) as usize);
        st.push_string(&out)
    } else if s > e {
        let out = utf::substring_utf16(&str_, e as usize, (s - e) as usize);
        st.push_string(&out)
    } else {
        st.push_literal("")
    }
}

fn sp_tolowercase(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match utf::tolowerrune_full(c) {
            Some(full) => out.push_str(&full),
            None => out.push(utf::tolowerrune(c)),
        }
    }
    st.push_string(&out)
}

fn sp_touppercase(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match utf::toupperrune_full(c) {
            Some(full) => out.push_str(&full),
            None => out.push(utf::toupperrune(c)),
        }
    }
    st.push_string(&out)
}

fn sp_trim(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    // mujs's istrim() is applied to (signed) C chars, so only ASCII
    // whitespace bytes are ever trimmed in practice.
    fn istrim(b: u8) -> bool {
        matches!(b, 0x9 | 0xA | 0xB | 0xC | 0xD | 0x20)
    }
    let b = s.as_bytes();
    let mut start = 0;
    let mut end = b.len();
    while start < end && istrim(b[start]) {
        start += 1;
    }
    while end > start && istrim(b[end - 1]) {
        end -= 1;
    }
    let out = s[start..end].to_string();
    st.push_string(&out)
}

fn sp_trimend(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    fn istrim(b: u8) -> bool {
        matches!(b, 0x9 | 0xA | 0xB | 0xC | 0xD | 0x20)
    }
    let b = s.as_bytes();
    let mut end = b.len();
    while end > 0 && istrim(b[end - 1]) {
        end -= 1;
    }
    st.push_string(&s[..end])
}

fn sp_trimstart(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    fn istrim(b: u8) -> bool {
        matches!(b, 0x9 | 0xA | 0xB | 0xC | 0xD | 0x20)
    }
    let b = s.as_bytes();
    let mut start = 0;
    while start < b.len() && istrim(b[start]) {
        start += 1;
    }
    st.push_string(&s[start..])
}

fn pad_impl(st: &mut State, pad_end: bool) -> R<()> {
    let s = checkstring(st, 0)?.to_string();
    let target_len = st.tointeger(1)?;
    if target_len <= 0 {
        return st.push_string(&s);
    }
    let target = target_len as usize;
    let s_len = utf::utflen(&s);
    if s_len >= target {
        return st.push_string(&s);
    }
    let pad_len = target - s_len;
    let fill = if st.isdefined(2) {
        let f = st.tostring(2)?.to_string();
        if f.is_empty() { " ".to_string() } else { f }
    } else {
        " ".to_string()
    };
    let fill_len = utf::utflen(&fill);
    let mut out = String::with_capacity(s.len() + pad_len * fill.len());
    // Build padding in UTF-16 code unit chunks
    let mut inserted = 0usize;
    if !pad_end {
        while inserted < pad_len {
            let space = pad_len - inserted;
            let chunk_len = space.min(fill_len);
            let sub = utf::substring_utf16(&fill, 0, chunk_len);
            out.push_str(&sub);
            inserted += chunk_len;
        }
        out.push_str(&s);
    } else {
        out.push_str(&s);
        while inserted < pad_len {
            let space = pad_len - inserted;
            let chunk_len = space.min(fill_len);
            let sub = utf::substring_utf16(&fill, 0, chunk_len);
            out.push_str(&sub);
            inserted += chunk_len;
        }
    }
    st.push_string(&out)
}

fn sp_padstart(st: &mut State) -> R<()> {
    pad_impl(st, false)
}

fn sp_padend(st: &mut State) -> R<()> {
    pad_impl(st, true)
}

fn s_fromcharcode(st: &mut State) -> R<()> {
    let top = st.gettop();
    let mut out = String::new();
    for i in 1..top {
        let c = st.touint32(i)?;
        utf::push_rune(&mut out, c);
    }
    st.push_string(&out)
}

// -- regexp-based methods ---------------------------------------------------

/// Fetch the regexp payload of the object at idx (js_toregexp).
fn toregexp(st: &mut State, idx: i32) -> R<crate::object::ObjRef> {
    match st.stackidx(idx).as_object() {
        Some(o) if st.heap.obj(o).class == Class::Regexp => Ok(o),
        _ => st.type_error("not a regexp"),
    }
}

fn sp_match(st: &mut State) -> R<()> {
    let text = checkstring(st, 0)?;

    if st.isregexp(1) {
        st.copy(1)?;
    } else if st.isundefined(1) {
        super::regexp::new_regexp(st, "", 0)?;
    } else {
        let p = st.tostring(1)?;
        super::regexp::new_regexp(st, &p, 0)?;
    }

    let re = toregexp(st, -1)?;
    let flags = match &st.heap.obj(re).payload {
        Payload::Regexp(r) => r.flags,
        _ => unreachable!(),
    };
    if flags & JS_REGEXP_G == 0 {
        return super::regexp::regexp_prototype_exec(st, re, &text);
    }

    if let Payload::Regexp(r) = &mut st.heap.obj_mut(re).payload {
        r.last = 0;
    }

    st.newarray()?;

    let mut len = 0;
    let mut a = 0usize; // byte offset into text
    while a <= text.len() {
        let m = {
            let prog = match &st.heap.obj(re).payload {
                Payload::Regexp(r) => &r.prog,
                _ => unreachable!(),
            };
            prog.exec(&text, a)
        };
        let sub = match m {
            Some(s) => s,
            None => break,
        };
        let (b, c) = sub[0].unwrap();

        st.push_string(&text[b..c])?;
        st.setindex(-2, len)?;
        len += 1;

        a = c;
        if c == b {
            // empty match: advance one rune
            a += utf::chartorune(&text[a..]).1.max(1);
        }
    }

    if len == 0 {
        st.pop(1);
        st.push_null()?;
    }
    Ok(())
}

fn sp_search(st: &mut State) -> R<()> {
    let text = checkstring(st, 0)?;

    if st.isregexp(1) {
        st.copy(1)?;
    } else if st.isundefined(1) {
        super::regexp::new_regexp(st, "", 0)?;
    } else {
        let p = st.tostring(1)?;
        super::regexp::new_regexp(st, &p, 0)?;
    }

    let re = toregexp(st, -1)?;
    let m = {
        let prog = match &st.heap.obj(re).payload {
            Payload::Regexp(r) => &r.prog,
            _ => unreachable!(),
        };
        prog.exec(&text, 0)
    };
    match m {
        Some(sub) => {
            let (b, _) = sub[0].unwrap();
            let idx = utf::byte_to_utf16_idx(&text, b);
            st.push_number(idx as f64)
        }
        None => st.push_number(-1.0),
    }
}

fn sp_replace_regexp(st: &mut State) -> R<()> {
    let source = checkstring(st, 0)?;
    let re = toregexp(st, 1)?;

    let first = {
        let prog = match &st.heap.obj(re).payload {
            Payload::Regexp(r) => &r.prog,
            _ => unreachable!(),
        };
        prog.exec(&source, 0)
    };
    let mut m = match first {
        Some(sub) => sub,
        None => {
            return st.copy(0);
        }
    };

    if let Payload::Regexp(r) = &mut st.heap.obj_mut(re).payload {
        r.last = 0;
    }

    let flags = match &st.heap.obj(re).payload {
        Payload::Regexp(r) => r.flags,
        _ => unreachable!(),
    };

    let mut sb = String::new();
    let mut pos = 0usize; // current byte offset in source

    loop {
        let (s, e) = m[0].unwrap();
        let n = e - s;

        if st.iscallable(2) {
            st.copy(2)?;
            st.push_undefined()?;
            let mut x = 0;
            for g in m.iter() {
                match g {
                    Some((gs, ge)) => st.push_string(&source[*gs..*ge])?,
                    None => break,
                }
                x += 1;
            }
            // offset within search string, in UTF-16 code units (ES5)
            st.push_number(utf::byte_to_utf16_idx(&source, s) as f64)?;
            st.copy(0)?;
            st.call(2 + x)?;
            let r = st.tostring(-1)?;
            sb.push_str(&source[pos..s]);
            sb.push_str(&r);
            st.pop(1);
        } else {
            let r = st.tostring(2)?;
            sb.push_str(&source[pos..s]);
            let rb = r.as_bytes();
            let mut i = 0;
            while i < rb.len() {
                if rb[i] == b'$' && i + 1 < rb.len() {
                    i += 1;
                    match rb[i] {
                        b'$' => sb.push('$'),
                        b'`' => sb.push_str(&source[..s]),
                        b'\'' => sb.push_str(&source[s + n..]),
                        b'&' => sb.push_str(&source[s..s + n]),
                        d @ b'0'..=b'9' => {
                            let mut x = (d - b'0') as usize;
                            if i + 1 < rb.len() && rb[i + 1].is_ascii_digit() {
                                i += 1;
                                x = x * 10 + (rb[i] - b'0') as usize;
                            }
                            if x > 0 && x < m.len() {
                                if let Some(Some((gs, ge))) = m.get(x) {
                                    sb.push_str(&source[*gs..*ge]);
                                }
                            } else {
                                sb.push('$');
                                if x > 10 {
                                    sb.push((b'0' + (x / 10) as u8) as char);
                                    sb.push((b'0' + (x % 10) as u8) as char);
                                } else {
                                    sb.push((b'0' + x as u8) as char);
                                }
                            }
                        }
                        c => {
                            sb.push('$');
                            sb.push(c as char);
                        }
                    }
                    i += 1;
                } else {
                    sb.push(rb[i] as char);
                    i += 1;
                }
            }
        }

        if flags & JS_REGEXP_G != 0 {
            pos = e;
            if n == 0 {
                if pos < source.len() {
                    // mujs copies a single byte here
                    sb.push(source.as_bytes()[pos] as char);
                    pos += 1;
                } else {
                    break;
                }
            }
            let next = {
                let prog = match &st.heap.obj(re).payload {
                    Payload::Regexp(r) => &r.prog,
                    _ => unreachable!(),
                };
                prog.exec(&source, pos)
            };
            match next {
                Some(sub) => {
                    m = sub;
                    continue;
                }
                None => break,
            }
        } else {
            pos = e;
        }
        break;
    }

    // append the tail after the last match
    sb.push_str(&source[pos..]);

    st.push_string(&sb)
}

fn sp_replace_string(st: &mut State) -> R<()> {
    let source = checkstring(st, 0)?;
    let needle = st.tostring(1)?;

    let s = match source.find(needle.as_ref()) {
        Some(i) => i,
        None => {
            return st.copy(0);
        }
    };
    let n = needle.len();

    let mut sb = String::new();

    if st.iscallable(2) {
        st.copy(2)?;
        st.push_undefined()?;
        st.push_string(&source[s..s + n])?; // arg 1: substring that matched
        // arg 2: offset within search string, in UTF-16 code units (ES5)
        st.push_number(utf::byte_to_utf16_idx(&source, s) as f64)?;
        st.copy(0)?; // arg 3: search string
        st.call(3)?;
        let r = st.tostring(-1)?;
        sb.push_str(&source[..s]);
        sb.push_str(&r);
        sb.push_str(&source[s + n..]);
        st.pop(1);
    } else {
        let r = st.tostring(2)?;
        sb.push_str(&source[..s]);
        let rb = r.as_bytes();
        let mut i = 0;
        while i < rb.len() {
            if rb[i] == b'$' && i + 1 < rb.len() {
                i += 1;
                match rb[i] {
                    b'$' => sb.push('$'),
                    b'&' => sb.push_str(&source[s..s + n]),
                    b'`' => sb.push_str(&source[..s]),
                    b'\'' => sb.push_str(&source[s + n..]),
                    c => {
                        sb.push('$');
                        sb.push(c as char);
                    }
                }
                i += 1;
            } else {
                sb.push(rb[i] as char);
                i += 1;
            }
        }
        sb.push_str(&source[s + n..]);
    }

    st.push_string(&sb)
}

fn sp_replace(st: &mut State) -> R<()> {
    if st.isregexp(1) {
        sp_replace_regexp(st)
    } else {
        sp_replace_string(st)
    }
}

fn sp_split_regexp(st: &mut State) -> R<()> {
    let text = checkstring(st, 0)?;
    let re = toregexp(st, 1)?;
    let limit = if st.isdefined(2) {
        st.tointeger(2)?
    } else {
        1 << 30
    };

    st.newarray()?;
    let mut len = 0;

    if limit == 0 {
        return Ok(());
    }

    // splitting the empty string
    if text.is_empty() {
        let m = {
            let prog = match &st.heap.obj(re).payload {
                Payload::Regexp(r) => &r.prog,
                _ => unreachable!(),
            };
            prog.exec(&text, 0)
        };
        if m.is_none() {
            st.push_literal("")?;
            st.setindex(-2, 0)?;
        }
        return Ok(());
    }

    let mut p = 0usize; // start of current piece
    let mut a = 0usize; // search position
    while a < text.len() {
        let m = {
            let prog = match &st.heap.obj(re).payload {
                Payload::Regexp(r) => &r.prog,
                _ => unreachable!(),
            };
            prog.exec(&text, a)
        };
        let sub = match m {
            Some(s) => s,
            None => break,
        };
        let (b, c) = sub[0].unwrap();

        // empty string at end of last match
        if b == c && b == p {
            a += utf::chartorune(&text[a..]).1.max(1);
            continue;
        }

        if len == limit {
            return Ok(());
        }
        st.push_string(&text[p..b])?;
        st.setindex(-2, len)?;
        len += 1;

        for g in sub.iter().skip(1) {
            if len == limit {
                return Ok(());
            }
            match g {
                Some((gs, ge)) => st.push_string(&text[*gs..*ge])?,
                None => st.push_string("")?,
            }
            st.setindex(-2, len)?;
            len += 1;
        }

        a = c;
        p = c;
    }

    if len == limit {
        return Ok(());
    }
    st.push_string(&text[p..])?;
    st.setindex(-2, len)?;
    Ok(())
}

fn sp_split_string(st: &mut State) -> R<()> {
    let str_ = checkstring(st, 0)?;
    let sep = st.tostring(1)?;
    let limit = if st.isdefined(2) {
        st.tointeger(2)?
    } else {
        1 << 30
    };

    st.newarray()?;

    if limit == 0 {
        return Ok(());
    }

    // empty separator: split into runes
    if sep.is_empty() {
        for (i, (b, c)) in str_.char_indices().enumerate() {
            if i as i32 >= limit {
                break;
            }
            st.push_string(&str_[b..b + c.len_utf8()])?;
            st.setindex(-2, i as i32)?;
        }
        return Ok(());
    }

    let mut i = 0;
    let mut rest: &str = &str_;
    loop {
        if i >= limit {
            break;
        }
        match rest.find(sep.as_ref()) {
            Some(s) => {
                st.push_string(&rest[..s])?;
                st.setindex(-2, i)?;
                rest = &rest[s + sep.len()..];
            }
            None => {
                st.push_string(rest)?;
                st.setindex(-2, i)?;
                break;
            }
        }
        i += 1;
    }
    Ok(())
}

fn sp_split(st: &mut State) -> R<()> {
    if st.isundefined(1) {
        st.newarray()?;
        let s = st.tostring(0)?;
        st.push_string_rc(s)?;
        st.setindex(-2, 0)?;
        Ok(())
    } else if st.isregexp(1) {
        sp_split_regexp(st)
    } else {
        sp_split_string(st)
    }
}

fn sp_iterator(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let chars: Vec<char> = s.chars().collect();
    let values: Vec<Value> = chars
        .iter()
        .map(|c| {
            let mut out = String::new();
            utf::push_rune(&mut out, *c as u32);
            Value::String(st.heap.intern(&out))
        })
        .collect();
    make_es6_iterator(st, values)
}

/// String.prototype.endsWith(search[, length]) — ES6.
fn sp_endswith(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let search = st.tostring(1)?;
    let len = if st.isdefined(2) {
        let n = st.tointeger(2)?;
        if n < 0 { 0 } else { n as usize }
    } else {
        s.len()
    };
    let end = len.min(s.len());
    let start = end.saturating_sub(search.len());
    st.push_boolean(end >= start && s[start..end] == *search)
}

/// String.prototype.startsWith(search[, position]) — ES6.
fn sp_startswith(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let search = st.tostring(1)?;
    let pos = if st.isdefined(2) {
        let n = st.tointeger(2)?;
        if n < 0 { 0 } else { n as usize }
    } else {
        0
    };
    st.push_boolean(s[pos..].starts_with(search.as_ref()))
}

/// String.prototype.includes(search[, position]) — ES6.
fn sp_includes(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let search = st.tostring(1)?;
    let pos = if st.isdefined(2) {
        let n = st.tointeger(2)?;
        if n < 0 { 0 } else { n as usize }
    } else {
        0
    };
    st.push_boolean(s[pos..].contains(search.as_ref()))
}

/// String.prototype.repeat(count) — ES6.
fn sp_repeat(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let n = st.tointeger(1)?;
    if n < 0 || n > JS_STRLIMIT as i32 {
        return st.range_error("invalid repeat count");
    }
    let mut out = String::with_capacity(s.len() * n as usize);
    for _ in 0..n {
        out.push_str(&s);
    }
    st.push_string(&out)
}

/// String.prototype.codePointAt(pos) — ES6.
fn sp_codepointat(st: &mut State) -> R<()> {
    let s = checkstring(st, 0)?;
    let pos = st.tointeger(1)?;
    match utf::runeat(&s, pos.max(0) as usize) {
        Some(r) => st.push_number(r as f64),
        None => st.push_number(f64::NAN),
    }
}

pub fn init(st: &mut State) {
    let proto = st.protos.string;
    st.push_object(proto).unwrap();
    {
        propf(st, "String.prototype.toString", sp_tostring, 0).unwrap();
        propf(st, "String.prototype.valueOf", sp_tostring, 0).unwrap();
        propf(st, "String.prototype.charAt", sp_charat, 1).unwrap();
        propf(st, "String.prototype.charCodeAt", sp_charcodeat, 1).unwrap();
        propf(st, "String.prototype.concat", sp_concat, 0).unwrap();
        propf(st, "String.prototype.indexOf", sp_indexof, 1).unwrap();
        propf(st, "String.prototype.lastIndexOf", sp_lastindexof, 1).unwrap();
        propf(st, "String.prototype.localeCompare", sp_localecompare, 1).unwrap();
        propf(st, "String.prototype.match", sp_match, 1).unwrap();
        propf(st, "String.prototype.replace", sp_replace, 2).unwrap();
        propf(st, "String.prototype.search", sp_search, 1).unwrap();
        propf(st, "String.prototype.slice", sp_slice, 2).unwrap();
        propf(st, "String.prototype.split", sp_split, 2).unwrap();
        propf(st, "String.prototype.substring", sp_substring, 2).unwrap();
        propf(st, "String.prototype.substr", sp_substr, 2).unwrap();
        propf(st, "String.prototype.toLowerCase", sp_tolowercase, 0).unwrap();
        propf(st, "String.prototype.toLocaleLowerCase", sp_tolowercase, 0).unwrap();
        propf(st, "String.prototype.toUpperCase", sp_touppercase, 0).unwrap();
        propf(st, "String.prototype.toLocaleUpperCase", sp_touppercase, 0).unwrap();

        // ES5
        propf(st, "String.prototype.trim", sp_trim, 0).unwrap();

        // ES2019
        propf(st, "String.prototype.trimEnd", sp_trimend, 0).unwrap();
        propf(st, "String.prototype.trimRight", sp_trimend, 0).unwrap();
        propf(st, "String.prototype.trimStart", sp_trimstart, 0).unwrap();
        propf(st, "String.prototype.trimLeft", sp_trimstart, 0).unwrap();

        // ES2017
        propf(st, "String.prototype.padStart", sp_padstart, 1).unwrap();
        propf(st, "String.prototype.padEnd", sp_padend, 1).unwrap();

        // ES6
        propf(st, "String.prototype.endsWith", sp_endswith, 1).unwrap();
        propf(st, "String.prototype.startsWith", sp_startswith, 1).unwrap();
        propf(st, "String.prototype.includes", sp_includes, 1).unwrap();
        propf(st, "String.prototype.repeat", sp_repeat, 1).unwrap();
        propf(st, "String.prototype.codePointAt", sp_codepointat, 1).unwrap();

        // ES6 iterator
        propf(st, "String.prototype.@@iterator", sp_iterator, 0).unwrap();
    }
    st.newcconstructor(jsb_string, jsb_new_string, "String", 0)
        .unwrap();
    {
        propf(st, "String.fromCharCode", s_fromcharcode, 0).unwrap();
    }
    st.defglobal("String", JS_DONTENUM).unwrap();
}
