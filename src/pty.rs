use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::FromRawFd;
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub session: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
}

impl SpawnSpec {
    pub fn new(session: String, command: Vec<String>, cwd: PathBuf) -> Self {
        let command = if command.is_empty() {
            vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())]
        } else {
            command
        };

        Self {
            session,
            command,
            cwd,
        }
    }
}

pub struct PtyProcess {
    pub child_pid: c_int,
    pub master: File,
}

pub fn spawn(spec: &SpawnSpec) -> io::Result<PtyProcess> {
    let mut master: c_int = -1;
    let winsize = WinSize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let pid = unsafe {
        forkpty(
            &mut master,
            std::ptr::null_mut(),
            std::ptr::null(),
            &winsize,
        )
    };

    if pid < 0 {
        return Err(io::Error::last_os_error());
    }

    if pid == 0 {
        child_exec(spec);
    }

    let master = unsafe { File::from_raw_fd(master) };
    Ok(PtyProcess {
        child_pid: pid,
        master,
    })
}

pub fn terminate(pid: c_int) {
    const SIGTERM: c_int = 15;
    unsafe {
        kill(pid, SIGTERM);
    }
}

fn child_exec(spec: &SpawnSpec) -> ! {
    let _ = std::env::set_current_dir(&spec.cwd);

    let args = spec
        .command
        .iter()
        .map(|arg| CString::new(arg.as_str()).unwrap_or_else(|_| CString::new("").unwrap()))
        .collect::<Vec<_>>();

    if args.is_empty() {
        unsafe {
            _exit(127);
        }
    }

    let mut argv = args
        .iter()
        .map(|arg| arg.as_ptr())
        .collect::<Vec<*const c_char>>();
    argv.push(std::ptr::null());

    unsafe {
        execvp(args[0].as_ptr(), argv.as_ptr());
        _exit(127);
    }
}

#[repr(C)]
struct WinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[cfg_attr(target_os = "linux", link(name = "util"))]
unsafe extern "C" {
    fn forkpty(
        amaster: *mut c_int,
        name: *mut c_char,
        termp: *const c_void,
        winp: *const WinSize,
    ) -> c_int;
    fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
    fn _exit(status: c_int) -> !;
    fn kill(pid: c_int, sig: c_int) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_defaults_to_user_shell_when_command_is_empty() {
        let spec = SpawnSpec::new(
            "dev".to_string(),
            Vec::new(),
            std::path::PathBuf::from("/tmp"),
        );
        assert!(!spec.command.is_empty());
    }
}
