//! path module: path string manipulation (no I/O).

#![cfg(feature = "path")]

use super::opt_str;
use crate::builtins::propf;
use crate::state::{State, R};

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

fn path_sep() -> &'static str {
    if cfg!(windows) {
        "\\"
    } else {
        "/"
    }
}

fn path_delimiter() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

fn normalize_parts(p: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let abs = is_sep(p.chars().next().unwrap_or(' '));
    for part in p.split(is_sep) {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|t| t != "..") {
                    out.pop();
                } else if !abs {
                    out.push("..".to_string());
                }
            }
            s => out.push(s.to_string()),
        }
    }
    if abs {
        out.insert(0, String::new());
    }
    out
}

fn path_join(st: &mut State) -> R<()> {
    let top = st.gettop();
    let mut parts = Vec::new();
    for i in 1..top {
        parts.push(st.tostring(i)?.to_string());
    }
    let joined = parts.join("/");
    let n = normalize_parts(&joined);
    let s = if joined.starts_with('/') || joined.starts_with('\\') {
        format!("/{}", n.join("/"))
    } else {
        n.join("/")
    };
    st.push_string(&s.replace('/', path_sep()))
}

fn path_resolve(st: &mut State) -> R<()> {
    let top = st.gettop();
    let mut parts = Vec::new();
    for i in 1..top {
        parts.push(st.tostring(i)?.to_string());
    }
    let joined = parts.join("/");
    let abs = if is_sep(joined.chars().next().unwrap_or(' ')) {
        joined
    } else {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{}/{}", cwd, joined)
    };
    let n = normalize_parts(&abs);
    let s = format!("/{}", n.join("/"));
    st.push_string(&s.replace('/', path_sep()))
}

fn path_dirname(st: &mut State) -> R<()> {
    let p = st.tostring(1)?;
    let trimmed = p.trim_end_matches(is_sep);
    match trimmed.rfind(is_sep) {
        Some(0) => st.push_string(&trimmed[..1]),
        Some(i) => st.push_string(&trimmed[..i]),
        None => st.push_string("."),
    }
}

fn path_basename(st: &mut State) -> R<()> {
    let p = st.tostring(1)?;
    let ext = opt_str(st, 2)?;
    let trimmed = p.trim_end_matches(is_sep);
    let base = trimmed.rsplit(is_sep).next().unwrap_or(trimmed);
    if let Some(ext) = ext
        && let Some(stripped) = base.strip_suffix(ext.as_ref()) {
            return st.push_string(stripped);
        }
    st.push_string(base)
}

fn path_extname(st: &mut State) -> R<()> {
    let p = st.tostring(1)?;
    let trimmed = p.trim_end_matches(is_sep);
    let base = trimmed.rsplit(is_sep).next().unwrap_or(trimmed);
    match base.rfind('.') {
        Some(i) if i > 0 => st.push_string(&base[i..]),
        _ => st.push_string(""),
    }
}

fn path_isabsolute(st: &mut State) -> R<()> {
    let p = st.tostring(1)?;
    let b = is_sep(p.chars().next().unwrap_or(' '))
        || (p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic());
    st.push_boolean(b)
}

fn path_normalize(st: &mut State) -> R<()> {
    let p = st.tostring(1)?;
    let n = normalize_parts(&p);
    let s = if is_sep(p.chars().next().unwrap_or(' ')) {
        format!("/{}", n.join("/"))
    } else if n.is_empty() {
        ".".to_string()
    } else {
        n.join("/")
    };
    st.push_string(&s.replace('/', path_sep()))
}

/// Create the `path` module object.
pub fn make(st: &mut State) -> R<()> {
    st.newobject()?;
    propf(st, "path.join", path_join, 0)?;
    propf(st, "path.resolve", path_resolve, 0)?;
    propf(st, "path.dirname", path_dirname, 1)?;
    propf(st, "path.basename", path_basename, 2)?;
    propf(st, "path.extname", path_extname, 1)?;
    propf(st, "path.isAbsolute", path_isabsolute, 1)?;
    propf(st, "path.normalize", path_normalize, 1)?;
    st.push_string(path_sep())?;
    st.defproperty(-2, "sep", 0)?;
    st.push_string(path_delimiter())?;
    st.defproperty(-2, "delimiter", 0)?;
    Ok(())
}
