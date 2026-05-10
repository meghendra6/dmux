use std::path::PathBuf;

pub fn socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("DEVMUX_SOCKET") {
        return PathBuf::from(path);
    }

    let mut base = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.push(format!("devmux-{}", uid()));
    base.push("default.sock");
    base
}

fn uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }

    unsafe { getuid() }
}
