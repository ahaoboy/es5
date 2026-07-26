//! Symbol constructor and Symbol.for / Symbol.keyFor (ES6).
//!
//! Symbols are represented as regular objects of class `Symbol` with a
//! `SymbolData` payload. `Symbol.for(key)` uses the global registry in
//! `State::symbol_registry`.

#![cfg(feature = "symbol")]

use crate::object::{Class, Payload, SymbolData};
use crate::state::{State, R};
use crate::value::JS_DONTENUM;

/// Helper: return the Symbol constructor's `.prototype` property.
fn symbol_proto(st: &State) -> u32 {
    st.heap
        .get_property(st.g, "Symbol")
        .and_then(|ctor| ctor.value.as_object())
        .and_then(|sym_ctor| {
            st.heap
                .get_property(sym_ctor, "prototype")
                .and_then(|pp| pp.value.as_object())
        })
        .unwrap_or(st.protos.object)
}

fn sym_for(st: &mut State) -> R<()> {
    let key = st.tostring(1)?;
    if let Some(&existing) = st.symbol_registry.get(&key) {
        return st.push_object(existing);
    }
    let desc = key.clone();
    let proto = symbol_proto(st);
    let sym = st.heap.alloc_object(Class::Symbol, Some(proto));
    st.heap.obj_mut(sym).payload = Payload::Symbol(SymbolData {
        description: desc.clone(),
        key: Some(desc),
    });
    st.symbol_registry.insert(key, sym);
    st.push_object(sym)
}

/// Symbol constructor: creates an unregistered symbol with optional description.
fn sym_ctor(st: &mut State) -> R<()> {
    let desc = if st.isdefined(1) {
        let s = st.tostring(1)?;
        s.to_string()
    } else {
        String::new()
    };
    let proto = symbol_proto(st);
    let sym = st.heap.alloc_object(Class::Symbol, Some(proto));
    let desc_rc = st.heap.intern(&desc);
    st.heap.obj_mut(sym).payload = Payload::Symbol(SymbolData {
        description: desc_rc,
        key: None,
    });
    st.push_object(sym)
}

fn sym_key_for(st: &mut State) -> R<()> {
    if !st.isobject(1) {
        return st.push_undefined();
    }
    let obj = st.toobject(1)?;
    if st.heap.obj(obj).class != Class::Symbol {
        return st.push_undefined();
    }
    match &st.heap.obj(obj).payload {
        Payload::Symbol(sd) => match &sd.key {
            Some(k) => st.push_string_rc(k.clone()),
            None => st.push_undefined(),
        },
        _ => st.push_undefined(),
    }
}

fn sym_tostring(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;
    if st.heap.obj(obj).class != Class::Symbol {
        return st.type_error("Symbol.prototype.toString called on non-Symbol");
    }
    let desc = match &st.heap.obj(obj).payload {
        Payload::Symbol(sd) => &sd.description,
        _ => return st.push_literal("Symbol()"),
    };
    let s = format!("Symbol({})", desc);
    st.push_string(&s)
}

fn make_well_known(st: &mut State, name: &str, desc: &str) -> R<()> {
    let proto = symbol_proto(st);
    let sym = st.heap.alloc_object(Class::Symbol, Some(proto));
    st.heap.obj_mut(sym).payload = Payload::Symbol(SymbolData {
        description: st.heap.intern(desc),
        key: None,
    });
    st.push_object(sym)?;
    st.defproperty(-2, name, JS_DONTENUM)
}

pub fn init(st: &mut State) {
    st.newobject().unwrap();
    {
        st.newcfunction(sym_tostring, "toString", 0).unwrap();
        st.defproperty(-2, "toString", JS_DONTENUM).unwrap();
    }
    st.newcconstructor(sym_ctor, sym_ctor, "Symbol", 1).unwrap();
    {
        st.newcfunction(sym_for, "for", 1).unwrap();
        st.defproperty(-2, "for", JS_DONTENUM).unwrap();
        st.newcfunction(sym_key_for, "keyFor", 1).unwrap();
        st.defproperty(-2, "keyFor", JS_DONTENUM).unwrap();

        make_well_known(st, "iterator", "Symbol.iterator").unwrap();
        make_well_known(st, "species", "Symbol.species").unwrap();
        make_well_known(st, "toPrimitive", "Symbol.toPrimitive").unwrap();
        make_well_known(st, "toStringTag", "Symbol.toStringTag").unwrap();
        make_well_known(st, "hasInstance", "Symbol.hasInstance").unwrap();
    }
    st.defglobal("Symbol", JS_DONTENUM).unwrap();

    // Well-known symbols — must happen AFTER Symbol is made global
    {
        let well_known = ["iterator", "asyncIterator", "hasInstance",
            "isConcatSpreadable", "match", "replace", "search",
            "species", "split", "toPrimitive", "toStringTag", "unscopables"];
        // Create missing symbols in the registry
        for name in well_known {
            let key = format!("Symbol.{name}");
            if !st.symbol_registry.contains_key(key.as_str()) {
                let proto = symbol_proto(st);
                let sym = st.heap.alloc_object(Class::Symbol, Some(proto));
                let key_rc = st.heap.intern(&key);
                st.heap.obj_mut(sym).payload = Payload::Symbol(SymbolData {
                    description: key_rc.clone(),
                    key: Some(key_rc.clone()),
                });
                st.symbol_registry.insert(key_rc, sym);
            }
        }
        // Register as Symbol.iterator etc.
        for name in well_known {
            let key = format!("Symbol.{name}");
            let &sym_obj = st.symbol_registry.get(key.as_str()).unwrap();
            st.getglobal("Symbol").unwrap();
            st.push_object(sym_obj).unwrap();
            st.defproperty(-2, name, JS_DONTENUM).unwrap();
            st.pop(1);
        }
    }
}
