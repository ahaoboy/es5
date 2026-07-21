//! Command line interface for the es5 interpreter (replaces main.c).

use es5::builtins::repr::{js_repr, js_tryrepr};
use es5::{R, State, VERSION};
use std::io::{IsTerminal, Read, Write};

const PS1: &str = "> ";

fn jsb_gc(st: &mut State) -> R<()> {
    let report = st.toboolean(1);
    st.gc(report);
    st.push_undefined()
}

fn jsb_load(st: &mut State) -> R<()> {
    let n = st.gettop();
    for i in 1..n {
        let filename = st.tostring(i)?;
        st.loadfile(&filename)?;
        st.push_undefined()?;
        st.call(0)?;
        st.pop(1);
    }
    st.push_undefined()
}

fn jsb_compile(st: &mut State) -> R<()> {
    let source = st.tostring(1)?;
    let filename = if st.isdefined(2) {
        st.tostring(2)?
    } else {
        st.heap.intern("[string]")
    };
    st.loadstring(&filename, &source)
}

fn jsb_print(st: &mut State) -> R<()> {
    let top = st.gettop();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for i in 1..top {
        let s = st.tostring(i)?;
        if i > 1 {
            write!(out, " ").unwrap();
        }
        write!(out, "{}", s).unwrap();
    }
    writeln!(out).unwrap();
    st.push_undefined()
}

fn jsb_write(st: &mut State) -> R<()> {
    let top = st.gettop();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for i in 1..top {
        let s = st.tostring(i)?;
        if i > 1 {
            write!(out, " ").unwrap();
        }
        write!(out, "{}", s).unwrap();
    }
    out.flush().unwrap();
    st.push_undefined()
}

fn jsb_read(st: &mut State) -> R<()> {
    let filename = st.tostring(1)?;
    match std::fs::read(&*filename) {
        Ok(data) => {
            let s = String::from_utf8_lossy(&data).into_owned();
            st.push_string(&s)
        }
        Err(e) => st.error(&format!("cannot open file '{}': {}", filename, e)),
    }
}

fn jsb_readline(st: &mut State) -> R<()> {
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

fn jsb_quit(st: &mut State) -> R<()> {
    let code = st.tonumber(1).unwrap_or(0.0);
    std::process::exit(code as i32);
}

fn jsb_repr(st: &mut State) -> R<()> {
    js_repr(st, 1)
}

const REQUIRE_JS: &str = "function require(name) {\n\
var cache = require.cache;\n\
if (name in cache) return cache[name];\n\
var exports = {};\n\
cache[name] = exports;\n\
Function('exports', read(name+'.js'))(exports);\n\
return exports;\n\
}\n\
require.cache = Object.create(null);\n";

const STACKTRACE_JS: &str = "Error.prototype.toString = function() { return this.stack }\n";

const CONSOLE_JS: &str =
    "var console = { log: print, debug: print, info: print, warn: print, error: print, trace: print };";

fn jsb_console_clear(st: &mut State) -> R<()> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    // ANSI: cursor home + clear screen + clear scrollback
    let _ = out.write_all(b"\x1b[H\x1b[2J\x1b[3J");
    let _ = out.flush();
    st.push_undefined()
}

fn jsb_console_time(st: &mut State) -> R<()> {
    let label = if st.isdefined(1) {
        st.tostring(1)?.to_string()
    } else {
        "default".to_string()
    };
    CONSOLE_TIMERS.with(|m| {
        m.borrow_mut().insert(label, std::time::Instant::now());
    });
    st.push_undefined()
}

fn jsb_console_timeend(st: &mut State) -> R<()> {
    let label = if st.isdefined(1) {
        st.tostring(1)?.to_string()
    } else {
        "default".to_string()
    };
    let start = CONSOLE_TIMERS.with(|m| m.borrow_mut().remove(&label));
    if let Some(t) = start {
        println!("{}: {}ms", label, t.elapsed().as_millis());
    }
    st.push_undefined()
}

thread_local! {
    static CONSOLE_TIMERS: std::cell::RefCell<std::collections::HashMap<String, std::time::Instant>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Extend the console object with clear/time methods.
fn init_console(st: &mut State) {
    if st.getglobal("console").is_ok() {
        st.newcfunction(jsb_console_clear, "clear", 0).unwrap();
        st.defproperty(-2, "clear", 0).unwrap();
        st.newcfunction(jsb_console_time, "time", 1).unwrap();
        st.defproperty(-2, "time", 0).unwrap();
        st.newcfunction(jsb_console_timeend, "timeEnd", 1).unwrap();
        st.defproperty(-2, "timeEnd", 0).unwrap();
        st.pop(1);
    }
}

/// Evaluate one line of REPL input and print the result value
/// (Node.js-style: also prints `undefined`).
fn eval_print(st: &mut State, source: &str) -> i32 {
    if st.ploadstring("[stdin]", source) != 0 {
        st.report_error(-1);
        st.pop(1);
        return 1;
    }
    st.push_undefined().unwrap();
    if st.pcall(0) {
        st.report_error(-1);
        st.pop(1);
        return 1;
    }
    let s = js_tryrepr(st, -1, "can't convert to string");
    println!("{}", s);
    st.pop(1);
    pump_timers(st);
    0
}

/// Run the timer queue + stdin event loop.
#[cfg(any(feature = "modules", feature = "timers"))]
fn pump_timers(st: &mut State) {
    let _ = es5::builtins::modules::timers::pump(st);
}

fn read_stdin() -> Option<String> {
    let mut buf = Vec::new();
    match std::io::stdin().read_to_end(&mut buf) {
        Ok(_) => Some(String::from_utf8_lossy(&buf).into_owned()),
        Err(_) => {
            eprintln!("error reading stdin");
            None
        }
    }
}

fn print_help() {
    println!("{}", VERSION);
    println!("A JavaScript ES5 interpreter.\n");
    println!("Usage: es5 [options] [script [scriptArgs*]]");
    println!();
    println!("Options:");
    println!("  -e <code> Evaluate the given code");
    println!("  -i        Enter the interactive prompt (REPL) after running code");
    println!("  -s        Enable strict mode by default");
    println!("  -R <n>    Stop execution after <n> instructions");
    println!("  -M <n>    Limit memory usage to <n> bytes");
    println!("  -h, --help       Show this help message");
    println!("  -V, --version    Show version information");
}

fn print_version() {
    println!("{}", VERSION);
}

fn main() {
    // JavaScript function calls recurse natively, so run the interpreter on
    // a thread with a large stack; MuJS's JS_ENVLIMIT/JS_STACKSIZE limits
    // will trigger a catchable "stack overflow" error long before the
    // native stack is exhausted.
    let child = std::thread::Builder::new()
        .name("es5-main".to_string())
        .stack_size(512 * 1024 * 1024)
        .spawn(real_main)
        .expect("failed to spawn main thread");
    let status = child.join().unwrap_or(1);
    std::process::exit(status);
}

fn real_main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let mut strict = false;
    let mut interactive = false;
    let mut runlimit = 0;
    let mut memlimit = 0;
    let mut eval_expr: Option<String> = None;

    // parse options: -e <code> -i -s -R <n> -M <n> -h -V
    let mut optind = 1;
    while optind < args.len() {
        let arg = &args[optind];
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        if arg == "--" {
            optind += 1;
            break;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return 0;
            }
            "-V" | "--version" => {
                print_version();
                return 0;
            }
            "-i" => interactive = true,
            "-s" => strict = true,
            "-e" => {
                optind += 1;
                match args.get(optind) {
                    Some(code) => eval_expr = Some(code.clone()),
                    None => {
                        eprintln!("es5: option -e requires an argument");
                        return 1;
                    }
                }
            }
            "-R" => {
                optind += 1;
                match args.get(optind).and_then(|s| s.parse().ok()) {
                    Some(n) => runlimit = n,
                    None => {
                        eprintln!("es5: option -R requires an argument");
                        return 1;
                    }
                }
            }
            "-M" => {
                optind += 1;
                match args.get(optind).and_then(|s| s.parse().ok()) {
                    Some(n) => memlimit = n,
                    None => {
                        eprintln!("es5: option -M requires an argument");
                        return 1;
                    }
                }
            }
            _ if arg.starts_with("-R") && arg.len() > 2 => {
                runlimit = arg[2..].parse().unwrap_or(0);
            }
            _ if arg.starts_with("-M") && arg.len() > 2 => {
                memlimit = arg[2..].parse().unwrap_or(0);
            }
            _ => {
                eprintln!("es5: unknown option '{}' (try -h)", arg);
                return 1;
            }
        }
        optind += 1;
    }

    let mut st = State::new(if strict { es5::state::JS_STRICT } else { 0 });

    st.newcfunction(jsb_gc, "gc", 0).unwrap();
    st.setglobal("gc").unwrap();

    st.newcfunction(jsb_load, "load", 1).unwrap();
    st.setglobal("load").unwrap();

    st.newcfunction(jsb_compile, "compile", 2).unwrap();
    st.setglobal("compile").unwrap();

    st.newcfunction(jsb_print, "print", 0).unwrap();
    st.setglobal("print").unwrap();

    st.newcfunction(jsb_write, "write", 0).unwrap();
    st.setglobal("write").unwrap();

    st.newcfunction(jsb_read, "read", 1).unwrap();
    st.setglobal("read").unwrap();

    st.newcfunction(jsb_readline, "readline", 0).unwrap();
    st.setglobal("readline").unwrap();

    st.newcfunction(jsb_repr, "repr", 0).unwrap();
    st.setglobal("repr").unwrap();

    st.newcfunction(jsb_quit, "quit", 1).unwrap();
    st.setglobal("quit").unwrap();

    st.dostring(REQUIRE_JS);
    st.dostring(STACKTRACE_JS);
    st.dostring(CONSOLE_JS);
    init_console(&mut st);

    #[cfg(any(
        feature = "require",
        feature = "modules",
        feature = "timers",
        feature = "process",
        feature = "fs",
        feature = "os",
        feature = "path",
        feature = "child_process"
    ))]
    es5::builtins::modules::init_cli(&mut st, &args[1..]).unwrap();

    let mut status = 0;

    if let Some(code) = eval_expr {
        st.setlimit(runlimit, memlimit);
        if st.dostring(&code) != 0 {
            status = 1;
        } else {
            pump_timers(&mut st);
        }
    } else if optind >= args.len() {
        interactive = true;
    } else {
        let script = args[optind].clone();
        optind += 1;

        st.newarray().unwrap();
        let mut i = 0;
        while optind < args.len() {
            st.push_string(&args[optind]).unwrap();
            st.setindex(-2, i).unwrap();
            i += 1;
            optind += 1;
        }
        st.setglobal("scriptArgs").unwrap();

        st.setlimit(runlimit, memlimit);
        if st.dofile(&script) != 0 {
            status = 1;
        } else {
            pump_timers(&mut st);
        }
    }

    if interactive {
        let is_tty = std::io::stdin().is_terminal();
        if is_tty {
            println!("Welcome to es5 {}.", VERSION);
            loop {
                print!("{}", PS1);
                std::io::stdout().flush().unwrap();
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        st.setlimit(runlimit, memlimit);
                        eval_print(&mut st, &line);
                    }
                    Err(_) => break,
                }
            }
            println!();
        } else {
            let input = read_stdin();
            st.setlimit(runlimit, memlimit);
            match input {
                Some(src) => {
                    if st.dostring(&src) != 0 {
                        status = 1;
                    }
                }
                None => status = 1,
            }
        }
    }

    st.gc(false);

    if std::env::var("ES5_STATS").is_ok() {
        let s = &es5::state::STATS;
        eprintln!(
            "[stats] concat: calls={} bytes={}MB | gc: calls={} time={}ms",
            s.concat_calls.load(std::sync::atomic::Ordering::Relaxed),
            s.concat_bytes.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000,
            s.gc_calls.load(std::sync::atomic::Ordering::Relaxed),
            s.gc_nanos.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000,
        );
    }

    status
}
