//! Function constructor and Function.prototype (jsfunction.c).

use super::propf;
use crate::object::{CFunctionData, Class, Payload};
use crate::state::{State, R};
use crate::value::{JS_DONTCONF, JS_DONTENUM, JS_READONLY};

fn jsb_function(st: &mut State) -> R<()> {
    let top = st.gettop();

    // p1, p2, ..., pn
    let mut params = String::new();
    if top > 2 {
        for i in 1..(top - 1) {
            if i > 1 {
                params.push(',');
            }
            let s = st.tostring(i)?;
            params.push_str(&s);
        }
    }

    // body
    let body = if st.isdefined(top - 1) {
        st.tostring(top - 1)?.to_string()
    } else {
        String::new()
    };

    let params = if top > 2 { Some(params) } else { None };
    let ast = crate::parse::parse_function(st, "[string]", params.as_deref(), &body)?;
    let fun = crate::compile::compile_function(st, &ast)?;

    let ge = st.ge;
    st.newfunction(fun, ge)
}

fn jsb_function_prototype(st: &mut State) -> R<()> {
    st.push_undefined()
}

fn fp_tostring(st: &mut State) -> R<()> {
    let obj = st.toobject(0)?;

    if !st.iscallable(0) {
        return st.type_error("not a function");
    }

    let class = st.heap.obj(obj).class;
    if class == Class::Function || class == Class::Script {
        let fun_ref = match &st.heap.obj(obj).payload {
            Payload::Function(fd) => fd.fun,
            _ => unreachable!(),
        };
        let (name, numparams, vartab) = {
            let f = st.heap.fun(fun_ref);
            (f.name.clone(), f.numparams, f.vartab.clone())
        };
        let mut out = String::from("function ");
        out.push_str(&name);
        out.push('(');
        for i in 0..numparams {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&vartab[i]);
        }
        out.push_str(") { [byte code] }");
        st.push_string(&out)
    } else if class == Class::CFunction {
        let name = match &st.heap.obj(obj).payload {
            Payload::CFunction(cd) => cd.name.clone(),
            _ => unreachable!(),
        };
        st.push_string(&format!("function {}() {{ [native code] }}", name))
    } else {
        st.push_literal("function () { }")
    }
}

fn fp_apply(st: &mut State) -> R<()> {
    if !st.iscallable(0) {
        return st.type_error("not a function");
    }

    st.copy(0)?;
    st.copy(1)?;

    let mut n = 0;
    if !(st.isnull(2) || st.isundefined(2)) {
        n = st.getlength(2)?;
        if n < 0 {
            n = 0;
        }
        for i in 0..n {
            st.getindex(2, i)?;
        }
    }

    st.call(n as usize)
}

fn fp_call(st: &mut State) -> R<()> {
    let top = st.gettop();

    if !st.iscallable(0) {
        return st.type_error("not a function");
    }

    for i in 0..top {
        st.copy(i)?;
    }

    st.call((top - 2).max(0) as usize)
}

fn callbound(st: &mut State) -> R<()> {
    let top = st.gettop();

    let fun = st.gettop();
    st.currentfunction()?;
    st.getproperty(fun, "__TargetFunction__")?;
    st.getproperty(fun, "__BoundThis__")?;

    let args = st.gettop();
    st.getproperty(fun, "__BoundArguments__")?;
    let mut n = st.getlength(args)?;
    if n < 0 {
        n = 0;
    }
    for i in 0..n {
        st.getindex(args, i)?;
    }
    st.remove(args)?;

    for i in 1..top {
        st.copy(i)?;
    }

    st.call((n + top - 1) as usize)
}

fn constructbound(st: &mut State) -> R<()> {
    let top = st.gettop();

    let fun = st.gettop();
    st.currentfunction()?;
    st.getproperty(fun, "__TargetFunction__")?;

    let args = st.gettop();
    st.getproperty(fun, "__BoundArguments__")?;
    let mut n = st.getlength(args)?;
    if n < 0 {
        n = 0;
    }
    for i in 0..n {
        st.getindex(args, i)?;
    }
    st.remove(args)?;

    for i in 1..top {
        st.copy(i)?;
    }

    st.construct((n + top - 1) as usize)
}

fn fp_bind(st: &mut State) -> R<()> {
    let top = st.gettop();

    if !st.iscallable(0) {
        return st.type_error("not a function");
    }

    let mut n = st.getlength(0)?;
    if n > top - 2 {
        n -= top - 2;
    } else {
        n = 0;
    }

    // Reuse target function's prototype for HasInstance check.
    st.getproperty(0, "prototype")?;
    st.newcconstructor(callbound, constructbound, "[bind]", n)?;

    // target function
    st.copy(0)?;
    st.defproperty(-2, "__TargetFunction__", JS_READONLY | JS_DONTENUM | JS_DONTCONF)?;

    // bound this
    st.copy(1)?;
    st.defproperty(-2, "__BoundThis__", JS_READONLY | JS_DONTENUM | JS_DONTCONF)?;

    // bound arguments
    st.newarray()?;
    for i in 2..top {
        st.copy(i)?;
        st.setindex(-2, i - 2)?;
    }
    st.defproperty(-2, "__BoundArguments__", JS_READONLY | JS_DONTENUM | JS_DONTCONF)?;

    Ok(())
}

pub fn init(st: &mut State) {
    // Function_prototype is itself a callable no-op function
    {
        let proto = st.protos.function;
        st.heap.obj_mut(proto).payload = Payload::CFunction(CFunctionData {
            name: st.heap.intern("Function.prototype"),
            function: jsb_function_prototype,
            constructor: None,
            length: 0,
        });
    }

    let proto = st.protos.function;
    st.push_object(proto).unwrap();
    {
        propf(st, "Function.prototype.toString", fp_tostring, 2).unwrap();
        propf(st, "Function.prototype.apply", fp_apply, 2).unwrap();
        propf(st, "Function.prototype.call", fp_call, 1).unwrap();
        propf(st, "Function.prototype.bind", fp_bind, 1).unwrap();
    }
    st.newcconstructor(jsb_function, jsb_function, "Function", 1).unwrap();
    st.defglobal("Function", JS_DONTENUM).unwrap();
}
