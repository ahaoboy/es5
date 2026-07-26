//! Node.js-style CLI modules, split by name. Each module is gated by its
//! own cargo feature (`fs`, `os`, `path`, `process`); the `modules`
//! feature enables all of them. APIs are synchronous (`*Sync`, throwing on
//! error) or classic callback style (`callback(err, result)` fired
//! synchronously).

pub mod child_process;
pub mod fs;
pub mod os;
pub mod path;
pub mod process;
pub mod timers;

use crate::state::{State, R};

/// Build an Error value from an io::Error.
pub(crate) fn io_error(
    st: &mut State,
    prefix: &str,
    path: &str,
    e: &std::io::Error,
) -> crate::value::Value {
    let msg = format!("{}: '{}' ({})", prefix, path, e);
    st.new_errorx(&msg, st.protos.error)
        .unwrap_or(crate::value::Value::Null)
}

pub(crate) fn opt_str(st: &mut State, idx: i32) -> R<Option<compact_str::CompactString>> {
    if st.isundefined(idx) {
        Ok(None)
    } else {
        Ok(Some(st.tostring(idx)?))
    }
}

/// Invoke `callback` (at stack slot 2) with (null, result) on success.
pub(crate) fn callback_ok(st: &mut State, result: impl Fn(&mut State) -> R<()>) -> R<()> {
    st.copy(2)?; // callback
    st.push_undefined()?; // this
    st.push_null()?; // err = null
    result(st)?;
    st.call(2)?;
    st.push_undefined()
}

/// Invoke `callback` (at stack slot 2) with (err) on failure.
pub(crate) fn callback_err(st: &mut State, err: crate::value::Value) -> R<()> {
    st.copy(2)?; // callback
    st.push_undefined()?; // this
    st.push_value(err)?; // err
    st.call(1)?;
    st.push_undefined()
}

/// Create and inject the global `process` object and register the enabled
/// builtin modules into `require.cache`.
pub fn init_cli(st: &mut State, argv: &[String]) -> R<()> {
    #[cfg(feature = "process")]
    process::register(st, argv)?;
    #[cfg(feature = "timers")]
    timers::register(st)?;

    // native modules in require.cache (require is the main.c JS snippet)
    st.getglobal("require")?;
    if st.isobject(-1) {
        st.getproperty(-1, "cache")?;
        if st.isobject(-1) {
            // stack: [require, cache]; defproperty(-2, ...) targets cache
            #[cfg(feature = "fs")]
            {
                fs::make(st)?;
                st.defproperty(-2, "fs", 0)?;
                st.getproperty(-1, "fs")?;
                st.defproperty(-2, "node:fs", 0)?;
            }
            #[cfg(feature = "os")]
            {
                os::make(st)?;
                st.defproperty(-2, "os", 0)?;
                st.getproperty(-1, "os")?;
                st.defproperty(-2, "node:os", 0)?;
            }
            #[cfg(feature = "path")]
            {
                path::make(st)?;
                st.defproperty(-2, "path", 0)?;
                st.getproperty(-1, "path")?;
                st.defproperty(-2, "node:path", 0)?;
            }
            #[cfg(feature = "child_process")]
            {
                child_process::make(st)?;
                st.defproperty(-2, "child_process", 0)?;
                st.getproperty(-1, "child_process")?;
                st.defproperty(-2, "node:child_process", 0)?;
            }
            #[cfg(feature = "process")]
            {
                st.getglobal("process")?;
                st.defproperty(-2, "process", 0)?;
                st.getproperty(-1, "process")?;
                st.defproperty(-2, "node:process", 0)?;
            }
        }
        st.pop(1); // cache
    }
    st.pop(1); // require

    Ok(())
}
