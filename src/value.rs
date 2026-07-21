//! JavaScript values (js_Value in jsvalue.c).

use crate::object::ObjRef;

/// A JavaScript value. Runtime strings are `Rc<str>` (freed promptly by
/// reference counting — much faster for string-building workloads than a
/// GC'd string arena); literals live in a permanent table by index.
/// Numbers, objects and literals remain `Copy`.
#[derive(Clone, Debug)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    /// runtime string (js_TMEMSTR)
    String(std::rc::Rc<str>),
    /// literal string (js_TLITSTR)
    LitStr(u32),
    Object(ObjRef),
}

/// Identity/reference equality (pointer identity for strings and objects).
/// Does **not** perform JS abstract equality — use `state.rs::equal()` for that.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Undefined, Value::Undefined)
            | (Value::Null, Value::Null) => true,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            // Rc identity — same allocation, not same content.
            (Value::String(a), Value::String(b)) => std::rc::Rc::ptr_eq(a, b),
            (Value::LitStr(a), Value::LitStr(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Value {
    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn is_boolean(&self) -> bool {
        matches!(self, Value::Boolean(_))
    }
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Number(_))
    }
    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_) | Value::LitStr(_))
    }
    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }
    pub fn is_primitive(&self) -> bool {
        !matches!(self, Value::Object(_))
    }
    pub fn as_object(&self) -> Option<ObjRef> {
        match self {
            Value::Object(r) => Some(*r),
            _ => None,
        }
    }
    pub fn as_number(&self) -> f64 {
        match self {
            Value::Number(n) => *n,
            _ => f64::NAN,
        }
    }
    pub fn as_boolean(&self) -> bool {
        matches!(self, Value::Boolean(true))
    }
}

/// Hint to ToPrimitive() (jsvalue.c).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    None,
    Number,
    String,
}

/// Property attribute flags (mujs.h).
pub const JS_READONLY: u32 = 1;
pub const JS_DONTENUM: u32 = 2;
pub const JS_DONTCONF: u32 = 4;

/// RegExp flags (mujs.h).
pub const JS_REGEXP_G: u32 = 1;
pub const JS_REGEXP_I: u32 = 2;
pub const JS_REGEXP_M: u32 = 4;
