//! Built-in objects and global functions (replaces jsbuiltin.c).

pub mod array;
pub mod boolean;
pub mod date;
pub mod error;
pub mod function;
pub mod json;
pub mod map;
pub mod math;
pub mod modules;
pub mod number;
pub mod object;
pub mod regexp;
pub mod repr;
pub mod set;
pub mod string;
use thin_vec::ThinVec;
#[cfg(feature = "symbol")]
pub mod symbol;

use crate::number as num;
use crate::object::{Class, ES6IteratorData, Payload};
use crate::state::{CFunction, State, R};
use crate::utf;
use crate::value::{JS_DONTCONF, JS_DONTENUM, JS_READONLY, Value};

/// ES6 iterator next() CFunction.
fn es6_iter_next(st: &mut State) -> R<()> {
    let iter_obj = st.toobject(0)?;
    let mut data = {
        let obj = st.heap.obj(iter_obj);
        match &obj.payload {
            Payload::ES6Iterator(data) => data.clone(),
            _ => return st.type_error("Iterator.next called on non-iterator"),
        }
    };
    let done = data.pos >= data.values.len();
    let current = if done {
        Value::Undefined
    } else {
        data.values[data.pos].clone()
    };
    if !done {
        data.pos += 1;
    }
    st.heap.obj_mut(iter_obj).payload = Payload::ES6Iterator(data);
    let result_obj = st.heap.alloc_object(Class::Object, Some(st.protos.object));
    st.push_object(result_obj)?;
    st.push_boolean(done)?;
    st.defproperty(-2, "done", 0)?;
    st.push_value(current)?;
    st.defproperty(-2, "value", 0)?;
    Ok(())
}

/// Create an ES6 iterator object with a snapshot of values.
pub fn make_es6_iterator(st: &mut State, values: ThinVec<Value>) -> R<()> {
    let iter = st.heap.alloc_object(Class::Iterator, Some(st.protos.object));
    st.heap.obj_mut(iter).payload = Payload::ES6Iterator(ES6IteratorData { values, pos: 0 });
    st.push_object(iter)?;
    st.newcfunction(es6_iter_next, "next", 0)?;
    st.defproperty(-2, "next", JS_DONTENUM)?;
    // @@iterator fallback for Babel helpers
    st.copy(-1)?;
    st.defproperty(-2, "@@iterator", JS_DONTENUM)?;
    Ok(())
}

/// Register a function property on the object at the top of the stack.
/// The name may be dotted; only the last component is used (jsB_propf).
pub fn propf(st: &mut State, name: &str, cfun: CFunction, n: i32) -> R<()> {
    let pname = name.rsplit('.').next().unwrap_or(name);
    st.newcfunction(cfun, name, n)?;
    st.defproperty(-2, pname, JS_DONTENUM)
}

/// Register a read-only number property (jsB_propn).
pub fn propn(st: &mut State, name: &str, number: f64) -> R<()> {
    st.push_number(number)?;
    st.defproperty(-2, name, JS_READONLY | JS_DONTENUM | JS_DONTCONF)
}

/// Register a string property (jsB_props).
pub fn props(st: &mut State, name: &str, string: &str) -> R<()> {
    st.push_literal(string)?;
    st.defproperty(-2, name, JS_DONTENUM)
}

// ---------------------------------------------------------------------------
// ES6 iterator protocol helpers
// ---------------------------------------------------------------------------

/// `next()` on an iterator object: yields {value, done:false} until the
/// buffered values are exhausted, then {done:true}.
fn iter_next(st: &mut State) -> R<()> {
    let idx = if let Ok(true) = st.hasproperty(0, "__index") {
        st.tointeger(-1)?
    } else {
        0
    };
    st.pop(1);

    let (has_vals, len) = if let Ok(true) = st.hasproperty(0, "__values") {
        let len = st.getlength(-1)?;
        (true, len)
    } else {
        st.pop(1);
        (false, 0)
    };

    if has_vals && idx < len {
        // fetch value at idx and bump the cursor
        st.getindex(-1, idx)?;
        st.push_number((idx + 1) as f64)?;
        st.defproperty(0, "__index", JS_DONTENUM)?;
        st.newobject()?;
        st.rot2();
        st.defproperty(-2, "value", 0)?;
        st.push_boolean(false)?;
        st.defproperty(-2, "done", 0)?;
    } else {
        st.pop(1); // __values array
        st.newobject()?;
        st.push_boolean(true)?;
        st.defproperty(-2, "done", 0)?;
    }
    Ok(())
}

/// Create an ES6 iterator object over a snapshot of values.
pub fn make_iterator(st: &mut State, values: Vec<Value>) -> R<()> {
    st.newobject()?;
    st.newarray()?;
    for (i, v) in values.into_iter().enumerate() {
        st.push_value(v)?;
        st.setindex(-2, i as i32)?;
    }
    st.defproperty(-2, "__values", JS_DONTENUM)?;
    st.push_number(0.0)?;
    st.defproperty(-2, "__index", JS_DONTENUM)?;
    st.newcfunction(iter_next, "next", 0)?;
    st.defproperty(-2, "next", JS_DONTENUM)?;
    Ok(())
}

fn globalf(st: &mut State, name: &str, cfun: CFunction, n: i32) -> R<()> {
    st.newcfunction(cfun, name, n)?;
    st.defglobal(name, JS_DONTENUM)
}

pub(crate) fn jsb_parse_int(st: &mut State) -> R<()> {
    let s = st.tostring(1)?;
    let mut radix = if st.isdefined(2) { st.tointeger(2)? } else { 0 };
    let mut sign = 1.0;

    let s = s.trim_start_matches(utf::is_js_white_or_newline);
    let s = if let Some(rest) = s.strip_prefix('-') {
        sign = -1.0;
        rest
    } else if let Some(rest) = s.strip_prefix('+') {
        rest
    } else {
        s
    };

    let s = if radix == 0 {
        radix = 10;
        if s.starts_with("0x") || s.starts_with("0X") {
            radix = 16;
            &s[2..]
        } else {
            s
        }
    } else if !(2..=36).contains(&radix) {
        return st.push_number(f64::NAN);
    } else {
        s
    };

    let (n, consumed) = num::strtol(s, radix as u32);
    if consumed == 0 {
        st.push_number(f64::NAN)
    } else {
        st.push_number(n * sign)
    }
}

pub(crate) fn jsb_parse_float(st: &mut State) -> R<()> {
    let s = st.tostring(1)?;
    let s = s.trim_start_matches(utf::is_js_white_or_newline);
    if s.starts_with("Infinity") || s.starts_with("+Infinity") {
        st.push_number(f64::INFINITY)
    } else if s.starts_with("-Infinity") {
        st.push_number(f64::NEG_INFINITY)
    } else {
        let (n, consumed) = num::string_to_float(s);
        if consumed == 0 {
            st.push_number(f64::NAN)
        } else {
            st.push_number(n)
        }
    }
}

fn jsb_is_nan(st: &mut State) -> R<()> {
    let n = st.tonumber(1)?;
    st.push_boolean(n.is_nan())
}

fn jsb_is_finite(st: &mut State) -> R<()> {
    let n = st.tonumber(1)?;
    st.push_boolean(n.is_finite())
}

/// The global eval() function; calls to it are indirect evals (ES5.1 10.4.2).
fn jsb_eval(st: &mut State) -> R<()> {
    st.indirect_eval()
}

const URI_RESERVED: &str = ";/?:@&=+$,";
const URI_UNESCAPED: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.!~*'()";

fn encode(st: &mut State, s: &str, unescaped: &str) -> R<()> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if unescaped.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize & 0xf] as char);
            out.push(HEX[b as usize & 0xf] as char);
        }
    }
    st.push_string(&out)
}

fn decode(st: &mut State, s: &str, reserved: &str) -> R<()> {
    // Work on raw bytes like MuJS; a decoded byte stream is expected to be
    // UTF-8 (lossy conversion at the end if it is not).
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        if i + 1 >= b.len() {
            return st.uri_error("truncated escape sequence");
        }
        let a = b[i] as char;
        let c2 = b[i + 1] as char;
        i += 2;
        if !a.is_ascii_hexdigit() || !c2.is_ascii_hexdigit() {
            return st.uri_error("invalid escape sequence");
        }
        let v = (a.to_digit(16).unwrap() << 4 | c2.to_digit(16).unwrap()) as u8;
        if !reserved.as_bytes().contains(&v) {
            out.push(v);
        } else {
            out.push(b'%');
            out.push(a as u8);
            out.push(c2 as u8);
        }
    }
    let s = String::from_utf8_lossy(&out).into_owned();
    st.push_string(&s)
}

fn jsb_decode_uri(st: &mut State) -> R<()> {
    let s = st.tostring(1)?;
    let reserved = format!("{}#", URI_RESERVED);
    decode(st, &s, &reserved)
}

fn jsb_decode_uri_component(st: &mut State) -> R<()> {
    let s = st.tostring(1)?;
    decode(st, &s, "")
}

fn jsb_encode_uri(st: &mut State) -> R<()> {
    let s = st.tostring(1)?;
    let unescaped = format!("{}{}#", URI_UNESCAPED, URI_RESERVED);
    encode(st, &s, &unescaped)
}

fn jsb_encode_uri_component(st: &mut State) -> R<()> {
    let s = st.tostring(1)?;
    encode(st, &s, URI_UNESCAPED)
}

/// jsB_init: create prototypes, constructors and the global functions.
pub fn init(st: &mut State) {
    // Create the prototype objects here, before the constructors
    let obj_proto = st.heap.alloc_object(Class::Object, None);
    st.protos.object = obj_proto;
    st.protos.array = st.heap.alloc_object(Class::Array, Some(obj_proto));
    st.protos.function = st.heap.alloc_object(Class::CFunction, Some(obj_proto));
    st.protos.boolean = st.heap.alloc_object(Class::Boolean, Some(obj_proto));
    st.protos.number = st.heap.alloc_object(Class::Number, Some(obj_proto));
    st.protos.string = st.heap.alloc_object(Class::String, Some(obj_proto));
    st.protos.date = st.heap.alloc_object(Class::Date, Some(obj_proto));
    st.protos.regexp = st.heap.alloc_object(Class::Regexp, Some(obj_proto));
    {
        // RegExp_prototype gets an empty regexp program
        let prog = crate::regexp::Regexp::compile("(?:)", 0).expect("empty regexp compiles");
        let source = st.heap.intern("(?:)");
        st.heap.obj_mut(st.protos.regexp).payload = crate::object::Payload::Regexp(
            crate::object::RegexpData {
                prog,
                source,
                flags: 0,
                last: 0,
            },
        );
    }

    st.protos.error = st.heap.alloc_object(Class::Error, Some(obj_proto));
    let err = st.protos.error;
    st.protos.eval_error = st.heap.alloc_object(Class::Error, Some(err));
    st.protos.range_error = st.heap.alloc_object(Class::Error, Some(err));
    st.protos.reference_error = st.heap.alloc_object(Class::Error, Some(err));
    st.protos.syntax_error = st.heap.alloc_object(Class::Error, Some(err));
    st.protos.type_error = st.heap.alloc_object(Class::Error, Some(err));
    st.protos.uri_error = st.heap.alloc_object(Class::Error, Some(err));

    // set default payload values for primitive prototypes
    st.heap.obj_mut(st.protos.array).payload = crate::object::Payload::Array(
        crate::object::ArrayData {
            length: 0,
            simple: true,
            flat: thin_vec::ThinVec::new(),
        },
    );
    st.heap.obj_mut(st.protos.boolean).payload = crate::object::Payload::Boolean(false);
    st.heap.obj_mut(st.protos.number).payload = crate::object::Payload::Number(0.0);
    st.heap.obj_mut(st.protos.string).payload = crate::object::Payload::String(
        crate::object::StringData {
            string: st.heap.intern(""),
            length: 0,
        },
    );
    st.heap.obj_mut(st.protos.date).payload = crate::object::Payload::Number(0.0);

    // Create the constructors and fill out the prototype objects
    object::init(st);
    array::init(st);
    function::init(st);
    boolean::init(st);
    number::init(st);
    string::init(st);
    regexp::init(st);
    date::init(st);
    error::init(st);
    math::init(st);
    json::init(st);

    map::init(st);
    set::init(st);

    #[cfg(feature = "symbol")]
    symbol::init(st);

    // Initialize the global object
    st.push_number(f64::NAN).unwrap();
    st.defglobal("NaN", JS_READONLY | JS_DONTENUM | JS_DONTCONF).unwrap();

    st.push_number(f64::INFINITY).unwrap();
    st.defglobal("Infinity", JS_READONLY | JS_DONTENUM | JS_DONTCONF).unwrap();

    st.push_undefined().unwrap();
    st.defglobal("undefined", JS_READONLY | JS_DONTENUM | JS_DONTCONF).unwrap();

    st.push_object(st.g).unwrap();
    st.defglobal("globalThis", JS_DONTENUM).unwrap();

    st.push_object(st.g).unwrap();
    st.defglobal("global", JS_DONTENUM).unwrap();

    globalf(st, "eval", jsb_eval, 1).unwrap();
    globalf(st, "parseInt", jsb_parse_int, 1).unwrap();
    globalf(st, "parseFloat", jsb_parse_float, 1).unwrap();
    globalf(st, "isNaN", jsb_is_nan, 1).unwrap();
    globalf(st, "isFinite", jsb_is_finite, 1).unwrap();

    globalf(st, "decodeURI", jsb_decode_uri, 1).unwrap();
    globalf(st, "decodeURIComponent", jsb_decode_uri_component, 1).unwrap();
    globalf(st, "encodeURI", jsb_encode_uri, 1).unwrap();
    globalf(st, "encodeURIComponent", jsb_encode_uri_component, 1).unwrap();
}
