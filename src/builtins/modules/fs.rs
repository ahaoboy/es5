//! fs module: synchronous (`*Sync`) and classic-callback file APIs.

#![cfg(feature = "fs")]

use super::{callback_err, callback_ok, io_error};
use crate::builtins::propf;
use crate::state::{State, R};
use crate::value::JS_DONTENUM;

fn fs_readfilesync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    match std::fs::read(&*path) {
        Ok(data) => {
            let s = String::from_utf8_lossy(&data).into_owned();
            st.push_string(&s)
        }
        Err(e) => {
            let v = io_error(st, "ENOENT: no such file or directory, open", &path, &e);
            Err(v)
        }
    }
}

fn fs_writefilesync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    let data = st.tostring(2)?;
    match std::fs::write(&*path, data.as_bytes()) {
        Ok(()) => st.push_undefined(),
        Err(e) => {
            let v = io_error(st, "EIO: write", &path, &e);
            Err(v)
        }
    }
}

fn fs_appendfilesync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    let data = st.tostring(2)?;
    let r = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&*path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, data.as_bytes()));
    match r {
        Ok(()) => st.push_undefined(),
        Err(e) => {
            let v = io_error(st, "EIO: append", &path, &e);
            Err(v)
        }
    }
}

fn fs_existssync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    let b = std::path::Path::new(&*path).exists();
    st.push_boolean(b)
}

fn fs_statsync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    match std::fs::metadata(&*path) {
        Ok(md) => {
            let size = md.len() as f64;
            let is_dir = md.is_dir();
            let is_file = md.is_file();
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as f64)
                .unwrap_or(0.0);
            st.newobject()?;
            st.push_number(size)?;
            st.defproperty(-2, "size", 0)?;
            st.push_number(mtime)?;
            st.defproperty(-2, "mtimeMs", 0)?;
            st.push_number(mtime / 1000.0)?;
            st.defproperty(-2, "mtime", 0)?;
            st.push_boolean(is_file)?;
            st.defproperty(-2, "__isfile", JS_DONTENUM)?;
            st.push_boolean(is_dir)?;
            st.defproperty(-2, "__isdir", JS_DONTENUM)?;
            st.newcfunction(fs_stat_isfile, "isFile", 0)?;
            st.defproperty(-2, "isFile", 0)?;
            st.newcfunction(fs_stat_isdir, "isDirectory", 0)?;
            st.defproperty(-2, "isDirectory", 0)?;
            Ok(())
        }
        Err(e) => {
            let v = io_error(st, "ENOENT: no such file or directory, stat", &path, &e);
            Err(v)
        }
    }
}

fn fs_stat_isfile(st: &mut State) -> R<()> {
    let b = if let Ok(true) = st.hasproperty(0, "__isfile") {
        st.toboolean(-1)
    } else {
        false
    };
    st.pop(1);
    st.push_boolean(b)
}

fn fs_stat_isdir(st: &mut State) -> R<()> {
    let b = if let Ok(true) = st.hasproperty(0, "__isdir") {
        st.toboolean(-1)
    } else {
        false
    };
    st.pop(1);
    st.push_boolean(b)
}

fn fs_readdirsync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    match std::fs::read_dir(&*path) {
        Ok(rd) => {
            st.newarray()?;
            for (i, e) in rd.flatten().enumerate() {
                let name = e.file_name().to_string_lossy().into_owned();
                st.push_string(&name)?;
                st.setindex(-2, i as i32)?;
            }
            Ok(())
        }
        Err(e) => {
            let v = io_error(st, "ENOENT: no such file or directory, scandir", &path, &e);
            Err(v)
        }
    }
}

fn fs_mkdirsync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    match std::fs::create_dir_all(&*path) {
        Ok(()) => st.push_undefined(),
        Err(e) => {
            let v = io_error(st, "EIO: mkdir", &path, &e);
            Err(v)
        }
    }
}

fn fs_unlinksync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    match std::fs::remove_file(&*path) {
        Ok(()) => st.push_undefined(),
        Err(e) => {
            let v = io_error(st, "ENOENT: no such file or directory, unlink", &path, &e);
            Err(v)
        }
    }
}

fn fs_rmdirsync(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    match std::fs::remove_dir(&*path) {
        Ok(()) => st.push_undefined(),
        Err(e) => {
            let v = io_error(st, "EIO: rmdir", &path, &e);
            Err(v)
        }
    }
}

fn fs_renamesync(st: &mut State) -> R<()> {
    let old = st.tostring(1)?;
    let new = st.tostring(2)?;
    match std::fs::rename(&*old, &*new) {
        Ok(()) => st.push_undefined(),
        Err(e) => {
            let v = io_error(st, "EIO: rename", &old, &e);
            Err(v)
        }
    }
}

// -- callback forms (cb invoked synchronously) --

fn fs_readfile(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    match std::fs::read(&*path) {
        Ok(data) => {
            let s = String::from_utf8_lossy(&data).into_owned();
            callback_ok(st, |st| st.push_string(&s))
        }
        Err(e) => {
            let v = io_error(st, "ENOENT: no such file or directory, open", &path, &e);
            callback_err(st, v)
        }
    }
}

fn fs_writefile(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    let data = st.tostring(2)?;
    match std::fs::write(&*path, data.as_bytes()) {
        Ok(()) => callback_ok(st, |_| Ok(())),
        Err(e) => {
            let v = io_error(st, "EIO: write", &path, &e);
            callback_err(st, v)
        }
    }
}

fn fs_appendfile(st: &mut State) -> R<()> {
    let path = st.tostring(1)?;
    let data = st.tostring(2)?;
    let r = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&*path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, data.as_bytes()));
    match r {
        Ok(()) => callback_ok(st, |_| Ok(())),
        Err(e) => {
            let v = io_error(st, "EIO: append", &path, &e);
            callback_err(st, v)
        }
    }
}

/// Create the `fs` module object.
pub fn make(st: &mut State) -> R<()> {
    st.newobject()?;
    propf(st, "fs.readFileSync", fs_readfilesync, 1)?;
    propf(st, "fs.writeFileSync", fs_writefilesync, 2)?;
    propf(st, "fs.appendFileSync", fs_appendfilesync, 2)?;
    propf(st, "fs.existsSync", fs_existssync, 1)?;
    propf(st, "fs.statSync", fs_statsync, 1)?;
    propf(st, "fs.readdirSync", fs_readdirsync, 1)?;
    propf(st, "fs.mkdirSync", fs_mkdirsync, 1)?;
    propf(st, "fs.unlinkSync", fs_unlinksync, 1)?;
    propf(st, "fs.rmdirSync", fs_rmdirsync, 1)?;
    propf(st, "fs.renameSync", fs_renamesync, 2)?;
    propf(st, "fs.readFile", fs_readfile, 2)?;
    propf(st, "fs.writeFile", fs_writefile, 3)?;
    propf(st, "fs.appendFile", fs_appendfile, 3)?;
    Ok(())
}
