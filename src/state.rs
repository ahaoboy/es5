//! The interpreter state (js_State), value stack, exceptions, property
//! access and the function call machinery.
//!
//! Replaces jsstate.c, jsvalue.c and jsrun.c (except the bytecode dispatch
//! loop, which lives in run.rs). C's setjmp/longjmp exception handling is
//! replaced by `Result` propagation: a thrown JavaScript exception is
//! `Err(Value)`, and `TryFrame` records the state to restore at catch sites.

use crate::compile;
use crate::number;
use crate::object::{
    ArrayData, CFunctionData, Class, ErrorData, FunRef, FunctionData, Heap, NONE, ObjRef, Payload,
    StringData, TraceFrame,
};
use crate::parse;
use crate::utf;
use crate::value::{Hint, JS_DONTCONF, JS_DONTENUM, JS_READONLY, Value};
use compact_str::CompactString;
use rustc_hash::FxHashMap;
use std::rc::Rc;
use thin_vec::ThinVec;

/// Result type used throughout the engine; `Err` is a thrown JS exception.
pub type R<T> = Result<T, Value>;

/// Native (built-in) function signature (js_CFunction).
pub type CFunction = fn(&mut State) -> R<()>;

pub const JS_STACKSIZE: usize = 4096;
pub const JS_ENVLIMIT: usize = 1024;
pub const JS_TRYLIMIT: usize = 64;
pub const JS_ARRAYLIMIT: i32 = 1 << 26;
pub const JS_STRLIMIT: usize = 1 << 28;

/// State constructor flag (mujs.h).
pub const JS_STRICT: i32 = 1;

/// Convert a property name that may be a Symbol.toString() result
/// (e.g. "Symbol(Symbol.iterator)") to its internal "@@name" form.
/// This allows `obj[Symbol.iterator]` to work with our `@@iterator` convention.
pub fn symbol_prop_key(name: &str) -> Option<&str> {
    let body = name.strip_prefix("Symbol(Symbol.")?.strip_suffix(')')?;
    Some(match body {
        "iterator" => "@@iterator",
        "asyncIterator" => "@@asyncIterator",
        "toPrimitive" => "@@toPrimitive",
        _ => return None,
    })
}

/// The well-known prototype objects (J->*_prototype).
#[derive(Clone, Copy)]
pub struct Prototypes {
    pub object: ObjRef,
    pub array: ObjRef,
    pub function: ObjRef,
    pub boolean: ObjRef,
    pub number: ObjRef,
    pub string: ObjRef,
    pub regexp: ObjRef,
    pub date: ObjRef,
    pub error: ObjRef,
    pub eval_error: ObjRef,
    pub range_error: ObjRef,
    pub reference_error: ObjRef,
    pub syntax_error: ObjRef,
    pub type_error: ObjRef,
    pub uri_error: ObjRef,
}

impl Prototypes {
    fn none() -> Prototypes {
        Prototypes {
            object: NONE,
            array: NONE,
            function: NONE,
            boolean: NONE,
            number: NONE,
            string: NONE,
            regexp: NONE,
            date: NONE,
            error: NONE,
            eval_error: NONE,
            range_error: NONE,
            reference_error: NONE,
            syntax_error: NONE,
            type_error: NONE,
            uri_error: NONE,
        }
    }

    pub fn all(&self) -> [ObjRef; 15] {
        [
            self.object,
            self.array,
            self.function,
            self.boolean,
            self.number,
            self.string,
            self.regexp,
            self.date,
            self.error,
            self.eval_error,
            self.range_error,
            self.reference_error,
            self.syntax_error,
            self.type_error,
            self.uri_error,
        ]
    }
}

/// One entry of the debug call stack (js_StackTrace).
/// JS frames store the compiled function (name/file resolved lazily);
/// native frames store an explicit name.
#[derive(Clone)]
pub struct StackTrace {
    pub fun: FunRef,
    pub name: Option<CompactString>,
    pub line: u32,
    pub col: u32,
    pub stack: usize,
}

/// Exception handler frame; replaces js_Jumpbuf/setjmp.
#[derive(Clone)]
pub struct TryFrame {
    pub e: ObjRef,
    pub envtop: usize,
    pub tracetop: usize,
    pub top: usize,
    pub bot: usize,
    pub strict: bool,
    /// Where to resume when this frame catches (None = host frame).
    pub catch_pc: Option<usize>,
}

macro_rules! def_error {
    ($name:ident, $proto:ident) => {
        pub fn $name<T>(&mut self, msg: &str) -> R<T> {
            let v = self.new_errorx(msg, self.protos.$proto)?;
            Err(v)
        }
    };
}

/// Simple cumulative instrumentation for performance analysis.
pub struct Stats {
    pub concat_calls: std::sync::atomic::AtomicU64,
    pub concat_bytes: std::sync::atomic::AtomicU64,
    pub gc_calls: std::sync::atomic::AtomicU64,
    pub gc_nanos: std::sync::atomic::AtomicU64,
}

impl Stats {
    const fn new() -> Stats {
        Stats {
            concat_calls: std::sync::atomic::AtomicU64::new(0),
            concat_bytes: std::sync::atomic::AtomicU64::new(0),
            gc_calls: std::sync::atomic::AtomicU64::new(0),
            gc_nanos: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

pub static STATS: Stats = Stats::new();

pub use std::sync::atomic::Ordering;

/// The interpreter state (js_State).
pub struct State {
    pub heap: Heap,

    pub default_strict: bool,
    pub strict: bool,

    pub protos: Prototypes,

    pub seed: u32,

    pub nextref: u32,
    pub r: ObjRef,  // registry of hidden values
    pub g: ObjRef,  // the global object
    pub e: ObjRef,  // current environment scope
    pub ge: ObjRef, // global environment scope

    // execution stack
    pub top: usize,
    pub bot: usize,
    pub stack: Vec<Value>,

    // environments on the call stack but currently not in scope
    pub envstack: Vec<ObjRef>,

    // debug info stack trace
    pub tracetop: usize,
    pub trace: Vec<StackTrace>,

    // exception stack
    pub trystk: Vec<TryFrame>,

    pub runlimit: i32,
    pub memlimit: i32,

    pub scratch: String,

    /// source text of every loaded script, for error diagnostics
    pub sources: FxHashMap<CompactString, CompactString>,

    /// global symbol registry: Symbol.for(key) -> symbol object
    pub symbol_registry: FxHashMap<CompactString, ObjRef>,

    /// scheduled timers (setTimeout/setInterval), pumped when idle
    #[cfg(any(feature = "modules", feature = "timers"))]
    pub timers: Vec<crate::builtins::modules::timers::Timer>,
    #[cfg(any(feature = "modules", feature = "timers"))]
    pub next_timer_id: u32,
}

impl State {
    pub fn new(flags: i32) -> State {
        let mut heap = Heap::new();
        let r = heap.alloc_object(Class::Object, None);
        let g = heap.alloc_object(Class::Object, None);
        let ge = heap.alloc_env(g, None);
        let mut st = State {
            heap,
            default_strict: flags & JS_STRICT != 0,
            strict: flags & JS_STRICT != 0,
            protos: Prototypes::none(),
            seed: 0,
            nextref: 0,
            r,
            g,
            e: ge,
            ge,
            top: 0,
            bot: 0,
            stack: vec![Value::Undefined; JS_STACKSIZE],
            envstack: Vec::with_capacity(64),
            tracetop: 0,
            trace: vec![
                StackTrace {
                    fun: NONE,
                    name: None,
                    line: 0,
                    col: 0,
                    stack: 0,
                };
                JS_ENVLIMIT
            ],
            trystk: Vec::with_capacity(16),
            runlimit: 0,
            memlimit: 0,
            scratch: String::new(),
            sources: rustc_hash::FxHashMap::default(),
            symbol_registry: rustc_hash::FxHashMap::default(),
            #[cfg(any(feature = "modules", feature = "timers"))]
            timers: Vec::new(),
            #[cfg(any(feature = "modules", feature = "timers"))]
            next_timer_id: 1,
        };
        st.trace[0] = StackTrace {
            fun: NONE,
            name: Some(CompactString::from("-top-")),
            line: 0,
            col: 0,
            stack: 0,
        };
        crate::builtins::init(&mut st);
        st
    }

    // ------------------------------------------------------------------
    // reporting and limits
    // ------------------------------------------------------------------

    pub fn report(&self, message: &str) {
        eprintln!("{}", message);
    }

    pub fn setlimit(&mut self, runlimit: i32, memlimit: i32) {
        self.runlimit = runlimit;
        self.memlimit = memlimit;
    }

    // ------------------------------------------------------------------
    // stack machinery
    // ------------------------------------------------------------------

    #[inline]
    pub fn checkstack(&self, n: usize) -> bool {
        self.top + n < JS_STACKSIZE
    }

    /// Translate an API stack index to an absolute index (stackidx).
    /// Negative indices count down from the top; non-negative from bot.
    #[inline]
    pub fn si(&self, idx: i32) -> Option<usize> {
        let i = if idx < 0 {
            self.top as i64 + idx as i64
        } else {
            self.bot as i64 + idx as i64
        };
        if i < 0 || i >= self.top as i64 {
            None
        } else {
            Some(i as usize)
        }
    }

    #[inline]
    pub fn stackidx(&self, idx: i32) -> &Value {
        match self.si(idx) {
            Some(i) => &self.stack[i],
            None => &Value::Undefined,
        }
    }

    #[inline]
    pub fn stackidx_mut(&mut self, idx: i32) -> &mut Value {
        match self.si(idx) {
            Some(i) => &mut self.stack[i],
            None => panic!("stack index out of range"),
        }
    }

    #[inline]
    pub fn tovalue(&self, idx: i32) -> &Value {
        self.stackidx(idx)
    }

    fn stack_overflow<T>(&mut self) -> R<T> {
        // write the error value directly (never recurse into push_value)
        let v = Value::LitStr(self.heap.lit("stack overflow"));
        if self.top < JS_STACKSIZE {
            self.stack[self.top] = v.clone();
            self.top += 1;
        }
        Err(v)
    }

    fn try_overflow<T>(&mut self) -> R<T> {
        let v = Value::LitStr(self.heap.lit("exception stack overflow"));
        if self.top < JS_STACKSIZE {
            self.stack[self.top] = v.clone();
            self.top += 1;
        }
        Err(v)
    }

    #[inline]
    pub fn push_value(&mut self, v: Value) -> R<()> {
        // CHECKSTACK(1): leave room for the error value itself
        if self.top + 1 >= JS_STACKSIZE {
            return self.stack_overflow();
        }
        self.stack[self.top] = v;
        self.top += 1;
        Ok(())
    }

    pub fn push_undefined(&mut self) -> R<()> {
        self.push_value(Value::Undefined)
    }

    pub fn push_null(&mut self) -> R<()> {
        self.push_value(Value::Null)
    }

    pub fn push_boolean(&mut self, v: bool) -> R<()> {
        self.push_value(Value::Boolean(v))
    }

    pub fn push_number(&mut self, v: f64) -> R<()> {
        self.push_value(Value::Number(v))
    }

    pub fn push_string(&mut self, v: &str) -> R<()> {
        if v.len() > JS_STRLIMIT {
            return self.range_error("invalid string length");
        }
        self.push_value(Value::String(CompactString::new(v)))
    }

    pub fn push_lstring(&mut self, v: &str) -> R<()> {
        self.push_string(v)
    }

    pub fn push_literal(&mut self, v: &str) -> R<()> {
        let i = self.heap.lit(v);
        self.push_value(Value::LitStr(i))
    }

    pub fn push_string_rc(&mut self, v: CompactString) -> R<()> {
        if v.len() > JS_STRLIMIT {
            return self.range_error("invalid string length");
        }
        self.push_value(Value::String(v))
    }

    /// Push an existing string value (any string variant).
    pub fn push_string_value(&mut self, v: Value) -> R<()> {
        self.push_value(v)
    }

    pub fn push_object(&mut self, o: ObjRef) -> R<()> {
        self.push_value(Value::Object(o))
    }

    pub fn push_global(&mut self) -> R<()> {
        self.push_object(self.g)
    }

    pub fn currentfunction(&mut self) -> R<()> {
        let v = if self.bot > 0 {
            self.stack[self.bot - 1].clone()
        } else {
            Value::Undefined
        };
        self.push_value(v)
    }

    #[inline]
    pub fn top_value(&self) -> Value {
        self.stack[self.top - 1].clone()
    }

    #[inline]
    pub fn gettop(&self) -> i32 {
        (self.top - self.bot) as i32
    }

    pub fn pop(&mut self, n: usize) {
        self.top = self.top.saturating_sub(n).max(self.bot);
    }

    pub fn pop_value(&mut self) -> Value {
        let v = self.top_value();
        self.pop(1);
        v
    }

    pub fn remove(&mut self, idx: i32) -> R<()> {
        let i = match self.si(idx) {
            Some(i) if i >= self.bot => i,
            _ => return self.error("stack error!"),
        };
        self.stack[i..self.top].rotate_left(1);
        self.top -= 1;
        Ok(())
    }

    pub fn replace(&mut self, idx: i32) -> R<()> {
        let i = match self.si(idx) {
            Some(i) if i >= self.bot => i,
            _ => return self.error("stack error!"),
        };
        self.top -= 1;
        self.stack.swap(i, self.top);
        Ok(())
    }

    pub fn copy(&mut self, idx: i32) -> R<()> {
        let v = self.stackidx(idx).clone();
        self.push_value(v)
    }

    pub fn dup(&mut self) -> R<()> {
        let v = self.top_value();
        self.push_value(v)
    }

    pub fn dup2(&mut self) -> R<()> {
        if self.top + 2 > JS_STACKSIZE {
            return self.stack_overflow();
        }
        self.stack[self.top] = self.stack[self.top - 2].clone();
        self.stack[self.top + 1] = self.stack[self.top - 1].clone();
        self.top += 2;
        Ok(())
    }

    pub fn rot2(&mut self) {
        // A B -> B A
        self.stack.swap(self.top - 2, self.top - 1);
    }

    pub fn rot3(&mut self) {
        // A B C -> C A B
        self.stack[self.top - 3..self.top].rotate_right(1);
    }

    pub fn rot4(&mut self) {
        // A B C D -> D A B C
        self.stack[self.top - 4..self.top].rotate_right(1);
    }

    pub fn rot2pop1(&mut self) {
        // A B -> B
        self.top -= 1;
        self.stack.swap(self.top - 1, self.top);
    }

    pub fn rot3pop2(&mut self) {
        // A B C -> C
        self.top -= 2;
        self.stack.swap(self.top - 1, self.top + 1);
    }

    pub fn rot(&mut self, n: i32) {
        self.stack[self.top - n as usize..self.top].rotate_right(1);
    }

    // ------------------------------------------------------------------
    // type predicates
    // ------------------------------------------------------------------

    pub fn isdefined(&self, idx: i32) -> bool {
        !self.stackidx(idx).is_undefined()
    }
    pub fn isundefined(&self, idx: i32) -> bool {
        self.stackidx(idx).is_undefined()
    }
    pub fn isnull(&self, idx: i32) -> bool {
        self.stackidx(idx).is_null()
    }
    pub fn isboolean(&self, idx: i32) -> bool {
        self.stackidx(idx).is_boolean()
    }
    pub fn isnumber(&self, idx: i32) -> bool {
        self.stackidx(idx).is_number()
    }
    pub fn isstring(&self, idx: i32) -> bool {
        self.stackidx(idx).is_string()
    }
    pub fn isprimitive(&self, idx: i32) -> bool {
        self.stackidx(idx).is_primitive()
    }
    pub fn isobject(&self, idx: i32) -> bool {
        self.stackidx(idx).is_object()
    }
    pub fn iscoercible(&self, idx: i32) -> bool {
        !self.isundefined(idx) && !self.isnull(idx)
    }

    pub fn iscallable(&self, idx: i32) -> bool {
        match self.stackidx(idx) {
            Value::Object(r) => matches!(
                self.heap.obj(*r).class,
                Class::Function | Class::Script | Class::CFunction
            ),
            _ => false,
        }
    }

    pub fn isarray(&self, idx: i32) -> bool {
        match self.stackidx(idx) {
            Value::Object(r) => self.heap.obj(*r).class == Class::Array,
            _ => false,
        }
    }

    pub fn isregexp(&self, idx: i32) -> bool {
        match self.stackidx(idx) {
            Value::Object(r) => self.heap.obj(*r).class == Class::Regexp,
            _ => false,
        }
    }

    pub fn iserror(&self, idx: i32) -> bool {
        match self.stackidx(idx) {
            Value::Object(r) => self.heap.obj(*r).class == Class::Error,
            _ => false,
        }
    }

    pub fn typeof_(&self, idx: i32) -> &'static str {
        match self.stackidx(idx) {
            Value::Undefined => "undefined",
            Value::Null => "object",
            Value::Boolean(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) | Value::LitStr(_) => "string",
            Value::Object(r) => match self.heap.obj(*r).class {
                Class::Function | Class::CFunction => "function",
                Class::Symbol => "symbol",
                _ => "object",
            },
        }
    }

    // ------------------------------------------------------------------
    // conversions (jsvalue.c)
    // ------------------------------------------------------------------

    /// ToBoolean() on a stack slot (jsV_toboolean).
    pub fn toboolean(&self, idx: i32) -> bool {
        match self.stackidx(idx) {
            Value::Undefined | Value::Null => false,
            Value::Boolean(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::String(_) | Value::LitStr(_) => !self.heap.js_str(self.stackidx(idx)).is_empty(),
            Value::Object(_) => true,
        }
    }

    pub fn tonumber(&mut self, idx: i32) -> R<f64> {
        let v = self.stackidx(idx).clone();
        Ok(match v {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Boolean(b) => b as i32 as f64,
            Value::Number(n) => n,
            Value::String(_) | Value::LitStr(_) => number::string_to_number(self.heap.js_str(&v)),
            Value::Object(_) => {
                self.toprimitive(idx, Hint::Number)?;
                self.tonumber(idx)?
            }
        })
    }

    pub fn tointeger(&mut self, idx: i32) -> R<i32> {
        Ok(number::number_to_integer(self.tonumber(idx)?))
    }

    pub fn toint32(&mut self, idx: i32) -> R<i32> {
        Ok(number::number_to_int32(self.tonumber(idx)?))
    }

    pub fn touint32(&mut self, idx: i32) -> R<u32> {
        Ok(number::number_to_uint32(self.tonumber(idx)?))
    }

    pub fn toint16(&mut self, idx: i32) -> R<i16> {
        Ok(number::number_to_int16(self.tonumber(idx)?))
    }

    pub fn touint16(&mut self, idx: i32) -> R<u16> {
        Ok(number::number_to_uint16(self.tonumber(idx)?))
    }

    /// ToString() on a stack slot (jsV_tostring); returns an owned CompactStr.
    pub fn tostring(&mut self, idx: i32) -> R<CompactString> {
        let v = self.stackidx(idx).clone();
        match v {
            Value::Undefined => Ok(self.heap.intern("undefined")),
            Value::Null => Ok(self.heap.intern("null")),
            Value::Boolean(b) => Ok(self.heap.intern(if b { "true" } else { "false" })),
            Value::String(_) | Value::LitStr(_) => Ok(self.heap.js_rcstr(&v)),
            Value::Number(n) => {
                let s = number::number_to_string(n);
                let rc = self.heap.intern(&s);
                *self.stackidx_mut(idx) = Value::String(rc.clone());
                Ok(rc)
            }
            Value::Object(_) => {
                self.toprimitive(idx, Hint::String)?;
                self.tostring(idx)
            }
        }
    }

    /// Convenience: a literal string value.
    fn litstr(&mut self, s: &str) -> Value {
        Value::LitStr(self.heap.lit(s))
    }

    /// jsV_toString / jsV_valueOf helper: call obj.name() and return true
    /// if it produced a primitive (left on the stack top).
    fn try_method(&mut self, obj: ObjRef, name: &str) -> R<bool> {
        self.push_object(obj)?;
        self.getproperty(-1, name)?;
        if self.iscallable(-1) {
            self.rot2();
            self.call(0)?;
            if self.isprimitive(-1) {
                return Ok(true);
            }
            self.pop(1);
            return Ok(false);
        }
        self.pop(2);
        Ok(false)
    }

    /// ToPrimitive() on a stack slot (jsV_toprimitive).
    pub fn toprimitive(&mut self, idx: i32, hint: Hint) -> R<()> {
        let obj = match self.stackidx(idx) {
            Value::Object(o) => *o,
            _ => return Ok(()),
        };
        let preferred = if hint == Hint::None {
            if self.heap.obj(obj).class == Class::Date {
                Hint::String
            } else {
                Hint::Number
            }
        } else {
            hint
        };
        #[allow(clippy::if_same_then_else)]
        let ok = if preferred == Hint::String {
            self.try_method(obj, "toString")? || self.try_method(obj, "valueOf")?
        } else {
            self.try_method(obj, "valueOf")? || self.try_method(obj, "toString")?
        };
        if ok {
            let nv = self.top_value();
            self.pop(1);
            *self.stackidx_mut(idx) = nv;
            return Ok(());
        }
        if self.strict {
            return self.type_error("cannot convert object to primitive");
        }
        let s = self.litstr("[object]");
        *self.stackidx_mut(idx) = s;
        Ok(())
    }

    /// ToObject() on a stack slot (jsV_toobject).
    pub fn toobject(&mut self, idx: i32) -> R<ObjRef> {
        let v = self.stackidx(idx).clone();
        let o = match v {
            Value::Object(r) => return Ok(r),
            Value::Undefined => return self.type_error("cannot convert undefined to object"),
            Value::Null => return self.type_error("cannot convert null to object"),
            Value::String(_) | Value::LitStr(_) => {
                let rc = self.heap.js_rcstr(&v);
                self.new_string_object(&rc)
            }
            Value::Boolean(b) => self.new_boolean_object(b),
            Value::Number(n) => self.new_number_object(n),
        };
        *self.stackidx_mut(idx) = Value::Object(o);
        Ok(o)
    }

    // ------------------------------------------------------------------
    // object constructors
    // ------------------------------------------------------------------

    pub fn new_object_class(&mut self, class: Class, prototype: Option<ObjRef>) -> ObjRef {
        self.heap.alloc_object(class, prototype)
    }

    pub fn new_boolean_object(&mut self, v: bool) -> ObjRef {
        let o = self
            .heap
            .alloc_object(Class::Boolean, Some(self.protos.boolean));
        self.heap.obj_mut(o).payload = Payload::Boolean(v);
        o
    }

    pub fn new_number_object(&mut self, v: f64) -> ObjRef {
        let o = self
            .heap
            .alloc_object(Class::Number, Some(self.protos.number));
        self.heap.obj_mut(o).payload = Payload::Number(v);
        o
    }

    pub fn new_string_object(&mut self, v: &str) -> ObjRef {
        let o = self
            .heap
            .alloc_object(Class::String, Some(self.protos.string));
        let length = utf::utflen(v) as i32;
        let rc = self.heap.intern(v);
        self.heap.obj_mut(o).payload = Payload::String(StringData { string: rc, length });
        o
    }

    pub fn newobjectx(&mut self) -> R<()> {
        let prototype = if self.isobject(-1) {
            Some(self.toobject(-1)?)
        } else {
            None
        };
        self.pop(1);
        let o = self.new_object_class(Class::Object, prototype);
        self.push_object(o)
    }

    pub fn newobject(&mut self) -> R<()> {
        let o = self.new_object_class(Class::Object, Some(self.protos.object));
        self.push_object(o)
    }

    pub fn newarguments(&mut self) -> R<()> {
        let o = self.new_object_class(Class::Arguments, Some(self.protos.object));
        self.push_object(o)
    }

    pub fn newarray(&mut self) -> R<()> {
        let o = self.new_object_class(Class::Array, Some(self.protos.array));
        self.heap.obj_mut(o).payload = Payload::Array(ArrayData {
            length: 0,
            simple: true,
            flat: thin_vec::ThinVec::new(),
        });
        self.push_object(o)
    }

    pub fn newboolean(&mut self, v: bool) -> R<()> {
        let o = self.new_boolean_object(v);
        self.push_object(o)
    }

    pub fn newnumber(&mut self, v: f64) -> R<()> {
        let o = self.new_number_object(v);
        self.push_object(o)
    }

    pub fn newstring(&mut self, v: &str) -> R<()> {
        let o = self.new_string_object(v);
        self.push_object(o)
    }

    /// js_newfunction: create a JS closure object with length/prototype.
    pub fn newfunction(&mut self, fun: u32, scope: ObjRef) -> R<()> {
        let numparams = self.heap.fun(fun).numparams;
        let o = self
            .heap
            .alloc_object(Class::Function, Some(self.protos.function));
        self.heap.obj_mut(o).payload = Payload::Function(FunctionData { fun, scope });
        self.push_object(o)?;
        self.push_number(numparams as f64)?;
        self.defproperty(-2, "length", JS_READONLY | JS_DONTENUM | JS_DONTCONF)?;
        self.newobject()?;
        self.copy(-2)?;
        self.defproperty(-2, "constructor", JS_DONTENUM)?;
        self.defproperty(-2, "prototype", JS_DONTENUM | JS_DONTCONF)?;
        Ok(())
    }

    /// js_newscript: create a script "function" object (global/eval code).
    pub fn newscript(&mut self, fun: u32, scope: ObjRef) -> R<()> {
        let o = self.heap.alloc_object(Class::Script, None);
        self.heap.obj_mut(o).payload = Payload::Function(FunctionData { fun, scope });
        self.push_object(o)
    }

    pub fn newcfunctionx(&mut self, cfun: CFunction, name: &str, length: i32) -> R<()> {
        let o = self
            .heap
            .alloc_object(Class::CFunction, Some(self.protos.function));
        self.heap.obj_mut(o).payload = Payload::CFunction(CFunctionData {
            name: self.heap.intern(name),
            function: cfun,
            constructor: None,
            length,
        });
        self.push_object(o)?;
        self.push_number(length as f64)?;
        self.defproperty(-2, "length", JS_READONLY | JS_DONTENUM | JS_DONTCONF)?;
        self.newobject()?;
        self.copy(-2)?;
        self.defproperty(-2, "constructor", JS_DONTENUM)?;
        self.defproperty(-2, "prototype", JS_DONTENUM | JS_DONTCONF)?;
        Ok(())
    }

    pub fn newcfunction(&mut self, cfun: CFunction, name: &str, length: i32) -> R<()> {
        self.newcfunctionx(cfun, name, length)
    }

    /// js_newcconstructor: prototype object is already on the stack.
    pub fn newcconstructor(
        &mut self,
        cfun: CFunction,
        ccon: CFunction,
        name: &str,
        length: i32,
    ) -> R<()> {
        let o = self
            .heap
            .alloc_object(Class::CFunction, Some(self.protos.function));
        self.heap.obj_mut(o).payload = Payload::CFunction(CFunctionData {
            name: self.heap.intern(name),
            function: cfun,
            constructor: Some(ccon),
            length,
        });
        self.push_object(o)?; // proto obj
        self.push_number(length as f64)?;
        self.defproperty(-2, "length", JS_READONLY | JS_DONTENUM | JS_DONTCONF)?;
        self.rot2(); // obj proto
        self.copy(-2)?; // obj proto obj
        self.defproperty(-2, "constructor", JS_DONTENUM)?;
        self.defproperty(-2, "prototype", JS_DONTENUM | JS_DONTCONF)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // errors and exceptions
    // ------------------------------------------------------------------

    /// js_throw: raise the value on top of the stack as an exception.
    pub fn throw<T>(&mut self) -> R<T> {
        Err(self.top_value())
    }

    /// Capture the current call stack as structured frames, innermost
    /// frame first (skipping `skip` innermost frames).
    pub fn capture_frames(&self, skip: i32) -> Vec<TraceFrame> {
        let n = self.tracetop as i32 - skip;
        if n <= 0 {
            return Vec::new();
        }
        (1..=n as usize)
            .rev()
            .map(|i| {
                let t = &self.trace[i];
                let (name, file) = self.trace_name_file(t);
                TraceFrame {
                    name,
                    file,
                    line: t.line,
                    col: t.col,
                }
            })
            .collect()
    }

    /// Build the stack trace string from captured frames (jsB_stacktrace).
    pub fn frames_to_string(frames: &[TraceFrame]) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for t in frames {
            if t.line > 0 {
                if !t.name.is_empty() {
                    let _ = write!(out, "\n\tat {} ({}:{}:{})", t.name, t.file, t.line, t.col);
                } else {
                    let _ = write!(out, "\n\tat {}:{}:{}", t.file, t.line, t.col);
                }
            } else {
                let _ = write!(out, "\n\tat {} ({})", t.name, t.file);
            }
        }
        out
    }

    /// js_newerrorx: create an error object with message and stackTrace.
    pub fn new_errorx(&mut self, message: &str, proto: ObjRef) -> R<Value> {
        let frames = self.capture_frames(0);
        self.new_errorx_frames(message, proto, frames)
    }

    /// js_newerrorx with an explicit set of trace frames.
    pub fn new_errorx_frames(
        &mut self,
        message: &str,
        proto: ObjRef,
        frames: Vec<TraceFrame>,
    ) -> R<Value> {
        let o = self.heap.alloc_object(Class::Error, Some(proto));
        let has_frames = !frames.is_empty();
        let trace = Self::frames_to_string(&frames);
        if has_frames {
            self.heap.obj_mut(o).payload = Payload::Error(ErrorData { frames: frames.into() });
        }
        // Derive the error name from the prototype's "name" property
        // (e.g. SyntaxError.prototype.name == "SyntaxError").
        let name: CompactString = self
            .heap
            .get_own_property(proto, "name")
            .and_then(|p| {
                if p.value.is_string() {
                    Some(self.heap.js_rcstr(&p.value))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| self.heap.intern("Error"));
        self.push_object(o)?;
        self.push_string_rc(name)?;
        self.setproperty(-2, "name")?;
        self.push_string(message)?;
        self.setproperty(-2, "message")?;
        if has_frames {
            self.push_string(&trace)?;
            self.setproperty(-2, "stackTrace")?;
        }
        Ok(self.pop_value())
    }

    /// A syntax error with an explicit source location (used by the lexer,
    /// parser and compiler, which have no JS call stack yet).
    pub fn syntax_error_loc<T>(&mut self, msg: &str, file: &str, line: u32, col: u32) -> R<T> {
        let full = format!("{}:{}:{}: {}", file, line, col, msg);
        let frames = vec![TraceFrame {
            name: CompactString::new(""),
            file: self.heap.intern(file),
            line,
            col,
        }];
        let v = self.new_errorx_frames(&full, self.protos.syntax_error, frames)?;
        Err(v)
    }

    def_error!(error, error);
    def_error!(eval_error, eval_error);
    def_error!(range_error, range_error);
    def_error!(reference_error, reference_error);
    def_error!(syntax_error, syntax_error);
    def_error!(type_error, type_error);
    def_error!(uri_error, uri_error);

    /// Host-side protected call: push a try frame, run `f`, restore state
    /// and report the error value on the stack on exception (js_try).
    pub fn protect<F>(&mut self, f: F) -> R<()>
    where
        F: FnOnce(&mut Self) -> R<()>,
    {
        if self.trystk.len() >= JS_TRYLIMIT {
            return self.try_overflow();
        }
        let frame = TryFrame {
            e: self.e,
            envtop: self.envstack.len(),
            tracetop: self.tracetop,
            top: self.top,
            bot: self.bot,
            strict: self.strict,
            catch_pc: None,
        };
        self.trystk.push(frame);
        let r = f(self);
        let frame = self.trystk.pop().expect("try frame");
        if let Err(v) = r {
            self.restore_frame(&frame);
            // Push a copy onto the stack so report_error(-1) can find it.
            // If push fails (stack overflow), we still return the original error.
            let _ = self.push_value(v.clone());
            return Err(v);
        }
        Ok(())
    }

    #[inline]
    pub fn restore_frame(&mut self, frame: &TryFrame) {
        self.e = frame.e;
        self.envstack.truncate(frame.envtop);
        self.tracetop = frame.tracetop;
        self.top = frame.top;
        self.bot = frame.bot;
        self.strict = frame.strict;
    }

    // ------------------------------------------------------------------
    // environments (jsrun.c)
    // ------------------------------------------------------------------

    pub fn new_environment(&mut self, variables: ObjRef, outer: Option<ObjRef>) -> ObjRef {
        self.heap.alloc_env(variables, outer)
    }

    pub fn initvar(&mut self, name: &str, idx: i32) -> R<()> {
        let vars = self.heap.env(self.e).variables;
        let v = self.stackidx(idx).clone();
        self.def_property_raw(
            vars,
            name,
            JS_DONTENUM | JS_DONTCONF,
            Some(v),
            None,
            None,
            false,
        )
    }

    pub fn hasvar(&mut self, name: &str) -> R<bool> {
        let mut e = Some(self.e);
        while let Some(er) = e {
            let env = self.heap.env(er);
            let vars = env.variables;
            if let Some(prop) = self.heap.get_property(vars, name) {
                let prop = prop.clone();
                if let Some(getter) = prop.getter {
                    self.push_object(getter)?;
                    self.push_object(vars)?;
                    self.call(0)?;
                } else {
                    self.push_value(prop.value)?;
                }
                return Ok(true);
            }
            e = env.outer;
        }
        Ok(false)
    }

    pub fn setvar(&mut self, name: &str) -> R<()> {
        let mut e = Some(self.e);
        while let Some(er) = e {
            let env = self.heap.env(er);
            let vars = env.variables;
            let outer = env.outer;
            if let Some(prop) = self.heap.get_property(vars, name) {
                let prop = prop.clone();
                if let Some(setter) = prop.setter {
                    self.push_object(setter)?;
                    self.push_object(vars)?;
                    self.copy(-3)?;
                    self.call(1)?;
                    self.pop(1);
                    return Ok(());
                }
                if prop.atts & JS_READONLY == 0 {
                    let v = self.stackidx(-1).clone();
                    self.heap
                        .set_property(vars, name)
                        .expect("existing property")
                        .value = v;
                } else if self.strict {
                    return self.type_error(&format!("'{}' is read-only", name));
                }
                return Ok(());
            }
            e = outer;
        }
        if self.strict {
            return self.reference_error(&format!("assignment to undeclared variable '{}'", name));
        }
        let g = self.g;
        self.set_property(g, name, false)?;
        Ok(())
    }

    pub fn delvar(&mut self, name: &str) -> R<bool> {
        let mut e = Some(self.e);
        while let Some(er) = e {
            let env = self.heap.env(er);
            let vars = env.variables;
            let outer = env.outer;
            if let Some(prop) = self.heap.get_own_property(vars, name) {
                if prop.atts & JS_DONTCONF != 0 {
                    if self.strict {
                        return self.type_error(&format!("'{}' is non-configurable", name));
                    }
                    return Ok(false);
                }
                self.heap.del_property(vars, name);
                return Ok(true);
            }
            e = outer;
        }
        let g = self.g;
        self.del_property(g, name)
    }

    // ------------------------------------------------------------------
    // property access (jsrun.c)
    // ------------------------------------------------------------------

    /// Move the flat array part into the property map (jsR_unflattenarray).
    pub fn unflatten_array(&mut self, obj: ObjRef) {
        let is_simple = matches!(
            &self.heap.obj(obj).payload,
            Payload::Array(a) if a.simple
        );
        if self.heap.obj(obj).class == Class::Array && is_simple {
            let flat = match &mut self.heap.obj_mut(obj).payload {
                Payload::Array(a) => {
                    a.simple = false;
                    std::mem::take(&mut a.flat)
                }
                _ => unreachable!(),
            };
            let mut buf = itoa::Buffer::new();
            for (i, v) in flat.into_iter().enumerate() {
                let name = buf.format(i);
                if let Some(p) = self.heap.set_property(obj, name) {
                    p.value = v;
                }
            }
        }
    }

    fn push_rune(&mut self, rune: Option<u32>) -> R<()> {
        match rune {
            Some(r) => {
                let mut s = String::new();
                utf::push_rune(&mut s, r);
                self.push_string(&s)
            }
            None => self.push_undefined(),
        }
    }

    /// jsR_hasproperty: pushes the value and returns true if found.
    pub fn has_property(&mut self, obj: ObjRef, name: &str) -> R<bool> {
        let class = self.heap.obj(obj).class;
        match class {
            Class::Array => {
                if name == "length" {
                    let len = match &self.heap.obj(obj).payload {
                        Payload::Array(a) => a.length,
                        _ => 0,
                    };
                    self.push_number(len as f64)?;
                    return Ok(true);
                }
                let simple = matches!(&self.heap.obj(obj).payload, Payload::Array(a) if a.simple);
                if simple && let Some(k) = is_array_index(name) {
                    let flat_len = match &self.heap.obj(obj).payload {
                        Payload::Array(a) => a.flat.len(),
                        _ => 0,
                    };
                    if k >= 0 && (k as usize) < flat_len {
                        let v = match &self.heap.obj(obj).payload {
                            Payload::Array(a) => a.flat[k as usize].clone(),
                            _ => Value::Undefined,
                        };
                        self.push_value(v)?;
                        return Ok(true);
                    }
                    return Ok(false);
                }
            }
            Class::String => {
                let (s, slen) = match &self.heap.obj(obj).payload {
                    Payload::String(sd) => (sd.string.clone(), sd.length),
                    _ => (CompactString::new(""), 0),
                };
                if name == "length" {
                    self.push_number(slen as f64)?;
                    return Ok(true);
                }
                if let Some(k) = is_array_index(name)
                    && k >= 0
                    && k < slen
                {
                    return self.push_rune(utf::runeat(&s, k as usize)).map(|_| true);
                }
            }
            Class::Regexp => {
                let matched = matches!(
                    name,
                    "source" | "global" | "ignoreCase" | "multiline" | "lastIndex"
                );
                if matched {
                    let (source, flags, last) = match &self.heap.obj(obj).payload {
                        Payload::Regexp(r) => (r.source.clone(), r.flags, r.last),
                        _ => unreachable!(),
                    };
                    match name {
                        "source" => self.push_string_rc(source)?,
                        "global" => self.push_boolean(flags & crate::value::JS_REGEXP_G != 0)?,
                        "ignoreCase" => {
                            self.push_boolean(flags & crate::value::JS_REGEXP_I != 0)?
                        }
                        "multiline" => self.push_boolean(flags & crate::value::JS_REGEXP_M != 0)?,
                        "lastIndex" => self.push_number(last as f64)?,
                        _ => unreachable!(),
                    }
                    return Ok(true);
                }
            }
            Class::Arguments => {
                // mapped arguments: alias index access to the parameters
                if let Some(k) = is_array_index(name) {
                    let target = match &self.heap.obj(obj).payload {
                        Payload::Arguments(a) => {
                            if k >= 0 && (k as u32) < a.mapped && !a.deleted.contains(&(k as u32)) {
                                let varname: CompactString = self.heap.fun(a.fun).vartab[k as usize].clone();
                                Some((a.env, varname))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some((env, varname)) = target {
                        let vars = self.heap.env(env).variables;
                        return self.has_property(vars, &varname);
                    }
                }
            }
            _ => {}
        }

        if let Some(prop) = self.heap.get_property(obj, name) {
            // extract only what is needed instead of cloning the Property
            let (getter, value) = (prop.getter, prop.value.clone());
            if let Some(getter) = getter {
                self.push_object(getter)?;
                self.push_object(obj)?;
                self.call(0)?;
            } else {
                self.push_value(value)?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// jsR_getproperty
    pub fn get_property(&mut self, obj: ObjRef, name: &str) -> R<()> {
        if !self.has_property(obj, name)? {
            self.push_undefined()?;
        }
        Ok(())
    }

    /// jsR_hasindex
    pub fn has_index(&mut self, obj: ObjRef, k: i32) -> R<bool> {
        let simple = matches!(&self.heap.obj(obj).payload, Payload::Array(a) if a.simple);
        if self.heap.obj(obj).class == Class::Array && simple {
            let flat_len = match &self.heap.obj(obj).payload {
                Payload::Array(a) => a.flat.len(),
                _ => 0,
            };
            if k >= 0 && (k as usize) < flat_len {
                let v = match &self.heap.obj(obj).payload {
                    Payload::Array(a) => a.flat[k as usize].clone(),
                    _ => Value::Undefined,
                };
                self.push_value(v)?;
                return Ok(true);
            }
            return Ok(false);
        }
        let name = number::itoa(k);
        self.has_property(obj, &name)
    }

    /// jsR_getindex
    pub fn get_index(&mut self, obj: ObjRef, k: i32) -> R<()> {
        if !self.has_index(obj, k)? {
            self.push_undefined()?;
        }
        Ok(())
    }

    /// jsR_setarrayindex (simple arrays only, k <= flat_length).
    fn set_array_index(&mut self, obj: ObjRef, k: i32, value: Value) -> R<()> {
        let newlen = k + 1;
        if newlen > JS_ARRAYLIMIT {
            return self.range_error("array too large");
        }
        match &mut self.heap.obj_mut(obj).payload {
            Payload::Array(a) => {
                if newlen as usize > a.flat.len() {
                    a.flat.resize(newlen as usize, Value::Undefined);
                }
                if newlen > a.length {
                    a.length = newlen;
                }
                a.flat[k as usize] = value;
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// jsR_setproperty: set a property, honoring setters/attributes.
    /// `transient` is true when the base was a primitive (no-op non-strict).
    pub fn set_property(&mut self, obj: ObjRef, name: &str, transient: bool) -> R<()> {
        let class = self.heap.obj(obj).class;
        match class {
            Class::Array => {
                if name == "length" {
                    let rawlen = self.tonumber(-1)?;
                    let newlen = number::number_to_integer(rawlen);
                    if newlen as f64 != rawlen || newlen < 0 {
                        return self.range_error("invalid array length");
                    }
                    if newlen > JS_ARRAYLIMIT {
                        return self.range_error("array too large");
                    }
                    let simple =
                        matches!(&self.heap.obj(obj).payload, Payload::Array(a) if a.simple);
                    if simple {
                        match &mut self.heap.obj_mut(obj).payload {
                            Payload::Array(a) => {
                                a.length = newlen;
                                if (newlen as usize) <= a.flat.len() {
                                    a.flat.truncate(newlen as usize);
                                }
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        self.heap.resize_array(obj, newlen);
                    }
                    return Ok(());
                }
                if let Some(k) = is_array_index(name) {
                    let simple =
                        matches!(&self.heap.obj(obj).payload, Payload::Array(a) if a.simple);
                    if simple {
                        let flat_len = match &self.heap.obj(obj).payload {
                            Payload::Array(a) => a.flat.len(),
                            _ => 0,
                        };
                        if k >= 0 && (k as usize) <= flat_len {
                            let v = self.stackidx(-1).clone();
                            self.set_array_index(obj, k, v)?;
                        } else {
                            self.unflatten_array(obj);
                            if let Payload::Array(a) = &mut self.heap.obj_mut(obj).payload
                                && a.length < k + 1
                            {
                                a.length = k + 1;
                            }
                        }
                    } else if let Payload::Array(a) = &mut self.heap.obj_mut(obj).payload
                        && a.length < k + 1
                    {
                        a.length = k + 1;
                    }
                }
            }
            Class::String => {
                if name == "length" {
                    return self.readonly(name);
                }
                if let Some(k) = is_array_index(name) {
                    let slen = match &self.heap.obj(obj).payload {
                        Payload::String(s) => s.length,
                        _ => 0,
                    };
                    if k >= 0 && k < slen {
                        return self.readonly(name);
                    }
                }
            }
            Class::Regexp => match name {
                "source" | "global" | "ignoreCase" | "multiline" => return self.readonly(name),
                "lastIndex" => {
                    let v = self.tointeger(-1)?;
                    if let Payload::Regexp(r) = &mut self.heap.obj_mut(obj).payload {
                        r.last = v as i64;
                    }
                    return Ok(());
                }
                _ => {}
            },
            Class::Arguments => {
                // mapped arguments: alias index writes to the parameters
                if let Some(k) = is_array_index(name) {
                    let target = match &self.heap.obj(obj).payload {
                        Payload::Arguments(a) => {
                            if k >= 0 && (k as u32) < a.mapped && !a.deleted.contains(&(k as u32)) {
                                let varname: CompactString = self.heap.fun(a.fun).vartab[k as usize].clone();
                                Some((a.env, varname))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some((env, varname)) = target {
                        let vars = self.heap.env(env).variables;
                        return self.set_property(vars, &varname, false);
                    }
                }
            }
            _ => {}
        }

        // first try to find a setter in the prototype chain
        let (prop, own) = {
            let (p, own) = self.heap.get_property_x(obj, name);
            (p.cloned(), own)
        };
        if let Some(prop) = &prop {
            if let Some(setter) = prop.setter {
                let v = self.stackidx(-1).clone();
                self.push_object(setter)?;
                self.push_object(obj)?;
                self.push_value(v)?;
                self.call(1)?;
                self.pop(1);
                return Ok(());
            } else {
                if self.strict && prop.getter.is_some() {
                    return self.type_error(&format!(
                        "setting property '{}' that only has a getter",
                        name
                    ));
                }
                if prop.atts & JS_READONLY != 0 {
                    return self.readonly(name);
                }
            }
        }

        // property not found on this object, so create one
        if prop.is_none() || !own {
            if transient {
                if self.strict {
                    return self.type_error(&format!(
                        "cannot create property '{}' on transient object",
                        name
                    ));
                }
                return Ok(());
            }
            let v = self.stackidx(-1).clone();
            let strict = self.strict;
            match self.heap.set_property(obj, name) {
                Some(p) => {
                    if p.atts & JS_READONLY == 0 {
                        p.value = v;
                    } else {
                        return self.readonly(name);
                    }
                }
                None => {
                    if strict {
                        return self.type_error("object is non-extensible");
                    }
                    // non-strict: silently ignore
                }
            }
        } else {
            // update existing own property
            let v = self.stackidx(-1).clone();
            let p = self
                .heap
                .set_property(obj, name)
                .expect("existing property");
            if p.atts & JS_READONLY == 0 {
                p.value = v;
            } else {
                return self.readonly(name);
            }
        }
        Ok(())
    }

    /// The shared `readonly:` exit of jsR_setproperty: throws in strict
    /// mode, silently ignores the write otherwise.
    fn readonly(&mut self, name: &str) -> R<()> {
        if self.strict {
            return self.type_error(&format!("'{}' is read-only", name));
        }
        Ok(())
    }

    /// jsR_setindex
    pub fn set_index(&mut self, obj: ObjRef, k: i32, transient: bool) -> R<()> {
        let simple = matches!(&self.heap.obj(obj).payload, Payload::Array(a) if a.simple);
        let flat_len = match &self.heap.obj(obj).payload {
            Payload::Array(a) => a.flat.len(),
            _ => 0,
        };
        if self.heap.obj(obj).class == Class::Array && simple && k >= 0 && (k as usize) <= flat_len
        {
            let v = self.stackidx(-1).clone();
            self.set_array_index(obj, k, v)
        } else {
            let name = number::itoa(k);
            self.set_property(obj, &name, transient)
        }
    }

    /// jsR_defproperty (define own property with attributes/accessors).
    #[allow(clippy::too_many_arguments)]
    pub fn def_property_raw(
        &mut self,
        obj: ObjRef,
        name: &str,
        atts: u32,
        value: Option<Value>,
        getter: Option<ObjRef>,
        setter: Option<ObjRef>,
        throw: bool,
    ) -> R<()> {
        let class = self.heap.obj(obj).class;
        match class {
            Class::Array => {
                if name == "length" {
                    return self.readonly_or_throw(name, throw);
                }
                let simple = matches!(&self.heap.obj(obj).payload, Payload::Array(a) if a.simple);
                if simple {
                    self.unflatten_array(obj);
                }
            }
            Class::String => {
                if name == "length" {
                    return self.readonly_or_throw(name, throw);
                }
                if let Some(k) = is_array_index(name) {
                    let slen = match &self.heap.obj(obj).payload {
                        Payload::String(s) => s.length,
                        _ => 0,
                    };
                    if k >= 0 && k < slen {
                        return self.readonly_or_throw(name, throw);
                    }
                }
            }
            Class::Regexp => {
                if matches!(
                    name,
                    "source" | "global" | "ignoreCase" | "multiline" | "lastIndex"
                ) {
                    return self.readonly_or_throw(name, throw);
                }
            }
            _ => {}
        }

        let strict = self.strict;
        match self.heap.set_property(obj, name) {
            Some(p) => {
                if let Some(v) = value {
                    if p.atts & JS_READONLY == 0 {
                        p.value = v;
                    } else if self.strict {
                        return self.type_error(&format!("'{}' is read-only", name));
                    }
                }
                if let Some(g) = getter {
                    if p.atts & JS_DONTCONF == 0 {
                        p.getter = Some(g);
                    } else if self.strict {
                        return self.type_error(&format!("'{}' is non-configurable", name));
                    }
                }
                if let Some(s) = setter {
                    if p.atts & JS_DONTCONF == 0 {
                        p.setter = Some(s);
                    } else if self.strict {
                        return self.type_error(&format!("'{}' is non-configurable", name));
                    }
                }
                p.atts |= atts;
                Ok(())
            }
            None => {
                if strict {
                    self.type_error("object is non-extensible")
                } else {
                    Ok(())
                }
            }
        }
    }

    fn readonly_or_throw(&mut self, name: &str, throw: bool) -> R<()> {
        if self.strict || throw {
            return self.type_error(&format!("'{}' is read-only or non-configurable", name));
        }
        Ok(())
    }

    /// jsR_delproperty
    pub fn del_property(&mut self, obj: ObjRef, name: &str) -> R<bool> {
        let class = self.heap.obj(obj).class;
        match class {
            Class::Array => {
                if name == "length" {
                    return self.dontconf(name);
                }
                let simple = matches!(&self.heap.obj(obj).payload, Payload::Array(a) if a.simple);
                if simple {
                    self.unflatten_array(obj);
                }
            }
            Class::String => {
                if name == "length" {
                    return self.dontconf(name);
                }
                if let Some(k) = is_array_index(name) {
                    let slen = match &self.heap.obj(obj).payload {
                        Payload::String(s) => s.length,
                        _ => 0,
                    };
                    if k >= 0 && k < slen {
                        return self.dontconf(name);
                    }
                }
            }
            Class::Regexp => {
                if matches!(
                    name,
                    "source" | "global" | "ignoreCase" | "multiline" | "lastIndex"
                ) {
                    return self.dontconf(name);
                }
            }
            Class::Arguments => {
                // deleting a mapped index only breaks the link (ES5.1 10.6)
                if let Some(k) = is_array_index(name)
                    && k >= 0
                    && let Payload::Arguments(a) = &mut self.heap.obj_mut(obj).payload
                    && (k as u32) < a.mapped
                {
                    a.deleted.insert(k as u32);
                }
            }
            _ => {}
        }

        if let Some(prop) = self.heap.get_own_property(obj, name) {
            if prop.atts & JS_DONTCONF != 0 {
                return self.dontconf(name);
            }
            self.heap.del_property(obj, name);
        }
        Ok(true)
    }

    fn dontconf(&mut self, name: &str) -> R<bool> {
        if self.strict {
            return self.type_error(&format!("'{}' is non-configurable", name));
        }
        Ok(false)
    }

    /// jsR_delindex
    pub fn del_index(&mut self, obj: ObjRef, k: i32) -> R<()> {
        // Allow deleting last element of a simple array without unflattening
        let is_last_flat = matches!(
            &self.heap.obj(obj).payload,
            Payload::Array(a) if a.simple && k >= 0 && k as usize == a.flat.len().wrapping_sub(1)
        );
        if self.heap.obj(obj).class == Class::Array && is_last_flat {
            if let Payload::Array(a) = &mut self.heap.obj_mut(obj).payload {
                a.flat.truncate(k as usize);
            }
            return Ok(());
        }
        let name = number::itoa(k);
        self.del_property(obj, &name)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // stack-based property API (public C API)
    // ------------------------------------------------------------------

    pub fn getproperty(&mut self, idx: i32, name: &str) -> R<()> {
        let o = self.toobject(idx)?;
        self.get_property(o, name)
    }

    pub fn setproperty(&mut self, idx: i32, name: &str) -> R<()> {
        let transient = !self.isobject(idx);
        let o = self.toobject(idx)?;
        self.set_property(o, name, transient)?;
        self.pop(1);
        Ok(())
    }

    pub fn defproperty(&mut self, idx: i32, name: &str, atts: u32) -> R<()> {
        let v = self.stackidx(-1).clone();
        let o = self.toobject(idx)?;
        self.def_property_raw(o, name, atts, Some(v), None, None, true)?;
        self.pop(1);
        Ok(())
    }

    pub fn delproperty(&mut self, idx: i32, name: &str) -> R<()> {
        let o = self.toobject(idx)?;
        self.del_property(o, name)?;
        Ok(())
    }

    pub fn defaccessor(&mut self, idx: i32, name: &str, atts: u32) -> R<()> {
        let getter = self.tofunction(-2)?;
        let setter = self.tofunction(-1)?;
        let o = self.toobject(idx)?;
        self.def_property_raw(o, name, atts, None, getter, setter, true)?;
        self.pop(2);
        Ok(())
    }

    fn tofunction(&mut self, idx: i32) -> R<Option<ObjRef>> {
        let v = self.stackidx(idx);
        match v {
            Value::Undefined | Value::Null => Ok(None),
            Value::Object(r)
                if matches!(self.heap.obj(*r).class, Class::Function | Class::CFunction) =>
            {
                Ok(Some(*r))
            }
            _ => self.type_error("not a function"),
        }
    }

    pub fn hasproperty(&mut self, idx: i32, name: &str) -> R<bool> {
        let o = self.toobject(idx)?;
        self.has_property(o, name)
    }

    pub fn getindex(&mut self, idx: i32, i: i32) -> R<()> {
        let o = self.toobject(idx)?;
        self.get_index(o, i)
    }

    pub fn hasindex(&mut self, idx: i32, i: i32) -> R<bool> {
        let o = self.toobject(idx)?;
        self.has_index(o, i)
    }

    pub fn setindex(&mut self, idx: i32, i: i32) -> R<()> {
        let transient = !self.isobject(idx);
        let o = self.toobject(idx)?;
        self.set_index(o, i, transient)?;
        self.pop(1);
        Ok(())
    }

    pub fn delindex(&mut self, idx: i32, i: i32) -> R<()> {
        let o = self.toobject(idx)?;
        self.del_index(o, i)
    }

    pub fn getlength(&mut self, idx: i32) -> R<i32> {
        self.getproperty(idx, "length")?;
        let len = self.tointeger(-1)?;
        self.pop(1);
        Ok(len)
    }

    pub fn setlength(&mut self, idx: i32, len: i32) -> R<()> {
        self.push_number(len as f64)?;
        self.setproperty(if idx < 0 { idx - 1 } else { idx }, "length")
    }

    // ------------------------------------------------------------------
    // registry and global object
    // ------------------------------------------------------------------

    pub fn js_ref(&mut self) -> R<Rc<str>> {
        let s = match self.stackidx(-1) {
            Value::Undefined => self.heap.intern("_Undefined"),
            Value::Null => self.heap.intern("_Null"),
            Value::Boolean(b) => self.heap.intern(if *b { "_True" } else { "_False" }),
            Value::Object(r) => {
                let s = format!("@{}", r);
                self.heap.intern(&s)
            }
            _ => {
                let s = format!("{}", self.nextref);
                self.nextref += 1;
                self.heap.intern(&s)
            }
        };
        self.setregistry(&s)?;
        Ok(s.into())
    }

    pub fn js_unref(&mut self, r: &str) -> R<()> {
        self.delregistry(r)
    }

    pub fn getregistry(&mut self, name: &str) -> R<()> {
        let r = self.r;
        self.get_property(r, name)
    }

    pub fn setregistry(&mut self, name: &str) -> R<()> {
        let r = self.r;
        self.set_property(r, name, false)?;
        self.pop(1);
        Ok(())
    }

    pub fn delregistry(&mut self, name: &str) -> R<()> {
        let r = self.r;
        self.del_property(r, name)?;
        Ok(())
    }

    pub fn getglobal(&mut self, name: &str) -> R<()> {
        let g = self.g;
        self.get_property(g, name)
    }

    pub fn setglobal(&mut self, name: &str) -> R<()> {
        let g = self.g;
        self.set_property(g, name, false)?;
        self.pop(1);
        Ok(())
    }

    pub fn defglobal(&mut self, name: &str, atts: u32) -> R<()> {
        let v = self.stackidx(-1).clone();
        let g = self.g;
        self.def_property_raw(g, name, atts, Some(v), None, None, false)?;
        self.pop(1);
        Ok(())
    }

    pub fn delglobal(&mut self, name: &str) -> R<()> {
        let g = self.g;
        self.del_property(g, name)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // iterators (jsproperty.c)
    // ------------------------------------------------------------------

    pub fn pushiterator(&mut self, idx: i32, own: bool) -> R<()> {
        let o = self.toobject(idx)?;
        let io = self.heap.new_iterator(o, own);
        self.push_object(io)
    }

    pub fn nextiterator(&mut self, idx: i32) -> R<Option<CompactString>> {
        let o = self.toobject(idx)?;
        let mut scratch = std::mem::take(&mut self.scratch);
        let r = self.heap.next_iterator(o, &mut scratch);
        self.scratch = scratch;
        Ok(r)
    }

    // ------------------------------------------------------------------
    // function calls (jsrun.c)
    // ------------------------------------------------------------------

    fn save_scope(&mut self, new_e: ObjRef) -> R<()> {
        if self.envstack.len() + 1 >= JS_ENVLIMIT {
            return self.stack_overflow();
        }
        self.envstack.push(self.e);
        self.e = new_e;
        Ok(())
    }

    fn restore_scope(&mut self) {
        self.e = self.envstack.pop().expect("scope underflow");
    }

    fn call_lwfunction(&mut self, mut n: usize, f: u32, scope: ObjRef) -> R<()> {
        self.save_scope(scope)?;

        let (numparams, varlen) = {
            let fun = self.heap.fun(f);
            (fun.numparams, fun.vartab.len())
        };

        if n > numparams {
            self.pop(n - numparams);
            n = numparams;
        }

        for _ in n..varlen {
            self.push_undefined()?;
        }

        match crate::run::run(self, f) {
            Ok(()) => {
                let v = self.top_value();
                self.bot -= 1;
                self.top = self.bot;
                self.push_value(v)?;
                self.restore_scope();
                Ok(())
            }
            Err(e) => {
                self.restore_scope();
                Err(e)
            }
        }
    }

    fn call_function(&mut self, n: usize, f: u32, scope: ObjRef) -> R<()> {
        let vars = self.heap.alloc_object(Class::Object, None);
        let new_scope = self.new_environment(vars, Some(scope));
        self.save_scope(new_scope)?;

        let (numparams, varlen, want_arguments, vartab) = {
            let fun = self.heap.fun(f);
            (
                fun.numparams,
                fun.vartab.len(),
                fun.arguments,
                fun.vartab.clone(),
            )
        };

        if want_arguments {
            if self.strict {
                // strict mode: plain, unmapped arguments object
                self.newarguments()?;
                self.push_number(n as f64)?;
                self.defproperty(-2, "length", JS_DONTENUM)?;
                for i in 0..n {
                    self.copy(i as i32 + 1)?;
                    self.setindex(-2, i as i32)?;
                }
                self.initvar("arguments", -1)?;
                self.pop(1);
            } else {
                // non-strict: mapped arguments object (indices alias the
                // formal parameters, ES5.1 10.6)
                let ao = self
                    .heap
                    .alloc_object(Class::Arguments, Some(self.protos.object));
                {
                    let vars = self.heap.env(new_scope).variables;
                    let _ = vars;
                    let mapped = (n.min(numparams)) as u32;
                    self.heap.obj_mut(ao).payload =
                        Payload::Arguments(crate::object::ArgumentsData {
                            env: new_scope,
                            fun: f,
                            mapped,
                            n: n as u32,
                            deleted: std::collections::BTreeSet::new(),
                        });
                }
                self.push_object(ao)?;
                self.push_number(n as f64)?;
                self.defproperty(-2, "length", JS_DONTENUM)?;
                self.currentfunction()?;
                self.defproperty(-2, "callee", JS_DONTENUM)?;
                // extra (unmapped) arguments live as ordinary properties
                for i in numparams..n {
                    self.copy(i as i32 + 1)?;
                    self.setindex(-2, i as i32)?;
                }
                self.initvar("arguments", -1)?;
                self.pop(1);
            }
        }

        let mut i = 0;
        while i < n && i < numparams {
            let name = vartab[i].clone();
            self.initvar(&name, i as i32 + 1)?;
            i += 1;
        }
        self.pop(n);

        while i < varlen {
            let name = vartab[i].clone();
            self.push_undefined()?;
            self.initvar(&name, -1)?;
            self.pop(1);
            i += 1;
        }

        match crate::run::run(self, f) {
            Ok(()) => {
                let v = self.top_value();
                self.bot -= 1;
                self.top = self.bot;
                self.push_value(v)?;
                self.restore_scope();
                Ok(())
            }
            Err(e) => {
                self.restore_scope();
                Err(e)
            }
        }
    }

    fn call_script(&mut self, n: usize, f: u32, scope: ObjRef) -> R<()> {
        let has_scope = scope != NONE;
        if has_scope {
            self.save_scope(scope)?;
        }

        let vartab = self.heap.fun(f).vartab.clone();

        // scripts take no arguments
        self.pop(n);

        for name in vartab.iter() {
            // Bug 701886: don't redefine existing vars in eval/scripts
            if !self.hasvar(name)? {
                self.push_undefined()?;
                self.initvar(name, -1)?;
                self.pop(1);
            }
        }

        match crate::run::run(self, f) {
            Ok(()) => {
                let v = self.top_value();
                self.bot -= 1;
                self.top = self.bot;
                self.push_value(v)?;
                if has_scope {
                    self.restore_scope();
                }
                Ok(())
            }
            Err(e) => {
                if has_scope {
                    self.restore_scope();
                }
                Err(e)
            }
        }
    }

    fn call_cfunction(&mut self, n: usize, min: i32, f: CFunction) -> R<()> {
        let min = min.max(0) as usize;
        for _ in n..min {
            self.push_undefined()?;
        }

        let save_top = self.top;
        let r = f(self);
        if r.is_ok() {
            let v = if self.top > save_top {
                self.top_value()
            } else {
                Value::Undefined
            };
            self.bot -= 1;
            self.top = self.bot;
            self.push_value(v)?;
        }
        r
    }

    fn pushtrace(&mut self, entry: StackTrace) -> R<()> {
        if self.tracetop + 1 >= JS_ENVLIMIT {
            return self.error("call stack overflow");
        }
        self.tracetop += 1;
        self.trace[self.tracetop] = entry;
        Ok(())
    }

    /// Resolve a trace entry's display name and file.
    pub fn trace_name_file(&self, t: &StackTrace) -> (CompactString, CompactString) {
        if t.fun != NONE {
            let f = self.heap.fun(t.fun);
            (f.name.clone(), f.filename.clone())
        } else {
            (
                t.name.clone().unwrap_or_else(|| CompactString::new("")),
                CompactString::new("native"),
            )
        }
    }

    /// js_call: call a function object with n arguments.
    pub fn call(&mut self, n: usize) -> R<()> {
        if !self.iscallable(-(n as i32) - 2) {
            let t = self.typeof_(-(n as i32) - 2);
            return self.type_error(&format!("{} is not callable", t));
        }

        let obj = self.toobject(-(n as i32) - 2)?;

        let savebot = self.bot;
        self.bot = self.top - n - 1;

        let class = self.heap.obj(obj).class;
        let r = match class {
            Class::Function => {
                let (fun, scope) = match &self.heap.obj(obj).payload {
                    Payload::Function(fd) => (fd.fun, fd.scope),
                    _ => unreachable!(),
                };
                let (line, col, lightweight) = {
                    let f = self.heap.fun(fun);
                    (f.line, f.col, f.lightweight)
                };
                self.pushtrace(StackTrace {
                    fun,
                    name: None,
                    line,
                    col,
                    stack: self.bot,
                })?;
                let r = if lightweight {
                    self.call_lwfunction(n, fun, scope)
                } else {
                    self.call_function(n, fun, scope)
                };
                self.tracetop -= 1;
                r
            }
            Class::Script => {
                let (fun, scope) = match &self.heap.obj(obj).payload {
                    Payload::Function(fd) => (fd.fun, fd.scope),
                    _ => unreachable!(),
                };
                let (line, col) = {
                    let f = self.heap.fun(fun);
                    (f.line, f.col)
                };
                self.pushtrace(StackTrace {
                    fun,
                    name: None,
                    line,
                    col,
                    stack: self.bot,
                })?;
                let r = self.call_script(n, fun, scope);
                self.tracetop -= 1;
                r
            }
            Class::CFunction => {
                let (name, length, cfun) = match &self.heap.obj(obj).payload {
                    Payload::CFunction(cd) => (cd.name.clone(), cd.length, cd.function),
                    _ => unreachable!(),
                };
                self.pushtrace(StackTrace {
                    fun: NONE,
                    name: Some(name),
                    line: 0,
                    col: 0,
                    stack: self.bot,
                })?;
                let r = self.call_cfunction(n, length, cfun);
                self.tracetop -= 1;
                r
            }
            _ => unreachable!(),
        };

        self.bot = savebot;
        r
    }

    /// js_construct: call a constructor with n arguments.
    pub fn construct(&mut self, n: usize) -> R<()> {
        if !self.iscallable(-(n as i32) - 1) {
            let t = self.typeof_(-(n as i32) - 1);
            return self.type_error(&format!("{} is not callable", t));
        }

        let obj = self.toobject(-(n as i32) - 1)?;

        // built-in constructors create their own objects, give them a 'null' this
        if self.heap.obj(obj).class == Class::CFunction
            && let Payload::CFunction(cd) = &self.heap.obj(obj).payload
            && let Some(ccon) = cd.constructor
        {
            let (name, length) = (cd.name.clone(), cd.length);
            let savebot = self.bot;
            self.push_null()?;
            if n > 0 {
                self.rot(n as i32 + 1);
            }
            self.bot = self.top - n - 1;
            self.pushtrace(StackTrace {
                fun: NONE,
                name: Some(name),
                line: 0,
                col: 0,
                stack: self.bot,
            })?;
            let r = self.call_cfunction(n, length, ccon);
            self.tracetop -= 1;
            self.bot = savebot;
            return r;
        }

        // extract the function object's prototype property
        self.getproperty(-(n as i32) - 1, "prototype")?;
        let prototype = if self.isobject(-1) {
            Some(self.toobject(-1)?)
        } else {
            Some(self.protos.object)
        };
        self.pop(1);

        // create a new object with above prototype, shift it into the 'this' slot
        let newobj = self.new_object_class(Class::Object, prototype);
        self.push_object(newobj)?;
        if n > 0 {
            self.rot(n as i32 + 1);
        }

        // save a copy to return
        self.push_object(newobj)?;
        self.rot(n as i32 + 3);

        // call the function
        self.call(n)?;

        // if result is not an object, return the original object we created
        if !self.isobject(-1) {
            self.pop(1);
        } else {
            self.rot2pop1();
        }
        Ok(())
    }

    /// js_eval: direct eval call.
    pub fn eval(&mut self) -> R<()> {
        if !self.isstring(-1) {
            return Ok(());
        }
        let source = self.tostring(-1)?;
        self.loadeval("(eval)", &source)?;
        self.rot2pop1();
        self.copy(0)?; // copy 'this'
        self.call(0)
    }

    /// Indirect eval: evaluate in the global scope with the global object as
    /// `this`; the code is non-strict unless it carries a "use strict"
    /// directive (ES5.1 10.4.2).
    pub fn indirect_eval(&mut self) -> R<()> {
        if !self.isstring(1) {
            return self.copy(1);
        }
        let source = self.tostring(1)?;
        let ast = parse::parse(self, "(eval)", &source)?;
        let fun = compile::compile_script(self, &ast, false)?;
        let ge = self.ge;
        self.newscript(fun, ge)?;
        self.push_global()?;
        self.call(0)
    }

    pub fn pconstruct(&mut self, n: usize) -> bool {
        let savetop = self.top - n - 1;
        let r = self.protect(|j| j.construct(n));
        match r {
            Ok(()) => false,
            Err(_) => {
                let v = self.top_value();
                self.stack[savetop] = v;
                self.top = savetop + 1;
                true
            }
        }
    }

    pub fn pcall(&mut self, n: usize) -> bool {
        let savetop = self.top - n - 2;
        let r = self.protect(|j| j.call(n));
        match r {
            Ok(()) => false,
            Err(_) => {
                let v = self.top_value();
                self.stack[savetop] = v;
                self.top = savetop + 1;
                true
            }
        }
    }

    // ------------------------------------------------------------------
    // non-trivial value operations (jsvalue.c)
    // ------------------------------------------------------------------

    pub fn instanceof(&mut self) -> R<bool> {
        if !self.iscallable(-1) {
            return self.type_error("instanceof: invalid operand");
        }
        if !self.isobject(-2) {
            return Ok(false);
        }
        self.getproperty(-1, "prototype")?;
        if !self.isobject(-1) {
            return self.type_error("instanceof: 'prototype' property is not an object");
        }
        let o = self.toobject(-1)?;
        self.pop(1);

        let mut v = self.toobject(-2)?;
        loop {
            match self.heap.obj(v).prototype {
                Some(p) => {
                    v = p;
                    if v == o {
                        return Ok(true);
                    }
                }
                None => return Ok(false),
            }
        }
    }

    pub fn concat(&mut self) -> R<()> {
        self.toprimitive(-2, Hint::None)?;
        self.toprimitive(-1, Hint::None)?;

        if self.isstring(-2) || self.isstring(-1) {
            let sa = self.tostring(-2)?;
            let sb = self.tostring(-1)?;
            self.pop(2);
            STATS.concat_calls.fetch_add(1, Ordering::Relaxed);
            let mut s = String::with_capacity(sa.len() + sb.len());
            STATS
                .concat_bytes
                .fetch_add((sa.len() + sb.len()) as u64, Ordering::Relaxed);
            s.push_str(&sa);
            s.push_str(&sb);
            // move the buffer into the arena (no second copy)
            let interned = self.heap.intern(&s);
            self.push_value(Value::String(interned))
        } else {
            let x = self.tonumber(-2)?;
            let y = self.tonumber(-1)?;
            self.pop(2);
            self.push_number(x + y)
        }
    }

    /// js_compare: returns (ordering, okay).
    pub fn compare(&mut self) -> R<(i32, bool)> {
        self.toprimitive(-2, Hint::Number)?;
        self.toprimitive(-1, Hint::Number)?;

        if self.isstring(-2) && self.isstring(-1) {
            let a = self.tostring(-2)?;
            let b = self.tostring(-1)?;
            let c = a.as_bytes().cmp(b.as_bytes());
            Ok((c as i32, true))
        } else {
            let x = self.tonumber(-2)?;
            let y = self.tonumber(-1)?;
            if x.is_nan() || y.is_nan() {
                return Ok((0, false));
            }
            Ok((
                if x < y {
                    -1
                } else if x > y {
                    1
                } else {
                    0
                },
                true,
            ))
        }
    }

    pub fn equal(&mut self) -> R<bool> {
        loop {
            let x = self.stackidx(-2).clone();
            let y = self.stackidx(-1).clone();

            if x.is_string() && y.is_string() {
                return Ok(self.heap.js_str(&x) == self.heap.js_str(&y));
            }

            match (&x, &y) {
                (Value::Undefined, Value::Undefined) => return Ok(true),
                (Value::Null, Value::Null) => return Ok(true),
                (Value::Number(a), Value::Number(b)) => return Ok(a == b),
                (Value::Boolean(a), Value::Boolean(b)) => return Ok(a == b),
                (Value::Object(a), Value::Object(b)) => return Ok(a == b),
                (Value::Null, Value::Undefined) => return Ok(true),
                (Value::Undefined, Value::Null) => return Ok(true),
                _ => {}
            }

            match (&x, &y) {
                (Value::Number(a), v) if v.is_string() => {
                    let a = *a;
                    let b = self.tonumber(-1)?;
                    return Ok(a == b);
                }
                (v, Value::Number(b)) if v.is_string() => {
                    let b = *b;
                    let a = self.tonumber(-2)?;
                    return Ok(a == b);
                }
                (Value::Boolean(b), _) => {
                    let b = *b;
                    *self.stackidx_mut(-2) = Value::Number(if b { 1.0 } else { 0.0 });
                    continue;
                }
                (_, Value::Boolean(b)) => {
                    let b = *b;
                    *self.stackidx_mut(-1) = Value::Number(if b { 1.0 } else { 0.0 });
                    continue;
                }
                (v, Value::Object(_)) if v.is_string() || v.is_number() => {
                    self.toprimitive(-1, Hint::None)?;
                    continue;
                }
                (Value::Object(_), v) if v.is_string() || v.is_number() => {
                    self.toprimitive(-2, Hint::None)?;
                    continue;
                }
                _ => return Ok(false),
            }
        }
    }

    pub fn strictequal(&mut self) -> R<bool> {
        let x = self.stackidx(-2).clone();
        let y = self.stackidx(-1).clone();
        if x.is_string() && y.is_string() {
            return Ok(self.heap.js_str(&x) == self.heap.js_str(&y));
        }
        Ok(match (x, y) {
            (Value::Undefined, Value::Undefined) => true,
            (Value::Null, Value::Null) => true,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            _ => false,
        })
    }

    // ------------------------------------------------------------------
    // loading and executing code (jsstate.c)
    // ------------------------------------------------------------------

    fn loadstringx(&mut self, filename: &str, source: &str, iseval: bool) -> R<()> {
        let fname = self.heap.intern(filename);
        self.sources.insert(fname, self.heap.intern(source));
        let ast = parse::parse(self, filename, source)?;
        let default_strict = if iseval {
            self.strict
        } else {
            self.default_strict
        };
        let fun = compile::compile_script(self, &ast, default_strict)?;
        let scope = if iseval {
            if self.strict { self.e } else { NONE }
        } else {
            self.ge
        };
        self.newscript(fun, scope)
    }

    pub fn loadeval(&mut self, filename: &str, source: &str) -> R<()> {
        self.loadstringx(filename, source, true)
    }

    pub fn loadstring(&mut self, filename: &str, source: &str) -> R<()> {
        self.loadstringx(filename, source, false)
    }

    pub fn loadfile(&mut self, filename: &str) -> R<()> {
        let data = match std::fs::read(filename) {
            Ok(d) => d,
            Err(e) => {
                return self.error(&format!("cannot open file '{}': {}", filename, e));
            }
        };
        let mut source = String::from_utf8_lossy(&data).into_owned();
        // skip first line if it starts with "#!"
        if source.starts_with("#!") {
            match source.find('\n') {
                Some(i) => source = source[i..].to_string(),
                None => source.clear(),
            }
        }
        let fname = self.heap.intern(filename);
        let src = self.heap.intern(&source);
        self.sources.insert(fname, src);
        self.loadstring(filename, &source)
    }

    pub fn dostring(&mut self, source: &str) -> i32 {
        let r = self.protect(|j| {
            j.loadstring("[string]", source)?;
            j.push_undefined()?;
            j.call(0)?;
            j.pop(1);
            Ok(())
        });
        match r {
            Ok(()) => 0,
            Err(_) => {
                self.report_error(-1);
                self.pop(1);
                1
            }
        }
    }

    pub fn dofile(&mut self, filename: &str) -> i32 {
        let r = self.protect(|j| {
            j.loadfile(filename)?;
            j.push_undefined()?;
            j.call(0)?;
            j.pop(1);
            Ok(())
        });
        match r {
            Ok(()) => 0,
            Err(_) => {
                self.report_error(-1);
                self.pop(1);
                1
            }
        }
    }

    pub fn ploadstring(&mut self, filename: &str, source: &str) -> i32 {
        match self.protect(|j| j.loadstring(filename, source)) {
            Ok(()) => 0,
            Err(_) => 1,
        }
    }

    pub fn ploadfile(&mut self, filename: &str) -> i32 {
        match self.protect(|j| j.loadfile(filename)) {
            Ok(()) => 0,
            Err(_) => 1,
        }
    }

    // ------------------------------------------------------------------
    // error reporting with miette diagnostics
    // ------------------------------------------------------------------

    /// Report an uncaught error value to stderr as a fancy diagnostic
    /// with a source snippet (when the location is known).
    #[cfg(feature = "cli")]
    pub fn report_error(&mut self, idx: i32) {
        let v = self.stackidx(idx).clone();
        let (kind, message, frames) = self.describe_error(&v);

        // find the primary (innermost) frame and its source
        let primary = frames.first();
        let src: Option<CompactString> = primary.and_then(|f| self.sources.get(&f.file).cloned());

        match (primary, src) {
            (Some(f), Some(source)) => {
                let (offset, len) = crate::diag::line_col_to_span(source.as_str(), f.line, f.col);
                let label = if message.is_empty() {
                    "error originated here".to_string()
                } else {
                    message.clone()
                };
                let diag = crate::diag::JsDiagnostic::new(
                    kind,
                    message,
                    miette::SourceSpan::new(offset.into(), len.max(1)),
                    label,
                    Vec::new(),
                );
                let named = miette::NamedSource::new(f.file.as_str(), source.to_string());
                eprintln!("{:?}", miette::Report::new(diag).with_source_code(named));
            }
            _ => {
                let diag = crate::diag::PlainDiagnostic::new(kind, message, Vec::new());
                eprintln!("{:?}", miette::Report::new(diag));
            }
        }

        // remaining stack frames as plain text (like mujs's stack trace)
        for f in frames.iter().skip(1) {
            eprintln!("\t{}", crate::diag::format_trace_frame(f));
        }
    }

    /// Report an uncaught error value to stderr as plain text
    /// (no `cli` feature: the classic mujs-style output).
    #[cfg(not(feature = "cli"))]
    pub fn report_error(&mut self, idx: i32) {
        let msg = self.trystring(idx, "Error");
        eprintln!("{}", msg);
    }

    /// Extract (kind, message, frames) from a thrown value.
    #[cfg(feature = "cli")]
    fn describe_error(&mut self, v: &Value) -> (String, String, ThinVec<TraceFrame>) {
        if let Value::Object(o) = v {
            let frames = match &self.heap.obj(*o).payload {
                Payload::Error(e) => e.frames.clone(),
                _ => ThinVec::new(),
            };
            let get_str = |name: &str| -> Option<String> {
                self.heap.get_property(*o, name).and_then(|p| {
                    if p.value.is_string() {
                        Some(self.heap.js_str(&p.value).to_string())
                    } else {
                        None
                    }
                })
            };
            let kind = get_str("name").unwrap_or_else(|| "Error".to_string());
            let message = get_str("message").unwrap_or_default();
            return (kind, message, frames);
        }
        // non-object thrown value: stringify it
        self.push_value(v.clone()).ok();
        let s = self
            .protect_result(|j| j.tostring(-1))
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "uncaught exception".to_string());
        self.pop(1);
        ("Uncaught exception".to_string(), s, Vec::new().into())
    }

    // ------------------------------------------------------------------
    // protected conversions (jsstate.c)
    // ------------------------------------------------------------------

    pub fn trystring(&mut self, idx: i32, error: &str) -> CompactString {
        match self.protect_result(|j| j.tostring(idx)) {
            Ok(s) => s,
            Err(_) => {
                self.pop(1);
                self.heap.intern(error)
            }
        }
    }

    pub fn trynumber(&mut self, idx: i32, error: f64) -> f64 {
        match self.protect_result(|j| j.tonumber(idx)) {
            Ok(v) => v,
            Err(_) => {
                self.pop(1);
                error
            }
        }
    }

    pub fn tryinteger(&mut self, idx: i32, error: i32) -> i32 {
        match self.protect_result(|j| j.tointeger(idx)) {
            Ok(v) => v,
            Err(_) => {
                self.pop(1);
                error
            }
        }
    }

    pub fn tryboolean(&mut self, idx: i32, error: bool) -> bool {
        match self.protect_result(|j| Ok(j.toboolean(idx))) {
            Ok(v) => v,
            Err(_) => {
                self.pop(1);
                error
            }
        }
    }

    /// Like protect, but returns a value produced by `f`.
    pub fn protect_result<T, F>(&mut self, f: F) -> Result<T, Value>
    where
        F: FnOnce(&mut Self) -> R<T>,
    {
        if self.trystk.len() >= JS_TRYLIMIT {
            let _ = self.try_overflow::<()>();
            return Err(self.pop_value());
        }
        let frame = TryFrame {
            e: self.e,
            envtop: self.envstack.len(),
            tracetop: self.tracetop,
            top: self.top,
            bot: self.bot,
            strict: self.strict,
            catch_pc: None,
        };
        self.trystk.push(frame);
        let r = f(self);
        let frame = self.trystk.pop().expect("try frame");
        match r {
            Ok(v) => Ok(v),
            Err(v) => {
                self.restore_frame(&frame);
                // Push a copy onto the stack so report_error(-1) can find it.
                let _ = self.push_value(v.clone());
                Err(v)
            }
        }
    }
}

/// js_isarrayindex: parse a canonical array index name.
pub fn is_array_index(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    if b[0] == b'0' {
        return if b.len() == 1 { Some(0) } else { None };
    }
    let mut n: i32 = 0;
    for &c in b {
        if c.is_ascii_digit() {
            if n >= i32::MAX / 10 {
                return None;
            }
            n = n * 10 + (c - b'0') as i32;
        } else {
            return None;
        }
    }
    Some(n)
}
