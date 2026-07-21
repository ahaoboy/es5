//! Error constructors and Error.prototype (jserror.c).

use super::{propf, props};
use crate::object::{Class, ObjRef};
use crate::state::{State, R};
use crate::value::{JS_DONTCONF, JS_DONTENUM, JS_READONLY};

fn ep_tostring(st: &mut State) -> R<()> {
    let mut name = "Error".to_string();
    let mut message = "".to_string();

    if !st.isobject(-1) {
        return st.type_error("not an object");
    }

    if st.hasproperty(0, "name")? {
        name = st.tostring(-1)?.to_string();
        st.pop(1);
    }
    if st.hasproperty(0, "message")? {
        message = st.tostring(-1)?.to_string();
        st.pop(1);
    }

    if name.is_empty() {
        st.push_string(&message)
    } else if message.is_empty() {
        st.push_string(&name)
    } else {
        st.push_string(&format!("{}: {}", name, message))
    }
}

fn ep_get_stack(st: &mut State) -> R<()> {
    ep_tostring(st)?;
    st.getproperty(0, "stackTrace")?;
    st.concat()
}

/// jsB_ErrorX: common constructor for all error classes.
fn error_x(st: &mut State, prototype: ObjRef) -> R<()> {
    let obj = st.heap.alloc_object(Class::Error, Some(prototype));
    let frames = st.capture_frames(1);
    if !frames.is_empty() {
        let trace = State::frames_to_string(&frames);
        st.heap.obj_mut(obj).payload = crate::object::Payload::Error(
            crate::object::ErrorData { frames },
        );
        st.push_object(obj)?;
        st.push_string(&trace)?;
        st.defproperty(-2, "stackTrace", JS_DONTENUM)?;
    } else {
        st.push_object(obj)?;
    }
    if st.isdefined(1) {
        let msg = st.tostring(1)?;
        st.push_string_rc(msg)?;
        st.defproperty(-2, "message", JS_DONTENUM)?;
    }
    Ok(())
}

macro_rules! deferror {
    ($fname:ident, $proto:ident) => {
        fn $fname(st: &mut State) -> R<()> {
            let p = st.protos.$proto;
            error_x(st, p)
        }
    };
}

deferror!(jsb_error, error);
deferror!(jsb_evalerror, eval_error);
deferror!(jsb_rangeerror, range_error);
deferror!(jsb_referenceerror, reference_error);
deferror!(jsb_syntaxerror, syntax_error);
deferror!(jsb_typeerror, type_error);
deferror!(jsb_urierror, uri_error);

pub fn init(st: &mut State) {
    let proto = st.protos.error;
    st.push_object(proto).unwrap();
    {
        props(st, "name", "Error").unwrap();
        propf(st, "Error.prototype.toString", ep_tostring, 0).unwrap();
        props(st, "message", "").unwrap();

        st.newcfunction(ep_get_stack, "stack", 0).unwrap();
        st.push_null().unwrap();
        st.defaccessor(-3, "stack", JS_READONLY | JS_DONTENUM | JS_DONTCONF).unwrap();
    }
    st.newcconstructor(jsb_error, jsb_error, "Error", 1).unwrap();
    st.defglobal("Error", JS_DONTENUM).unwrap();

    macro_rules! ierror {
        ($proto:ident, $ctor:ident, $name:expr) => {
            let p = st.protos.$proto;
            st.push_object(p).unwrap();
            props(st, "name", $name).unwrap();
            st.newcconstructor($ctor, $ctor, $name, 1).unwrap();
            st.defglobal($name, JS_DONTENUM).unwrap();
        };
    }

    ierror!(eval_error, jsb_evalerror, "EvalError");
    ierror!(range_error, jsb_rangeerror, "RangeError");
    ierror!(reference_error, jsb_referenceerror, "ReferenceError");
    ierror!(syntax_error, jsb_syntaxerror, "SyntaxError");
    ierror!(type_error, jsb_typeerror, "TypeError");
    ierror!(uri_error, jsb_urierror, "URIError");
}
