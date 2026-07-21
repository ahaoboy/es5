//! The global `process` object: stdio streams, exit, argv, env, cwd,
//! platform/arch, pid, uptime.

#![cfg(feature = "process")]

use crate::state::{State, R};
use crate::value::{JS_DONTENUM, Value};

/// Track raw-mode state (crossterm has no query API).
static RAW_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Shared read buffer for raw-mode stdin.
static READ_BUF: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
/// Line accumulator for "data" events in raw mode.
static LINE_BUF: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

pub fn raw_mode_enabled() -> bool {
    RAW_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Wrapper accepting any crossterm Event for poll_stdin_loop in main.
pub fn key_to_str(ev: crossterm::event::Event) -> Option<String> {
    match ev {
        crossterm::event::Event::Key(key_ev) => key_event_to_string(&key_ev),
        _ => None,
    }
}

/// Push a key string into the shared buffer (consumed by stdin.read()).
pub fn push_read_data(key: &str) {
    READ_BUF.lock().unwrap().push(key.to_string());
}

/// Check whether stdin has any active listeners (keeps the pump alive).
pub fn stdin_active(st: &mut State) -> bool {
    if !raw_mode_enabled() {
        return false;
    }
    let g = st.g;
    if let Some(process_ref) = st.heap.get_property(g, "process")
        .and_then(|p| p.value.as_object())
        && let Some(stdin_ref) = st.heap.get_property(process_ref, "stdin")
            .and_then(|p| p.value.as_object())
            && let Some(listeners_obj) = st.heap.get_property(stdin_ref, "__listeners")
                .and_then(|p| p.value.as_object()) {
                let has_readable = st.heap.get_property(listeners_obj, "readable")
                    .and_then(|p| p.value.as_object())
                    .map(|arr| match &st.heap.obj(arr).payload {
                        crate::object::Payload::Array(a) => !a.flat.is_empty(),
                        _ => false,
                    })
                    .unwrap_or(false);
                let has_data = st.heap.get_property(listeners_obj, "data")
                    .and_then(|p| p.value.as_object())
                    .map(|arr| match &st.heap.obj(arr).payload {
                        crate::object::Payload::Array(a) => !a.flat.is_empty(),
                        _ => false,
                    })
                    .unwrap_or(false);
                return has_readable || has_data;
            }
    false
}

/// Map crossterm KeyEvent to a Node-style key string.
fn key_event_to_string(ev: &crossterm::event::KeyEvent) -> Option<String> {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    // Only process key Press events (skip Release and Repeat)
    if ev.kind != KeyEventKind::Press {
        return None;
    }
    match &ev.code {
        KeyCode::Char(c) => {
            if ev.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_lowercase() {
                Some((((*c as u8) - b'a' + 1) as char).to_string())
            } else {
                Some(c.to_string())
            }
        }
        KeyCode::Enter => Some("\r".to_string()),
        KeyCode::Backspace => Some("\x08".to_string()),
        KeyCode::Tab => Some("\t".to_string()),
        KeyCode::Esc => Some("\x1b".to_string()),
        KeyCode::Left => Some("\x1b[D".to_string()),
        KeyCode::Right => Some("\x1b[C".to_string()),
        KeyCode::Up => Some("\x1b[A".to_string()),
        KeyCode::Down => Some("\x1b[B".to_string()),
        KeyCode::Home => Some("\x1b[H".to_string()),
        KeyCode::End => Some("\x1b[F".to_string()),
        KeyCode::Delete => Some("\x1b[3~".to_string()),
        _ => None,
    }
    .map(|s| {
        if ev.modifiers.contains(KeyModifiers::CONTROL) && !matches!(ev.code, KeyCode::Char(_)) {
            format!("\x1b[1;5{}", &s[1..])
        } else {
            s
        }
    })
}

/// Fire "end" listeners (no args).
fn fire_end_listeners(st: &mut State) -> R<()> {
    let g = st.g;
    let stdin_ref = match st.heap.get_property(g, "process")
        .and_then(|p| p.value.as_object())
        .and_then(|process_ref| st.heap.get_property(process_ref, "stdin"))
        .and_then(|p| p.value.as_object()) {
        Some(o) => o,
        None => return Ok(()),
    };
    let listeners_obj = match st.heap.get_property(stdin_ref, "__listeners")
        .and_then(|p| p.value.as_object()) {
        Some(o) => o,
        None => return Ok(()),
    };
    let end_arr = st.heap.get_property(listeners_obj, "end")
        .and_then(|p| p.value.as_object());
    if let Some(arr) = end_arr {
        let listeners: Vec<Value> = match &st.heap.obj(arr).payload {
            crate::object::Payload::Array(a) => a.flat.clone(),
            _ => vec![],
        };
        for listener in &listeners {
            if let Value::Object(_) = listener {
                st.push_value(listener.clone())?;
                st.push_undefined()?;
                let _ = st.call(0);
                st.pop(1);
            }
        }
    }
    Ok(())
}

/// Poll stdin for a single key event (non-blocking) and notify listeners.
pub fn poll_stdin(st: &mut State) -> R<()> {
    use crossterm::event::Event;
    if !raw_mode_enabled() {
        return Ok(());
    }
    // Check if there are any listeners
    let g = st.g;
    let stdin_ref = match st.heap.get_property(g, "process")
        .and_then(|p| p.value.as_object())
        .and_then(|process_ref| st.heap.get_property(process_ref, "stdin"))
        .and_then(|p| p.value.as_object()) {
        Some(o) => o,
        None => return Ok(()),
    };
    let listeners_obj = match st.heap.get_property(stdin_ref, "__listeners")
        .and_then(|p| p.value.as_object()) {
        Some(o) => o,
        None => return Ok(()),
    };

    if !crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        return Ok(());
    }
    let ev = match crossterm::event::read() {
        Ok(Event::Key(key_ev)) => key_ev,
        _ => return Ok(()),
    };
    let key_str = match key_event_to_string(&ev) {
        Some(s) => s,
        None => return Ok(()),
    };

    // Ctrl+D = EOF — fire "end" event
    if key_str == "\x04" {
        fire_end_listeners(st)?;
        return Ok(());
    }

    // Store key data in the shared buffer for read() to consume
    READ_BUF.lock().unwrap().push(key_str.clone());

    // Fire "readable" and "data" event listeners
    let readable_arr = st.heap.get_property(listeners_obj, "readable")
        .and_then(|p| p.value.as_object());
    let data_arr = st.heap.get_property(listeners_obj, "data")
        .and_then(|p| p.value.as_object());

    // Fire readable (no args)
    if let Some(arr) = readable_arr {
        let listeners: Vec<Value> = match &st.heap.obj(arr).payload {
            crate::object::Payload::Array(a) => a.flat.clone(),
            _ => vec![],
        };
        for listener in &listeners {
            if let Value::Object(_) = listener {
                st.push_value(listener.clone())?;
                st.push_undefined()?;
                st.call(0)?;
                st.pop(1);
            }
        }
    }

    // Fire data — accumulate until Enter, then fire with whole line
    if let Some(arr) = data_arr {
        let is_enter = key_str == "\r" || key_str == "\n";
        let mut fire_line: Option<String> = None;
        if is_enter {
            let line = {
                let mut buf = LINE_BUF.lock().unwrap();
                let line = std::mem::take(&mut *buf);
                buf.clear();
                line
            };
            if !line.is_empty() { fire_line = Some(line); }
        } else {
            LINE_BUF.lock().unwrap().push_str(&key_str);
        }
        if let Some(line) = fire_line {
            let listeners: Vec<Value> = match &st.heap.obj(arr).payload {
                crate::object::Payload::Array(a) => a.flat.clone(),
                _ => vec![],
            };
            for listener in &listeners {
                if let Value::Object(_) = listener {
                    st.push_value(listener.clone())?;
                    st.push_undefined()?;
                    st.push_string(&line)?;
                    st.call(1)?;
                    st.pop(1);
                }
            }
        }
    }
    Ok(())
}

/// Blocking stdin poll: waits for a key event, stores it, and fires listeners.
pub fn poll_stdin_blocking(st: &mut State) -> R<()> {
    if !raw_mode_enabled() {
        return Ok(());
    }
    let ev = match crossterm::event::read() {
        Ok(ev) => ev,
        Err(_) => return Ok(()),
    };
    let Some(key_str) = key_to_str(ev) else { return Ok(()) };

    // Ctrl+D = EOF — fire "end" event
    if key_str == "\x04" {
        fire_end_listeners(st)?;
        return Ok(());
    }
    READ_BUF.lock().unwrap().push(key_str.clone());

    let saved = st.gettop();
    st.getglobal("process")?;
    st.getproperty(-1, "stdin")?;
    st.getproperty(-1, "__listeners")?;

    // Fire "readable" listeners (no args)
    let readable_fns = if st.hasproperty(-1, "readable")? {
        st.getproperty(-2, "readable")?;
        let len = st.getlength(-1)?;
        let mut fns = Vec::new();
        for i in 0..len {
            st.getindex(-1, i)?;
            fns.push(st.stackidx(-1).clone());
            st.pop(1);
        }
        st.pop(1);
        fns
    } else { Vec::new() };

    // Fire "data" listeners — accumulate until Enter, then fire with whole line
    let mut data_line: Option<String> = None;
    let data_fns = if st.hasproperty(-1, "data")? {
        let is_enter = key_str == "\r" || key_str == "\n";
        if is_enter {
            let line = {
                let mut buf = LINE_BUF.lock().unwrap();
                let line = std::mem::take(&mut *buf);
                buf.clear();
                line
            };
            if !line.is_empty() {
                data_line = Some(line);
            }
        } else {
            LINE_BUF.lock().unwrap().push_str(&key_str);
        }
        if data_line.is_some() {
            st.getproperty(-2, "data")?;
            let len = st.getlength(-1)?;
            let mut fns = Vec::new();
            for i in 0..len {
                st.getindex(-1, i)?;
                fns.push(st.stackidx(-1).clone());
                st.pop(1);
            }
            st.pop(1);
            fns
        } else { Vec::new() }
    } else { Vec::new() };

    while st.gettop() > saved { st.pop(1); }

    for f in &readable_fns {
        if let crate::value::Value::Object(_) = f {
            st.push_value(f.clone())?;
            st.push_undefined()?;
            let _ = st.call(0);
            st.pop(1);
        }
    }
    for f in &data_fns {
        if let crate::value::Value::Object(_) = f
            && let Some(line) = &data_line {
                st.push_value(f.clone())?;
                st.push_undefined()?;
                st.push_string(line)?;
                let _ = st.call(1);
                st.pop(1);
            }
    }
    Ok(())
}

/// Node-style platform name.
fn platform_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

/// Node-style architecture name.
fn arch_name() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        other => other,
    }
}

fn process_exit(st: &mut State) -> R<()> {
    let code = st.tonumber(1).unwrap_or(0.0);
    std::process::exit(code as i32);
}

fn process_cwd(st: &mut State) -> R<()> {
    let d = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    st.push_string(&d)
}

fn process_uptime(st: &mut State) -> R<()> {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let secs = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64();
    st.push_number(secs)
}

fn stdout_write(st: &mut State) -> R<()> {
    let top = st.gettop();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for i in 1..top {
        let s = st.tostring(i)?;
        std::io::Write::write_all(&mut out, s.as_bytes()).ok();
    }
    st.push_undefined()
}

fn stderr_write(st: &mut State) -> R<()> {
    let top = st.gettop();
    let stderr = std::io::stderr();
    let mut out = stderr.lock();
    for i in 1..top {
        let s = st.tostring(i)?;
        std::io::Write::write_all(&mut out, s.as_bytes()).ok();
    }
    st.push_undefined()
}

fn stdin_read(st: &mut State) -> R<()> {
    // In raw mode: read from the shared buffer (populated by poll_stdin_loop)
    if raw_mode_enabled() {
        let s = READ_BUF.lock().unwrap().pop();
        return match s {
            Some(s) => st.push_string(&s),
            None => st.push_null(),
        };
    }
    // Cooked mode: blocking read
    use std::io::Read;
    let size: Option<usize> = if st.isdefined(1) {
        let n = st.tointeger(1)?;
        if n > 0 { Some(n as usize) } else { None }
    } else {
        None
    };
    let buf: Vec<u8> = match size {
        Some(n) => {
            let mut v = vec![0u8; n];
            match std::io::stdin().read(&mut v) {
                Ok(0) => return st.push_null(),
                Ok(m) => { v.truncate(m); v }
                Err(_) => return st.push_null(),
            }
        }
        None => {
            let mut v = Vec::new();
            match std::io::stdin().read_to_end(&mut v) {
                Ok(_) => v,
                Err(_) => return st.push_null(),
            }
        }
    };
    let s = String::from_utf8_lossy(&buf).into_owned();
    st.push_string(&s)
}

fn stdin_resume(st: &mut State) -> R<()> {
    // No-op: stdin is always "flowing" in the event loop
    st.push_undefined()
}

fn stdin_set_encoding(st: &mut State) -> R<()> {
    // No-op: always UTF-8
    st.copy(0)
}

fn stdin_set_raw_mode(st: &mut State) -> R<()> {
    let enable = st.toboolean(1);
    if enable {
        match crossterm::terminal::enable_raw_mode() {
            Ok(()) => RAW_MODE.store(true, std::sync::atomic::Ordering::Relaxed),
            Err(e) => return st.error(&format!("cannot enable raw mode: {}", e)),
        }
    } else {
        RAW_MODE.store(false, std::sync::atomic::Ordering::Relaxed);
        crossterm::terminal::disable_raw_mode().ok();
    }
    st.push_undefined()
}

fn stdin_is_raw_getter(st: &mut State) -> R<()> {
    st.push_boolean(RAW_MODE.load(std::sync::atomic::Ordering::Relaxed))
}

// -- stdin event emitter ---------------------------------------------------

/// Listeners stored under `stdin.__listeners.<event>` as arr of functions.
const STDIN_LISTENERS: &str = "__listeners";

fn stdin_ensure_listeners(st: &mut State) -> R<()> {
    if !st.hasproperty(0, STDIN_LISTENERS)? {
        st.newobject()?;
        st.defproperty(0, STDIN_LISTENERS, JS_DONTENUM)?;
    }
    Ok(())
}

fn stdin_on(st: &mut State) -> R<()> {
    let event = st.tostring(1)?.to_string();
    stdin_ensure_listeners(st)?;
    // stack: stdin event fn ...
    st.getproperty(0, STDIN_LISTENERS)?;
    // stack: stdin event fn ... [listeners]
    if !st.hasproperty(-1, &event)? {
        st.newarray()?;
        // stack: stdin event fn ... [listeners] [arr]
        st.defproperty(-2, &event, JS_DONTENUM)?;
    }
    st.getproperty(-1, &event)?;
    // stack: stdin event fn ... [listeners] [arr]
    let len = st.getlength(-1)?;
    st.copy(2)?; // copy fn
    st.setindex(-2, len)?;
    st.pop(2); // [listeners] + [arr]
    st.push_undefined()
}

fn stdin_remove_listener(st: &mut State) -> R<()> {
    let event = st.tostring(1)?.to_string();
    if !st.hasproperty(0, STDIN_LISTENERS)? {
        st.push_undefined()?;
        return Ok(());
    }
    st.getproperty(0, STDIN_LISTENERS)?;
    if !st.hasproperty(-1, &event)? {
        st.pop(1);
        st.push_undefined()?;
        return Ok(());
    }
    let len = st.getlength(-1)?;
    let fn_value = st.stackidx(2).clone();
    for i in 0..len {
        st.getindex(-1, i)?;
        let is_match = fn_value == st.stackidx(-1).clone();
        st.pop(1);
        if is_match {
            st.push_null()?;
            st.setindex(-2, i)?;
            break;
        }
    }
    st.pop(1);
    st.push_undefined()
}

/// Simple emit: `stdin.emit(event, data)` — calls listeners with (data).
fn stdin_emit(st: &mut State) -> R<()> {
    // stack: stdin event data
    let event = st.tostring(1)?.to_string();
    let has_data = st.isdefined(2);

    if !st.hasproperty(0, STDIN_LISTENERS)? {
        while st.gettop() > 1 { st.pop(1); }
        st.push_undefined()?;
        return Ok(());
    }
    st.getproperty(0, STDIN_LISTENERS)?;
    // stack: stdin event data [listeners_obj]
    if !st.hasproperty(-1, &event)? {
        st.pop(2);
        while st.gettop() > 1 { st.pop(1); }
        st.push_undefined()?;
        return Ok(());
    }
    // stack: stdin event data [listeners_obj] [fn_array]
    let len = st.getlength(-1)?;
    if len == 0 {
        st.pop(2);
        while st.gettop() > 1 { st.pop(1); }
        st.push_undefined()?;
        return Ok(());
    }
    for i in 0..len {
        st.getindex(-1, i)?;
        // stack: ... [listeners_obj] [arr] fn
        if !st.iscallable(-1) {
            st.pop(1);
            continue;
        }
        st.copy(0)?; // this = stdin object
        if has_data {
            st.copy(2)?; // the data arg
        }
        let nargs = if has_data { 2 } else { 1 };
        st.call(nargs)?;
        st.pop(1); // result
        // Need to restore [listeners_obj] and [arr] for next iteration
        // They were at st.gettop()-3 and st.gettop()-2 before pushing fn,
        // now shifted... this is the problem.
        // Workaround: re-read the array each time
        st.getproperty(0, STDIN_LISTENERS)?;
        st.getproperty(-1, &event)?;
    }
    while st.gettop() > 1 { st.pop(1); }
    st.push_undefined()?;
    Ok(())
}

fn stdout_columns_getter(st: &mut State) -> R<()> {
    match crossterm::terminal::size() {
        Ok((cols, _rows)) => st.push_number(cols as f64),
        Err(_) => st.push_undefined(),
    }
}

fn stdout_rows_getter(st: &mut State) -> R<()> {
    match crossterm::terminal::size() {
        Ok((_cols, rows)) => st.push_number(rows as f64),
        Err(_) => st.push_undefined(),
    }
}

fn stdin_readline(st: &mut State) -> R<()> {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => st.push_null(),
        Ok(_) => {
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            st.push_string(&line)
        }
        Err(_) => st.push_null(),
    }
}

/// Create and inject the global `process` object.
pub fn register(st: &mut State, argv: &[String]) -> R<()> {
    st.newobject()?;

    // stdout / stderr streams
    st.newobject()?;
    st.newcfunction(stdout_write, "write", 1)?;
    st.defproperty(-2, "write", 0)?;
    // stdout.columns / stdout.rows (dynamic terminal-size getters)
    st.newcfunction(stdout_columns_getter, "get columns", 0)?;
    st.push_null()?;
    st.defaccessor(-3, "columns", JS_DONTENUM)?;
    st.newcfunction(stdout_rows_getter, "get rows", 0)?;
    st.push_null()?;
    st.defaccessor(-3, "rows", JS_DONTENUM)?;
    st.defproperty(-2, "stdout", 0)?;

    st.newobject()?;
    st.newcfunction(stderr_write, "write", 1)?;
    st.defproperty(-2, "write", 0)?;
    st.defproperty(-2, "stderr", 0)?;

    // stdin stream
    st.newobject()?;
    st.newcfunction(stdin_read, "read", 0)?;
    st.defproperty(-2, "read", 0)?;
    st.newcfunction(stdin_readline, "readline", 0)?;
    st.defproperty(-2, "readline", 0)?;
    st.newcfunction(stdin_set_raw_mode, "setRawMode", 1)?;
    st.defproperty(-2, "setRawMode", 0)?;
    st.newcfunction(stdin_resume, "resume", 0)?;
    st.defproperty(-2, "resume", 0)?;
    st.newcfunction(stdin_set_encoding, "setEncoding", 0)?;
    st.defproperty(-2, "setEncoding", 0)?;
    // isRaw getter
    st.newcfunction(stdin_is_raw_getter, "get isRaw", 0)?;
    st.push_null()?;
    st.defaccessor(-3, "isRaw", JS_DONTENUM)?;
    // event emitter
    st.newcfunction(stdin_on, "on", 2)?;
    st.defproperty(-2, "on", 0)?;
    st.newcfunction(stdin_on, "addListener", 2)?;
    st.defproperty(-2, "addListener", 0)?;
    st.newcfunction(stdin_remove_listener, "removeListener", 2)?;
    st.defproperty(-2, "removeListener", 0)?;
    st.newcfunction(stdin_emit, "emit", 2)?;
    st.defproperty(-2, "emit", 0)?;
    st.defproperty(-2, "stdin", 0)?;

    // functions
    st.newcfunction(process_exit, "exit", 1)?;
    st.defproperty(-2, "exit", 0)?;
    st.newcfunction(process_cwd, "cwd", 0)?;
    st.defproperty(-2, "cwd", 0)?;
    st.newcfunction(process_uptime, "uptime", 0)?;
    st.defproperty(-2, "uptime", 0)?;

    // plain properties
    st.push_string(platform_name())?;
    st.defproperty(-2, "platform", 0)?;
    st.push_string(arch_name())?;
    st.defproperty(-2, "arch", 0)?;
    st.push_number(std::process::id() as f64)?;
    st.defproperty(-2, "pid", 0)?;

    // argv
    st.newarray()?;
    for (i, a) in argv.iter().enumerate() {
        st.push_string(a)?;
        st.setindex(-2, i as i32)?;
    }
    st.defproperty(-2, "argv", 0)?;

    // env
    st.newobject()?;
    for (k, v) in std::env::vars() {
        st.push_string(&v)?;
        st.defproperty(-2, &k, 0)?;
    }
    st.defproperty(-2, "env", 0)?;

    st.defglobal("process", JS_DONTENUM)?;
    Ok(())
}
