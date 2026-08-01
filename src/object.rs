//! Objects, properties, environments, functions and the garbage-collected
//! heap (replaces jsproperty.c, jsgc.c and jsintern.c).
//!
//! MuJS uses raw pointers and a mark/sweep collector over linked lists; the
//! Rust port uses index-based arenas which provide the same pointer-identity
//! semantics while staying memory safe. Property maps are B-trees ordered by
//! name, exactly matching the AA-tree iteration order used by MuJS (which
//! means property enumeration order is byte-wise lexicographic).

use crate::compile::Function;
use crate::state::{CFunction, State};
use crate::value::Value;
use compact_str::CompactString;
use indexmap::IndexMap;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use thin_vec::ThinVec;

pub type ObjRef = u32;
pub type EnvRef = u32;
pub type FunRef = u32;
pub type StrRef = u32;

pub const NONE: u32 = u32::MAX;

/// Object classes (enum js_Class in jsi.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Object,
    Array,
    Function,
    Script,
    CFunction,
    Error,
    Boolean,
    Number,
    String,
    Regexp,
    Date,
    Math,
    Json,
    Arguments,
    Iterator,
    Symbol,
    Map,
    WeakMap,
    Set,
}

/// A property entry (js_Property).
#[derive(Clone)]
pub struct Property {
    pub atts: u32,
    pub value: Value,
    pub getter: Option<ObjRef>,
    pub setter: Option<ObjRef>,
    /// insertion sequence number (for ES5 enumeration order)
    pub order: u32,
}

impl Property {
    pub fn new(order: u32) -> Property {
        Property {
            atts: 0,
            value: Value::Undefined,
            getter: None,
            setter: None,
            order,
        }
    }
}

impl Default for Property {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Compiled regular expression payload (js_Regexp).
pub struct RegexpData {
    pub prog: crate::regexp::Regexp,
    pub source: CompactString,
    pub flags: u32,
    /// lastIndex in UTF-16 code units (ES5); converted to byte offsets at
    /// match time.
    pub last: i64,
}

/// Array payload: dense "simple" part plus the spec length.
pub struct ArrayData {
    pub length: i32,
    pub simple: bool,
    pub flat: ThinVec<Value>,
}

/// Function/Script closure payload.
#[derive(Clone, Copy)]
pub struct FunctionData {
    pub fun: FunRef,
    pub scope: EnvRef,
}

/// Built-in (C) function payload.
#[derive(Clone)]
pub struct CFunctionData {
    pub name: CompactString,
    pub function: CFunction,
    pub constructor: Option<CFunction>,
    pub length: i32,
}

/// Map collection payload (ES6): stores key-value pairs as a ThinVec.
#[derive(Clone)]
pub struct MapData {
    pub entries: ThinVec<(crate::value::Value, crate::value::Value)>,
}

/// Set collection payload (ES6): stores unique values as a ThinVec.
#[derive(Clone)]
pub struct SetData {
    pub values: ThinVec<crate::value::Value>,
}

/// Symbol payload: a global symbol's description and unique registration key.
#[derive(Clone)]
pub struct SymbolData {
    pub description: CompactString,
    pub key: Option<CompactString>, // Some(key) if created via Symbol.for
}

/// Iterator object payload (js_Object.u.iter + js_Iterator).
pub struct IteratorData {
    pub target: ObjRef,
    pub i: u32,
    pub n: u32,
    pub keys: ThinVec<CompactString>,
    pub pos: usize,
}

/// ES6 iterator payload: stores a snapshot of values and a cursor position.
#[derive(Clone)]
pub struct ES6IteratorData {
    pub values: ThinVec<crate::value::Value>,
    pub pos: usize,
}

/// Non-strict arguments object payload (ES5.1 10.6 mapped arguments).
/// Indices < `mapped` are aliases of the formal parameters stored in `env`.
pub struct ArgumentsData {
    pub env: EnvRef,
    pub fun: FunRef,
    /// number of aliased indices = min(actual args, formal params)
    pub mapped: u32,
    /// actual number of arguments (for iterator prefix)
    pub n: u32,
    /// indices whose link was broken by `delete`
    pub deleted: std::collections::BTreeSet<u32>,
}

/// String payload: value plus its UTF-16 code unit length.
#[derive(Clone)]
pub struct StringData {
    pub string: CompactString,
    pub length: i32,
}

/// Class-specific payload (the union in js_Object).
pub enum Payload {
    None,
    Boolean(bool),
    Number(f64),
    String(StringData),
    Array(ArrayData),
    Arguments(ArgumentsData),
    Error(ErrorData),
    Function(FunctionData),
    CFunction(CFunctionData),
    Regexp(RegexpData),
    Iterator(IteratorData),
    ES6Iterator(ES6IteratorData),
    #[cfg(feature = "require")]
    Child(crate::builtins::modules::child_process::ChildData),
    #[cfg(feature = "symbol")]
    Symbol(SymbolData),
    Map(MapData),
    Set(SetData),
}

/// One captured stack frame for error diagnostics (innermost first).
#[derive(Clone)]
pub struct TraceFrame {
    pub name: CompactString,
    pub file: CompactString,
    pub line: u32,
    pub col: u32,
}

/// Error object payload: the stack trace captured at creation time.
pub struct ErrorData {
    pub frames: ThinVec<TraceFrame>,
}

/// A JavaScript object (js_Object).
pub struct Object {
    pub class: Class,
    pub extensible: bool,
    pub properties: IndexMap<CompactString, Property, FxBuildHasher>,
    pub prototype: Option<ObjRef>,
    pub payload: Payload,
    /// next insertion sequence number for new properties
    pub next_order: u32,
}

/// A lexical environment (js_Environment).
pub struct Environment {
    pub outer: Option<EnvRef>,
    pub variables: ObjRef,
}

/// A GC arena slot: freed slots hold None.
pub struct Slot<T> {
    pub mark: u8,
    pub value: Option<T>,
}

/// The garbage-collected heap.
pub struct Heap {
    pub objects: Vec<Slot<Object>>,
    pub envs: Vec<Slot<Environment>>,
    pub funs: Vec<Slot<Function>>,
    /// Permanent literal strings (JS_TLITSTR in mujs): string constants
    /// referenced by bytecode. They live forever, so no GC scan needed.
    pub lits: ThinVec<CompactString>,
    free_objects: Vec<ObjRef>,
    free_envs: Vec<EnvRef>,
    free_funs: Vec<FunRef>,
    /// Interned strings (jsintern.c): used for source text, identifiers and
    /// property names to reduce memory churn.
    intern: FxHashSet<CompactString>,
    /// Deduplication map for the literal table.
    litmap: FxHashMap<CompactString, u32>,
    pub gcmark: u8,
    pub gccounter: u32,
    pub gcthresh: u32,
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            objects: Vec::with_capacity(256),
            envs: Vec::with_capacity(64),
            funs: Vec::with_capacity(64),
            lits: ThinVec::with_capacity(64),
            free_objects: Vec::new(),
            free_envs: Vec::new(),
            free_funs: Vec::new(),
            intern: FxHashSet::default(),
            litmap: FxHashMap::default(),
            gcmark: 1,
            gccounter: 0,
            gcthresh: 0,
        }
    }

    /// Intern a string (js_intern).
    pub fn intern(&mut self, s: &str) -> CompactString {
        if let Some(cs) = self.intern.get(s) {
            return cs.clone();
        }
        let cs = CompactString::new(s);
        self.intern.insert(cs.clone());
        cs
    }

    /// Register a literal string, returning its (deduplicated) index.
    pub fn lit(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.litmap.get(s) {
            return i;
        }
        let cs = CompactString::new(s);
        let i = self.lits.len() as u32;
        self.lits.push(cs.clone());
        self.litmap.insert(cs, i);
        i
    }

    /// Borrowed view of a string value (immediate use only).
    #[inline]
    pub fn js_str<'a>(&'a self, v: &'a Value) -> &'a str {
        match v {
            Value::String(r) => r.as_ref(),
            Value::LitStr(i) => self.lits[*i as usize].as_str(),
            _ => "",
        }
    }

    /// Owned CompactString copy of a string value (for values that must survive calls).
    #[inline]
    pub fn js_rcstr(&self, v: &Value) -> CompactString {
        match v {
            Value::String(r) => CompactString::from(&**r),
            Value::LitStr(i) => self.lits[*i as usize].clone(),
            _ => CompactString::new(""),
        }
    }

    // -- allocation ---------------------------------------------------------

    pub fn alloc_object(&mut self, class: Class, prototype: Option<ObjRef>) -> ObjRef {
        let obj = Object {
            class,
            extensible: true,
            properties: IndexMap::default(),
            prototype,
            payload: Payload::None,
            next_order: 0,
        };
        self.gccounter += 1;
        match self.free_objects.pop() {
            Some(i) => {
                self.objects[i as usize] = Slot { mark: 0, value: Some(obj) };
                i
            }
            None => {
                self.objects.push(Slot { mark: 0, value: Some(obj) });
                (self.objects.len() - 1) as ObjRef
            }
        }
    }

    pub fn alloc_env(&mut self, variables: ObjRef, outer: Option<EnvRef>) -> EnvRef {
        let env = Environment { outer, variables };
        self.gccounter += 1;
        match self.free_envs.pop() {
            Some(i) => {
                self.envs[i as usize] = Slot { mark: 0, value: Some(env) };
                i
            }
            None => {
                self.envs.push(Slot { mark: 0, value: Some(env) });
                (self.envs.len() - 1) as EnvRef
            }
        }
    }

    pub fn alloc_fun(&mut self, fun: Function) -> FunRef {
        self.gccounter += 1;
        match self.free_funs.pop() {
            Some(i) => {
                self.funs[i as usize] = Slot { mark: 0, value: Some(fun) };
                i
            }
            None => {
                self.funs.push(Slot { mark: 0, value: Some(fun) });
                (self.funs.len() - 1) as FunRef
            }
        }
    }

    // -- accessors ------------------------------------------------------------

    #[inline]
    pub fn obj(&self, r: ObjRef) -> &Object {
        self.objects[r as usize].value.as_ref().expect("dangling object ref")
    }

    #[inline]
    pub fn obj_mut(&mut self, r: ObjRef) -> &mut Object {
        self.objects[r as usize].value.as_mut().expect("dangling object ref")
    }

    #[inline]
    pub fn env(&self, r: EnvRef) -> &Environment {
        self.envs[r as usize].value.as_ref().expect("dangling env ref")
    }

    #[inline]
    pub fn fun(&self, r: FunRef) -> &Function {
        self.funs[r as usize].value.as_ref().expect("dangling fun ref")
    }

    #[inline]
    pub fn fun_mut(&mut self, r: FunRef) -> &mut Function {
        self.funs[r as usize].value.as_mut().expect("dangling fun ref")
    }

    // -- low level property access (jsproperty.c) -----------------------------

    /// jsV_getownproperty
    pub fn get_own_property(&self, obj: ObjRef, name: &str) -> Option<&Property> {
        self.obj(obj).properties.get(name)
    }

    /// jsV_getproperty (walk the prototype chain)
    pub fn get_property(&self, obj: ObjRef, name: &str) -> Option<&Property> {
        let mut o = Some(obj);
        while let Some(r) = o {
            let object = self.obj(r);
            if let Some(p) = object.properties.get(name) {
                return Some(p);
            }
            o = object.prototype;
        }
        None
    }

    /// jsV_getpropertyx: also report whether the property is an own property.
    pub fn get_property_x(&self, obj: ObjRef, name: &str) -> (Option<&Property>, bool) {
        let mut own = true;
        let mut o = Some(obj);
        while let Some(r) = o {
            let object = self.obj(r);
            if let Some(p) = object.properties.get(name) {
                return (Some(p), own);
            }
            o = object.prototype;
            own = false;
        }
        (None, own)
    }

    /// Find an enumerable property in the chain (jsV_getenumproperty).
    pub fn get_enum_property(&self, obj: ObjRef, name: &str) -> Option<&Property> {
        let mut o = Some(obj);
        while let Some(r) = o {
            let object = self.obj(r);
            if let Some(p) = object.properties.get(name)
                && p.atts & crate::value::JS_DONTENUM == 0 {
                    return Some(p);
                }
            o = object.prototype;
        }
        None
    }

    /// jsV_setproperty: get or create an own property slot.
    /// Returns None when the object is non-extensible and the property is
    /// missing (callers decide whether that is an error).
    pub fn set_property(&mut self, obj: ObjRef, name: &str) -> Option<&mut Property> {
        // fast path: property already exists (no intern hash needed)
        if self.obj(obj).properties.contains_key(name) {
            return self.obj_mut(obj).properties.get_mut(name);
        }
        if !self.obj(obj).extensible {
            return None;
        }
        self.gccounter += 1;
        let key = self.intern(name);
        // single lookup: reserve the slot (with its insertion order) then
        // fill it, avoiding a second hash
        let order = self.obj_mut(obj).next_order;
        self.obj_mut(obj).next_order += 1;
        let prop = Property::new(order);
        Some(
            self.obj_mut(obj)
                .properties
                .entry(key)
                .or_insert(prop),
        )
    }

    /// jsV_delproperty
    pub fn del_property(&mut self, obj: ObjRef, name: &str) {
        self.obj_mut(obj).properties.shift_remove(name);
    }

    /// jsV_resizearray: delete array elements >= newlen for sparse arrays.
    pub fn resize_array(&mut self, obj: ObjRef, newlen: i32) {
        let (oldlen, count) = {
            let o = self.obj(obj);
            let (len, count) = match &o.payload {
                Payload::Array(a) => (a.length, o.properties.len()),
                _ => return,
            };
            (len, count)
        };
        if newlen < oldlen {
            if oldlen > (count as i32) * 2 {
                // sparse: iterate own keys and delete canonical indices >= newlen
                let keys: Vec<CompactString> = self.obj(obj).properties.keys().cloned().collect();
                for name in keys {
                    let k = crate::number::string_to_number(&name);
                    let k = crate::number::number_to_integer(k);
                    if k >= newlen
                        && name.as_ref() as &str == crate::number::number_to_string(k as f64)
                    {
                        self.del_property(obj, &name);
                    }
                }
            } else {
                for k in newlen..oldlen {
                    self.del_property(obj, &k.to_string());
                }
            }
        }
        if let Payload::Array(a) = &mut self.obj_mut(obj).payload {
            a.length = newlen;
        }
    }

    // -- iterators ------------------------------------------------------------

    /// Sort own property names into ES5 enumeration order: integer indices
    /// in ascending numeric order first, then all other keys in insertion
    /// Sort own property names into ES5 enumeration order: integer indices
    /// in ascending numeric order first, then all other keys in insertion
    /// order. Returns (integer_index_keys, insertion_ordered_keys).
    fn ordered_keys<I>(names: I) -> (ThinVec<CompactString>, ThinVec<CompactString>)
    where
        I: IntoIterator<Item = (CompactString, u32)>,
    {
        let mut ints: Vec<(u32, CompactString)> = Vec::new();
        let mut rest: Vec<(u32, CompactString)> = Vec::new();
        for (name, order) in names {
            if let Some(i) = crate::state::is_array_index(&name) {
                ints.push((i as u32, name));
            } else {
                rest.push((order, name));
            }
        }
        ints.sort_by_key(|(i, _)| *i);
        rest.sort_by_key(|(o, _)| *o);
        (
            ints.into_iter().map(|(_, n)| n).collect(),
            rest.into_iter().map(|(_, n)| n).collect(),
        )
    }

    /// The own property names of `obj` in ES5 enumeration order.
    /// `enumerable_only` selects Object.keys vs Object.getOwnPropertyNames.
    pub fn ordered_own_keys(&self, obj: ObjRef, enumerable_only: bool) -> ThinVec<CompactString> {
        let object = self.obj(obj);
        let (ints, rest) = Self::ordered_keys(object.properties.iter().filter_map(|(n, p)| {
            if !enumerable_only || p.atts & crate::value::JS_DONTENUM == 0 {
                Some((n.clone(), p.order))
            } else {
                None
            }
        }));
        ints.into_iter().chain(rest).collect()
    }

    /// Walk the own properties of `obj` in ES5 enumeration order,
    /// skipping DONTENUM entries and names enumerable in `seen`.
    fn walk_keys(
        &self,
        obj: ObjRef,
        seen: Option<ObjRef>,
        out: &mut ThinVec<CompactString>,
    ) {
        let names = self.ordered_own_keys(obj, true);
        for name in names {
            let shadowed = match seen {
                Some(s) => self.get_enum_property(s, &name).is_some(),
                None => false,
            };
            if !shadowed {
                out.push(name);
            }
        }
    }

    /// Flatten the enumerable properties of the whole prototype chain
    /// (itflatten in jsproperty.c): own keys first (in ES5 order), then
    /// the prototype's, then the prototype's prototype, etc. Names
    /// shadowed by an enumerable property higher in the chain are skipped.
    fn flatten_keys(&self, obj: ObjRef) -> ThinVec<CompactString> {
        let object = self.obj(obj);
        let mut out = ThinVec::new();
        self.walk_keys(obj, object.prototype, &mut out);
        if let Some(p) = object.prototype {
            out.extend(self.flatten_keys(p));
        }
        out
    }

    /// jsV_newiterator
    pub fn new_iterator(&mut self, obj: ObjRef, own: bool) -> ObjRef {
        let mut keys = if own {
            let mut out = ThinVec::new();
            self.walk_keys(obj, None, &mut out);
            out
        } else {
            self.flatten_keys(obj)
        };
        let n = {
            let object = self.obj(obj);
            match &object.payload {
                Payload::String(s) => s.length.max(0) as u32,
                Payload::Array(a) if a.simple => a.flat.len() as u32,
                _ => 0,
            }
        };
        // mapped arguments: prepend aliased indices (not deleted), which do
        // not exist as tree properties
        {
            let mapped_info = match &self.obj(obj).payload {
                Payload::Arguments(a) => Some((a.mapped, a.deleted.clone())),
                _ => None,
            };
            if let Some((mapped, deleted)) = mapped_info {
                let mut mapped_keys: ThinVec<CompactString> = ThinVec::new();
                for k in 0..mapped {
                    if !deleted.contains(&k) {
                        let name = self.intern(&k.to_string());
                        mapped_keys.push(name);
                    }
                }
                for k in keys {
                    if !mapped_keys.contains(&k) {
                        mapped_keys.push(k);
                    }
                }
                keys = mapped_keys;
            }
        }
        let io = self.alloc_object(Class::Iterator, None);
        self.obj_mut(io).payload = Payload::Iterator(IteratorData {
            target: obj,
            i: 0,
            n,
            keys,
            pos: 0,
        });
        io
    }

    /// jsV_nextiterator
    pub fn next_iterator(&mut self, io: ObjRef, scratch: &mut String) -> Option<CompactString> {
        let it = match &self.obj(io).payload {
            Payload::Iterator(it) => it,
            _ => return None,
        };
        if it.i < it.n {
            let i = it.i;
            if let Payload::Iterator(it) = &mut self.obj_mut(io).payload {
                it.i += 1;
            }
            *scratch = itoa_u32(i);
            return Some(self.intern(scratch));
        }
        loop {
            let (target, name) = {
                let it = match &self.obj(io).payload {
                    Payload::Iterator(it) => it,
                    _ => return None,
                };
                if it.pos >= it.keys.len() {
                    return None;
                }
                (it.target, it.keys[it.pos].clone())
            };
            if let Payload::Iterator(it) = &mut self.obj_mut(io).payload {
                it.pos += 1;
            }
            if self.get_property(target, &name).is_some() {
                return Some(name);
            }
        }
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

fn itoa_u32(v: u32) -> String {
    v.to_string()
}

// ---------------------------------------------------------------------------
// Garbage collection (jsgc.c)
// ---------------------------------------------------------------------------

impl State {
    fn mark_object(&mut self, mark: u8, r: ObjRef, worklist: &mut Vec<ObjRef>) {
        let slot = &mut self.heap.objects[r as usize];
        if slot.mark != mark {
            slot.mark = mark;
            worklist.push(r);
        }
    }

    fn mark_value(&mut self, mark: u8, v: &Value, worklist: &mut Vec<ObjRef>) {
        if let Value::Object(r) = v {
            self.mark_object(mark, *r, worklist);
        }
    }

    fn mark_env(&mut self, mark: u8, mut r: EnvRef, worklist: &mut Vec<ObjRef>) {
        loop {
            let (outer, variables) = {
                let slot = &mut self.heap.envs[r as usize];
                if slot.mark == mark {
                    return;
                }
                slot.mark = mark;
                let env = slot.value.as_ref().expect("env");
                (env.outer, env.variables)
            };
            self.mark_object(mark, variables, worklist);
            match outer {
                Some(o) => r = o,
                None => return,
            }
        }
    }

    fn mark_fun(&mut self, mark: u8, r: FunRef) {
        let funtab = {
            let slot = &mut self.heap.funs[r as usize];
            if slot.mark == mark {
                return;
            }
            slot.mark = mark;
            slot.value.as_ref().expect("fun").funtab.clone()
        };
        for f in funtab.iter() {
            self.mark_fun(mark, *f);
        }
    }

    fn scan_object(
        &mut self,
        mark: u8,
        r: ObjRef,
        worklist: &mut Vec<ObjRef>,
        scratch: &mut Vec<ObjRef>,
    ) {
        // gather references into a reusable scratch buffer (no allocation
        // in the common case) to appease the borrow checker
        scratch.clear();
        let mut scope: Option<EnvRef> = None;
        let mut fun: Option<FunRef> = None;
        {
            let obj = self.heap.obj(r);
            if let Some(p) = obj.prototype {
                scratch.push(p);
            }
            for prop in obj.properties.values() {
                if let Value::Object(v) = prop.value {
                    scratch.push(v);
                }
                if let Some(g) = prop.getter {
                    scratch.push(g);
                }
                if let Some(s) = prop.setter {
                    scratch.push(s);
                }
            }
            match &obj.payload {
                Payload::Array(a) => {
                    for v in &a.flat {
                        if let Value::Object(r) = v {
                            scratch.push(*r);
                        }
                    }
                }
                Payload::Function(f) => {
                    scope = Some(f.scope);
                    fun = Some(f.fun);
                }
                Payload::Arguments(a) => {
                    scope = Some(a.env);
                    fun = Some(a.fun);
                }
                Payload::Iterator(it) => scratch.push(it.target),
                Payload::Map(m) => {
                    for (k, v) in &m.entries {
                        if let Value::Object(r) = k {
                            scratch.push(*r);
                        }
                        if let Value::Object(r) = v {
                            scratch.push(*r);
                        }
                    }
                }
                Payload::Set(s) => {
                    for v in &s.values {
                        if let Value::Object(r) = v {
                            scratch.push(*r);
                        }
                    }
                }
                _ => {}
            }
        }
        for &r in scratch.iter() {
            self.mark_object(mark, r, worklist);
        }
        if let Some(e) = scope {
            self.mark_env(mark, e, worklist);
        }
        if let Some(f) = fun {
            self.mark_fun(mark, f);
        }
    }

    /// js_gc: mark and sweep.
    pub fn gc(&mut self, report: bool) {
        let gc_start = std::time::Instant::now();
        crate::state::STATS.gc_calls.fetch_add(1, crate::state::Ordering::Relaxed);
        let mark = if self.heap.gcmark == 1 { 2 } else { 1 };
        self.heap.gcmark = mark;

        let mut worklist: Vec<ObjRef> = Vec::new();

        // roots: prototypes, registry, global
        let protos = self.protos.all();
        for r in protos {
            self.mark_object(mark, r, &mut worklist);
        }
        let (r_reg, r_glob) = (self.r, self.g);
        self.mark_object(mark, r_reg, &mut worklist);
        self.mark_object(mark, r_glob, &mut worklist);

        // roots: value stack (extract only ObjRefs, no Value clones)
        for i in 0..self.top {
            if let Value::Object(r) = &self.stack[i] {
                self.mark_object(mark, *r, &mut worklist);
            }
        }

        // roots: environments
        let (e, ge) = (self.e, self.ge);
        self.mark_env(mark, e, &mut worklist);
        self.mark_env(mark, ge, &mut worklist);
        let envstack = self.envstack.clone();
        for env in envstack {
            self.mark_env(mark, env, &mut worklist);
        }

        // roots: scheduled timer callbacks and their arguments
        #[cfg(any(feature = "modules", feature = "timers"))]
        {
            let mut timer_vals: Vec<Value> = Vec::new();
            for t in &self.timers {
                timer_vals.push(t.callback.clone());
                timer_vals.extend(t.args.iter().cloned());
            }
            for v in &timer_vals {
                self.mark_value(mark, v, &mut worklist);
            }
        }

        // scan until fixpoint
        let mut scratch: Vec<ObjRef> = Vec::with_capacity(64);
        while let Some(r) = worklist.pop() {
            self.scan_object(mark, r, &mut worklist, &mut scratch);
        }

        // sweep
        let (mut nenv, mut nfun, mut nobj, mut nprop) = (0u32, 0u32, 0u32, 0u32);
        let (mut genv, mut gfun, mut gobj, mut gprop) = (0u32, 0u32, 0u32, 0u32);
        for i in 0..self.heap.envs.len() {
            if self.heap.envs[i].value.is_some() {
                nenv += 1;
                if self.heap.envs[i].mark != mark {
                    genv += 1;
                    self.heap.envs[i].value = None;
                    self.heap.free_envs.push(i as EnvRef);
                }
            }
        }
        for i in 0..self.heap.funs.len() {
            if self.heap.funs[i].value.is_some() {
                nfun += 1;
                if self.heap.funs[i].mark != mark {
                    gfun += 1;
                    self.heap.funs[i].value = None;
                    self.heap.free_funs.push(i as FunRef);
                }
            }
        }
        for i in 0..self.heap.objects.len() {
            if self.heap.objects[i].value.is_some() {
                let count = self.heap.objects[i]
                    .value
                    .as_ref()
                    .map(|o| o.properties.len() as u32)
                    .unwrap_or(0);
                nobj += 1;
                nprop += count;
                if self.heap.objects[i].mark != mark {
                    gobj += 1;
                    gprop += count;
                    self.heap.objects[i].value = None;
                    self.heap.free_objects.push(i as ObjRef);
                }
            }
        }

        let ntot = nenv + nfun + nobj + nprop;
        let gtot = genv + gfun + gobj + gprop;
        let remaining = ntot - gtot;

        self.heap.gccounter = remaining;
        self.heap.gcthresh = (remaining as f64 * 5.0) as u32; // JS_GCFACTOR
        crate::state::STATS
            .gc_nanos
            .fetch_add(gc_start.elapsed().as_nanos() as u64, crate::state::Ordering::Relaxed);

        if std::env::var("ES5_GCDEBUG").is_ok() {
            eprintln!(
                "[gc] remaining={} thresh={} objs={} envs={} funs={}",
                remaining,
                self.heap.gcthresh,
                self.heap.objects.len(),
                self.heap.envs.len(),
                self.heap.funs.len()
            );
        }

        if report {
            #[allow(clippy::manual_checked_ops)]
            let pct = if ntot > 0 { 100 * gtot / ntot } else { 0 };
            self.report(&format!(
                "garbage collected ({}%): {}/{} envs, {}/{} funs, {}/{} objs, {}/{} props",
                pct, genv, nenv, gfun, nfun, gobj, nobj, gprop, nprop
            ));
        }
    }
}
