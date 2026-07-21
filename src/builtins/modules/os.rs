//! os module: platform/system information.

#![cfg(feature = "os")]

use crate::builtins::propf;
use crate::state::{State, R};

fn os_platform(st: &mut State) -> R<()> {
    st.push_string(std::env::consts::OS)
}

fn os_arch(st: &mut State) -> R<()> {
    st.push_string(std::env::consts::ARCH)
}

fn os_homedir(st: &mut State) -> R<()> {
    let d = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    st.push_string(&d)
}

fn os_tmpdir(st: &mut State) -> R<()> {
    let d = std::env::temp_dir().to_string_lossy().into_owned();
    st.push_string(&d)
}

fn os_hostname(st: &mut State) -> R<()> {
    let h = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    st.push_string(&h)
}

fn os_uptime(st: &mut State) -> R<()> {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let secs = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64();
    st.push_number(secs)
}

fn os_cpus(st: &mut State) -> R<()> {
    let n = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1);
    st.newarray()?;
    for i in 0..n {
        st.newobject()?;
        st.push_string("CPU")?;
        st.defproperty(-2, "model", 0)?;
        st.setindex(-2, i as i32)?;
    }
    Ok(())
}

/// Create the `os` module object.
pub fn make(st: &mut State) -> R<()> {
    st.newobject()?;
    propf(st, "os.platform", os_platform, 0)?;
    propf(st, "os.arch", os_arch, 0)?;
    propf(st, "os.homedir", os_homedir, 0)?;
    propf(st, "os.tmpdir", os_tmpdir, 0)?;
    propf(st, "os.hostname", os_hostname, 0)?;
    propf(st, "os.uptime", os_uptime, 0)?;
    propf(st, "os.cpus", os_cpus, 0)?;
    st.push_string(if cfg!(windows) { "\r\n" } else { "\n" })?;
    st.defproperty(-2, "EOL", 0)?;
    Ok(())
}
