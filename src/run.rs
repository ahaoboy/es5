//! The bytecode interpreter (jsR_run in jsrun.c).
//!
//! The dispatch loop mirrors MuJS instruction-for-instruction. Exceptions
//! thrown by JavaScript are `Err(Value)`; OP_TRY frames pushed by this
//! invocation are caught and resumed here, everything else unwinds to the
//! caller (an outer run loop or the host).

use crate::compile::{Inst, Op};
use crate::object::{Class, FunRef};
use crate::state::{State, TryFrame, R, JS_TRYLIMIT};
use crate::value::Value;

fn js_trap(st: &State, pc: usize) {
    println!("stack trace:");
    for n in (0..=st.tracetop).rev() {
        let t = &st.trace[n];
        let (name, file) = st.trace_name_file(t);
        if t.line > 0 {
            if !name.is_empty() {
                println!("\tat {} ({}:{}:{})", name, file, t.line, t.col);
            } else {
                println!("\tat {}:{}:{}", file, t.line, t.col);
            }
        } else {
            println!("\tat {} ({})", name, file);
        }
    }
    let _ = pc;
}

/// Check if a stack slot holds a non-negative integer array index.
fn isindex(st: &State, idx: i32) -> Option<i32> {
    if let Value::Number(n) = st.stackidx(idx) {
        let k = *n as i32;
        if k as f64 == *n && k >= 0 {
            return Some(k);
        }
    }
    None
}

/// Execute the compiled function `f` (jsR_run).
pub fn run(st: &mut State, f: FunRef) -> R<()> {
    let (code, funtab, vartab, strtab, lightweight, funstrict) = {
        let fun = st.heap.fun(f);
        (
            fun.code.clone(),
            fun.funtab.clone(),
            fun.vartab.clone(),
            fun.strtab.clone(),
            fun.lightweight,
            fun.strict,
        )
    };

    let base_try = st.trystk.len();
    let savestrict = st.strict;
    st.strict = funstrict;

    let mut pc = 0usize;

    loop {
        if pc >= code.len() {
            // functions always end with OP_RETURN; unreachable in practice
            st.strict = savestrict;
            return Ok(());
        }

        let mut limit_err: Option<Value> = None;
        if st.runlimit > 0 {
            if st.runlimit == 1 {
                let v = Value::LitStr(st.heap.lit("script ran too long"));
                let _ = st.push_value(v.clone());
                limit_err = Some(v);
            } else {
                st.runlimit -= 1;
            }
        }

        let r: R<()>;
        if let Some(v) = limit_err {
            r = Err(v);
        } else {
            let inst: &Inst = &code[pc];
            pc += 1;

            // MuJS jsR_run: update the trace line BEFORE executing the
            // instruction so error objects created mid-instruction capture
            // the correct source position. Two 4-byte stores to the same
            // address; no branch.
            st.trace[st.tracetop].line = inst.line;
            st.trace[st.tracetop].col = inst.col;

            if matches!(inst.op, Op::Return) {
                st.strict = savestrict;
                return Ok(());
            }

            r = match &inst.op {
                Op::Pop => {
                    st.pop(1);
                    Ok(())
                }
                Op::Dup => st.dup(),
            Op::Dup2 => st.dup2(),
            Op::Rot2 => {
                st.rot2();
                Ok(())
            }
            Op::Rot3 => {
                st.rot3();
                Ok(())
            }
            Op::Rot4 => {
                st.rot4();
                Ok(())
            }

            Op::Integer(k) => st.push_number(*k as f64),
            Op::Number(x) => st.push_number(*x),
            Op::String(i) => st.push_value(Value::LitStr(*i)),
            Op::Closure(i) => st.newfunction(funtab[*i as usize], st.e),
            Op::NewObject => st.newobject(),
            Op::NewArray => st.newarray(),
            Op::NewRegexp(si, flags) => {
                let s = &strtab[*si as usize];
                crate::builtins::regexp::new_regexp(st, s, *flags)
            }

            Op::Undef => st.push_undefined(),
            Op::Null => st.push_null(),
            Op::True => st.push_boolean(true),
            Op::False => st.push_boolean(false),

            Op::This => {
                if st.strict || st.iscoercible(0) {
                    st.copy(0)
                } else {
                    st.push_global()
                }
            }
            Op::Current => st.currentfunction(),

            Op::GetLocal(i) => {
                let i = *i as usize;
                if lightweight {
                    // hot path: direct stack slot, no bounds check
                    let v = unsafe { st.stack.get_unchecked(st.bot + i).clone() };
                    st.push_value(v)
                } else {
                    let name = vartab[i - 1].clone();
                    if !st.hasvar(&name)? {
                        st.reference_error(&format!("'{}' is not defined", name))
                    } else {
                        Ok(())
                    }
                }
            }
            Op::SetLocal(i) => {
                let i = *i as usize;
                if lightweight {
                    let v = unsafe { st.stack.get_unchecked(st.top - 1).clone() };
                    *unsafe { st.stack.get_unchecked_mut(st.bot + i) } = v;
                    Ok(())
                } else {
                    let name = vartab[i - 1].clone();
                    st.setvar(&name)
                }
            }
            Op::DelLocal(i) => {
                let i = *i as usize;
                if lightweight {
                    st.push_boolean(false)
                } else {
                    let name = vartab[i - 1].clone();
                    let b = st.delvar(&name)?;
                    st.push_boolean(b)
                }
            }

            Op::GetVar(si) => {
                let name = &strtab[*si as usize];
                if !st.hasvar(name)? {
                    st.reference_error(&format!("'{}' is not defined", name))
                } else {
                    Ok(())
                }
            }
            Op::HasVar(si) => {
                let name = &strtab[*si as usize];
                if !st.hasvar(name)? {
                    st.push_undefined()?;
                }
                Ok(())
            }
            Op::SetVar(si) => {
                let name = &strtab[*si as usize];
                st.setvar(name)
            }
            Op::DelVar(si) => {
                let name = &strtab[*si as usize];
                let b = st.delvar(name)?;
                st.push_boolean(b)
            }

            Op::In => {
                let name = st.tostring(-2)?;
                if !st.isobject(-1) {
                    st.type_error("operand to 'in' is not an object")
                } else {
                    let mapped = crate::state::symbol_prop_key(&name);
                    let sym_name = mapped.unwrap_or(&name);
                    let b = st.hasproperty(-1, sym_name)?;
                    st.pop(2 + b as usize);
                    st.push_boolean(b)
                }
            }

            Op::SkipArray => {
                let len = st.getlength(-1)?;
                st.setlength(-1, len + 1)
            }
            Op::InitArray => {
                let len = st.getlength(-2)?;
                st.setindex(-2, len)
            }

            Op::InitProp => {
                let obj = st.toobject(-3)?;
                let name = st.tostring(-2)?;
                st.set_property(obj, &name, false)?;
                st.pop(2);
                Ok(())
            }
            Op::InitGetter => {
                let obj = st.toobject(-3)?;
                let name = st.tostring(-2)?;
                let getter = tofunction(st, -1)?;
                st.def_property_raw(obj, &name, 0, None, getter, None, false)?;
                st.pop(2);
                Ok(())
            }
            Op::InitSetter => {
                let obj = st.toobject(-3)?;
                let name = st.tostring(-2)?;
                let setter = tofunction(st, -1)?;
                st.def_property_raw(obj, &name, 0, None, None, setter, false)?;
                st.pop(2);
                Ok(())
            }

            Op::GetProp => {
                if let Some(ix) = isindex(st, -1) {
                    let obj = st.toobject(-2)?;
                    st.get_index(obj, ix)?;
                } else {
                    let name = st.tostring(-1)?;
                    let obj = st.toobject(-2)?;
                    // Map Symbol.toString() key to @@name form
                    let mapped = crate::state::symbol_prop_key(&name);
                    let sym_name = mapped.unwrap_or(&name);
                    st.get_property(obj, sym_name)?;
                }
                st.rot3pop2();
                Ok(())
            }
            Op::GetPropS(si) => {
                let name = &strtab[*si as usize];
                let obj = st.toobject(-1)?;
                st.get_property(obj, name)?;
                st.rot2pop1();
                Ok(())
            }

            Op::SetProp => {
                if let Some(ix) = isindex(st, -2) {
                    let obj = st.toobject(-3)?;
                    let transient = !st.isobject(-3);
                    st.set_index(obj, ix, transient)?;
                } else {
                    let name = st.tostring(-2)?;
                    let obj = st.toobject(-3)?;
                    let transient = !st.isobject(-3);
                    let mapped = crate::state::symbol_prop_key(&name);
                    let sym_name = mapped.unwrap_or(&name);
                    st.set_property(obj, sym_name, transient)?;
                }
                st.rot3pop2();
                Ok(())
            }
            Op::SetPropS(si) => {
                let name = &strtab[*si as usize];
                let obj = st.toobject(-2)?;
                let transient = !st.isobject(-2);
                st.set_property(obj, name, transient)?;
                st.rot2pop1();
                Ok(())
            }

            Op::DelProp => {
                let name = st.tostring(-1)?;
                let obj = st.toobject(-2)?;
                let mapped = crate::state::symbol_prop_key(&name);
                let sym_name = mapped.unwrap_or(&name);
                let b = st.del_property(obj, sym_name)?;
                st.pop(2);
                st.push_boolean(b)
            }
            Op::DelPropS(si) => {
                let name = &strtab[*si as usize];
                let obj = st.toobject(-1)?;
                let b = st.del_property(obj, name)?;
                st.pop(1);
                st.push_boolean(b)
            }

            Op::Iterator => {
                if st.iscoercible(-1) {
                    let target = st.toobject(-1)?;
                    let io = st.heap.new_iterator(target, false);
                    st.pop(1);
                    st.push_object(io)?;
                }
                Ok(())
            }
            Op::NextIter => {
                if st.isobject(-1) {
                    let io = st.toobject(-1)?;
                    let mut scratch = std::mem::take(&mut st.scratch);
                    let name = st.heap.next_iterator(io, &mut scratch);
                    st.scratch = scratch;
                    match name {
                        Some(name) => {
                            st.push_string_rc(name)?;
                            st.push_boolean(true)?;
                        }
                        None => {
                            st.pop(1);
                            st.push_boolean(false)?;
                        }
                    }
                } else {
                    st.pop(1);
                    st.push_boolean(false)?;
                }
                Ok(())
            }

            Op::Eval => st.eval(),
            Op::Call(n) => st.call(*n as usize),
            Op::New(n) => st.construct(*n as usize),

            Op::Typeof => {
                let t = st.typeof_(-1);
                st.pop(1);
                st.push_literal(t)
            }
            Op::Pos => {
                let x = st.tonumber(-1)?;
                st.pop(1);
                st.push_number(x)
            }
            Op::Neg => {
                let x = st.tonumber(-1)?;
                st.pop(1);
                st.push_number(-x)
            }
            Op::BitNot => {
                let ix = st.toint32(-1)?;
                st.pop(1);
                st.push_number(!ix as f64)
            }
            Op::LogNot => {
                let b = st.toboolean(-1);
                st.pop(1);
                st.push_boolean(!b)
            }
            Op::Inc => {
                let x = st.tonumber(-1)?;
                st.pop(1);
                st.push_number(x + 1.0)
            }
            Op::Dec => {
                let x = st.tonumber(-1)?;
                st.pop(1);
                st.push_number(x - 1.0)
            }
            Op::PostInc => {
                let x = st.tonumber(-1)?;
                st.pop(1);
                st.push_number(x + 1.0)?;
                st.push_number(x)
            }
            Op::PostDec => {
                let x = st.tonumber(-1)?;
                st.pop(1);
                st.push_number(x - 1.0)?;
                st.push_number(x)
            }

            Op::Mul => {
                let x = st.tonumber(-2)?;
                let y = st.tonumber(-1)?;
                st.pop(2);
                st.push_number(x * y)
            }
            Op::Div => {
                let x = st.tonumber(-2)?;
                let y = st.tonumber(-1)?;
                st.pop(2);
                st.push_number(x / y)
            }
            Op::Mod => {
                let x = st.tonumber(-2)?;
                let y = st.tonumber(-1)?;
                st.pop(2);
                st.push_number(crate::number::fmod(x, y))
            }
            Op::Add => st.concat(),
            Op::Sub => {
                let x = st.tonumber(-2)?;
                let y = st.tonumber(-1)?;
                st.pop(2);
                st.push_number(x - y)
            }

            Op::Shl => {
                let ix = st.toint32(-2)?;
                let uy = st.touint32(-1)?;
                st.pop(2);
                st.push_number((ix << (uy & 0x1F)) as f64)
            }
            Op::Shr => {
                let ix = st.toint32(-2)?;
                let uy = st.touint32(-1)?;
                st.pop(2);
                st.push_number((ix >> (uy & 0x1F)) as f64)
            }
            Op::Ushr => {
                let ux = st.touint32(-2)?;
                let uy = st.touint32(-1)?;
                st.pop(2);
                st.push_number((ux >> (uy & 0x1F)) as f64)
            }

            Op::Lt => {
                let (b, okay) = st.compare()?;
                st.pop(2);
                st.push_boolean(okay && b < 0)
            }
            Op::Gt => {
                let (b, okay) = st.compare()?;
                st.pop(2);
                st.push_boolean(okay && b > 0)
            }
            Op::Le => {
                let (b, okay) = st.compare()?;
                st.pop(2);
                st.push_boolean(okay && b <= 0)
            }
            Op::Ge => {
                let (b, okay) = st.compare()?;
                st.pop(2);
                st.push_boolean(okay && b >= 0)
            }

            Op::Instanceof => {
                let b = st.instanceof()?;
                st.pop(2);
                st.push_boolean(b)
            }

            Op::Eq => {
                let b = st.equal()?;
                st.pop(2);
                st.push_boolean(b)
            }
            Op::Ne => {
                let b = st.equal()?;
                st.pop(2);
                st.push_boolean(!b)
            }
            Op::StrictEq => {
                let b = st.strictequal()?;
                st.pop(2);
                st.push_boolean(b)
            }
            Op::StrictNe => {
                let b = st.strictequal()?;
                st.pop(2);
                st.push_boolean(!b)
            }

            Op::JCase(offset) => {
                let b = st.strictequal()?;
                if b {
                    st.pop(2);
                    pc = *offset;
                } else {
                    st.pop(1);
                }
                Ok(())
            }

            Op::BitAnd => {
                let ix = st.toint32(-2)?;
                let iy = st.toint32(-1)?;
                st.pop(2);
                st.push_number((ix & iy) as f64)
            }
            Op::BitXor => {
                let ix = st.toint32(-2)?;
                let iy = st.toint32(-1)?;
                st.pop(2);
                st.push_number((ix ^ iy) as f64)
            }
            Op::BitOr => {
                let ix = st.toint32(-2)?;
                let iy = st.toint32(-1)?;
                st.pop(2);
                st.push_number((ix | iy) as f64)
            }

            Op::Throw => Err(st.top_value()),

            Op::Try(offset) => {
                if st.trystk.len() >= JS_TRYLIMIT {
                    let v = Value::LitStr(st.heap.lit("exception stack overflow"));
                    st.push_value(v.clone())?;
                    Err(v)
                } else {
                    st.trystk.push(TryFrame {
                        e: st.e,
                        envtop: st.envstack.len(),
                        tracetop: st.tracetop,
                        top: st.top,
                        bot: st.bot,
                        strict: st.strict,
                        catch_pc: Some(pc),
                    });
                    pc = *offset;
                    Ok(())
                }
            }
            Op::EndTry => {
                st.trystk.pop();
                Ok(())
            }

            Op::Catch(si) => {
                let name = &strtab[*si as usize];
                let obj = st.heap.alloc_object(Class::Object, None);
                st.push_object(obj)?;
                st.rot2();
                st.setproperty(-2, name)?;
                let e = st.e;
                let newenv = st.heap.alloc_env(obj, Some(e));
                st.e = newenv;
                st.pop(1);
                Ok(())
            }
            Op::EndCatch => {
                let e = st.e;
                let outer = st.heap.env(e).outer.expect("catch env has outer");
                st.e = outer;
                Ok(())
            }

            Op::With => {
                let obj = st.toobject(-1)?;
                let e = st.e;
                let newenv = st.heap.alloc_env(obj, Some(e));
                st.e = newenv;
                st.pop(1);
                Ok(())
            }
            Op::EndWith => {
                let e = st.e;
                let outer = st.heap.env(e).outer.expect("with env has outer");
                st.e = outer;
                Ok(())
            }

            Op::Debugger => {
                js_trap(st, pc - 1);
                Ok(())
            }

            Op::Jump(offset) => {
                pc = *offset;
                Ok(())
            }
            Op::JTrue(offset) => {
                let b = st.toboolean(-1);
                st.pop(1);
                if b {
                    pc = *offset;
                }
                Ok(())
            }
            Op::JFalse(offset) => {
                let b = st.toboolean(-1);
                st.pop(1);
                if !b {
                    pc = *offset;
                }
                Ok(())
            }

            Op::Return => {
                unreachable!()
            }
        };
        }

        match r {
            Ok(()) => {}
            Err(v) => {
                // the trace was updated before execution, so the error
                // object already captured the correct line/col
                if st.trystk.len() > base_try {
                    let frame = st.trystk.pop().expect("try frame");
                    st.restore_frame(&frame);
                    st.push_value(v)?;
                    pc = frame.catch_pc.expect("run try frame has pc");
                } else {
                    return Err(v);
                }
            }
        }
    }
}

fn tofunction(st: &mut State, idx: i32) -> R<Option<u32>> {
    match st.stackidx(idx) {
        Value::Undefined | Value::Null => Ok(None),
        Value::Object(r)
            if matches!(
                st.heap.obj(*r).class,
                Class::Function | Class::CFunction
            ) =>
        {
            Ok(Some(*r))
        }
        _ => st.type_error("not a function"),
    }
}
