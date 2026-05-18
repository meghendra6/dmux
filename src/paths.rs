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

pub fn workspace_registry_path() -> PathBuf {
    if let Ok(path) = std::env::var("DEVMUX_WORKSPACE_REGISTRY") {
        return PathBuf::from(path);
    }

    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(path).join("dmux").join("workspaces.tsv");
    }

    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path)
            .join(".local")
            .join("state")
            .join("dmux")
            .join("workspaces.tsv");
    }

    std::env::temp_dir().join(format!("dmux-workspaces-{}.tsv", uid()))
}

fn uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }

    unsafe { getuid() }
}
