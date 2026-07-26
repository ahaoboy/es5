//! Array constructor and Array.prototype (jsarray.c).

use super::propf;
use crate::builtins::make_es6_iterator;
use crate::object::{Class, Payload};
use crate::state::{State, R};
use crate::value::{JS_DONTENUM, Value};

fn jsb_new_array(st: &mut State) -> R<()> {
    let top = st.gettop();

    st.newarray()?;

    if top == 2 {
        if st.isnumber(1) {
            st.copy(1)?;
            st.setproperty(-2, "length")?;
        } else {
            st.copy(1)?;
            st.setindex(-2, 0)?;
        }
    } else {
        for i in 1..top {
            st.copy(i)?;
            st.setindex(-2, i - 1)?;
        }
    }
    Ok(())
}

fn ap_concat(st: &mut State) -> R<()> {
    let top = st.gettop();

    st.newarray()?;
    let mut n = 0;

    for i in 0..top {
        st.copy(i)?;
        if st.isarray(-1) {
            let len = st.getlength(-1)?;
            for k in 0..len {
                if st.hasindex(-1, k)? {
                    st.setindex(-3, n)?;
                    n += 1;
                }
            }
            st.pop(1);
        } else {
            st.setindex(-2, n)?;
            n += 1;
        }
    }
    Ok(())
}

/// ugly cycle detection for Array.prototype.join
fn ap_join_cycle(st: &mut State) -> bool {
    let needle = match st.stackidx(0).as_object() {
        Some(o) => o,
        None => return false,
    };
    let mut top = st.tracetop as i32 - 1;
    while top > 0 {
        let stk = st.trace[top as usize].stack;
        if stk == 0 {
            return false;
        }
        let fun = &st.stack[stk - 1];
        let fun_obj = match fun.as_object() {
            Some(o) => o,
            None => return false,
        };
        if st.heap.obj(fun_obj).class != Class::CFunction {
            return false;
        }
        let f = match &st.heap.obj(fun_obj).payload {
            Payload::CFunction(cd) => cd.function,
            _ => return false,
        };
        let fp = f as *const ();
        if fp == ap_join as *const () {
            let obj = &st.stack[stk];
            let obj = match obj.as_object() {
                Some(o) => o,
                None => return false,
            };
            if obj == needle {
                return true;
            }
        } else if fp == ap_tostring as *const () {
            // join calls toString which calls join which calls toString, etc
        } else {
            return false;
        }
        top -= 1;
    }
    false
}

fn ap_join(st: &mut State) -> R<()> {
    if ap_join_cycle(st) {
        return st.push_literal("");
    }

    let len = st.getlength(0)?;

    let sep = if st.isdefined(1) {
        st.tostring(1)?.to_string()
    } else {
        ",".to_string()
    };

    if len <= 0 {
        return st.push_literal("");
    }

    // estimate: average element ~10 chars + separator
    let mut out = String::with_capacity(len as usize * (sep.len() + 10));
    for k in 0..len {
        st.getindex(0, k)?;
        if st.iscoercible(-1) {
            let r = st.tostring(-1)?;
            if k > 0 {
                out.push_str(&sep);
            }
            out.push_str(&r);
        } else if k > 0 {
            out.push_str(&sep);
        }
        st.pop(1);
    }

    st.push_string(&out)
}

fn ap_pop(st: &mut State) -> R<()> {
    let n = st.getlength(0)?;

    if n > 0 {
        st.getindex(0, n - 1)?;
        st.delindex(0, n - 1)?;
        st.setlength(0, n - 1)?;
    } else {
        st.setlength(0, 0)?;
        st.push_undefined()?;
    }
    Ok(())
}

fn ap_push(st: &mut State) -> R<()> {
    let top = st.gettop();
    let mut n = st.getlength(0)?;

    for i in 1..top {
        st.copy(i)?;
        st.setindex(0, n)?;
        n += 1;
    }

    st.setlength(0, n)?;

    st.push_number(n as f64)
}

fn ap_reverse(st: &mut State) -> R<()> {
    let len = st.getlength(0)?;
    let middle = len / 2;
    let mut lower = 0;

    while lower != middle {
        let upper = len - lower - 1;
        let haslower = st.hasindex(0, lower)?;
        let hasupper = st.hasindex(0, upper)?;
        if haslower && hasupper {
            st.setindex(0, lower)?;
            st.setindex(0, upper)?;
        } else if hasupper {
            st.setindex(0, lower)?;
            st.delindex(0, upper)?;
        } else if haslower {
            st.setindex(0, upper)?;
            st.delindex(0, lower)?;
        }
        lower += 1;
    }

    st.copy(0)
}

fn ap_shift(st: &mut State) -> R<()> {
    let len = st.getlength(0)?;

    if len == 0 {
        st.setlength(0, 0)?;
        st.push_undefined()?;
        return Ok(());
    }

    st.getindex(0, 0)?;

    for k in 1..len {
        if st.hasindex(0, k)? {
            st.setindex(0, k - 1)?;
        } else {
            st.delindex(0, k - 1)?;
        }
    }

    st.delindex(0, len - 1)?;
    st.setlength(0, len - 1)?;
    Ok(())
}

fn ap_slice(st: &mut State) -> R<()> {
    st.newarray()?;

    let len = st.getlength(0)?;
    let mut sv = st.tointeger(1)? as f64;
    let mut ev = if st.isdefined(2) {
        st.tointeger(2)? as f64
    } else {
        len as f64
    };

    if sv < 0.0 {
        sv += len as f64;
    }
    if ev < 0.0 {
        ev += len as f64;
    }

    let mut s = if sv < 0.0 {
        0
    } else if sv > len as f64 {
        len
    } else {
        sv as i32
    };
    let e = if ev < 0.0 {
        0
    } else if ev > len as f64 {
        len
    } else {
        ev as i32
    };

    let mut n = 0;
    while s < e {
        if st.hasindex(0, s)? {
            st.setindex(-2, n)?;
        }
        s += 1;
        n += 1;
    }
    Ok(())
}

fn ap_sort_cmp(st: &mut State, idx_a: i32, idx_b: i32) -> R<i32> {
    let obj = st.stackidx(0).as_object().expect("array this");
    let (simple, flat_len) = match &st.heap.obj(obj).payload {
        Payload::Array(a) => (a.simple, a.flat.len()),
        _ => (false, 0),
    };
    if simple && idx_b >= 0 && (idx_b as usize) < flat_len {
        let (val_a, val_b) = match &st.heap.obj(obj).payload {
            Payload::Array(a) => (
                a.flat[idx_a as usize].clone(),
                a.flat[idx_b as usize].clone(),
            ),
            _ => unreachable!(),
        };
        let und_a = val_a.is_undefined();
        let und_b = val_b.is_undefined();
        if und_a {
            return Ok(und_b as i32);
        }
        if und_b {
            return Ok(-1);
        }
        if st.iscallable(1) {
            st.copy(1)?; // copy function
            st.push_undefined()?; // no 'this' binding
            st.push_value(val_a)?;
            st.push_value(val_b)?;
            st.call(2)?;
            let v = st.tonumber(-1)?;
            st.pop(1);
            if v.is_nan() || v == 0.0 {
                return Ok(0);
            }
            Ok(if v < 0.0 { -1 } else { 1 })
        } else {
            st.push_value(val_a)?;
            st.push_value(val_b)?;
            let str_a = st.tostring(-2)?;
            let str_b = st.tostring(-1)?;
            let c = str_a.as_bytes().cmp(str_b.as_bytes());
            st.pop(2);
            Ok(c as i32)
        }
    } else {
        let has_a = st.hasindex(0, idx_a)?;
        let has_b = st.hasindex(0, idx_b)?;
        if !has_a && !has_b {
            return Ok(0);
        }
        if has_a && !has_b {
            st.pop(1);
            return Ok(-1);
        }
        if !has_a && has_b {
            st.pop(1);
            return Ok(1);
        }

        let und_a = st.isundefined(-2);
        let und_b = st.isundefined(-1);
        if und_a {
            st.pop(2);
            return Ok(und_b as i32);
        }
        if und_b {
            st.pop(2);
            return Ok(-1);
        }

        if st.iscallable(1) {
            st.copy(1)?; // copy function
            st.push_undefined()?; // no 'this' binding
            st.copy(-4)?;
            st.copy(-4)?;
            st.call(2)?;
            let v = st.tonumber(-1)?;
            st.pop(3);
            if v.is_nan() || v == 0.0 {
                return Ok(0);
            }
            Ok(if v < 0.0 { -1 } else { 1 })
        } else {
            let str_a = st.tostring(-2)?;
            let str_b = st.tostring(-1)?;
            let c = str_a.as_bytes().cmp(str_b.as_bytes());
            st.pop(2);
            Ok(c as i32)
        }
    }
}

fn ap_sort_swap(st: &mut State, idx_a: i32, idx_b: i32) -> R<()> {
    let obj = st.stackidx(0).as_object().expect("array this");
    let (simple, flat_len) = match &st.heap.obj(obj).payload {
        Payload::Array(a) => (a.simple, a.flat.len()),
        _ => (false, 0),
    };
    if simple && idx_b >= 0 && (idx_b as usize) < flat_len {
        if let Payload::Array(a) = &mut st.heap.obj_mut(obj).payload {
            a.flat.swap(idx_a as usize, idx_b as usize);
        }
        Ok(())
    } else {
        let has_a = st.hasindex(0, idx_a)?;
        let has_b = st.hasindex(0, idx_b)?;
        if has_a && has_b {
            st.setindex(0, idx_a)?;
            st.setindex(0, idx_b)?;
        } else if has_a && !has_b {
            st.delindex(0, idx_a)?;
            st.setindex(0, idx_b)?;
        } else if !has_a && has_b {
            st.delindex(0, idx_b)?;
            st.setindex(0, idx_a)?;
        }
        Ok(())
    }
}

// A bottom-up/bouncing heapsort implementation

fn ap_sort_leaf(st: &mut State, i: i32, end: i32) -> R<i32> {
    let mut j = i;
    let mut lc = (j << 1) + 1; // left child
    let mut rc = (j << 1) + 2; // right child
    while rc < end {
        if ap_sort_cmp(st, lc, rc)? <= 0 {
            j = rc;
        } else {
            j = lc;
        }
        lc = (j << 1) + 1;
        rc = (j << 1) + 2;
    }
    if lc < end {
        j = lc;
    }
    Ok(j)
}

fn ap_sort_sift(st: &mut State, i: i32, end: i32) -> R<()> {
    let mut j = ap_sort_leaf(st, i, end)?;
    while j > i && ap_sort_cmp(st, i, j)? > 0 {
        j = (j - 1) >> 1; // parent
    }
    while j > i {
        ap_sort_swap(st, i, j)?;
        j = (j - 1) >> 1; // parent
    }
    Ok(())
}

fn ap_sort_heapsort(st: &mut State, n: i32) -> R<()> {
    let mut i = n / 2 - 1;
    while i >= 0 {
        ap_sort_sift(st, i, n)?;
        i -= 1;
    }
    let mut i = n - 1;
    while i > 0 {
        ap_sort_swap(st, 0, i)?;
        ap_sort_sift(st, 0, i)?;
        i -= 1;
    }
    Ok(())
}

fn ap_sort(st: &mut State) -> R<()> {
    let len = st.getlength(0)?;
    if len <= 1 {
        return st.copy(0);
    }

    if !st.iscallable(1) && !st.isundefined(1) {
        return st.type_error("comparison function must be a function or undefined");
    }

    if len == i32::MAX {
        return st.range_error("array is too large to sort");
    }

    ap_sort_heapsort(st, len)?;

    st.copy(0)
}

fn ap_splice(st: &mut State) -> R<()> {
    let top = st.gettop();

    let len = st.getlength(0)?;
    let mut start = st.tointeger(1)?;
    if start < 0 {
        start = if len + start > 0 { len + start } else { 0 };
    } else if start > len {
        start = len;
    }

    let mut del = if st.isdefined(2) {
        st.tointeger(2)?
    } else {
        len - start
    };
    if del > len - start {
        del = len - start;
    }
    if del < 0 {
        del = 0;
    }

    st.newarray()?;

    // copy deleted items to return array
    for k in 0..del {
        if st.hasindex(0, start + k)? {
            st.setindex(-2, k)?;
        }
    }
    st.setlength(-1, del)?;

    // shift the tail to resize the hole left by deleted items
    let add = top - 3;
    if add < del {
        for k in start..(len - del) {
            if st.hasindex(0, k + del)? {
                st.setindex(0, k + add)?;
            } else {
                st.delindex(0, k + add)?;
            }
        }
        let mut k = len;
        while k > len - del + add {
            st.delindex(0, k - 1)?;
            k -= 1;
        }
    } else if add > del {
        let mut k = len - del;
        while k > start {
            if st.hasindex(0, k + del - 1)? {
                st.setindex(0, k + add - 1)?;
            } else {
                st.delindex(0, k + add - 1)?;
            }
            k -= 1;
        }
    }

    // copy new items into the hole
    for k in 0..add {
        st.copy(3 + k)?;
        st.setindex(0, start + k)?;
    }

    st.setlength(0, len - del + add)?;
    Ok(())
}

fn ap_unshift(st: &mut State) -> R<()> {
    let top = st.gettop();

    let len = st.getlength(0)?;

    let mut k = len;
    while k > 0 {
        let from = k - 1;
        let to = k + top - 2;
        if st.hasindex(0, from)? {
            st.setindex(0, to)?;
        } else {
            st.delindex(0, to)?;
        }
        k -= 1;
    }

    for i in 1..top {
        st.copy(i)?;
        st.setindex(0, i - 1)?;
    }

    st.setlength(0, len.saturating_add(top.saturating_sub(1)))?;

    st.push_number((len.saturating_add(top.saturating_sub(1))) as f64)
}

fn ap_tostring(st: &mut State) -> R<()> {
    if !st.iscoercible(0) {
        return st.type_error("'this' is not an object");
    }
    st.getproperty(0, "join")?;
    if !st.iscallable(-1) {
        st.pop(1);
        // TODO: call Object.prototype.toString implementation directly
        st.getglobal("Object")?;
        st.getproperty(-1, "prototype")?;
        st.rot2pop1();
        st.getproperty(-1, "toString")?;
        st.rot2pop1();
    }
    st.copy(0)?;
    st.call(0)
}

fn ap_indexof(st: &mut State) -> R<()> {
    let len = st.getlength(0)?.max(0);
    let mut from = if st.isdefined(2) { st.tointeger(2)? } else { 0 };
    if from < 0 {
        from = (from as i64 + len as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    }
    if from < 0 {
        from = 0;
    }

    st.copy(1)?;
    for k in from..len {
        if st.hasindex(0, k)? {
            if st.strictequal()? {
                return st.push_number(k as f64);
            }
            st.pop(1);
        }
    }

    st.push_number(-1.0)
}

fn ap_lastindexof(st: &mut State) -> R<()> {
    let len = st.getlength(0)?.max(0);
    let mut from = if st.isdefined(2) {
        st.tointeger(2)?
    } else {
        len - 1
    };
    if from > len - 1 {
        from = len - 1;
    }
    if from < 0 {
        from = (from as i64 + len as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    }

    st.copy(1)?;
    let mut k = from;
    while k >= 0 {
        if st.hasindex(0, k)? {
            if st.strictequal()? {
                return st.push_number(k as f64);
            }
            st.pop(1);
        }
        k -= 1;
    }

    st.push_number(-1.0)
}

fn ap_every(st: &mut State) -> R<()> {
    let hasthis = st.gettop() >= 3;

    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }

    let len = st.getlength(0)?;
    for k in 0..len {
        if st.hasindex(0, k)? {
            st.copy(1)?;
            if hasthis {
                st.copy(2)?;
            } else {
                st.push_undefined()?;
            }
            st.copy(-3)?;
            st.push_number(k as f64)?;
            st.copy(0)?;
            st.call(3)?;
            if !st.toboolean(-1) {
                return Ok(());
            }
            st.pop(2);
        }
    }

    st.push_boolean(true)
}

fn ap_some(st: &mut State) -> R<()> {
    let hasthis = st.gettop() >= 3;

    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }

    let len = st.getlength(0)?;
    for k in 0..len {
        if st.hasindex(0, k)? {
            st.copy(1)?;
            if hasthis {
                st.copy(2)?;
            } else {
                st.push_undefined()?;
            }
            st.copy(-3)?;
            st.push_number(k as f64)?;
            st.copy(0)?;
            st.call(3)?;
            if st.toboolean(-1) {
                return Ok(());
            }
            st.pop(2);
        }
    }

    st.push_boolean(false)
}

fn ap_foreach(st: &mut State) -> R<()> {
    let hasthis = st.gettop() >= 3;

    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }

    let len = st.getlength(0)?;
    for k in 0..len {
        if st.hasindex(0, k)? {
            st.copy(1)?;
            if hasthis {
                st.copy(2)?;
            } else {
                st.push_undefined()?;
            }
            st.copy(-3)?;
            st.push_number(k as f64)?;
            st.copy(0)?;
            st.call(3)?;
            st.pop(2);
        }
    }

    st.push_undefined()
}

fn ap_map(st: &mut State) -> R<()> {
    let hasthis = st.gettop() >= 3;

    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }

    st.newarray()?;

    let len = st.getlength(0)?;
    for k in 0..len {
        if st.hasindex(0, k)? {
            st.copy(1)?;
            if hasthis {
                st.copy(2)?;
            } else {
                st.push_undefined()?;
            }
            st.copy(-3)?;
            st.push_number(k as f64)?;
            st.copy(0)?;
            st.call(3)?;
            st.setindex(-3, k)?;
            st.pop(1);
        }
    }
    st.setlength(-1, len)?;
    Ok(())
}

fn ap_filter(st: &mut State) -> R<()> {
    let hasthis = st.gettop() >= 3;

    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }

    st.newarray()?;
    let mut to = 0;

    let len = st.getlength(0)?;
    for k in 0..len {
        if st.hasindex(0, k)? {
            st.copy(1)?;
            if hasthis {
                st.copy(2)?;
            } else {
                st.push_undefined()?;
            }
            st.copy(-3)?;
            st.push_number(k as f64)?;
            st.copy(0)?;
            st.call(3)?;
            if st.toboolean(-1) {
                st.pop(1);
                st.setindex(-2, to)?;
                to += 1;
            } else {
                st.pop(2);
            }
        }
    }
    Ok(())
}

fn ap_reduce(st: &mut State) -> R<()> {
    let hasinitial = st.gettop() >= 3;

    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }

    let len = st.getlength(0)?;
    let mut k = 0;

    if len == 0 && !hasinitial {
        return st.type_error("no initial value");
    }

    // initial value of accumulator
    if hasinitial {
        st.copy(2)?;
    } else {
        while k < len {
            let has = st.hasindex(0, k)?;
            k += 1;
            if has {
                break;
            }
        }
        if k == len {
            return st.type_error("no initial value");
        }
    }

    while k < len {
        if st.hasindex(0, k)? {
            st.copy(1)?;
            st.push_undefined()?;
            st.rot(4); // accumulator on top
            st.rot(4); // property on top
            st.push_number(k as f64)?;
            st.copy(0)?;
            st.call(4)?; // calculate new accumulator
        }
        k += 1;
    }

    // return accumulator
    Ok(())
}

fn ap_reduceright(st: &mut State) -> R<()> {
    let hasinitial = st.gettop() >= 3;

    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }

    let len = st.getlength(0)?;
    let mut k = len - 1;

    if len == 0 && !hasinitial {
        return st.type_error("no initial value");
    }

    // initial value of accumulator
    if hasinitial {
        st.copy(2)?;
    } else {
        while k >= 0 {
            let has = st.hasindex(0, k)?;
            k -= 1;
            if has {
                break;
            }
        }
        if k < 0 {
            return st.type_error("no initial value");
        }
    }

    while k >= 0 {
        if st.hasindex(0, k)? {
            st.copy(1)?;
            st.push_undefined()?;
            st.rot(4); // accumulator on top
            st.rot(4); // property on top
            st.push_number(k as f64)?;
            st.copy(0)?;
            st.call(4)?; // calculate new accumulator
        }
        k -= 1;
    }

    // return accumulator
    Ok(())
}

fn a_isarray(st: &mut State) -> R<()> {
    if st.isobject(1) {
        let t = st.toobject(1)?;
        let b = st.heap.obj(t).class == Class::Array;
        st.push_boolean(b)
    } else {
        st.push_boolean(false)
    }
}

/// Array.prototype.fill(value, start, end) — ES6.
fn ap_fill(st: &mut State) -> R<()> {
    let len = st.getlength(0)?;
    if len == 0 {
        return st.copy(0);
    }
    let value = st.stackidx(1).clone();
    let mut k = if st.isdefined(2) {
        let s = st.tointeger(2)?;
        if s < 0 { (s + len).max(0) } else { s.min(len) }
    } else {
        0
    };
    let end = if st.isdefined(3) {
        let e = st.tointeger(3)?;
        if e < 0 { (e + len).max(0) } else { e.min(len) }
    } else {
        len
    };
    while k < end {
        st.push_value(value.clone())?;
        st.setindex(0, k)?;
        k += 1;
    }
    st.copy(0)
}

/// Array.prototype.find(predicate[, thisArg]) — ES6.
fn ap_find(st: &mut State) -> R<()> {
    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }
    let hasthis = st.gettop() >= 3;
    let len = st.getlength(0)?;
    for k in 0..len {
        if st.hasindex(0, k)? {
            st.copy(1)?; // callback
            if hasthis { st.copy(2)?; } else { st.push_undefined()?; } // thisArg
            st.copy(-3)?; // element
            st.push_number(k as f64)?; // index
            st.copy(0)?; // array
            st.call(3)?;
            if st.toboolean(-1) {
                st.pop(1); // pop result
                return st.getindex(0, k);
            }
            st.pop(2); // pop result + element
        }
    }
    st.push_undefined()
}

/// Array.prototype.findIndex(predicate[, thisArg]) — ES6.
fn ap_findindex(st: &mut State) -> R<()> {
    if !st.iscallable(1) {
        return st.type_error("callback is not a function");
    }
    let hasthis = st.gettop() >= 3;
    let len = st.getlength(0)?;
    for k in 0..len {
        if st.hasindex(0, k)? {
            st.copy(1)?;
            if hasthis { st.copy(2)?; } else { st.push_undefined()?; }
            st.copy(-3)?;
            st.push_number(k as f64)?;
            st.copy(0)?;
            st.call(3)?;
            if st.toboolean(-1) {
                st.pop(1);
                return st.push_number(k as f64);
            }
            st.pop(2);
        }
    }
    st.push_number(-1_f64)
}

/// Array.from(arrayLike, mapFn, thisArg) — ES6.
fn a_from(st: &mut State) -> R<()> {
    if !st.isdefined(1) || st.isundefined(1) || st.isnull(1) {
        st.newarray()?;
        return Ok(());
    }
    let has_mapfn = st.iscallable(2);
    let _this_arg = if has_mapfn && st.isdefined(3) { Some(st.stackidx(3).clone()) } else { None };

    // Try iterable path via @@iterator
    let src = st.toobject(1)?;
    if st.heap.get_property(src, "@@iterator").is_some() {
        st.getproperty(1, "@@iterator")?;
        st.copy(1)?;
        st.call(0)?;
        let iter_val = st.stackidx(-1).clone();
        st.pop(1);
        if let Value::Object(iter_obj) = iter_val {
            let mut result: Vec<Value> = Vec::new();
            loop {
                // Call iter.next()
                st.push_object(iter_obj)?;
                st.getproperty(-1, "next")?;
                st.copy(-2)?;
                st.call(0)?;
                let next_val = st.stackidx(-1).clone();
                let is_done = if let Value::Object(ro) = &next_val {
                    st.heap.get_property(*ro, "done")
                        .is_some_and(|p| matches!(&p.value, Value::Boolean(true)))
                } else { true };
                let val = if let Value::Object(ro) = &next_val {
                    st.heap.get_property(*ro, "value").map(|p| p.value.clone())
                        .unwrap_or(Value::Undefined)
                } else { Value::Undefined };
                st.pop(1);
                if is_done { break; }
                let v = if has_mapfn {
                    st.push_value(_this_arg.clone().unwrap_or(Value::Undefined))?;
                    st.push_value(val)?;
                    st.copy(2)?;
                    st.call(1)?;
                    let r = st.stackidx(-1).clone();
                    st.pop(1);
                    r
                } else { val };
                result.push(v);
            }
            st.newarray()?;
            let arr_ref = st.stackidx(-1).clone();
            if let Value::Object(a) = arr_ref {
                st.heap.obj_mut(a).payload = Payload::Array(crate::object::ArrayData {
                    length: result.len() as i32,
                    simple: true,
                    flat: result.into(),
                });
            }
            return Ok(());
        }
    }

    // Array-like path: extract values first to avoid borrow conflicts
    let flat: Vec<Value> = match &st.heap.obj(src).payload {
        Payload::Array(a) => a.flat.to_vec(),
        _ => {
            let len = st.heap.get_property(src, "length")
                .and_then(|p| match &p.value { Value::Number(n) => Some(*n as usize), _ => None })
                .unwrap_or(0);
            let mut v = Vec::with_capacity(len);
            for i in 0..len {
                let val = st.heap.get_property(src, &i.to_string())
                    .map(|p| p.value.clone())
                    .unwrap_or(Value::Undefined);
                v.push(val);
            }
            v
        }
    };

    let mut result: Vec<Value> = Vec::with_capacity(flat.len());
    for val in flat {
        if has_mapfn {
            st.push_value(_this_arg.clone().unwrap_or(Value::Undefined))?;
            st.push_value(val)?;
            st.copy(2)?;
            st.call(1)?;
            let r = st.stackidx(-1).clone();
            st.pop(1);
            result.push(r);
        } else {
            result.push(val);
        }
    }
    st.newarray()?;
    let arr_ref = st.stackidx(-1).clone();
    if let Value::Object(a) = arr_ref {
        st.heap.obj_mut(a).payload = Payload::Array(crate::object::ArrayData {
            length: result.len() as i32,
            simple: true,
            flat: result.into(),
        });
    }
    Ok(())
}

fn ap_values(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    let values = match &st.heap.obj(obj).payload {
        Payload::Array(a) => a.flat.to_vec(),
        _ => vec![],
    };
    make_es6_iterator(st, values)
}

fn ap_entries(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    let flat = match &st.heap.obj(obj).payload {
        Payload::Array(a) => a.flat.to_vec(),
        _ => vec![],
    };
    let values: Vec<Value> = flat
        .into_iter()
        .enumerate()
        .map(|(i, val)| {
            let pa = st.heap.alloc_object(Class::Array, Some(st.protos.array));
            st.heap.obj_mut(pa).payload = Payload::Array(crate::object::ArrayData {
                length: 2,
                simple: true,
                flat: vec![Value::Number(i as f64), val].into(),
            });
            Value::Object(pa)
        })
        .collect();
    make_es6_iterator(st, values)
}

fn ap_keys(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    let len = match &st.heap.obj(obj).payload {
        Payload::Array(a) => a.flat.len(),
        _ => 0,
    };
    let values: Vec<Value> = (0..len).map(|i| Value::Number(i as f64)).collect();
    make_es6_iterator(st, values)
}

pub fn init(st: &mut State) {
    let proto = st.protos.array;
    st.push_object(proto).unwrap();
    {
        propf(st, "Array.prototype.toString", ap_tostring, 0).unwrap();
        propf(st, "Array.prototype.concat", ap_concat, 0).unwrap();
        propf(st, "Array.prototype.join", ap_join, 1).unwrap();
        propf(st, "Array.prototype.pop", ap_pop, 0).unwrap();
        propf(st, "Array.prototype.push", ap_push, 0).unwrap();
        propf(st, "Array.prototype.reverse", ap_reverse, 0).unwrap();
        propf(st, "Array.prototype.shift", ap_shift, 0).unwrap();
        propf(st, "Array.prototype.slice", ap_slice, 2).unwrap();
        propf(st, "Array.prototype.sort", ap_sort, 1).unwrap();
        propf(st, "Array.prototype.splice", ap_splice, 2).unwrap();
        propf(st, "Array.prototype.unshift", ap_unshift, 0).unwrap();

        // ES5
        propf(st, "Array.prototype.indexOf", ap_indexof, 1).unwrap();
        propf(st, "Array.prototype.lastIndexOf", ap_lastindexof, 1).unwrap();
        propf(st, "Array.prototype.every", ap_every, 1).unwrap();
        propf(st, "Array.prototype.some", ap_some, 1).unwrap();
        propf(st, "Array.prototype.forEach", ap_foreach, 1).unwrap();
        propf(st, "Array.prototype.map", ap_map, 1).unwrap();
        propf(st, "Array.prototype.filter", ap_filter, 1).unwrap();
        propf(st, "Array.prototype.reduce", ap_reduce, 1).unwrap();
        propf(st, "Array.prototype.reduceRight", ap_reduceright, 1).unwrap();

        // ES6+
        propf(st, "Array.prototype.fill", ap_fill, 1).unwrap();
        propf(st, "Array.prototype.find", ap_find, 1).unwrap();
        propf(st, "Array.prototype.findIndex", ap_findindex, 1).unwrap();
        // iterators
        propf(st, "Array.prototype.values", ap_values, 0).unwrap();
        propf(st, "Array.prototype.entries", ap_entries, 0).unwrap();
        propf(st, "Array.prototype.keys", ap_keys, 0).unwrap();
        // @@iterator = values
        propf(st, "Array.prototype.@@iterator", ap_values, 0).unwrap();
    }
    st.newcconstructor(jsb_new_array, jsb_new_array, "Array", 0).unwrap();
    {
        // ES5
        propf(st, "Array.isArray", a_isarray, 1).unwrap();
        // ES6
        propf(st, "Array.from", a_from, 1).unwrap();
    }
    st.defglobal("Array", JS_DONTENUM).unwrap();
}
