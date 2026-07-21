//! child_process module: `spawn()` with a Node-style ChildProcess object.
//!
//! This engine is synchronous, so there is no async event loop: spawn
//! starts the child immediately (with piped stdout/stderr) and returns.
//! Output is buffered and the `data`/`close` events are pumped
//! synchronously when the first handler is attached (any handler replays
//! everything captured so far).
//!
//! Node-style usage (ES5 spelling):
//! ```js
//! var cp = require("child_process");
//! var ls = cp.spawn("ls", ["-lh", "/usr"]);
//! ls.stdout.on("data", function (data) { print("stdout: " + data); });
//! ls.stderr.on("data", function (data) { print("stderr: " + data); });
//! ls.on("close", function (code) { print("exit code " + code); });
//! ```

#![cfg(feature = "child_process")]

use crate::builtins::propf;
use crate::object::{Class, ObjRef, Payload};
use crate::state::{State, R};
use crate::value::JS_DONTENUM;
use std::io::Read;

/// Payload of a ChildProcess object (Rust-side process handle + buffers).
pub struct ChildData {
    pub child: Option<std::process::Child>,
    pub out: String,
    pub err: String,
    pub exit: Option<i32>,
    pub pumped: bool,
    pub out_fired: bool,
    pub err_fired: bool,
    pub close_fired: bool,
}

impl Drop for ChildData {
    fn drop(&mut self) {
        // don't let the child outlive the interpreter
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

const KIND_STDOUT: f64 = 1.0;
const KIND_STDERR: f64 = 2.0;

/// Get the child's payload fields we need, cloned out to avoid borrows.
fn child_state(st: &State, obj: ObjRef) -> Option<(bool, bool, bool, bool, Option<i32>)> {
    match &st.heap.obj(obj).payload {
        Payload::Child(d) => Some((d.pumped, d.out_fired, d.err_fired, d.close_fired, d.exit)),
        _ => None,
    }
}

/// Read the child's stdout/stderr fully into the payload buffers and wait
/// for exit. No JS is invoked while pumping, so no re-entrancy issues.
fn pump_child(st: &mut State, obj: ObjRef) {
    let (mut child, already) = match &mut st.heap.obj_mut(obj).payload {
        Payload::Child(d) => (d.child.take(), d.pumped),
        _ => return,
    };
    if already {
        return;
    }
    let mut out = String::new();
    let mut err = String::new();
    let mut exit = None;
    if let Some(mut c) = child.take() {
        if let Some(mut so) = c.stdout.take() {
            let mut buf = [0u8; 8192];
            loop {
                match so.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => out.push_str(&String::from_utf8_lossy(&buf[..n])),
                    Err(_) => break,
                }
            }
        }
        if let Some(mut se) = c.stderr.take() {
            let mut buf = [0u8; 8192];
            loop {
                match se.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => err.push_str(&String::from_utf8_lossy(&buf[..n])),
                    Err(_) => break,
                }
            }
        }
        exit = c.wait().ok().and_then(|s| s.code());
    }
    if let Payload::Child(d) = &mut st.heap.obj_mut(obj).payload {
        d.out = out;
        d.err = err;
        d.exit = exit;
        d.pumped = true;
    }
}

/// Fetch a stored callback function property from an object.
fn get_cb(st: &mut State, obj: ObjRef, name: &str) -> R<Option<ObjRef>> {
    if st.has_property(obj, name)? {
        let v = st.top_value();
        st.pop(1);
        match v.as_object() {
            Some(o) if st.heap.obj(o).class == Class::Function
                || st.heap.obj(o).class == Class::CFunction =>
            {
                return Ok(Some(o));
            }
            _ => return Ok(None),
        }
    }
    Ok(None)
}

/// Fire the stored `__cb_data` of a stream object with a chunk.
fn fire_data(st: &mut State, stream: ObjRef, chunk: &str) -> R<()> {
    if let Some(cb) = get_cb(st, stream, "__cb_data")? {
        st.push_object(cb)?;
        st.push_undefined()?; // this
        st.push_string(chunk)?;
        st.call(1)?;
        st.pop(1);
    }
    Ok(())
}

/// Fire `__cb_close` of the child process with the exit code (once only).
fn fire_close(st: &mut State, obj: ObjRef) -> R<()> {
    let (pumped, _, _, close_fired, exit) = match child_state(st, obj) {
        Some(s) => s,
        None => return Ok(()),
    };
    if !pumped || close_fired {
        return Ok(());
    }
    // only mark as fired when a handler actually exists
    if let Some(cb) = get_cb(st, obj, "__cb_close")? {
        if let Payload::Child(d) = &mut st.heap.obj_mut(obj).payload {
            d.close_fired = true;
        }
        st.push_object(cb)?;
        st.push_undefined()?; // this
        st.push_number(exit.unwrap_or(-1) as f64)?;
        st.call(1)?;
        st.pop(1);
    }
    Ok(())
}

/// Replay all buffered events for a child process object (each stream
/// fires at most once).
fn replay(st: &mut State, obj: ObjRef) -> R<()> {
    pump_child(st, obj);
    let (out_fired, err_fired, out_s, err_s, out, err) = {
        let (pumped, out_f, err_f, out, err) = match &st.heap.obj(obj).payload {
            Payload::Child(d) => (d.pumped, d.out_fired, d.err_fired, d.out.clone(), d.err.clone()),
            _ => return Ok(()),
        };
        let _ = pumped;
        let out_s = stream_of(st, obj, "stdout")?;
        let err_s = stream_of(st, obj, "stderr")?;
        (out_f, err_f, out_s, err_s, out, err)
    };
    if !out_fired
        && let Some(s) = out_s {
            // only mark as fired when a data handler actually exists
            if get_cb(st, s, "__cb_data")?.is_some() {
                if !out.is_empty() {
                    fire_data(st, s, &out)?;
                }
                if let Payload::Child(d) = &mut st.heap.obj_mut(obj).payload {
                    d.out_fired = true;
                }
            }
        }
    if !err_fired
        && let Some(s) = err_s
            && get_cb(st, s, "__cb_data")?.is_some() {
                if !err.is_empty() {
                    fire_data(st, s, &err)?;
                }
                if let Payload::Child(d) = &mut st.heap.obj_mut(obj).payload {
                    d.err_fired = true;
                }
            }
    fire_close(st, obj)
}

fn stream_of(st: &mut State, obj: ObjRef, name: &str) -> R<Option<ObjRef>> {
    st.push_object(obj)?;
    let s = if st.has_property(obj, name)? {
        let v = st.top_value();
        st.pop(1);
        v.as_object()
    } else {
        st.pop(1);
        None
    };
    Ok(s)
}

/// stream.on(name, cb)
fn stream_on(st: &mut State) -> R<()> {
    let name = st.tostring(1)?;
    if name.as_ref() == "data" && st.iscallable(2) {
        st.copy(2)?;
        st.defproperty(0, "__cb_data", JS_DONTENUM)?;
    }
    // find the parent child process and replay events
    let this = st.toobject(0)?;
    if st.has_property(this, "__parent")? {
        let parent = st.top_value();
        st.pop(1);
        if let Some(p) = parent.as_object() {
            replay(st, p)?;
        }
    } else {
        st.pop(1);
    }
    st.copy(0)
}

/// childprocess.on(name, cb) — handles "close".
fn cp_on(st: &mut State) -> R<()> {
    let name = st.tostring(1)?;
    if name.as_ref() == "close" && st.iscallable(2) {
        st.copy(2)?;
        st.defproperty(0, "__cb_close", JS_DONTENUM)?;
    }
    let this = st.toobject(0)?;
    replay(st, this)?;
    st.copy(0)
}

/// Create a stream object (stdout/stderr) for a child process.
fn make_stream(st: &mut State, parent: ObjRef, kind: f64) -> R<()> {
    st.newobject()?;
    st.push_object(parent)?;
    st.defproperty(-2, "__parent", JS_DONTENUM)?;
    st.push_number(kind)?;
    st.defproperty(-2, "__kind", JS_DONTENUM)?;
    st.newcfunction(stream_on, "on", 2)?;
    st.defproperty(-2, "on", 0)?;
    Ok(())
}

/// spawn(command[, args][, options])
fn cp_spawn(st: &mut State) -> R<()> {
    let cmd = st.tostring(1)?;
    let mut argv: Vec<String> = Vec::new();
    if st.isarray(2) {
        let n = st.getlength(2)?;
        for i in 0..n {
            st.getindex(2, i)?;
            let a = st.tostring(-1)?.to_string();
            st.pop(1);
            argv.push(a);
        }
    }

    let child = std::process::Command::new(&*cmd)
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let child = match child {
        Ok(c) => c,
        Err(e) => {
            return st.error(&format!("spawn {}: {}", cmd, e));
        }
    };

    let proto = Some(st.protos.object);
    let obj = st.heap.alloc_object(Class::Object, proto);
    st.heap.obj_mut(obj).payload = Payload::Child(ChildData {
        child: Some(child),
        out: String::new(),
        err: String::new(),
        exit: None,
        pumped: false,
        out_fired: false,
        err_fired: false,
        close_fired: false,
    });

    st.push_object(obj)?;

    // stdout / stderr streams
    make_stream(st, obj, KIND_STDOUT)?;
    st.defproperty(-2, "stdout", 0)?;
    make_stream(st, obj, KIND_STDERR)?;
    st.defproperty(-2, "stderr", 0)?;

    // .on / pid
    st.newcfunction(cp_on, "on", 2)?;
    st.defproperty(-2, "on", 0)?;

    let pid = match &st.heap.obj(obj).payload {
        Payload::Child(d) => d.child.as_ref().map(|c| c.id()),
        _ => None,
    };
    st.push_number(pid.unwrap_or(0) as f64)?;
    st.defproperty(-2, "pid", 0)?;

    Ok(())
}

/// execSync(command) -> captured stdout (convenience, throws on failure)
fn cp_execsync(st: &mut State) -> R<()> {
    let cmd = st.tostring(1)?;
    let output = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
        .arg(if cfg!(windows) { "/C" } else { "-c" })
        .arg(&*cmd)
        .output();
    match output {
        Ok(o) => {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout).into_owned();
                st.push_string(&s)
            } else {
                let s = String::from_utf8_lossy(&o.stderr).into_owned();
                st.error(&format!("execSync: command failed ({}): {}", o.status, s))
            }
        }
        Err(e) => st.error(&format!("execSync: {}", e)),
    }
}

/// Create the `child_process` module object.
pub fn make(st: &mut State) -> R<()> {
    st.newobject()?;
    propf(st, "child_process.spawn", cp_spawn, 2)?;
    propf(st, "child_process.execSync", cp_execsync, 1)?;
    Ok(())
}
