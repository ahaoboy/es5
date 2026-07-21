//! Boolean constructor and Boolean.prototype (jsboolean.c).

use super::propf;
use crate::object::{Class, Payload};
use crate::state::{State, R};
use crate::value::JS_DONTENUM;

fn jsb_new_boolean(st: &mut State) -> R<()> {
    let b = st.toboolean(1);
    st.newboolean(b)
}

fn jsb_boolean(st: &mut State) -> R<()> {
    let b = st.toboolean(1);
    st.push_boolean(b)
}

fn bp_tostring(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    let b = match &st.heap.obj(obj).payload {
        Payload::Boolean(b) if st.heap.obj(obj).class == Class::Boolean => *b,
        _ => return st.type_error("not a boolean"),
    };
    st.push_literal(if b { "true" } else { "false" })
}

fn bp_valueof(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    let b = match &st.heap.obj(obj).payload {
        Payload::Boolean(b) if st.heap.obj(obj).class == Class::Boolean => *b,
        _ => return st.type_error("not a boolean"),
    };
    st.push_boolean(b)
}

pub fn init(st: &mut State) {
    let proto = st.protos.boolean;
    st.push_object(proto).unwrap();
    {
        propf(st, "Boolean.prototype.toString", bp_tostring, 0).unwrap();
        propf(st, "Boolean.prototype.valueOf", bp_valueof, 0).unwrap();
    }
    st.newcconstructor(jsb_boolean, jsb_new_boolean, "Boolean", 1).unwrap();
    st.defglobal("Boolean", JS_DONTENUM).unwrap();
}
