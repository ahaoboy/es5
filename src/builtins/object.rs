//! Object constructor and Object.prototype (jsobject.c).

use super::{propf, R};
use crate::object::{Class, ObjRef, Payload};
use crate::state::{is_array_index, State};
use crate::value::{Value, JS_DONTCONF, JS_DONTENUM, JS_READONLY};

fn jsb_new_object(st: &mut State) -> R<()> {
    if st.isundefined(1) || st.isnull(1) {
        st.newobject()
    } else {
        let o = st.toobject(1)?;
        st.push_object(o)
    }
}

fn jsb_object(st: &mut State) -> R<()> {
    if st.isundefined(1) || st.isnull(1) {
        st.newobject()
    } else {
        let o = st.toobject(1)?;
        st.push_object(o)
    }
}

fn op_tostring(st: &mut State) -> R<()> {
    if st.isundefined(0) {
        return st.push_literal("[object Undefined]");
    }
    if st.isnull(0) {
        return st.push_literal("[object Null]");
    }
    let obj = st.toobject(0)?;
    let s = match st.heap.obj(obj).class {
        Class::Object => "[object Object]",
        Class::Array => "[object Array]",
        Class::Function => "[object Function]",
        Class::Script => "[object Function]",
        Class::CFunction => "[object Function]",
        Class::Error => "[object Error]",
        Class::Boolean => "[object Boolean]",
        Class::Number => "[object Number]",
        Class::String => "[object String]",
        Class::Regexp => "[object RegExp]",
        Class::Date => "[object Date]",
        Class::Math => "[object Math]",
        Class::Json => "[object JSON]",
        Class::Arguments => "[object Arguments]",
        Class::Iterator => "[object Iterator]",
        Class::Symbol => "[object Symbol]",
        Class::Map => "[object Map]",
        Class::WeakMap => "[object WeakMap]",
        Class::Set => "[object Set]",
    };
    st.push_literal(s)
}

fn op_valueof(st: &mut State) -> R<()> {
    st.copy(0)
}

fn op_hasownproperty(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    let name = st.tostring(1)?;

    if st.heap.obj(obj).class == Class::String
        && let Some(k) = is_array_index(&name) {
            let slen = match &st.heap.obj(obj).payload {
                Payload::String(s) => s.length,
                _ => 0,
            };
            if k >= 0 && k < slen {
                return st.push_boolean(true);
            }
        }

    if st.heap.obj(obj).class == Class::Array
        && let Payload::Array(a) = &st.heap.obj(obj).payload
            && a.simple
                && let Some(k) = is_array_index(&name)
                    && k >= 0 && (k as usize) < a.flat.len() {
                        return st.push_boolean(true);
                    }

    let found = st.heap.get_own_property(obj, &name).is_some();
    st.push_boolean(found)
}

fn op_isprototypeof(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    if st.isobject(1) {
        let mut v = st.toobject(1)?;
        while let Some(p) = st.heap.obj(v).prototype {
            v = p;
            if v == obj {
                return st.push_boolean(true);
            }
        }
    }
    st.push_boolean(false)
}

fn op_propertyisenumerable(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    let name = st.tostring(1)?;
    let b = match st.heap.get_own_property(obj, &name) {
        Some(p) => p.atts & JS_DONTENUM == 0,
        None => false,
    };
    st.push_boolean(b)
}

fn o_getprototypeof(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    match st.heap.obj(obj).prototype {
        Some(p) => st.push_object(p),
        None => st.push_null(),
    }
}

/// Object.setPrototypeOf(obj, prototype) — ES6.
fn o_setprototypeof(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("Object.setPrototypeOf: not an object");
    }
    let obj = st.toobject(1)?;
    if st.isnull(2) || st.isundefined(2) {
        st.heap.obj_mut(obj).prototype = None;
    } else if st.isobject(2) {
        let proto = st.toobject(2)?;
        st.heap.obj_mut(obj).prototype = Some(proto);
    } else {
        return st.type_error("Object.setPrototypeOf: prototype must be an object or null");
    }
    st.copy(1)
}

/// Object.assign(target, ...sources): copy enumerable own properties.
fn o_assign(st: &mut State) -> R<()> {
    let top = st.gettop();
    if top < 2 {
        return st.type_error("Object.assign requires at least 2 arguments");
    }
    if !st.isobject(1) {
        return st.type_error("Object.assign target is not an object");
    }
    let target = st.toobject(1)?;
    for i in 2..top {
        if !st.iscoercible(i) {
            continue;
        }
        let src = st.toobject(i)?;
        let keys = st.heap.ordered_own_keys(src, true); // enumerable only
        for key in keys {
            let (getter, value) = {
                let prop = st.heap.get_own_property(src, &key).expect("key vanished");
                (prop.getter, prop.value.clone())
            };
            if let Some(getter) = getter {
                st.push_object(getter)?;
                st.push_object(src)?;
                st.call(0)?;
                st.set_property(target, &key, false)?;
                st.pop(1);
            } else {
                st.push_value(value)?;
                st.set_property(target, &key, false)?;
                st.pop(1);
            }
        }
    }
    st.push_object(target)
}

/// Build a data-descriptor object {value, writable, enumerable, configurable}.
fn data_descriptor(st: &mut State, v: Value, w: bool, e: bool, c: bool) -> R<()> {
    st.newobject()?;
    st.push_value(v)?;
    st.defproperty(-2, "value", 0)?;
    st.push_boolean(w)?;
    st.defproperty(-2, "writable", 0)?;
    st.push_boolean(e)?;
    st.defproperty(-2, "enumerable", 0)?;
    st.push_boolean(c)?;
    st.defproperty(-2, "configurable", 0)?;
    Ok(())
}

fn o_getownpropertydescriptor(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    let name = st.tostring(2)?;
    let class = st.heap.obj(obj).class;

    // virtual properties first (array/string indices and lengths, regexp)
    match class {
        Class::Array => {
            if name.as_ref() as &str == "length" {
                let len = match &st.heap.obj(obj).payload {
                    Payload::Array(a) => a.length,
                    _ => 0,
                };
                return data_descriptor(st, Value::Number(len as f64), true, false, false);
            }
            if let Some(k) = is_array_index(&name)
                && let Payload::Array(a) = &st.heap.obj(obj).payload
                    && a.simple && k >= 0 && (k as usize) < a.flat.len() {
                        let v = a.flat[k as usize].clone();
                        return data_descriptor(st, v, true, true, true);
                    }
        }
        Class::String => {
            let (s, slen) = match &st.heap.obj(obj).payload {
                Payload::String(sd) => (sd.string.clone(), sd.length),
                _ => (st.heap.intern(""), 0),
            };
            if name.as_ref() as &str == "length" {
                return data_descriptor(st, Value::Number(slen as f64), false, false, false);
            }
            if let Some(k) = is_array_index(&name)
                && k >= 0 && k < slen {
                    let rune = crate::utf::runeat(&s, k as usize).unwrap_or(0xFFFD);
                    let mut ch = String::new();
                    crate::utf::push_rune(&mut ch, rune);
                    let ch = compact_str::CompactString::new(&ch);
                    return data_descriptor(st, Value::String(ch), false, true, false);
                }
        }
        Class::Regexp => {
            let matched = matches!(
                name.as_ref(),
                "source" | "global" | "ignoreCase" | "multiline" | "lastIndex"
            );
            if matched {
                let (source, flags, last) = match &st.heap.obj(obj).payload {
                    Payload::Regexp(r) => (r.source.clone(), r.flags, r.last),
                    _ => unreachable!(),
                };
                let (v, w): (Value, bool) = match name.as_ref() {
                    "source" => (Value::String(source), false),
                    "global" => (Value::Boolean(flags & crate::value::JS_REGEXP_G != 0), false),
                    "ignoreCase" => (Value::Boolean(flags & crate::value::JS_REGEXP_I != 0), false),
                    "multiline" => (Value::Boolean(flags & crate::value::JS_REGEXP_M != 0), false),
                    "lastIndex" => (Value::Number(last as f64), true),
                    _ => unreachable!(),
                };
                return data_descriptor(st, v, w, false, false);
            }
        }
        _ => {}
    }

    let prop = st.heap.get_own_property(obj, &name).cloned();
    match prop {
        None => st.push_undefined(),
        Some(p) => {
            st.newobject()?;
            if p.getter.is_none() && p.setter.is_none() {
                st.push_value(p.value)?;
                st.defproperty(-2, "value", 0)?;
                st.push_boolean(p.atts & JS_READONLY == 0)?;
                st.defproperty(-2, "writable", 0)?;
            } else {
                match p.getter {
                    Some(g) => st.push_object(g)?,
                    None => st.push_undefined()?,
                }
                st.defproperty(-2, "get", 0)?;
                match p.setter {
                    Some(s) => st.push_object(s)?,
                    None => st.push_undefined()?,
                }
                st.defproperty(-2, "set", 0)?;
            }
            st.push_boolean(p.atts & JS_DONTENUM == 0)?;
            st.defproperty(-2, "enumerable", 0)?;
            st.push_boolean(p.atts & JS_DONTCONF == 0)?;
            st.defproperty(-2, "configurable", 0)?;
            Ok(())
        }
    }
}

fn o_getownpropertynames(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;

    st.newarray()?;

    // ES5 ownKeys order: integer indices ascending, then named properties
    // in insertion order, then the virtual "length"-like properties
    let mut i = 0;
    if st.heap.obj(obj).class == Class::Array
        && let Payload::Array(a) = &st.heap.obj(obj).payload
            && a.simple {
                for k in 0..a.flat.len() {
                    let name = k.to_string();
                    st.push_string(&name)?;
                    st.setindex(-2, i)?;
                    i += 1;
                }
            }

    if st.heap.obj(obj).class == Class::String {
        let slen = match &st.heap.obj(obj).payload {
            Payload::String(s) => s.length,
            _ => 0,
        };
        for k in 0..slen {
            let name = k.to_string();
            st.push_string(&name)?;
            st.setindex(-2, i)?;
            i += 1;
        }
    }

    let keys = st.heap.ordered_own_keys(obj, false);
    for name in keys {
        // skip duplicate index keys already emitted for the flat array part
        if let Some(k) = is_array_index(&name) {
            if st.heap.obj(obj).class == Class::Array
                && let Payload::Array(a) = &st.heap.obj(obj).payload
                    && a.simple && (k as usize) < a.flat.len() {
                        continue;
                    }
            if st.heap.obj(obj).class == Class::String {
                continue;
            }
        }
        st.push_string_rc(name)?;
        st.setindex(-2, i)?;
        i += 1;
    }

    if st.heap.obj(obj).class == Class::Array {
        st.push_literal("length")?;
        st.setindex(-2, i)?;
        i += 1;
    }

    if st.heap.obj(obj).class == Class::String {
        st.push_literal("length")?;
        st.setindex(-2, i)?;
        i += 1;
    }

    if st.heap.obj(obj).class == Class::Regexp {
        for name in ["source", "global", "ignoreCase", "multiline", "lastIndex"] {
            st.push_literal(name)?;
            st.setindex(-2, i)?;
            i += 1;
        }
    }
    Ok(())
}

/// ToPropertyDescriptor (jsobject.c).
fn to_property_descriptor(st: &mut State, obj: ObjRef, name: &str, desc: ObjRef) -> R<()> {
    let mut haswritable = false;
    let mut hasvalue = false;
    let mut enumerable = false;
    let mut configurable = false;
    let mut writable = false;
    let mut atts = 0;

    st.push_object(obj)?;
    st.push_object(desc)?;

    if st.hasproperty(-1, "writable")? {
        haswritable = true;
        writable = st.toboolean(-1);
        st.pop(1);
    }
    if st.hasproperty(-1, "enumerable")? {
        enumerable = st.toboolean(-1);
        st.pop(1);
    }
    if st.hasproperty(-1, "configurable")? {
        configurable = st.toboolean(-1);
        st.pop(1);
    }
    if st.hasproperty(-1, "value")? {
        hasvalue = true;
        st.defproperty(-3, name, 0)?;
    }

    if !writable {
        atts |= JS_READONLY;
    }
    if !enumerable {
        atts |= JS_DONTENUM;
    }
    if !configurable {
        atts |= JS_DONTCONF;
    }

    if st.hasproperty(-1, "get")? {
        if haswritable || hasvalue {
            return st.type_error("value/writable and get/set attributes are exclusive");
        }
    } else {
        st.push_undefined()?;
    }

    if st.hasproperty(-2, "set")? {
        if haswritable || hasvalue {
            return st.type_error("value/writable and get/set attributes are exclusive");
        }
    } else {
        st.push_undefined()?;
    }

    st.defaccessor(-4, name, atts)?;

    st.pop(2);
    Ok(())
}

fn o_defineproperty(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    if !st.isobject(3) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    let name = st.tostring(2)?;
    let desc = st.toobject(3)?;
    to_property_descriptor(st, obj, &name, desc)?;
    st.copy(1)?;
    Ok(())
}

fn o_defineproperties_imp(st: &mut State, obj: ObjRef) -> R<()> {
    if !st.isobject(2) {
        return st.type_error("not an object");
    }

    let props = st.toobject(2)?;
    let mut keys = Vec::new();
    for name in st.heap.ordered_own_keys(props, true) {
        let p = st.heap.get_own_property(props, &name).expect("own key");
        if !p.value.is_object() {
            return st.type_error("not an object");
        }
        keys.push(name);
    }

    for name in keys {
        let has = st.has_property(props, &name)?;
        if has {
            let desc = st.toobject(-1)?;
            st.pop(1);
            to_property_descriptor(st, obj, &name, desc)?;
        } else {
            st.pop(1);
        }
    }
    Ok(())
}

fn o_defineproperties(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    o_defineproperties_imp(st, obj)?;
    st.copy(1)?;
    Ok(())
}

fn o_create(st: &mut State) -> R<()> {

    let proto: Option<ObjRef> = if st.isobject(1) {
        Some(st.toobject(1)?)
    } else if st.isnull(1) {
        None
    } else {
        return st.type_error("not an object or null");
    };

    let obj = st.heap.alloc_object(Class::Object, proto);
    st.push_object(obj)?;

    if st.isdefined(2) {
        o_defineproperties_imp(st, obj)?;
    }
    Ok(())
}

fn o_keys(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;

    st.newarray()?;

    let keys = st.heap.ordered_own_keys(obj, true);
    let mut i = 0;
    for name in keys {
        st.push_string_rc(name)?;
        st.setindex(-2, i)?;
        i += 1;
    }

    if st.heap.obj(obj).class == Class::String {
        let slen = match &st.heap.obj(obj).payload {
            Payload::String(s) => s.length,
            _ => 0,
        };
        for k in 0..slen {
            let name = k.to_string();
            st.push_string(&name)?;
            st.setindex(-2, i)?;
            i += 1;
        }
    }

    if st.heap.obj(obj).class == Class::Array
        && let Payload::Array(a) = &st.heap.obj(obj).payload
            && a.simple {
                for k in 0..a.flat.len() {
                    let name = k.to_string();
                    st.push_string(&name)?;
                    st.setindex(-2, i)?;
                    i += 1;
                }
            }
    Ok(())
}

fn o_preventextensions(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    st.unflatten_array(obj);
    st.heap.obj_mut(obj).extensible = false;
    st.copy(1)?;
    Ok(())
}

fn o_isextensible(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    let e = st.heap.obj(obj).extensible;
    st.push_boolean(e)
}

fn o_seal(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    st.unflatten_array(obj);
    st.heap.obj_mut(obj).extensible = false;
    for p in st.heap.obj_mut(obj).properties.values_mut() {
        p.atts |= JS_DONTCONF;
    }
    st.copy(1)?;
    Ok(())
}

fn o_issealed(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    let o = st.heap.obj(obj);
    if o.extensible {
        return st.push_boolean(false);
    }
    let sealed = o.properties.values().all(|p| p.atts & JS_DONTCONF != 0);
    st.push_boolean(sealed)
}

fn o_freeze(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    st.unflatten_array(obj);
    st.heap.obj_mut(obj).extensible = false;
    for p in st.heap.obj_mut(obj).properties.values_mut() {
        p.atts |= JS_READONLY | JS_DONTCONF;
    }
    st.copy(1)?;
    Ok(())
}

fn o_isfrozen(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.type_error("not an object");
    }
    let obj = st.toobject(1)?;
    let o = st.heap.obj(obj);
    let frozen = o
        .properties
        .values()
        .all(|p| p.atts & JS_READONLY != 0 && p.atts & JS_DONTCONF != 0);
    if !frozen {
        return st.push_boolean(false);
    }
    st.push_boolean(!o.extensible)
}

pub fn init(st: &mut State) {
    let proto = st.protos.object;
    st.push_object(proto).unwrap();
    {
        propf(st, "Object.prototype.toString", op_tostring, 0).unwrap();
        propf(st, "Object.prototype.toLocaleString", op_tostring, 0).unwrap();
        propf(st, "Object.prototype.valueOf", op_valueof, 0).unwrap();
        propf(st, "Object.prototype.hasOwnProperty", op_hasownproperty, 1).unwrap();
        propf(st, "Object.prototype.isPrototypeOf", op_isprototypeof, 1).unwrap();
        propf(st, "Object.prototype.propertyIsEnumerable", op_propertyisenumerable, 1).unwrap();
    }
    st.newcconstructor(jsb_object, jsb_new_object, "Object", 1).unwrap();
    {
        propf(st, "Object.getPrototypeOf", o_getprototypeof, 1).unwrap();
        propf(st, "Object.setPrototypeOf", o_setprototypeof, 2).unwrap();
        propf(st, "Object.getOwnPropertyDescriptor", o_getownpropertydescriptor, 2).unwrap();
        propf(st, "Object.getOwnPropertyNames", o_getownpropertynames, 1).unwrap();
        propf(st, "Object.create", o_create, 2).unwrap();
        propf(st, "Object.defineProperty", o_defineproperty, 3).unwrap();
        propf(st, "Object.defineProperties", o_defineproperties, 2).unwrap();
        propf(st, "Object.seal", o_seal, 1).unwrap();
        propf(st, "Object.freeze", o_freeze, 1).unwrap();
        propf(st, "Object.preventExtensions", o_preventextensions, 1).unwrap();
        propf(st, "Object.isSealed", o_issealed, 1).unwrap();
        propf(st, "Object.isFrozen", o_isfrozen, 1).unwrap();
        propf(st, "Object.isExtensible", o_isextensible, 1).unwrap();
        propf(st, "Object.keys", o_keys, 1).unwrap();
        propf(st, "Object.assign", o_assign, 2).unwrap();
    }
    st.defglobal("Object", JS_DONTENUM).unwrap();
}
