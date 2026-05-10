use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

impl PtySize {
    pub fn new(cols: u16, rows: u16) -> io::Result<Self> {
        if cols == 0 || rows == 0 {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pty size dimensions must be non-zero",
            ))
        } else {
            Ok(Self { cols, rows })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub session: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub size: PtySize,
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
            size: PtySize { cols: 80, rows: 24 },
        }
    }
}

pub struct PtyProcess {
    pub child_pid: c_int,
    pub master: File,
}

pub fn spawn(spec: &SpawnSpec) -> io::Result<PtyProcess> {
    let mut master: c_int = -1;
    let winsize = WinSize::from(spec.size);

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

pub fn terminate(pid: c_int) -> io::Result<()> {
    signal_process_group(pid, SIGTERM)?;
    let _ = wait_for_exit(pid, Duration::from_millis(1_000))?;

    if !process_group_exists(pid)? {
        return Ok(());
    }

    signal_process_group(pid, SIGKILL)?;
    let _ = wait_for_exit(pid, Duration::from_millis(1_000))?;
    if wait_for_group_exit(pid, Duration::from_millis(1_000))? {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("process group {pid} did not exit after SIGKILL"),
    ))
}

fn signal_process_group(pgid: c_int, signal: c_int) -> io::Result<()> {
    let result = unsafe { kill(-pgid, signal) };
    if result == -1 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(ESRCH) {
            Ok(())
        } else {
            Err(err)
        }
    } else {
        Ok(())
    }
}

fn process_group_exists(pgid: c_int) -> io::Result<bool> {
    let result = unsafe { kill(-pgid, 0) };
    if result == -1 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(ESRCH) {
            Ok(false)
        } else {
            Err(err)
        }
    } else {
        Ok(true)
    }
}

fn wait_for_group_exit(pgid: c_int, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if !process_group_exists(pgid)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_exit(pid: c_int, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut status = 0;
        let result = unsafe { waitpid(pid, &mut status, WNOHANG) };
        if result == pid {
            return Ok(true);
        }
        if result == -1 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ECHILD) {
                return Ok(true);
            }
            return Err(err);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn resize(master: &File, size: PtySize) -> io::Result<()> {
    let winsize = WinSize::from(size);
    let result = unsafe { ioctl(master.as_raw_fd(), TIOCSWINSZ, &winsize) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
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

impl From<PtySize> for WinSize {
    fn from(size: PtySize) -> Self {
        Self {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

#[cfg(target_os = "macos")]
const TIOCSWINSZ: CULong = 0x8008_7467;

#[cfg(target_os = "linux")]
const TIOCSWINSZ: CULong = 0x5414;

type CULong = std::os::raw::c_ulong;

const SIGKILL: c_int = 9;
const SIGTERM: c_int = 15;
const WNOHANG: c_int = 1;
const ECHILD: i32 = 10;
const ESRCH: i32 = 3;

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
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn ioctl(fd: c_int, request: CULong, ...) -> c_int;
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

    #[test]
    fn pty_size_rejects_zero_dimensions() {
        assert!(PtySize::new(0, 24).is_err());
        assert!(PtySize::new(80, 0).is_err());
        assert_eq!(
            PtySize::new(80, 24).unwrap(),
            PtySize { cols: 80, rows: 24 }
        );
    }
}
