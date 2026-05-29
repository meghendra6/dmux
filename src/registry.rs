use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub name: String,
    pub path: PathBuf,
    pub state: String,
    pub last_seen: u64,
    pub last_window: Option<usize>,
    pub last_pane: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceRegistry {
    pub workspaces: Vec<WorkspaceRecord>,
    pub sessions: Vec<SessionRecord>,
    pub pinned_workspaces: Vec<PathBuf>,
    pub pinned_sessions: Vec<String>,
}

impl WorkspaceRegistry {
    pub fn is_workspace_pinned(&self, path: &Path) -> bool {
        self.workspace_pin_rank(path).is_some()
    }

    pub fn is_session_pinned(&self, name: &str) -> bool {
        self.session_pin_rank(name).is_some()
    }

    /// Position of a workspace within the pin order, or `None` when unpinned.
    /// Pinned rows render in this order; unpinned rows keep their natural order.
    pub fn workspace_pin_rank(&self, path: &Path) -> Option<usize> {
        self.pinned_workspaces
            .iter()
            .position(|pinned| pinned == path)
    }

    pub fn session_pin_rank(&self, name: &str) -> Option<usize> {
        self.pinned_sessions
            .iter()
            .position(|pinned| pinned == name)
    }
}

pub fn load(path: &Path) -> io::Result<WorkspaceRegistry> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(WorkspaceRegistry::default());
        }
        Err(error) => return Err(error),
    };
    parse(&contents)
}

pub fn save(path: &Path, registry: &WorkspaceRegistry) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&temp_path)?;
        file.write_all(render(registry).as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(temp_path, path)
}

pub fn register_workspace(path: &Path, workspace: PathBuf) -> io::Result<()> {
    let mut registry = load(path)?;
    upsert_workspace(&mut registry, normalize_path(workspace));
    save(path, &registry)
}

pub fn record_session(
    path: &Path,
    workspace: PathBuf,
    session: &str,
    state: &str,
) -> io::Result<()> {
    let mut registry = load(path)?;
    let workspace = normalize_path(workspace);
    upsert_workspace(&mut registry, workspace.clone());
    let last_seen = now_seconds();
    if let Some(record) = registry
        .sessions
        .iter_mut()
        .find(|record| record.name == session)
    {
        record.path = workspace;
        record.state = state.to_string();
        record.last_seen = last_seen;
    } else {
        registry.sessions.push(SessionRecord {
            name: session.to_string(),
            path: workspace,
            state: state.to_string(),
            last_seen,
            last_window: None,
            last_pane: None,
        });
    }
    registry
        .sessions
        .sort_by(|left, right| left.name.cmp(&right.name));
    save(path, &registry)
}

pub fn mark_session_stopped(path: &Path, session: &str) -> io::Result<()> {
    let mut registry = load(path)?;
    let mut changed = false;
    let last_seen = now_seconds();
    for record in registry
        .sessions
        .iter_mut()
        .filter(|record| record.name == session)
    {
        record.state = "stopped".to_string();
        record.last_seen = last_seen;
        changed = true;
    }
    if changed {
        save(path, &registry)?;
    }
    Ok(())
}

pub fn toggle_workspace_pin(path: &Path, workspace: PathBuf) -> io::Result<bool> {
    let mut registry = load(path)?;
    let workspace = normalize_path(workspace);
    // Pin order is meaningful (it drives row ordering), so a newly pinned entry
    // appends to the end rather than sorting.
    let pinned = toggle_membership(&mut registry.pinned_workspaces, workspace);
    save(path, &registry)?;
    Ok(pinned)
}

pub fn toggle_session_pin(path: &Path, session: &str) -> io::Result<bool> {
    let mut registry = load(path)?;
    let pinned = toggle_membership(&mut registry.pinned_sessions, session.to_string());
    save(path, &registry)?;
    Ok(pinned)
}

/// Move a pinned workspace one slot earlier (`up`) or later in the pin order.
/// No-op when the workspace is not pinned or is already at the boundary.
pub fn move_workspace_pin(path: &Path, workspace: PathBuf, up: bool) -> io::Result<()> {
    let mut registry = load(path)?;
    let workspace = normalize_path(workspace);
    if move_in_order(&mut registry.pinned_workspaces, &workspace, up) {
        save(path, &registry)?;
    }
    Ok(())
}

pub fn move_session_pin(path: &Path, session: &str, up: bool) -> io::Result<()> {
    let mut registry = load(path)?;
    let session = session.to_string();
    if move_in_order(&mut registry.pinned_sessions, &session, up) {
        save(path, &registry)?;
    }
    Ok(())
}

fn toggle_membership<T: PartialEq>(items: &mut Vec<T>, value: T) -> bool {
    if let Some(position) = items.iter().position(|item| *item == value) {
        items.remove(position);
        false
    } else {
        items.push(value);
        true
    }
}

/// Swap `value` with its neighbour in the requested direction. Returns whether
/// the order changed.
fn move_in_order<T: PartialEq>(items: &mut [T], value: &T, up: bool) -> bool {
    let Some(index) = items.iter().position(|item| item == value) else {
        return false;
    };
    let target = if up {
        index.checked_sub(1)
    } else {
        (index + 1 < items.len()).then_some(index + 1)
    };
    match target {
        Some(target) => {
            items.swap(index, target);
            true
        }
        None => false,
    }
}

fn upsert_workspace(registry: &mut WorkspaceRegistry, path: PathBuf) {
    if !registry.workspaces.iter().any(|record| record.path == path) {
        registry.workspaces.push(WorkspaceRecord { path });
        registry
            .workspaces
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
}

fn normalize_path(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn parse(contents: &str) -> io::Result<WorkspaceRegistry> {
    let mut registry = WorkspaceRegistry::default();
    for (line_index, line) in contents.lines().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["workspace", path] => registry.workspaces.push(WorkspaceRecord {
                path: decode_path(path).map_err(|message| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("line {}: {message}", line_index + 1),
                    )
                })?,
            }),
            ["session", rest @ ..] => {
                registry
                    .sessions
                    .push(parse_session_record(rest, line_index + 1)?);
            }
            ["pin-workspace", path] => registry
                .pinned_workspaces
                .push(decode_path_field(path, line_index + 1)?),
            ["pin-session", name] => registry
                .pinned_sessions
                .push(decode_text_field(name, line_index + 1)?),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line {}: invalid registry record", line_index + 1),
                ));
            }
        }
    }
    registry
        .workspaces
        .sort_by(|left, right| left.path.cmp(&right.path));
    registry
        .sessions
        .sort_by(|left, right| left.name.cmp(&right.name));
    // Pin order is preserved as written; only drop duplicates, keeping the
    // first occurrence so the order stays stable.
    dedup_preserving_order(&mut registry.pinned_workspaces);
    dedup_preserving_order(&mut registry.pinned_sessions);
    Ok(registry)
}

fn dedup_preserving_order<T: PartialEq + Clone>(items: &mut Vec<T>) {
    let mut result: Vec<T> = Vec::with_capacity(items.len());
    for item in items.iter() {
        if !result.contains(item) {
            result.push(item.clone());
        }
    }
    *items = result;
}

fn parse_session_record(fields: &[&str], line_number: usize) -> io::Result<SessionRecord> {
    let (name, path, state, last_seen, last_window, last_pane) = match fields {
        [name, path, state, last_seen] => (name, path, state, last_seen, None, None),
        [name, path, state, last_seen, last_window, last_pane] => (
            name,
            path,
            state,
            last_seen,
            Some(last_window),
            Some(last_pane),
        ),
        _ => {
            return Err(invalid_registry_data(
                line_number,
                "invalid session record field count",
            ));
        }
    };
    Ok(SessionRecord {
        name: decode_text_field(name, line_number)?,
        path: decode_path_field(path, line_number)?,
        state: (*state).to_string(),
        last_seen: parse_u64_field(last_seen, line_number, "last_seen")?,
        last_window: parse_optional_usize_field(last_window, line_number, "last_window")?,
        last_pane: parse_optional_usize_field(last_pane, line_number, "last_pane")?,
    })
}

fn decode_text_field(value: &str, line_number: usize) -> io::Result<String> {
    decode_text(value).map_err(|message| invalid_registry_data(line_number, &message))
}

fn decode_path_field(value: &str, line_number: usize) -> io::Result<PathBuf> {
    decode_path(value).map_err(|message| invalid_registry_data(line_number, &message))
}

fn parse_u64_field(value: &str, line_number: usize, field: &str) -> io::Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| invalid_registry_data(line_number, &format!("invalid {field}")))
}

fn parse_optional_usize_field(
    value: Option<&&str>,
    line_number: usize,
    field: &str,
) -> io::Result<Option<usize>> {
    match value {
        None | Some(&"") => Ok(None),
        Some(value) => value
            .parse::<usize>()
            .map(Some)
            .map_err(|_| invalid_registry_data(line_number, &format!("invalid {field}"))),
    }
}

fn invalid_registry_data(line_number: usize, message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("line {line_number}: {message}"),
    )
}

fn render(registry: &WorkspaceRegistry) -> String {
    let mut lines = vec!["# dmux workspace registry v1".to_string()];
    for workspace in &registry.workspaces {
        lines.push(format!("workspace\t{}", encode_path(&workspace.path)));
    }
    for session in &registry.sessions {
        lines.push(format!(
            "session\t{}\t{}\t{}\t{}\t{}\t{}",
            encode_text(&session.name),
            encode_path(&session.path),
            session.state,
            session.last_seen,
            render_optional_usize(session.last_window),
            render_optional_usize(session.last_pane)
        ));
    }
    for path in &registry.pinned_workspaces {
        lines.push(format!("pin-workspace\t{}", encode_path(path)));
    }
    for name in &registry.pinned_sessions {
        lines.push(format!("pin-session\t{}", encode_text(name)));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn render_optional_usize(value: Option<usize>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn encode_text(value: &str) -> String {
    encode_hex(value.as_bytes())
}

fn decode_text(value: &str) -> Result<String, String> {
    String::from_utf8(decode_hex(value)?).map_err(|_| "non-utf8 text".to_string())
}

fn encode_path(path: &Path) -> String {
    encode_hex(path.as_os_str().as_bytes())
}

fn decode_path(value: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(OsString::from_vec(decode_hex(value)?)))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("invalid hex length".to_string());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let chars = value.as_bytes();
    let mut i = 0;
    while i < chars.len() {
        let high = decode_hex_nibble(chars[i])?;
        let low = decode_hex_nibble(chars[i + 1])?;
        bytes.push((high << 4) | low);
        i += 2;
    }
    Ok(bytes)
}

fn decode_hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trips_paths_and_sessions() {
        let registry = WorkspaceRegistry {
            workspaces: vec![WorkspaceRecord {
                path: PathBuf::from("/tmp/project"),
            }],
            sessions: vec![SessionRecord {
                name: "dev".to_string(),
                path: PathBuf::from("/tmp/project"),
                state: "detached".to_string(),
                last_seen: 42,
                last_window: None,
                last_pane: None,
            }],
            ..Default::default()
        };

        assert_eq!(parse(&render(&registry)).unwrap(), registry);
    }

    #[test]
    fn registry_round_trips_optional_session_metadata() {
        let registry = WorkspaceRegistry {
            workspaces: vec![WorkspaceRecord {
                path: PathBuf::from("/tmp/project"),
            }],
            sessions: vec![SessionRecord {
                name: "dev".to_string(),
                path: PathBuf::from("/tmp/project"),
                state: "detached".to_string(),
                last_seen: 42,
                last_window: Some(1),
                last_pane: Some(2),
            }],
            ..Default::default()
        };

        assert_eq!(parse(&render(&registry)).unwrap(), registry);
    }

    #[test]
    fn registry_round_trips_partial_optional_session_metadata() {
        let registry = WorkspaceRegistry {
            workspaces: vec![WorkspaceRecord {
                path: PathBuf::from("/tmp/project"),
            }],
            sessions: vec![SessionRecord {
                name: "dev".to_string(),
                path: PathBuf::from("/tmp/project"),
                state: "detached".to_string(),
                last_seen: 42,
                last_window: Some(1),
                last_pane: None,
            }],
            ..Default::default()
        };

        assert_eq!(parse(&render(&registry)).unwrap(), registry);
    }

    #[test]
    fn registry_round_trips_pins() {
        let registry = WorkspaceRegistry {
            workspaces: vec![WorkspaceRecord {
                path: PathBuf::from("/tmp/project"),
            }],
            sessions: Vec::new(),
            pinned_workspaces: vec![PathBuf::from("/tmp/project")],
            pinned_sessions: vec!["dev".to_string()],
        };

        assert_eq!(parse(&render(&registry)).unwrap(), registry);
    }

    #[test]
    fn toggle_pins_persist_and_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "dmux-pin-test-{}-{:?}.tsv",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);

        assert!(toggle_workspace_pin(&path, PathBuf::from("/tmp/project")).unwrap());
        assert!(toggle_session_pin(&path, "dev").unwrap());

        let loaded = load(&path).unwrap();
        assert!(loaded.is_workspace_pinned(Path::new("/tmp/project")));
        assert!(loaded.is_session_pinned("dev"));

        assert!(!toggle_workspace_pin(&path, PathBuf::from("/tmp/project")).unwrap());
        let loaded = load(&path).unwrap();
        assert!(!loaded.is_workspace_pinned(Path::new("/tmp/project")));
        assert!(loaded.is_session_pinned("dev"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pins_preserve_insertion_order_and_expose_rank() {
        let registry = parse(&render(&WorkspaceRegistry {
            pinned_sessions: vec!["zeta".to_string(), "alpha".to_string()],
            ..Default::default()
        }))
        .unwrap();

        // Order is preserved (not alphabetised) and rank reflects it.
        assert_eq!(registry.pinned_sessions, vec!["zeta", "alpha"]);
        assert_eq!(registry.session_pin_rank("zeta"), Some(0));
        assert_eq!(registry.session_pin_rank("alpha"), Some(1));
        assert_eq!(registry.session_pin_rank("missing"), None);
    }

    #[test]
    fn move_session_pin_swaps_with_neighbour_and_clamps() {
        let path = std::env::temp_dir().join(format!(
            "dmux-pin-move-{}-{:?}.tsv",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);

        toggle_session_pin(&path, "first").unwrap();
        toggle_session_pin(&path, "second").unwrap();
        assert_eq!(
            load(&path).unwrap().pinned_sessions,
            vec!["first", "second"]
        );

        // Move "second" up past "first".
        move_session_pin(&path, "second", true).unwrap();
        assert_eq!(
            load(&path).unwrap().pinned_sessions,
            vec!["second", "first"]
        );

        // Moving the top entry up is a no-op (clamped at the boundary).
        move_session_pin(&path, "second", true).unwrap();
        assert_eq!(
            load(&path).unwrap().pinned_sessions,
            vec!["second", "first"]
        );

        // Unpinned entries cannot be moved.
        move_session_pin(&path, "missing", true).unwrap();
        assert_eq!(
            load(&path).unwrap().pinned_sessions,
            vec!["second", "first"]
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn registry_parses_legacy_session_metadata_as_empty() {
        let contents = format!(
            "session\t{}\t{}\tdetached\t42\n",
            encode_text("dev"),
            encode_path(Path::new("/tmp/project"))
        );

        assert_eq!(
            parse(&contents).unwrap(),
            WorkspaceRegistry {
                workspaces: Vec::new(),
                sessions: vec![SessionRecord {
                    name: "dev".to_string(),
                    path: PathBuf::from("/tmp/project"),
                    state: "detached".to_string(),
                    last_seen: 42,
                    last_window: None,
                    last_pane: None,
                }],
                ..Default::default()
            }
        );
    }
}
