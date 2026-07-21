//! Timer functions (setTimeout/clearTimeout/setInterval/clearInterval/
//! setImmediate/clearImmediate).
//!
//! This engine is synchronous, so timers cannot preempt running code. They
//! are scheduled on a task queue that is pumped when the main script (or
//! the current REPL input) finishes — the same model QuickJS uses for its
//! job queue. An interval timer reschedules itself after each fire, and
//! timers scheduled from within timer callbacks run in subsequent passes.

#![cfg(feature = "timers")]

use crate::state::{State, R};
use crate::value::{Value, JS_DONTENUM};
use std::time::{Duration, Instant};

/// One scheduled timer.
pub struct Timer {
    pub id: u32,
    pub due: Instant,
    /// repeat delay in milliseconds (None = one-shot setTimeout)
    pub interval: Option<u32>,
    pub callback: Value,
    pub args: Vec<Value>,
}

fn get_cb_value(st: &mut State, idx: i32) -> R<Value> {
    if !st.iscallable(idx) {
        return st.type_error("callback must be a function");
    }
    Ok(st.stackidx(idx).clone())
}

/// Create the opaque Timeout object returned to JS.
fn make_timeout_obj(st: &mut State, id: u32) -> R<()> {
    st.newobject()?;
    st.push_number(id as f64)?;
    st.defproperty(-2, "id", 0)?;
    st.newcfunction(timeout_ref, "ref", 0)?;
    st.defproperty(-2, "ref", 0)?;
    st.newcfunction(timeout_unref, "unref", 0)?;
    st.defproperty(-2, "unref", 0)?;
    Ok(())
}

fn timeout_ref(st: &mut State) -> R<()> {
    st.copy(0)
}

fn timeout_unref(st: &mut State) -> R<()> {
    st.copy(0)
}

fn schedule(st: &mut State, interval: Option<u32>) -> R<()> {
    let cb = get_cb_value(st, 1)?;
    let delay = if st.isdefined(2) {
        st.tointeger(2)?.max(0) as u32
    } else {
        0
    };
    let top = st.gettop();
    let mut args = Vec::new();
    for i in 3..top {
        args.push(st.stackidx(i).clone());
    }
    let id = st.next_timer_id;
    st.next_timer_id += 1;
    st.timers.push(Timer {
        id,
        due: Instant::now() + Duration::from_millis(delay as u64),
        interval,
        callback: cb,
        args,
    });
    make_timeout_obj(st, id)
}

fn set_timeout(st: &mut State) -> R<()> {
    schedule(st, None)
}

fn set_interval(st: &mut State) -> R<()> {
    let delay = if st.isdefined(2) {
        st.tointeger(2)?.max(0) as u32
    } else {
        0
    };
    schedule(st, Some(delay))
}

fn set_immediate(st: &mut State) -> R<()> {
    let cb = get_cb_value(st, 1)?;
    let top = st.gettop();
    let mut args = Vec::new();
    for i in 2..top {
        args.push(st.stackidx(i).clone());
    }
    let id = st.next_timer_id;
    st.next_timer_id += 1;
    st.timers.push(Timer {
        id,
        due: Instant::now(),
        interval: None,
        callback: cb,
        args,
    });
    make_timeout_obj(st, id)
}

fn timer_id_of(st: &mut State, idx: i32) -> R<Option<u32>> {
    if st.isundefined(idx) || st.isnull(idx) {
        return Ok(None);
    }
    if st.isnumber(idx) {
        return Ok(Some(st.tointeger(idx)? as u32));
    }
    if st.isobject(idx) {
        if st.hasproperty(idx, "id")? {
            let id = st.tointeger(-1)?;
            st.pop(1);
            return Ok(Some(id as u32));
        }
        st.pop(1);
    }
    Ok(None)
}

fn clear_timer(st: &mut State) -> R<()> {
    if let Some(id) = timer_id_of(st, 1)? {
        st.timers.retain(|t| t.id != id);
    }
    st.push_undefined()
}

/// Register the timer globals.
pub fn register(st: &mut State) -> R<()> {
    st.newcfunction(set_timeout, "setTimeout", 2)?;
    st.defglobal("setTimeout", JS_DONTENUM)?;
    st.newcfunction(clear_timer, "clearTimeout", 1)?;
    st.defglobal("clearTimeout", JS_DONTENUM)?;
    st.newcfunction(set_interval, "setInterval", 2)?;
    st.defglobal("setInterval", JS_DONTENUM)?;
    st.newcfunction(clear_timer, "clearInterval", 1)?;
    st.defglobal("clearInterval", JS_DONTENUM)?;
    st.newcfunction(set_immediate, "setImmediate", 1)?;
    st.defglobal("setImmediate", JS_DONTENUM)?;
    st.newcfunction(clear_timer, "clearImmediate", 1)?;
    st.defglobal("clearImmediate", JS_DONTENUM)?;
    Ok(())
}

/// Pump the timer queue to completion: run every due timer, sleeping until
/// the next due time when timers remain, and reschedule intervals. Stdin
/// key events are polled each iteration while raw mode is enabled.
pub fn pump(st: &mut State) -> R<()> {
    loop {
        // Non-blocking stdin poll — process keys even while timers are active
        #[cfg(feature = "process")]
        crate::builtins::modules::process::poll_stdin(st)?;

        let now = Instant::now();
        // find the earliest due timer
        let mut earliest: Option<usize> = None;
        for (i, t) in st.timers.iter().enumerate() {
            if t.due <= now && earliest.is_none_or(|e| st.timers[e].due > t.due) {
                earliest = Some(i);
            }
        }

        let Some(i) = earliest else {
            // nothing due yet
            match st.timers.iter().min_by_key(|t| t.due) {
                None => {
                    // No timers at all — if stdin active, keep polling
                    #[cfg(feature = "process")]
                    if crate::builtins::modules::process::stdin_active(st) {
                        // Block on stdin with timeout so timers added later can fire
                        if let Err(_e) = crate::builtins::modules::process::poll_stdin_blocking(st) {
                            // error in listener — log and continue
                        }
                        continue;
                    }
                    return Ok(());
                }
                Some(t) => {
                    let wait = t.due.saturating_duration_since(now);
                    if !wait.is_zero() {
                        std::thread::sleep(wait);
                    }
                    continue;
                }
            }
        };

        let t = st.timers.remove(i);
        // intervals reschedule BEFORE the callback runs (like Node)
        if let Some(iv) = t.interval {
            st.timers.push(Timer {
                id: t.id,
                due: Instant::now() + Duration::from_millis(iv as u64),
                interval: Some(iv),
                callback: t.callback.clone(),
                args: t.args.clone(),
            });
        }

        // call the callback with its args
        st.push_value(t.callback)?;
        st.push_undefined()?; // this
        let n = t.args.len();
        for a in t.args {
            st.push_value(a)?;
        }
        st.call(n)?;
        st.pop(1);
    }
}
