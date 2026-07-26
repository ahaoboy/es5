//! Set constructor and Set.prototype (ES6).

use crate::builtins::make_es6_iterator;
use crate::object::{Class, ObjRef, Payload, SetData};
use crate::state::{State, R};
use crate::value::{JS_DONTENUM, Value};

fn get_set_data(st: &mut State, idx: i32) -> R<ObjRef> {
    let obj = st.toobject(idx)?;
    if st.heap.obj(obj).class != Class::Set {
        return st.type_error("Set.prototype method called on non-Set");
    }
    Ok(obj)
}

/// Helper: return the Set constructor's `.prototype` property.
fn set_proto(st: &State) -> ObjRef {
    st.heap
        .get_property(st.g, "Set")
        .and_then(|ctor| ctor.value.as_object())
        .and_then(|s_ctor| {
            st.heap
                .get_property(s_ctor, "prototype")
                .and_then(|pp| pp.value.as_object())
        })
        .unwrap_or(st.protos.object)
}

fn s_constructor(st: &mut State) -> R<()> {
    let set_obj = st.heap.alloc_object(Class::Set, Some(set_proto(st)));
    st.heap.obj_mut(set_obj).payload = Payload::Set(SetData { values: Vec::new().into() });

    let mut values: Vec<Value> = Vec::new();

    // optional iterable: iterate by array indices
    if st.isdefined(1) && !st.isundefined(1) && !st.isnull(1) {
        let src = st.toobject(1)?;
        let simple = matches!(&st.heap.obj(src).payload, Payload::Array(a) if a.simple);
        if simple {
            let flat = match &st.heap.obj(src).payload {
                Payload::Array(a) => a.flat.to_vec(),
                _ => vec![],
            };
            for v in flat {
                if !values.iter().any(|x| same_set_value(x, &v)) {
                    values.push(v);
                }
            }
        } else {
            let len = st.getlength(1)?;
            for i in 0..len {
                if st.hasindex(1, i)? {
                    let val = st.pop_value();
                    if !values.iter().any(|x| same_set_value(x, &val)) {
                        values.push(val);
                    }
                }
            }
        }
    }

    if let Payload::Set(s) = &mut st.heap.obj_mut(set_obj).payload {
        s.values = values.into();
    }
    st.push_object(set_obj)
}

fn s_add(st: &mut State) -> R<()> {
    let obj = get_set_data(st, 0)?;
    let val = st.stackidx(1).clone();
    let values = match &mut st.heap.obj_mut(obj).payload {
        Payload::Set(s) => &mut s.values,
        _ => unreachable!(),
    };
    for v in values.iter() {
        if same_set_value(v, &val) {
            return st.copy(0);
        }
    }
    values.push(val);
    st.copy(0)
}

fn s_has(st: &mut State) -> R<()> {
    let obj = get_set_data(st, 0)?;
    let val = st.stackidx(1).clone();
    let values = match &st.heap.obj(obj).payload {
        Payload::Set(s) => &s.values,
        _ => unreachable!(),
    };
    let found = values.iter().any(|v| same_set_value(v, &val));
    st.push_boolean(found)
}

fn s_delete(st: &mut State) -> R<()> {
    let obj = get_set_data(st, 0)?;
    let val = st.stackidx(1).clone();
    let values = match &mut st.heap.obj_mut(obj).payload {
        Payload::Set(s) => &mut s.values,
        _ => unreachable!(),
    };
    let idx = values.iter().position(|v| same_set_value(v, &val));
    match idx {
        Some(i) => {
            values.remove(i);
            st.push_boolean(true)
        }
        None => st.push_boolean(false),
    }
}

fn s_clear(st: &mut State) -> R<()> {
    let obj = get_set_data(st, 0)?;
    match &mut st.heap.obj_mut(obj).payload {
        Payload::Set(s) => s.values.clear(),
        _ => unreachable!(),
    }
    st.push_undefined()
}

fn s_size_getter(st: &mut State) -> R<()> {
    let obj = get_set_data(st, 0)?;
    let len = match &st.heap.obj(obj).payload {
        Payload::Set(s) => s.values.len(),
        _ => 0,
    };
    st.push_number(len as f64)
}

fn s_foreach(st: &mut State) -> R<()> {
    if !st.iscallable(1) {
        return st.type_error("Set.prototype.forEach: callback is not callable");
    }
    let obj = get_set_data(st, 0)?;
    let values: Vec<_> = match &st.heap.obj(obj).payload {
        Payload::Set(s) => s.values.to_vec(),
        _ => return Ok(()),
    };
    let this_arg = if st.isdefined(2) {
        st.stackidx(2).clone()
    } else {
        Value::Undefined
    };
    for v in values {
        st.copy(1)?; // callback
        st.push_value(this_arg.clone())?;
        st.push_value(v.clone())?; // value (both as v and k)
        st.push_value(v)?;         // key (same as value for Set)
        st.copy(0)?;               // the Set itself
        st.call(3)?;
        st.pop(1);
    }
    st.push_undefined()
}

fn s_values_fn(st: &mut State) -> R<()> {
    let obj = get_set_data(st, 0)?;
    let values = match &st.heap.obj(obj).payload {
        Payload::Set(s) => s.values.to_vec(),
        _ => vec![],
    };
    make_es6_iterator(st, values)
}

fn s_entries_fn(st: &mut State) -> R<()> {
    let obj = get_set_data(st, 0)?;
    let values = match &st.heap.obj(obj).payload {
        Payload::Set(s) => s.values.to_vec(),
        _ => vec![],
    };
    let vals: Vec<Value> = values
        .into_iter()
        .map(|v| {
            let pa = st.heap.alloc_object(Class::Array, Some(st.protos.array));
            st.heap.obj_mut(pa).payload = Payload::Array(crate::object::ArrayData {
                length: 2,
                simple: true,
                flat: vec![v.clone(), v.clone()].into(),
            });
            Value::Object(pa)
        })
        .collect();
    make_es6_iterator(st, vals)
}

/// SameValueZero comparison: NaN == NaN, +0 == -0, objects by identity.
fn same_set_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Boolean(x), Value::Boolean(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => {
            if x.is_nan() && y.is_nan() {
                return true;
            }
            x == y
        }
        (Value::String(x), Value::String(y)) => x == y,
        (Value::LitStr(x), Value::LitStr(y)) => x == y,
        (Value::Object(x), Value::Object(y)) => x == y,
        _ => false,
    }
}

pub fn init(st: &mut State) {
    st.newobject().unwrap();
    {
        st.newcfunction(s_add, "add", 1).unwrap();
        st.defproperty(-2, "add", JS_DONTENUM).unwrap();
        st.newcfunction(s_has, "has", 1).unwrap();
        st.defproperty(-2, "has", JS_DONTENUM).unwrap();
        st.newcfunction(s_delete, "delete", 1).unwrap();
        st.defproperty(-2, "delete", JS_DONTENUM).unwrap();
        st.newcfunction(s_clear, "clear", 0).unwrap();
        st.defproperty(-2, "clear", JS_DONTENUM).unwrap();
        st.newcfunction(s_foreach, "forEach", 1).unwrap();
        st.defproperty(-2, "forEach", JS_DONTENUM).unwrap();
        // values / keys (both return values)
        st.newcfunction(s_values_fn, "values", 0).unwrap();
        st.defproperty(-2, "values", JS_DONTENUM).unwrap();
        st.newcfunction(s_values_fn, "keys", 0).unwrap();
        st.defproperty(-2, "keys", JS_DONTENUM).unwrap();
        st.newcfunction(s_entries_fn, "entries", 0).unwrap();
        st.defproperty(-2, "entries", JS_DONTENUM).unwrap();
        // @@iterator fallback = values for Set
        st.newcfunction(s_values_fn, "@@iterator", 0).unwrap();
        st.defproperty(-2, "@@iterator", JS_DONTENUM).unwrap();
        // size getter
        st.newcfunction(s_size_getter, "get size", 0).unwrap();
        st.push_null().unwrap();
        st.defaccessor(-3, "size", JS_DONTENUM).unwrap();
    }
    st.newcconstructor(s_constructor, s_constructor, "Set", 0).unwrap();
    st.defglobal("Set", JS_DONTENUM).unwrap();
}
