use crate::ids::{PaneId, TabId};
use crate::layout::{LayoutNode, PaneRegion, layout_regions_for_size, split_extent_weighted};
use crate::protocol::{
    self, BufferSelection, CaptureMode, KeyBinding, LayoutPreset, OptionEntry, PaneDirection,
    PaneResizeDirection, PaneSelectTarget, PaneTarget, Request, SplitDirection, Target,
    WindowTarget,
};
use crate::pty::{self, PtySize, SpawnSpec};
use crate::term::{TerminalChanges, TerminalState};
use crate::terminal_query::PtyOutputFilter;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use unicode_width::UnicodeWidthChar;

const MAX_HISTORY_BYTES: usize = 1024 * 1024;
const MAX_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_BUFFERS: usize = 50;
const MAX_CONTROL_LINE_BYTES: usize = protocol::MAX_SAVE_BUFFER_TEXT_BYTES * 2 + 4096;
const ATTACH_REDRAW_EVENT: &[u8] = b"REDRAW\n";
const ATTACH_RENDER_STATUS_FORMAT: &str = "#{session.name} #{window.list} pane #{pane.index} clients #{client.count} buffers #{buffer.count} | #{status.help}";
const ATTACH_RENDER_RESPONSE: &[u8] = b"OK\tRENDER_OUTPUT_META\n";
const ATTACH_RENDER_DEBOUNCE_INTERVAL: Duration = Duration::from_millis(16);
const SYNCHRONIZED_OUTPUT_REDRAW_TIMEOUT: Duration = Duration::from_millis(100);
const TRANSIENT_MESSAGE_DURATION: Duration = Duration::from_millis(1500);
const RAW_ATTACH_RECONNECT_ALIAS_GRACE: Duration = Duration::from_millis(500);
const CURSOR_HOME: &[u8] = b"\x1b[H";
const CLEAR_LINE: &[u8] = b"\x1b[2K";
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";
const HIDE_CURSOR: &[u8] = b"\x1b[?25l";

type TrackedStreamClients = Arc<Mutex<Vec<TrackedStream>>>;
type AttachEventClients = TrackedStreamClients;
type AttachLifetimeStreams = TrackedStreamClients;
type AttachRenderClients = TrackedStreamClients;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientType {
    Raw,
    Event,
    Render,
}

impl ClientType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Event => "event",
            Self::Render => "render",
        }
    }
}

struct TrackedStream {
    id: usize,
    client_id: usize,
    client_type: ClientType,
    stream: UnixStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClientDescription {
    id: usize,
    session: String,
    client_type: ClientType,
    attached: bool,
    width: u16,
    height: u16,
}

struct StreamRegistration {
    clients: TrackedStreamClients,
    id: usize,
}

impl Drop for StreamRegistration {
    fn drop(&mut self) {
        self.clients
            .lock()
            .unwrap()
            .retain(|client| client.id != self.id);
    }
}

pub fn run(socket_path: PathBuf) -> io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    let state = Arc::new(ServerState {
        sessions: Mutex::new(HashMap::new()),
        session_aliases: Mutex::new(HashMap::new()),
        buffers: Mutex::new(BufferStore::new()),
        key_bindings: Mutex::new(default_key_binding_map()),
        options: Mutex::new(default_option_map()),
        socket_path,
        next_session_created_id: AtomicUsize::new(1),
        next_client_id: AtomicUsize::new(1),
    });

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let _ = handle_connection(state, stream);
                });
            }
            Err(err) => return Err(err),
        }
    }

    Ok(())
}

struct ServerState {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    session_aliases: Mutex<HashMap<String, String>>,
    buffers: Mutex<BufferStore>,
    key_bindings: Mutex<HashMap<String, String>>,
    options: Mutex<HashMap<String, String>>,
    socket_path: PathBuf,
    next_session_created_id: AtomicUsize,
    next_client_id: AtomicUsize,
}

fn default_key_binding_map() -> HashMap<String, String> {
    crate::config::default_key_bindings()
        .into_iter()
        .map(|binding| (binding.key, binding.command))
        .collect()
}

fn default_option_map() -> HashMap<String, String> {
    crate::config::default_options()
        .into_iter()
        .map(|option| (option.name, option.value))
        .collect()
}

impl ServerState {
    fn buffer_status_summary(&self) -> BufferStatusSummary {
        self.buffers.lock().unwrap().status_summary()
    }

    fn publish_buffer_status(&self) {
        let summary = self.buffer_status_summary();
        let sessions = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            session.set_buffer_summary(summary.clone());
            session.notify_attach_redraw_immediate();
        }
    }
}

struct Buffer {
    name: String,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BufferDescription {
    index: usize,
    name: String,
    bytes: usize,
    lines: usize,
    latest: bool,
    preview: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BufferStatusSummary {
    count: usize,
    latest: Option<BufferDescription>,
}

struct BufferStore {
    buffers: Vec<Buffer>,
    next_auto: usize,
}

impl BufferStore {
    fn new() -> Self {
        Self {
            buffers: Vec::new(),
            next_auto: 0,
        }
    }

    fn save(&mut self, name: Option<&str>, text: String) -> Result<String, String> {
        if text.len() > MAX_BUFFER_BYTES {
            return Err("buffer exceeds maximum size".to_string());
        }

        let name = match name {
            Some(name) if name.is_empty() => return Err("buffer name cannot be empty".to_string()),
            Some(name) => name.to_string(),
            None => self.next_name(),
        };

        if let Some(index) = self.buffers.iter().position(|buffer| buffer.name == name) {
            self.buffers.remove(index);
        }

        self.buffers.push(Buffer {
            name: name.clone(),
            text,
        });
        while self.buffers.len() > MAX_BUFFERS {
            self.buffers.remove(0);
        }
        Ok(name)
    }

    fn list(&self) -> Vec<BufferDescription> {
        let latest_index = self.buffers.len().checked_sub(1);
        self.buffers
            .iter()
            .enumerate()
            .map(|(index, buffer)| BufferDescription {
                index,
                name: buffer.name.clone(),
                bytes: buffer.text.len(),
                lines: buffer_line_count(&buffer.text),
                latest: latest_index == Some(index),
                preview: buffer_preview(&buffer.text),
            })
            .collect()
    }

    fn status_summary(&self) -> BufferStatusSummary {
        BufferStatusSummary {
            count: self.buffers.len(),
            latest: self.list().pop(),
        }
    }

    fn resolve(&self, name: Option<&str>) -> Option<&Buffer> {
        match name {
            Some(name) => self.buffers.iter().find(|buffer| buffer.name == name),
            None => self.buffers.last(),
        }
    }

    fn delete(&mut self, name: &str) -> bool {
        let Some(index) = self.buffers.iter().position(|buffer| buffer.name == name) else {
            return false;
        };
        self.buffers.remove(index);
        true
    }

    fn next_name(&mut self) -> String {
        loop {
            let name = format!("buffer-{}", self.next_auto);
            self.next_auto += 1;
            if !self.buffers.iter().any(|buffer| buffer.name == name) {
                return name;
            }
        }
    }
}

fn buffer_preview(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .replace('\t', " ")
        .chars()
        .take(40)
        .collect()
}

fn buffer_line_count(text: &str) -> usize {
    text.lines().count()
}

fn format_copy_mode_lines(
    text: &str,
    search: Option<&str>,
    match_index: Option<usize>,
) -> Result<String, String> {
    let mut output = String::new();
    let mut matched = 0;
    for (index, line) in text.lines().enumerate() {
        if search.is_none_or(|needle| line.contains(needle)) {
            matched += 1;
            if match_index.is_some_and(|wanted| wanted != matched) {
                continue;
            }
            output.push_str(&(index + 1).to_string());
            output.push('\t');
            output.push_str(line);
            output.push('\n');
        }
    }
    if search.is_some() && matched == 0 {
        return Err("missing match".to_string());
    }
    if let Some(match_index) = match_index {
        if match_index == 0 {
            return Err("invalid match index".to_string());
        }
        if matched < match_index {
            return Err("missing match".to_string());
        }
    }
    Ok(output)
}

fn select_buffer_text(text: &str, selection: &BufferSelection) -> Result<String, String> {
    match selection {
        BufferSelection::All => Ok(text.to_string()),
        BufferSelection::LineRange { start, end } => select_line_range(text, *start, *end),
        BufferSelection::Search {
            needle,
            match_index,
        } => select_search_match(text, needle, *match_index),
    }
}

fn select_line_range(text: &str, start: isize, end: isize) -> Result<String, String> {
    if start == 0 || end == 0 {
        return Err("invalid line range".to_string());
    }

    let lines = text.lines().collect::<Vec<_>>();
    let start = resolve_line_offset(start, lines.len())?;
    let end = resolve_line_offset(end, lines.len())?;
    if start > end {
        return Err("invalid line range".to_string());
    }
    if end > lines.len() {
        return Err("missing line".to_string());
    }

    Ok(join_selected_lines(&lines[start - 1..end]))
}

fn resolve_line_offset(value: isize, len: usize) -> Result<usize, String> {
    if value > 0 {
        return usize::try_from(value).map_err(|_| "invalid line range".to_string());
    }
    let len = isize::try_from(len).map_err(|_| "invalid line range".to_string())?;
    let resolved = len + value + 1;
    if resolved <= 0 {
        return Err("missing line".to_string());
    }
    usize::try_from(resolved).map_err(|_| "invalid line range".to_string())
}

fn select_search_match(text: &str, needle: &str, match_index: usize) -> Result<String, String> {
    if needle.is_empty() {
        return Err("search text cannot be empty".to_string());
    }
    if match_index == 0 {
        return Err("invalid match index".to_string());
    }

    let Some(line) = text
        .lines()
        .filter(|line| line.contains(needle))
        .nth(match_index - 1)
    else {
        return Err("missing match".to_string());
    };

    Ok(join_selected_lines(&[line]))
}

fn join_selected_lines(lines: &[&str]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }
}

struct Session {
    name: Mutex<String>,
    created_at: usize,
    windows: Mutex<WindowSet>,
    buffer_summary: Mutex<BufferStatusSummary>,
    transient_message: Mutex<Option<String>>,
    prefix_key: Mutex<String>,
    status_hints: Mutex<bool>,
    current_size: Mutex<PtySize>,
    next_pane_id: AtomicUsize,
    next_tab_id: AtomicUsize,
    attach_events: AttachEventClients,
    attach_streams: AttachLifetimeStreams,
    attach_render_clients: AttachRenderClients,
    attach_render_pending: AtomicBool,
    attach_render_immediate_pending: AtomicBool,
    attach_render_epoch: AtomicU64,
    attach_render_immediate_epoch: AtomicU64,
    attach_render_rendered_epoch: AtomicU64,
    transient_message_epoch: AtomicU64,
    raw_attach_layout_epoch: AtomicU64,
    next_attach_event_id: AtomicUsize,
    next_attach_stream_id: AtomicUsize,
    next_attach_render_id: AtomicUsize,
}

impl Session {
    fn new(
        name: String,
        created_at: usize,
        pane: Arc<Pane>,
        attach_events: AttachEventClients,
    ) -> Self {
        let next_pane_id = pane.id.as_usize() + 1;
        let current_size = *pane.size.lock().unwrap();
        Self {
            name: Mutex::new(name),
            created_at,
            windows: Mutex::new(WindowSet::new(Window::new(
                TabId::new(0),
                "0".to_string(),
                pane,
            ))),
            buffer_summary: Mutex::new(BufferStatusSummary::default()),
            transient_message: Mutex::new(None),
            prefix_key: Mutex::new(crate::config::DEFAULT_PREFIX_KEY.to_string()),
            status_hints: Mutex::new(true),
            current_size: Mutex::new(current_size),
            next_pane_id: AtomicUsize::new(next_pane_id),
            next_tab_id: AtomicUsize::new(1),
            attach_events,
            attach_streams: Arc::new(Mutex::new(Vec::new())),
            attach_render_clients: Arc::new(Mutex::new(Vec::new())),
            attach_render_pending: AtomicBool::new(false),
            attach_render_immediate_pending: AtomicBool::new(false),
            attach_render_epoch: AtomicU64::new(0),
            attach_render_immediate_epoch: AtomicU64::new(0),
            attach_render_rendered_epoch: AtomicU64::new(0),
            transient_message_epoch: AtomicU64::new(0),
            raw_attach_layout_epoch: AtomicU64::new(0),
            next_attach_event_id: AtomicUsize::new(0),
            next_attach_stream_id: AtomicUsize::new(0),
            next_attach_render_id: AtomicUsize::new(0),
        }
    }

    fn name(&self) -> String {
        self.name.lock().unwrap().clone()
    }

    fn rename(&self, new_name: String) {
        *self.name.lock().unwrap() = new_name;
    }

    fn set_buffer_summary(&self, summary: BufferStatusSummary) {
        *self.buffer_summary.lock().unwrap() = summary;
    }

    fn set_runtime_options(&self, prefix_key: String, status_hints: bool) {
        *self.prefix_key.lock().unwrap() = prefix_key;
        *self.status_hints.lock().unwrap() = status_hints;
    }

    fn set_transient_message(self: &Arc<Self>, message: String) {
        let epoch = {
            let mut current = self.transient_message.lock().unwrap();
            let epoch = self.transient_message_epoch.fetch_add(1, Ordering::SeqCst) + 1;
            *current = Some(message);
            epoch
        };
        self.notify_attach_redraw_immediate();

        let session = Arc::downgrade(self);
        std::thread::spawn(move || {
            std::thread::sleep(TRANSIENT_MESSAGE_DURATION);
            let Some(session) = session.upgrade() else {
                return;
            };
            if session.clear_transient_message_if_epoch(epoch) {
                session.notify_attach_redraw_immediate();
            }
        });
    }

    fn clear_transient_message_if_epoch(&self, epoch: u64) -> bool {
        let mut message = self.transient_message.lock().unwrap();
        if self.transient_message_epoch.load(Ordering::SeqCst) != epoch {
            return false;
        }
        *message = None;
        true
    }

    fn next_pane_id(&self) -> PaneId {
        PaneId::new(self.next_pane_id.fetch_add(1, Ordering::SeqCst))
    }

    fn next_tab_id(&self) -> TabId {
        TabId::new(self.next_tab_id.fetch_add(1, Ordering::SeqCst))
    }

    fn active_pane(&self) -> Option<Arc<Pane>> {
        self.windows.lock().unwrap().active_pane()
    }

    fn active_live_pane(&self) -> Option<Arc<Pane>> {
        self.windows.lock().unwrap().active_live_pane()
    }

    fn is_active_pane(&self, pane: &Arc<Pane>) -> bool {
        self.windows
            .lock()
            .unwrap()
            .active_pane()
            .is_some_and(|active| Arc::ptr_eq(&active, pane))
    }

    fn clear_active_pane_alerts(&self) {
        if let Some(pane) = self.windows.lock().unwrap().active_pane() {
            pane.clear_alerts();
        }
    }

    fn target_pane(&self, target: &Target) -> Result<Arc<Pane>, String> {
        self.windows
            .lock()
            .unwrap()
            .pane(target.window.clone(), target.pane.clone())
    }

    fn target_live_pane(&self, target: &Target) -> Result<Arc<Pane>, String> {
        self.windows
            .lock()
            .unwrap()
            .live_pane(target.window.clone(), target.pane.clone())
    }

    fn add_pane(
        &self,
        target: &Target,
        direction: SplitDirection,
        pane: Arc<Pane>,
    ) -> Result<(), String> {
        self.windows.lock().unwrap().add_pane(
            target.window.clone(),
            target.pane.clone(),
            direction,
            pane,
        )
    }

    fn next_split_pane_size(
        &self,
        target: &Target,
        direction: SplitDirection,
    ) -> Result<PtySize, String> {
        self.windows.lock().unwrap().next_split_pane_size(
            target.window.clone(),
            target.pane.clone(),
            direction,
        )
    }

    fn target_is_active_window(&self, target: &Target) -> Result<bool, String> {
        self.windows
            .lock()
            .unwrap()
            .target_is_active_window(target.window.clone())
    }

    fn window_is_active(&self, window: WindowTarget) -> Result<bool, String> {
        self.windows.lock().unwrap().target_is_active_window(window)
    }

    fn resize_visible_panes(&self, size: PtySize) -> io::Result<bool> {
        *self.current_size.lock().unwrap() = size;
        let pane_resizes = self.windows.lock().unwrap().resize_visible_panes(size);
        resize_panes(pane_resizes)
    }

    fn resize_current_visible_panes(&self) -> io::Result<bool> {
        let size = *self.current_size.lock().unwrap();
        let pane_resizes = self.windows.lock().unwrap().resize_visible_panes(size);
        resize_panes(pane_resizes)
    }

    fn visible_pane_resizes_for_window(
        &self,
        window: WindowTarget,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let size = *self.current_size.lock().unwrap();
        self.windows
            .lock()
            .unwrap()
            .resize_visible_panes_for_window(window, size)
    }

    fn resize_pane(
        &self,
        target: &Target,
        direction: PaneResizeDirection,
        amount: usize,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        self.windows.lock().unwrap().resize_pane(
            target.window.clone(),
            target.pane.clone(),
            direction,
            amount,
        )
    }

    fn select_layout(
        &self,
        window: WindowTarget,
        preset: LayoutPreset,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let size = *self.current_size.lock().unwrap();
        self.windows
            .lock()
            .unwrap()
            .select_layout(window, preset, size)
    }

    fn active_window_size(&self) -> Option<PtySize> {
        self.windows.lock().unwrap().active_window()?;
        Some(*self.current_size.lock().unwrap())
    }

    fn select_pane(&self, window: WindowTarget, target: PaneSelectTarget) -> Result<bool, String> {
        let changed = self.windows.lock().unwrap().select_pane(window, target)?;
        self.clear_active_pane_alerts();
        Ok(changed)
    }

    fn kill_pane(&self, target: &Target) -> Result<Arc<Pane>, String> {
        self.windows
            .lock()
            .unwrap()
            .kill_pane(target.window.clone(), target.pane.clone())
    }

    fn swap_panes(&self, source: &Target, destination: &Target) -> Result<(), String> {
        self.windows.lock().unwrap().swap_panes(
            source.window.clone(),
            source.pane.clone(),
            destination.window.clone(),
            destination.pane.clone(),
        )
    }

    fn move_pane(
        &self,
        source: &Target,
        destination: &Target,
        direction: SplitDirection,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let size = *self.current_size.lock().unwrap();
        self.windows.lock().unwrap().move_pane(
            source.window.clone(),
            source.pane.clone(),
            destination.window.clone(),
            destination.pane.clone(),
            direction,
            size,
        )
    }

    fn break_pane(&self, target: &Target) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let id = self.next_tab_id();
        let size = *self.current_size.lock().unwrap();
        self.windows.lock().unwrap().break_pane(
            target.window.clone(),
            target.pane.clone(),
            id,
            size,
        )
    }

    fn respawn_pane(
        self: &Arc<Self>,
        target: &Target,
        command: Vec<String>,
        force: bool,
        cwd: Option<PathBuf>,
    ) -> Result<Option<i32>, String> {
        let session_name = self.name();
        let attach_events = self.attach_event_clients();
        let reconnect_live_raw =
            self.target_is_active_window(target)? && !has_attach_pane_snapshot(self);
        let (id, size, old_pid, existing_cwd) = self.windows.lock().unwrap().prepare_respawn_pane(
            target.window.clone(),
            target.pane.clone(),
            force,
        )?;
        let cwd = cwd.unwrap_or(existing_cwd);
        let pane = spawn_pane(
            id,
            session_name,
            command,
            cwd,
            size,
            attach_events,
            Some(Arc::downgrade(self)),
        )
        .map_err(|err| err.to_string())?;
        if let Err(message) =
            self.windows
                .lock()
                .unwrap()
                .replace_pane(target.window.clone(), id, Arc::clone(&pane))
        {
            terminate_pane_if_running_async(&pane);
            return Err(message);
        }
        pane.set_session(self);
        if !pane.is_running() {
            self.mark_pane_exited(&pane);
        }
        let pane_resizes = match self.visible_pane_resizes_for_window(target.window.clone()) {
            Ok(pane_resizes) => pane_resizes,
            Err(message) => {
                if let Some(pid) = old_pid {
                    terminate_pane_async(pid);
                }
                return Err(message);
            }
        };
        if let Err(error) = resize_panes(pane_resizes) {
            if let Some(pid) = old_pid {
                terminate_pane_async(pid);
            }
            return Err(error.to_string());
        }
        self.notify_attach_redraw_immediate();
        if reconnect_live_raw {
            self.reconnect_raw_attach_streams();
        }
        Ok(old_pid)
    }

    fn mark_pane_exited(self: &Arc<Self>, pane: &Arc<Pane>) {
        let changed = self.windows.lock().unwrap().mark_pane_exited(pane);
        if changed {
            self.notify_attach_redraw_immediate();
            self.reconnect_raw_attach_streams();
        }
    }

    #[allow(dead_code)]
    fn pane_descriptions(&self) -> Vec<PaneDescription> {
        self.windows.lock().unwrap().pane_descriptions()
    }

    fn pane_descriptions_for_window(
        &self,
        target: WindowTarget,
    ) -> Result<Vec<PaneDescription>, String> {
        self.windows
            .lock()
            .unwrap()
            .pane_descriptions_for_window(target)
    }

    fn panes(&self) -> Vec<Arc<Pane>> {
        self.windows.lock().unwrap().panes()
    }

    fn add_window(&self, pane: Arc<Pane>) {
        let id = self.next_tab_id();
        self.windows
            .lock()
            .unwrap()
            .add(Window::new(id, id.as_usize().to_string(), pane));
    }

    fn select_window(&self, target: WindowTarget) -> Result<(), String> {
        self.windows.lock().unwrap().select(target)?;
        self.clear_active_pane_alerts();
        Ok(())
    }

    fn rename_window(&self, target: WindowTarget, name: String) -> Result<(), String> {
        self.windows.lock().unwrap().rename_window(target, name)
    }

    fn select_next_window(&self) -> Result<(), String> {
        self.windows.lock().unwrap().select_next()?;
        self.clear_active_pane_alerts();
        Ok(())
    }

    fn select_previous_window(&self) -> Result<(), String> {
        self.windows.lock().unwrap().select_previous()?;
        self.clear_active_pane_alerts();
        Ok(())
    }

    fn window_descriptions(&self) -> Vec<WindowDescription> {
        self.windows.lock().unwrap().window_descriptions()
    }

    fn kill_window(&self, target: WindowTarget) -> Result<(), String> {
        self.windows.lock().unwrap().kill_window(target)
    }

    fn zoom_pane(&self, target: &Target) -> Result<bool, String> {
        let changed = self
            .windows
            .lock()
            .unwrap()
            .zoom_pane(target.window.clone(), target.pane.clone())?;
        self.clear_active_pane_alerts();
        Ok(changed)
    }

    fn status_context(&self) -> Option<StatusContext> {
        let mut context = self.windows.lock().unwrap().status_context(&self.name())?;
        let attached_count = self.attached_count();
        context.session_attached_count = attached_count;
        context.session_created_at = self.created_at;
        context.buffer_summary = self.buffer_summary.lock().unwrap().clone();
        context.transient_message = self.transient_message.lock().unwrap().clone();
        context.prefix_key = self.prefix_key.lock().unwrap().clone();
        context.status_hints = *self.status_hints.lock().unwrap();
        Some(context)
    }

    fn window_count(&self) -> usize {
        self.windows.lock().unwrap().window_count()
    }

    fn attached_count(&self) -> usize {
        self.attach_streams.lock().unwrap().len()
    }

    fn client_descriptions(&self) -> Vec<ClientDescription> {
        let size = *self.current_size.lock().unwrap();
        let name = self.name();
        let mut clients = Vec::new();
        collect_client_descriptions(&mut clients, &self.attach_streams, &name, size);
        collect_client_descriptions(&mut clients, &self.attach_events, &name, size);
        collect_client_descriptions(&mut clients, &self.attach_render_clients, &name, size);
        clients.sort_by_key(|client| client.id);
        clients
    }

    fn detach_client(&self, client_id: usize) -> bool {
        detach_tracked_client(&self.attach_streams, client_id)
            || detach_tracked_client(&self.attach_events, client_id)
            || detach_tracked_client(&self.attach_render_clients, client_id)
    }

    fn detach_all_clients(&self) -> usize {
        let count = detach_all_tracked_clients(&self.attach_streams)
            + detach_all_tracked_clients(&self.attach_events)
            + detach_all_tracked_clients(&self.attach_render_clients);
        count
    }

    fn attach_panes(&self) -> Vec<IndexedPane> {
        self.windows.lock().unwrap().attach_panes()
    }

    fn attach_layout_snapshot(&self) -> AttachLayoutSnapshot {
        self.windows.lock().unwrap().attach_layout_snapshot()
    }

    fn attach_event_clients(&self) -> AttachEventClients {
        Arc::clone(&self.attach_events)
    }

    fn notify_attach_redraw_for_pane_output(self: &Arc<Self>, terminal_changes: TerminalChanges) {
        notify_attach_redraw(&self.attach_events);
        if terminal_changes.requires_immediate_render() {
            self.schedule_attach_render_immediate();
        } else {
            self.schedule_attach_render();
        }
    }

    fn notify_attach_redraw_immediate(self: &Arc<Self>) {
        notify_attach_redraw(&self.attach_events);
        self.schedule_attach_render_immediate();
    }

    fn schedule_attach_render(self: &Arc<Self>) {
        if self.attach_render_clients.lock().unwrap().is_empty() {
            return;
        }
        self.mark_attach_render_dirty();
        if self
            .attach_render_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let session = Arc::downgrade(self);
        std::thread::spawn(move || {
            std::thread::sleep(ATTACH_RENDER_DEBOUNCE_INTERVAL);
            let Some(session) = session.upgrade() else {
                return;
            };
            session.attach_render_pending.store(false, Ordering::SeqCst);
            session.notify_attach_render_if_dirty();
        });
    }

    fn schedule_attach_render_immediate(self: &Arc<Self>) {
        if self.attach_render_clients.lock().unwrap().is_empty() {
            return;
        }
        let epoch = self.mark_attach_render_dirty();
        self.attach_render_immediate_epoch
            .fetch_max(epoch, Ordering::SeqCst);
        if self
            .attach_render_immediate_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let session = Arc::downgrade(self);
        std::thread::spawn(move || run_attach_render_immediate_scheduler(session));
    }

    fn register_attach_stream(
        &self,
        stream: &UnixStream,
        client_id: usize,
    ) -> io::Result<StreamRegistration> {
        register_tracked_stream(
            &self.attach_streams,
            &self.next_attach_stream_id,
            stream,
            client_id,
            ClientType::Raw,
        )
    }

    fn register_attach_event_stream(
        &self,
        stream: &UnixStream,
        client_id: usize,
    ) -> io::Result<Option<StreamRegistration>> {
        register_attach_event_client(
            &self.attach_events,
            &self.next_attach_event_id,
            stream,
            client_id,
        )
    }

    fn register_attach_render_stream(
        &self,
        stream: &UnixStream,
        client_id: usize,
    ) -> io::Result<Option<StreamRegistration>> {
        register_attach_render_client(
            &self.attach_render_clients,
            &self.next_attach_render_id,
            stream,
            client_id,
            self.attach_render_frame(),
        )
    }

    fn attach_render_frame(&self) -> Option<Vec<u8>> {
        format_attach_render_stream_frame(self)
    }

    fn mark_attach_render_dirty(&self) -> u64 {
        self.attach_render_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn notify_attach_render_if_dirty(&self) {
        let epoch = self.attach_render_epoch.load(Ordering::SeqCst);
        self.notify_attach_render_until_epoch(epoch);
    }

    fn notify_attach_render_if_immediate_dirty(&self) {
        let epoch = self.attach_render_immediate_epoch.load(Ordering::SeqCst);
        self.notify_attach_render_until_epoch(epoch);
    }

    fn notify_attach_render_until_epoch(&self, epoch: u64) {
        loop {
            let rendered = self.attach_render_rendered_epoch.load(Ordering::SeqCst);
            if rendered >= epoch {
                return;
            }
            if self
                .attach_render_rendered_epoch
                .compare_exchange(rendered, epoch, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                self.notify_attach_render_now();
                return;
            }
        }
    }

    fn notify_attach_render_now(&self) {
        notify_attach_render(&self.attach_render_clients, || self.attach_render_frame());
    }

    fn raw_attach_layout_epoch(&self) -> u64 {
        self.raw_attach_layout_epoch.load(Ordering::SeqCst)
    }

    fn mark_raw_attach_layout_transition(&self) {
        self.raw_attach_layout_epoch.fetch_add(1, Ordering::SeqCst);
    }

    fn reconnect_raw_attach_streams(&self) {
        self.mark_raw_attach_layout_transition();
        self.close_raw_attach_streams();
    }

    fn reconnect_live_raw_pane_streams(&self) {
        self.mark_raw_attach_layout_transition();
        for pane in self.panes() {
            shutdown_tracked_clients(&pane.clients);
        }
    }

    fn close_raw_attach_streams(&self) {
        shutdown_tracked_clients(&self.attach_streams);
        for pane in self.panes() {
            shutdown_tracked_clients(&pane.clients);
        }
    }

    fn close_attach_streams(&self) {
        self.close_raw_attach_streams();
        shutdown_tracked_clients(&self.attach_events);
        shutdown_tracked_clients(&self.attach_render_clients);
    }
}

struct Window {
    id: TabId,
    name: String,
    panes: PaneSet<Arc<Pane>>,
    layout: LayoutNode,
    size: PtySize,
    zoomed: Option<usize>,
}

impl Window {
    fn new(id: TabId, name: String, pane: Arc<Pane>) -> Self {
        let size = *pane.size.lock().unwrap();
        Self {
            id,
            name,
            panes: PaneSet::new(pane),
            layout: LayoutNode::Pane(0),
            size,
            zoomed: None,
        }
    }

    fn active_pane(&self) -> Option<Arc<Pane>> {
        self.panes.active()
    }

    fn active_live_pane(&self) -> Option<Arc<Pane>> {
        self.panes.active().filter(|pane| pane.is_running())
    }

    fn pane(&self, target: PaneTarget) -> Result<Arc<Pane>, String> {
        let index = self.resolve_pane_target(target)?;
        self.panes
            .get(index)
            .cloned()
            .ok_or_else(|| "missing pane".to_string())
    }

    fn live_pane(&self, target: PaneTarget) -> Result<Arc<Pane>, String> {
        let pane = self.pane(target)?;
        if pane.is_running() {
            Ok(pane)
        } else {
            Err("pane is not running".to_string())
        }
    }

    #[cfg(test)]
    fn add_pane(&mut self, direction: SplitDirection, pane: Arc<Pane>) {
        self.add_pane_at_index(self.panes.active_index(), direction, pane)
            .unwrap();
    }

    fn add_pane_to_target(
        &mut self,
        target: PaneTarget,
        direction: SplitDirection,
        pane: Arc<Pane>,
    ) -> Result<(), String> {
        let split_index = self.resolve_pane_target(target)?;
        self.add_pane_at_index(split_index, direction, pane)
    }

    fn add_pane_at_index(
        &mut self,
        split_index: usize,
        direction: SplitDirection,
        pane: Arc<Pane>,
    ) -> Result<(), String> {
        let layout = self.layout_with_added_pane(split_index, direction, self.size)?;
        self.add_pane_with_layout(pane, layout);
        Ok(())
    }

    fn layout_with_added_pane(
        &self,
        split_index: usize,
        direction: SplitDirection,
        size: PtySize,
    ) -> Result<LayoutNode, String> {
        let new_index = self.panes.len();
        let mut layout = self.layout.clone();
        if !layout.split_pane(split_index, direction, new_index) {
            return Err("missing pane".to_string());
        }
        validate_layout_regions(&layout, new_index + 1, size)?;
        Ok(layout)
    }

    fn add_pane_with_layout(&mut self, pane: Arc<Pane>, layout: LayoutNode) {
        self.panes.add(pane);
        self.layout = layout;
        if self.zoomed.is_some() {
            self.zoomed = Some(self.panes.active_index());
        }
    }

    fn next_split_pane_size_for(
        &self,
        target: PaneTarget,
        direction: SplitDirection,
    ) -> Result<PtySize, String> {
        let index = self.resolve_pane_target(target)?;
        self.next_split_pane_size_for_index(index, direction)
            .ok_or_else(|| "missing pane".to_string())
    }

    fn next_split_pane_size_for_index(
        &self,
        split_index: usize,
        direction: SplitDirection,
    ) -> Option<PtySize> {
        if self.zoomed.is_some() {
            return Some(self.size);
        }

        let new_index = self.panes.len();
        let mut layout = self.layout.clone();
        if !layout.split_pane(split_index, direction, new_index) {
            return None;
        }

        layout_regions_for_size(&layout, self.size)
            .into_iter()
            .find(|region| region.pane == new_index)
            .and_then(|region| {
                PtySize::new(
                    (region.col_end - region.col_start) as u16,
                    (region.row_end - region.row_start) as u16,
                )
                .ok()
            })
    }

    #[cfg(test)]
    fn select_pane(&mut self, target: PaneSelectTarget) -> Result<(), String> {
        let index = self.resolve_pane_select_target(target)?;
        self.select_pane_index(index);
        Ok(())
    }

    fn select_pane_index(&mut self, index: usize) {
        let selected = self.panes.select(index);
        if selected && self.zoomed.is_some() {
            self.zoomed = Some(index);
        }
    }

    fn resolve_pane_select_target(&self, target: PaneSelectTarget) -> Result<usize, String> {
        match target {
            PaneSelectTarget::Index(index) => {
                if index >= self.panes.len() {
                    Err("missing pane".to_string())
                } else {
                    Ok(index)
                }
            }
            PaneSelectTarget::Id(id) => self
                .panes
                .panes
                .iter()
                .position(|pane| pane.id == PaneId::new(id))
                .ok_or_else(|| "missing pane".to_string()),
            PaneSelectTarget::Direction(direction) => self
                .directional_pane_target(direction)
                .ok_or_else(|| "missing adjacent pane".to_string()),
        }
    }

    fn resolve_pane_target(&self, target: PaneTarget) -> Result<usize, String> {
        match target {
            PaneTarget::Active => Ok(self.panes.active_index()),
            PaneTarget::Index(index) => {
                if index >= self.panes.len() {
                    Err("missing pane".to_string())
                } else {
                    Ok(index)
                }
            }
            PaneTarget::Id(id) => self
                .panes
                .panes
                .iter()
                .position(|pane| pane.id == PaneId::new(id))
                .ok_or_else(|| "missing pane".to_string()),
        }
    }

    fn directional_pane_target(&self, direction: PaneDirection) -> Option<usize> {
        let active_index = self.panes.active_index();
        let regions = layout_regions_for_size(&self.layout, self.size);
        directional_pane_target_from_regions(&regions, active_index, direction)
    }

    fn kill_pane(&mut self, target: PaneTarget) -> Result<Arc<Pane>, String> {
        if self.panes.len() <= 1 {
            return Err("cannot kill last pane; use kill-session".to_string());
        }
        let index = self.resolve_pane_target(target)?;
        Ok(self.remove_pane_at(index))
    }

    fn swap_panes(&mut self, source: PaneTarget, destination: PaneTarget) -> Result<(), String> {
        let source = self.resolve_pane_target(source)?;
        let destination = self.resolve_pane_target(destination)?;
        if source == destination {
            return Err("cannot swap pane with itself".to_string());
        }
        self.panes.panes.swap(source, destination);
        if self.panes.active == source {
            self.panes.active = destination;
        } else if self.panes.active == destination {
            self.panes.active = source;
        }
        if self.zoomed == Some(source) {
            self.zoomed = Some(destination);
        } else if self.zoomed == Some(destination) {
            self.zoomed = Some(source);
        }
        Ok(())
    }

    fn mark_pane_exited(&mut self, target: &Arc<Pane>) -> bool {
        let Some(index) = self
            .panes
            .panes
            .iter()
            .position(|pane| Arc::ptr_eq(pane, target))
        else {
            return false;
        };
        if self.panes.active_index() == index {
            if let Some(live_index) = self.panes.panes.iter().position(|pane| pane.is_running()) {
                self.panes.select(live_index);
                if self.zoomed.is_some() {
                    self.zoomed = Some(live_index);
                }
            }
        }
        true
    }

    fn prepare_respawn_pane(
        &self,
        target: PaneTarget,
        force: bool,
    ) -> Result<(PaneId, PtySize, Option<i32>, PathBuf), String> {
        let index = self.resolve_pane_target(target)?;
        let pane = self
            .panes
            .get(index)
            .ok_or_else(|| "missing pane".to_string())?;
        let old_pid = if pane.is_running() {
            if !force {
                return Err("pane is still running; use -k to force".to_string());
            }
            Some(pane.child_pid)
        } else {
            None
        };
        Ok((pane.id, *pane.size.lock().unwrap(), old_pid, pane.cwd()))
    }

    fn replace_pane(&mut self, id: PaneId, pane: Arc<Pane>) -> Result<(), String> {
        let index = self
            .panes
            .panes
            .iter()
            .position(|existing| existing.id == id)
            .ok_or_else(|| "missing pane".to_string())?;
        self.panes.panes[index] = pane;
        Ok(())
    }

    fn remove_pane_at(&mut self, index: usize) -> Arc<Pane> {
        let pane = self.panes.kill_at(index);
        self.layout.remove_pane(index);
        self.adjust_zoom_after_pane_removal(index);
        pane
    }

    fn take_pane_at(&mut self, index: usize) -> Arc<Pane> {
        if self.panes.len() == 1 {
            let pane = self.panes.panes.remove(index);
            self.panes.active = 0;
            self.layout = LayoutNode::Pane(0);
            self.zoomed = None;
            pane
        } else {
            self.remove_pane_at(index)
        }
    }

    fn is_empty(&self) -> bool {
        self.panes.len() == 0
    }

    fn contains_pane(&self, target: &Arc<Pane>) -> bool {
        self.panes
            .panes
            .iter()
            .any(|pane| Arc::ptr_eq(pane, target))
    }

    fn panes(&self) -> Vec<Arc<Pane>> {
        self.panes.all()
    }

    #[allow(dead_code)]
    fn pane_descriptions(&self) -> Vec<PaneDescription> {
        let window_zoomed = self.zoomed.is_some();
        (0..self.panes.len())
            .filter_map(|index| {
                self.panes.get(index).map(|pane| PaneDescription {
                    id: pane.id,
                    index,
                    active: index == self.panes.active_index(),
                    zoomed: self.zoomed == Some(index),
                    window_zoomed,
                    process: pane.process_status(),
                    cwd: pane.cwd(),
                    title: pane.title(),
                    bell: pane.bell(),
                    activity: pane.activity(),
                })
            })
            .collect()
    }

    fn attach_panes(&self) -> Vec<IndexedPane> {
        if let Some(index) = self.zoomed {
            return self
                .panes
                .get(index)
                .cloned()
                .map(|pane| vec![IndexedPane { index, pane }])
                .unwrap_or_default();
        }

        (0..self.panes.len())
            .filter_map(|index| {
                self.panes
                    .get(index)
                    .cloned()
                    .map(|pane| IndexedPane { index, pane })
            })
            .collect()
    }

    fn attach_layout_snapshot(&self) -> AttachLayoutSnapshot {
        if let Some(index) = self.zoomed {
            let panes = self
                .panes
                .get(index)
                .cloned()
                .map(|pane| vec![IndexedPane { index, pane }])
                .unwrap_or_default();
            return AttachLayoutSnapshot {
                layout: LayoutNode::Pane(index),
                panes,
                size: self.size,
                active_pane_index: index,
            };
        }

        AttachLayoutSnapshot {
            layout: self.layout.clone(),
            panes: (0..self.panes.len())
                .filter_map(|index| {
                    self.panes
                        .get(index)
                        .cloned()
                        .map(|pane| IndexedPane { index, pane })
                })
                .collect(),
            size: self.size,
            active_pane_index: self.panes.active_index(),
        }
    }

    fn resize_visible_panes(&mut self, size: PtySize) -> Vec<(Arc<Pane>, PtySize)> {
        self.size = size;
        self.visible_pane_resizes()
    }

    fn resize_pane(
        &mut self,
        target: PaneTarget,
        direction: PaneResizeDirection,
        amount: usize,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let index = self.resolve_pane_target(target)?;
        self.layout
            .resize_pane(index, direction, amount, self.size)?;
        Ok(self.visible_pane_resizes())
    }

    fn apply_layout_preset(
        &mut self,
        preset: LayoutPreset,
        size: PtySize,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        self.layout
            .apply_preset(preset, self.panes.len(), self.panes.active_index(), size)?;
        self.size = size;
        Ok(self.visible_pane_resizes())
    }

    fn visible_pane_resizes(&self) -> Vec<(Arc<Pane>, PtySize)> {
        let layout = self
            .zoomed
            .map_or_else(|| self.layout.clone(), LayoutNode::Pane);
        layout_regions_for_size(&layout, self.size)
            .into_iter()
            .filter_map(|region| {
                let size = PtySize::new(
                    (region.col_end - region.col_start) as u16,
                    (region.row_end - region.row_start) as u16,
                )
                .ok()?;
                self.panes
                    .get(region.pane)
                    .cloned()
                    .map(|pane| (pane, size))
            })
            .collect()
    }

    fn active_pane_index(&self) -> usize {
        self.panes.active_index()
    }

    #[cfg(test)]
    fn active_pane_id(&self) -> Option<PaneId> {
        self.panes
            .get(self.panes.active_index())
            .map(|pane| pane.id)
    }

    fn active_pane_zoomed(&self) -> bool {
        self.zoomed == Some(self.panes.active_index())
    }

    fn is_zoomed(&self) -> bool {
        self.zoomed.is_some()
    }

    fn zoom_pane_index(&mut self, index: usize) {
        if self.zoomed == Some(index) {
            self.zoomed = None;
            return;
        }

        self.panes.select(index);
        self.zoomed = Some(index);
    }

    fn adjust_zoom_after_pane_removal(&mut self, removed: usize) {
        match self.zoomed {
            Some(zoomed) if zoomed == removed => self.zoomed = None,
            Some(zoomed) if zoomed > removed => self.zoomed = Some(zoomed - 1),
            _ => {}
        }
    }

    fn terminate_panes(&self) -> Result<(), String> {
        for pane in self.panes() {
            terminate_pane_if_running_async(&pane);
        }
        Ok(())
    }
}

struct WindowSet {
    windows: Vec<Window>,
    active: usize,
}

impl WindowSet {
    fn new(window: Window) -> Self {
        Self {
            windows: vec![window],
            active: 0,
        }
    }

    fn add(&mut self, mut window: Window) {
        if self
            .windows
            .iter()
            .any(|existing| existing.name == window.name)
        {
            window.name = self.next_default_window_name(window.id.as_usize());
        }
        self.windows.push(window);
        self.active = self.windows.len() - 1;
    }

    fn next_default_window_name(&self, start: usize) -> String {
        let mut candidate = start;
        loop {
            let name = candidate.to_string();
            if !self.windows.iter().any(|window| window.name == name) {
                return name;
            }
            candidate += 1;
        }
    }

    fn select(&mut self, target: WindowTarget) -> Result<(), String> {
        let index = self.resolve_target(target)?;
        self.active = index;
        Ok(())
    }

    fn select_next(&mut self) -> Result<(), String> {
        if self.windows.is_empty() {
            return Err("missing window".to_string());
        }
        self.active = (self.active + 1) % self.windows.len();
        Ok(())
    }

    fn select_previous(&mut self) -> Result<(), String> {
        if self.windows.is_empty() {
            return Err("missing window".to_string());
        }
        self.active = if self.active == 0 {
            self.windows.len() - 1
        } else {
            self.active - 1
        };
        Ok(())
    }

    fn rename_window(&mut self, target: WindowTarget, name: String) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("window name cannot be empty".to_string());
        }
        if name.chars().any(char::is_control) {
            return Err("window name cannot contain control characters".to_string());
        }
        let index = self.resolve_target(target)?;
        if self
            .windows
            .iter()
            .enumerate()
            .any(|(candidate, window)| candidate != index && window.name == name)
        {
            return Err("window name already exists".to_string());
        }
        self.windows[index].name = name;
        Ok(())
    }

    fn resolve_target(&self, target: WindowTarget) -> Result<usize, String> {
        match target {
            WindowTarget::Active => Ok(self.active),
            WindowTarget::Index(index) if index < self.windows.len() => Ok(index),
            WindowTarget::Index(_) => Err("missing window".to_string()),
            WindowTarget::Id(id) => self
                .windows
                .iter()
                .position(|window| window.id == TabId::new(id))
                .ok_or_else(|| "missing window".to_string()),
            WindowTarget::Name(name) => self
                .windows
                .iter()
                .position(|window| window.name == name)
                .ok_or_else(|| "missing window".to_string()),
        }
    }

    fn active_window(&self) -> Option<&Window> {
        self.windows.get(self.active)
    }

    fn active_window_mut(&mut self) -> Option<&mut Window> {
        self.windows.get_mut(self.active)
    }

    fn active_pane(&self) -> Option<Arc<Pane>> {
        self.active_window()?.active_pane()
    }

    fn active_live_pane(&self) -> Option<Arc<Pane>> {
        self.active_window()?.active_live_pane()
    }

    fn pane(&self, window: WindowTarget, pane: PaneTarget) -> Result<Arc<Pane>, String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].pane(pane)
    }

    fn live_pane(&self, window: WindowTarget, pane: PaneTarget) -> Result<Arc<Pane>, String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].live_pane(pane)
    }

    fn add_pane(
        &mut self,
        window: WindowTarget,
        target: PaneTarget,
        direction: SplitDirection,
        pane: Arc<Pane>,
    ) -> Result<(), String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].add_pane_to_target(target, direction, pane)
    }

    fn next_split_pane_size(
        &self,
        window: WindowTarget,
        target: PaneTarget,
        direction: SplitDirection,
    ) -> Result<PtySize, String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].next_split_pane_size_for(target, direction)
    }

    fn target_is_active_window(&self, target: WindowTarget) -> Result<bool, String> {
        Ok(self.resolve_target(target)? == self.active)
    }

    fn select_pane(
        &mut self,
        window: WindowTarget,
        target: PaneSelectTarget,
    ) -> Result<bool, String> {
        let active_before = self.active;
        let window_index = self.resolve_target(window)?;
        let pane_index = self.windows[window_index].resolve_pane_select_target(target)?;
        self.active = window_index;
        self.windows[window_index].select_pane_index(pane_index);
        Ok(active_before != window_index)
    }

    fn kill_pane(&mut self, window: WindowTarget, target: PaneTarget) -> Result<Arc<Pane>, String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].kill_pane(target)
    }

    fn swap_panes(
        &mut self,
        source_window: WindowTarget,
        source: PaneTarget,
        destination_window: WindowTarget,
        destination: PaneTarget,
    ) -> Result<(), String> {
        let source_window = self.resolve_target(source_window)?;
        let destination_window = self.resolve_target(destination_window)?;
        if source_window != destination_window {
            return Err("swap-pane requires panes in the same window".to_string());
        }
        self.windows[source_window].swap_panes(source, destination)
    }

    fn move_pane(
        &mut self,
        source_window: WindowTarget,
        source: PaneTarget,
        destination_window: WindowTarget,
        destination: PaneTarget,
        direction: SplitDirection,
        size: PtySize,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let source_window = self.resolve_target(source_window)?;
        let destination_window = self.resolve_target(destination_window)?;
        if source_window == destination_window {
            return Err(
                "source and destination panes are in the same window; use swap-pane".to_string(),
            );
        }
        let source_pane = self.windows[source_window].resolve_pane_target(source)?;
        let destination_pane = self.windows[destination_window].resolve_pane_target(destination)?;
        let destination_layout = self.windows[destination_window].layout_with_added_pane(
            destination_pane,
            direction,
            size,
        )?;
        let active_before = self.windows.get(self.active).map(|window| window.id);

        let moved = self.windows[source_window].take_pane_at(source_pane);
        let source_empty = self.windows[source_window].is_empty();
        let adjusted_destination = if source_empty && source_window < destination_window {
            destination_window - 1
        } else {
            destination_window
        };

        if source_empty {
            self.windows.remove(source_window);
            if self.active == source_window {
                self.active = source_window
                    .saturating_sub(1)
                    .min(self.windows.len().saturating_sub(1));
            } else if self.active > source_window {
                self.active -= 1;
            }
        }

        self.windows[adjusted_destination].add_pane_with_layout(moved, destination_layout);
        self.restore_active_window_or(active_before, adjusted_destination);

        let mut resizes = Vec::new();
        if !source_empty {
            resizes.extend(self.windows[source_window].resize_visible_panes(size));
        }
        resizes.extend(self.windows[adjusted_destination].resize_visible_panes(size));
        Ok(resizes)
    }

    fn restore_active_window_or(&mut self, active_before: Option<TabId>, fallback: usize) {
        if let Some(active_before) = active_before {
            if let Some(index) = self
                .windows
                .iter()
                .position(|window| window.id == active_before)
            {
                self.active = index;
                return;
            }
        }

        if !self.windows.is_empty() {
            self.active = fallback.min(self.windows.len() - 1);
        }
    }

    fn break_pane(
        &mut self,
        source_window: WindowTarget,
        source: PaneTarget,
        new_window_id: TabId,
        size: PtySize,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let source_window = self.resolve_target(source_window)?;
        if self.windows[source_window].panes.len() <= 1 {
            return Err("cannot break last pane in window".to_string());
        }
        let source_pane = self.windows[source_window].resolve_pane_target(source)?;
        let source_was_active = self.active == source_window;
        let active_before = self.windows.get(self.active).map(|window| window.id);
        let moved = self.windows[source_window].take_pane_at(source_pane);
        let mut new_window = Window::new(
            new_window_id,
            self.next_default_window_name(new_window_id.as_usize()),
            moved,
        );
        new_window.resize_visible_panes(size);

        let mut resizes = self.windows[source_window].resize_visible_panes(size);
        self.windows.push(new_window);
        let new_window_index = self.windows.len() - 1;
        if source_was_active {
            self.active = new_window_index;
        } else {
            self.restore_active_window_or(active_before, new_window_index);
        }
        resizes.extend(self.windows[new_window_index].resize_visible_panes(size));
        Ok(resizes)
    }

    fn mark_pane_exited(&mut self, target: &Arc<Pane>) -> bool {
        let Some(window_index) = self
            .windows
            .iter()
            .position(|window| window.contains_pane(target))
        else {
            return false;
        };

        self.windows[window_index].mark_pane_exited(target)
    }

    fn prepare_respawn_pane(
        &self,
        window: WindowTarget,
        target: PaneTarget,
        force: bool,
    ) -> Result<(PaneId, PtySize, Option<i32>, PathBuf), String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].prepare_respawn_pane(target, force)
    }

    fn replace_pane(
        &mut self,
        window: WindowTarget,
        id: PaneId,
        pane: Arc<Pane>,
    ) -> Result<(), String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].replace_pane(id, pane)
    }

    fn panes(&self) -> Vec<Arc<Pane>> {
        self.windows
            .iter()
            .flat_map(Window::panes)
            .collect::<Vec<_>>()
    }

    fn pane_descriptions(&self) -> Vec<PaneDescription> {
        self.active_window()
            .map_or_else(Vec::new, Window::pane_descriptions)
    }

    fn pane_descriptions_for_window(
        &self,
        target: WindowTarget,
    ) -> Result<Vec<PaneDescription>, String> {
        let window_index = self.resolve_target(target)?;
        Ok(self.windows[window_index].pane_descriptions())
    }

    fn window_descriptions(&self) -> Vec<WindowDescription> {
        self.windows
            .iter()
            .enumerate()
            .map(|(index, window)| WindowDescription {
                index,
                id: window.id,
                name: window.name.clone(),
                active: index == self.active,
                panes: window.panes.len(),
            })
            .collect()
    }

    fn attach_panes(&self) -> Vec<IndexedPane> {
        self.active_window()
            .map_or_else(Vec::new, Window::attach_panes)
    }

    fn attach_layout_snapshot(&self) -> AttachLayoutSnapshot {
        self.active_window()
            .map_or_else(AttachLayoutSnapshot::empty, Window::attach_layout_snapshot)
    }

    fn resize_visible_panes(&mut self, size: PtySize) -> Vec<(Arc<Pane>, PtySize)> {
        self.active_window_mut()
            .map_or_else(Vec::new, |window| window.resize_visible_panes(size))
    }

    fn resize_visible_panes_for_window(
        &mut self,
        target: WindowTarget,
        size: PtySize,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let window_index = self.resolve_target(target)?;
        Ok(self.windows[window_index].resize_visible_panes(size))
    }

    fn resize_pane(
        &mut self,
        window: WindowTarget,
        target: PaneTarget,
        direction: PaneResizeDirection,
        amount: usize,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].resize_pane(target, direction, amount)
    }

    fn select_layout(
        &mut self,
        window: WindowTarget,
        preset: LayoutPreset,
        size: PtySize,
    ) -> Result<Vec<(Arc<Pane>, PtySize)>, String> {
        let window_index = self.resolve_target(window)?;
        self.windows[window_index].apply_layout_preset(preset, size)
    }

    fn status_context(&self, session_name: &str) -> Option<StatusContext> {
        let window = self.active_window()?;
        let pane = window.active_pane()?;
        Some(StatusContext {
            session_name: session_name.to_string(),
            window_index: self.active,
            window_id: window.id,
            window_name: window.name.clone(),
            window_count: self.windows.len(),
            pane_index: window.active_pane_index(),
            pane_id: pane.id,
            pane_zoomed: window.active_pane_zoomed(),
            window_zoomed: window.is_zoomed(),
            pane_process: pane.process_status(),
            pane_cwd: pane.cwd(),
            pane_title: pane.title(),
            pane_bell: pane.bell(),
            pane_activity: pane.activity(),
            buffer_summary: BufferStatusSummary::default(),
            transient_message: None,
            session_attached_count: 0,
            session_created_at: 0,
            status_hints: true,
            prefix_key: crate::config::DEFAULT_PREFIX_KEY.to_string(),
        })
    }

    fn window_count(&self) -> usize {
        self.windows.len()
    }

    fn kill_window(&mut self, target: WindowTarget) -> Result<(), String> {
        if self.windows.len() <= 1 {
            return Err("cannot kill last window; use kill-session".to_string());
        }

        let index = self.resolve_target(target)?;

        self.windows[index].terminate_panes()?;
        self.windows.remove(index);
        if self.active == index {
            self.active = index.saturating_sub(1).min(self.windows.len() - 1);
        } else if self.active > index {
            self.active -= 1;
        }
        Ok(())
    }

    fn zoom_pane(&mut self, window: WindowTarget, target: PaneTarget) -> Result<bool, String> {
        let active_before = self.active;
        let window_index = self.resolve_target(window)?;
        let pane_index = self.windows[window_index].resolve_pane_target(target)?;
        self.active = window_index;
        self.windows[window_index].zoom_pane_index(pane_index);
        Ok(active_before != window_index)
    }
}

struct PaneDescription {
    id: PaneId,
    index: usize,
    active: bool,
    zoomed: bool,
    window_zoomed: bool,
    process: PaneProcessStatus,
    cwd: PathBuf,
    title: String,
    bell: bool,
    activity: bool,
}

struct IndexedPane {
    index: usize,
    pane: Arc<Pane>,
}

struct AttachLayoutSnapshot {
    layout: LayoutNode,
    panes: Vec<IndexedPane>,
    size: PtySize,
    active_pane_index: usize,
}

impl AttachLayoutSnapshot {
    fn empty() -> Self {
        Self {
            layout: LayoutNode::Pane(0),
            panes: Vec::new(),
            size: PtySize { cols: 1, rows: 1 },
            active_pane_index: 0,
        }
    }
}

fn validate_layout_regions(
    layout: &LayoutNode,
    pane_count: usize,
    size: PtySize,
) -> Result<(), String> {
    let regions = layout_regions_for_size(layout, size);
    if regions.len() != pane_count {
        return Err("resize would exceed minimum pane size".to_string());
    }

    let mut seen = vec![false; pane_count];
    for region in regions {
        if region.pane >= pane_count
            || seen[region.pane]
            || region.row_start >= region.row_end
            || region.col_start >= region.col_end
        {
            return Err("resize would exceed minimum pane size".to_string());
        }
        seen[region.pane] = true;
    }

    if seen.into_iter().all(|seen| seen) {
        Ok(())
    } else {
        Err("resize would exceed minimum pane size".to_string())
    }
}

fn directional_pane_target_from_regions(
    regions: &[PaneRegion],
    active_index: usize,
    direction: PaneDirection,
) -> Option<usize> {
    let active = regions.iter().find(|region| region.pane == active_index)?;
    regions
        .iter()
        .filter(|region| region.pane != active_index)
        .filter_map(|region| {
            directional_pane_score(active, region, direction).map(|score| (region.pane, score))
        })
        .min_by(|(left_pane, left_score), (right_pane, right_score)| {
            left_score
                .distance
                .cmp(&right_score.distance)
                .then_with(|| right_score.overlap.cmp(&left_score.overlap))
                .then_with(|| left_pane.cmp(right_pane))
        })
        .map(|(pane, _)| pane)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectionalPaneScore {
    distance: usize,
    overlap: usize,
}

fn directional_pane_score(
    active: &PaneRegion,
    candidate: &PaneRegion,
    direction: PaneDirection,
) -> Option<DirectionalPaneScore> {
    let (distance, overlap) = match direction {
        PaneDirection::Left if candidate.col_end <= active.col_start => (
            active.col_start - candidate.col_end,
            span_overlap(
                active.row_start,
                active.row_end,
                candidate.row_start,
                candidate.row_end,
            ),
        ),
        PaneDirection::Right if candidate.col_start >= active.col_end => (
            candidate.col_start - active.col_end,
            span_overlap(
                active.row_start,
                active.row_end,
                candidate.row_start,
                candidate.row_end,
            ),
        ),
        PaneDirection::Up if candidate.row_end <= active.row_start => (
            active.row_start - candidate.row_end,
            span_overlap(
                active.col_start,
                active.col_end,
                candidate.col_start,
                candidate.col_end,
            ),
        ),
        PaneDirection::Down if candidate.row_start >= active.row_end => (
            candidate.row_start - active.row_end,
            span_overlap(
                active.col_start,
                active.col_end,
                candidate.col_start,
                candidate.col_end,
            ),
        ),
        _ => return None,
    };

    (overlap > 0).then_some(DirectionalPaneScore { distance, overlap })
}

fn span_overlap(
    first_start: usize,
    first_end: usize,
    second_start: usize,
    second_end: usize,
) -> usize {
    first_end
        .min(second_end)
        .saturating_sub(first_start.max(second_start))
}

struct PaneSnapshot {
    index: usize,
    screen: String,
}

#[allow(dead_code)]
struct PaneRenderSnapshot<'a> {
    index: usize,
    terminal: &'a TerminalState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedAttachLayout {
    lines: Vec<String>,
    regions: Vec<PaneRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedAttachSnapshot {
    text: String,
    regions: Vec<PaneRegion>,
    cursor: Option<RenderedCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderedCursor {
    row: usize,
    col: usize,
    visible: bool,
}

struct StatusContext {
    session_name: String,
    session_attached_count: usize,
    session_created_at: usize,
    window_index: usize,
    window_id: TabId,
    window_name: String,
    window_count: usize,
    pane_index: usize,
    pane_id: PaneId,
    pane_zoomed: bool,
    window_zoomed: bool,
    pane_process: PaneProcessStatus,
    pane_cwd: PathBuf,
    pane_title: String,
    pane_bell: bool,
    pane_activity: bool,
    buffer_summary: BufferStatusSummary,
    transient_message: Option<String>,
    status_hints: bool,
    prefix_key: String,
}

struct WindowDescription {
    index: usize,
    id: TabId,
    name: String,
    active: bool,
    panes: usize,
}

struct PaneSet<T> {
    panes: Vec<T>,
    active: usize,
}

impl<T> PaneSet<T> {
    fn new(pane: T) -> Self {
        Self {
            panes: vec![pane],
            active: 0,
        }
    }

    fn add(&mut self, pane: T) {
        self.panes.push(pane);
        self.active = self.panes.len() - 1;
    }

    fn select(&mut self, index: usize) -> bool {
        if index >= self.panes.len() {
            return false;
        }
        self.active = index;
        true
    }

    #[cfg(test)]
    fn kill(&mut self, target: Option<usize>) -> Result<T, &'static str> {
        let index = self.kill_index(target)?;
        Ok(self.kill_at(index))
    }

    #[allow(dead_code)]
    fn kill_index(&self, target: Option<usize>) -> Result<usize, &'static str> {
        if self.panes.len() <= 1 {
            return Err("cannot kill last pane; use kill-session");
        }

        let index = target.unwrap_or(self.active);
        if index >= self.panes.len() {
            return Err("missing pane");
        }

        Ok(index)
    }

    fn kill_at(&mut self, index: usize) -> T {
        let pane = self.panes.remove(index);
        if self.active == index {
            self.active = index
                .saturating_sub(1)
                .min(self.panes.len().saturating_sub(1));
        } else if self.active > index {
            self.active -= 1;
        }
        pane
    }

    fn get(&self, index: usize) -> Option<&T> {
        self.panes.get(index)
    }

    fn active_index(&self) -> usize {
        self.active
    }

    fn len(&self) -> usize {
        self.panes.len()
    }
}

impl<T: Clone> PaneSet<T> {
    fn active(&self) -> Option<T> {
        self.panes.get(self.active).cloned()
    }

    fn all(&self) -> Vec<T> {
        self.panes.clone()
    }
}

struct Pane {
    id: PaneId,
    child_pid: i32,
    writer: Arc<Mutex<File>>,
    process: Mutex<PaneProcessStatus>,
    cwd: Mutex<PathBuf>,
    title: Mutex<String>,
    bell: AtomicBool,
    activity: AtomicBool,
    synchronized_output_redraw_pending: AtomicBool,
    size: Mutex<PtySize>,
    raw_history: Arc<Mutex<Vec<u8>>>,
    terminal: Arc<Mutex<TerminalState>>,
    output_filter: Mutex<PtyOutputFilter>,
    clients: TrackedStreamClients,
    next_client_id: AtomicUsize,
    attach_events: AttachEventClients,
    session: Mutex<Option<Weak<Session>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneProcessStatus {
    Running {
        pid: i32,
    },
    Exited {
        pid: i32,
        exit_status: Option<i32>,
        exit_signal: Option<i32>,
    },
}

impl PaneProcessStatus {
    fn state(self) -> &'static str {
        match self {
            Self::Running { .. } => "running",
            Self::Exited { .. } => "exited",
        }
    }

    fn pid(self) -> i32 {
        match self {
            Self::Running { pid } | Self::Exited { pid, .. } => pid,
        }
    }
}

impl Pane {
    fn register_client(&self, stream: &UnixStream) -> io::Result<StreamRegistration> {
        register_tracked_stream(
            &self.clients,
            &self.next_client_id,
            stream,
            self.next_client_id.load(Ordering::Relaxed),
            ClientType::Raw,
        )
    }

    fn set_session(&self, session: &Arc<Session>) {
        *self.session.lock().unwrap() = Some(Arc::downgrade(session));
    }

    fn session(&self) -> Option<Arc<Session>> {
        self.session
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
    }

    fn cwd(&self) -> PathBuf {
        self.cwd.lock().unwrap().clone()
    }

    fn set_cwd(&self, cwd: PathBuf) {
        *self.cwd.lock().unwrap() = cwd;
    }

    fn title(&self) -> String {
        self.title.lock().unwrap().clone()
    }

    fn set_title(&self, title: String) {
        *self.title.lock().unwrap() = title;
    }

    fn mark_bell(&self) {
        self.bell.store(true, Ordering::SeqCst);
    }

    fn bell(&self) -> bool {
        self.bell.load(Ordering::SeqCst)
    }

    fn mark_activity(&self) {
        self.activity.store(true, Ordering::SeqCst);
    }

    fn activity(&self) -> bool {
        self.activity.load(Ordering::SeqCst)
    }

    fn clear_alerts(&self) {
        self.bell.store(false, Ordering::SeqCst);
        self.activity.store(false, Ordering::SeqCst);
    }

    fn mark_synchronized_output_redraw_pending(&self) -> bool {
        self.synchronized_output_redraw_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    fn clear_synchronized_output_redraw_pending(&self) {
        self.synchronized_output_redraw_pending
            .store(false, Ordering::SeqCst);
    }

    fn take_synchronized_output_redraw_pending(&self) -> bool {
        self.synchronized_output_redraw_pending
            .swap(false, Ordering::SeqCst)
    }

    fn is_running(&self) -> bool {
        matches!(
            *self.process.lock().unwrap(),
            PaneProcessStatus::Running { .. }
        )
    }

    fn process_status(&self) -> PaneProcessStatus {
        *self.process.lock().unwrap()
    }

    fn mark_exited(&self, status: pty::PtyExitStatus) -> bool {
        let mut process = self.process.lock().unwrap();
        let PaneProcessStatus::Running { pid } = *process else {
            return false;
        };
        *process = PaneProcessStatus::Exited {
            pid,
            exit_status: status.code,
            exit_signal: status.signal,
        };
        true
    }
}

fn handle_connection(state: Arc<ServerState>, mut stream: UnixStream) -> io::Result<()> {
    let line = {
        let mut reader = io::BufReader::new(stream.try_clone()?);
        match read_control_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                write_err(&mut stream, &error.to_string())?;
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    };

    let request = match protocol::decode_request(&line) {
        Ok(request) => request,
        Err(err) => {
            write_err(&mut stream, &err)?;
            return Ok(());
        }
    };

    match request {
        Request::New {
            session,
            command,
            cwd,
        } => handle_new(&state, &mut stream, session, command, cwd),
        Request::List => handle_list(&state, &mut stream),
        Request::ListSessions { format } => {
            handle_list_sessions(&state, &mut stream, format.as_deref())
        }
        Request::RenameSession { old_name, new_name } => {
            handle_rename_session(&state, &mut stream, &old_name, new_name)
        }
        Request::ListClients { session, format } => {
            handle_list_clients(&state, &mut stream, session.as_deref(), format.as_deref())
        }
        Request::DetachClient { session, client_id } => {
            handle_detach_client(&state, &mut stream, session.as_deref(), client_id)
        }
        Request::Capture {
            target,
            mode,
            selection,
        } => handle_capture(&state, &mut stream, &target, mode, &selection),
        Request::SaveBuffer {
            target,
            buffer,
            mode,
            selection,
        } => handle_save_buffer(
            &state,
            &mut stream,
            &target,
            buffer.as_deref(),
            mode,
            &selection,
        ),
        Request::SaveBufferText {
            session,
            buffer,
            text,
        } => handle_save_buffer_text(&state, &mut stream, &session, buffer.as_deref(), text),
        Request::CopyMode {
            session,
            mode,
            search,
            match_index,
        } => handle_copy_mode(
            &state,
            &mut stream,
            &session,
            mode,
            search.as_deref(),
            match_index,
        ),
        Request::ListBuffers { format } => {
            handle_list_buffers(&state, &mut stream, format.as_deref())
        }
        Request::PasteBuffer { target, buffer } => {
            handle_paste_buffer(&state, &mut stream, &target, buffer.as_deref())
        }
        Request::DeleteBuffer { buffer } => handle_delete_buffer(&state, &mut stream, &buffer),
        Request::Resize {
            session,
            cols,
            rows,
        } => handle_resize(&state, &mut stream, &session, cols, rows),
        Request::ResizePane {
            target,
            direction,
            amount,
        } => handle_resize_pane(&state, &mut stream, &target, direction, amount),
        Request::SelectLayout {
            session,
            window,
            preset,
        } => handle_select_layout(&state, &mut stream, &session, window, preset),
        Request::Send { target, bytes } => handle_send(&state, &mut stream, &target, &bytes),
        Request::Split {
            target,
            direction,
            command,
            cwd,
        } => handle_split(&state, &mut stream, &target, direction, command, cwd),
        Request::ListPanes {
            session,
            window,
            format,
        } => handle_list_panes(&state, &mut stream, &session, window, format.as_deref()),
        Request::SelectPane {
            session,
            window,
            target,
        } => handle_select_pane(&state, &mut stream, &session, window, target),
        Request::KillPane { target } => handle_kill_pane(&state, &mut stream, &target),
        Request::SwapPane {
            source,
            destination,
        } => handle_swap_pane(&state, &mut stream, &source, &destination),
        Request::MovePane {
            source,
            destination,
            direction,
        } => handle_move_pane(&state, &mut stream, &source, &destination, direction),
        Request::BreakPane { target } => handle_break_pane(&state, &mut stream, &target),
        Request::JoinPane {
            source,
            destination,
            direction,
        } => handle_move_pane(&state, &mut stream, &source, &destination, direction),
        Request::RespawnPane {
            target,
            force,
            command,
            cwd,
        } => handle_respawn_pane(&state, &mut stream, &target, force, command, cwd),
        Request::NewWindow {
            session,
            command,
            cwd,
        } => handle_new_window(&state, &mut stream, &session, command, cwd),
        Request::ListWindows { session, format } => {
            handle_list_windows(&state, &mut stream, &session, format.as_deref())
        }
        Request::SelectWindow { session, target } => {
            handle_select_window(&state, &mut stream, &session, target)
        }
        Request::RenameWindow {
            session,
            target,
            name,
        } => handle_rename_window(&state, &mut stream, &session, target, name),
        Request::NextWindow { session } => {
            handle_cycle_window(&state, &mut stream, &session, WindowCycle::Next)
        }
        Request::PreviousWindow { session } => {
            handle_cycle_window(&state, &mut stream, &session, WindowCycle::Previous)
        }
        Request::KillWindow { session, target } => {
            handle_kill_window(&state, &mut stream, &session, target)
        }
        Request::ZoomPane { target } => handle_zoom_pane(&state, &mut stream, &target),
        Request::StatusLine { session, format } => {
            handle_status_line(&state, &mut stream, &session, format.as_deref())
        }
        Request::DisplayMessage { session, format } => {
            handle_display_message(&state, &mut stream, &session, &format)
        }
        Request::ListKeys { format } => handle_list_keys(&state, &mut stream, format.as_deref()),
        Request::BindKey { key, command } => handle_bind_key(&state, &mut stream, &key, &command),
        Request::UnbindKey { key } => handle_unbind_key(&state, &mut stream, &key),
        Request::ShowOptions { format } => {
            handle_show_options(&state, &mut stream, format.as_deref())
        }
        Request::SetOption { name, value } => handle_set_option(&state, &mut stream, &name, &value),
        Request::Kill { session } => handle_kill(&state, &mut stream, &session),
        Request::KillServer => handle_kill_server(&state, &mut stream),
        Request::Attach { session } => handle_attach(&state, stream, &session),
        Request::AttachRawState { session } => {
            handle_attach_raw_state(&state, &mut stream, &session)
        }
        Request::AttachSnapshot { session } => {
            handle_attach_snapshot(&state, &mut stream, &session)
        }
        Request::AttachLayoutSnapshot { session } => {
            handle_attach_layout_snapshot(&state, &mut stream, &session)
        }
        Request::AttachLayoutFrame { session } => {
            handle_attach_layout_frame(&state, &mut stream, &session)
        }
        Request::AttachEvents { session } => handle_attach_events(&state, &mut stream, &session),
        Request::AttachRender { session } => handle_attach_render(&state, &mut stream, &session),
    }
}

fn read_control_line<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut line = Vec::new();

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > MAX_CONTROL_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request line too long",
            ));
        }

        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }

    String::from_utf8(line)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request line is not utf-8"))
}

fn handle_new(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: String,
    command: Vec<String>,
    cwd: Option<PathBuf>,
) -> io::Result<()> {
    if let Err(message) = validate_session_name(&name) {
        write_err(stream, &message)?;
        return Ok(());
    }
    if session_name_exists(state, &name) {
        write_err(
            stream,
            &format!("session already exists; use dmux attach -t {name}"),
        )?;
        return Ok(());
    }

    let cwd = cwd.unwrap_or(std::env::current_dir()?);
    let attach_events = Arc::new(Mutex::new(Vec::new()));
    let pane = spawn_pane(
        PaneId::new(0),
        name.clone(),
        command,
        cwd,
        PtySize { cols: 80, rows: 24 },
        Arc::clone(&attach_events),
        None,
    )?;
    let created_at = state.next_session_created_id.fetch_add(1, Ordering::SeqCst);
    let session = Arc::new(Session::new(
        name.clone(),
        created_at,
        Arc::clone(&pane),
        attach_events,
    ));
    session.set_buffer_summary(state.buffer_status_summary());
    let (prefix, status_hints) = current_runtime_options(state);
    session.set_runtime_options(prefix, status_hints);
    pane.set_session(&session);
    if !insert_session_if_name_available(state, &name, session) {
        terminate_pane_async(pane.child_pid);
        write_err(
            stream,
            &format!("session already exists; use dmux attach -t {name}"),
        )?;
        return Ok(());
    }
    write_ok(stream)
}

fn spawn_pane(
    id: PaneId,
    session_name: String,
    command: Vec<String>,
    cwd: PathBuf,
    size: PtySize,
    attach_events: AttachEventClients,
    initial_session: Option<Weak<Session>>,
) -> io::Result<Arc<Pane>> {
    let mut spec = SpawnSpec::new(session_name, command, cwd);
    spec.size = size;
    let process = pty::spawn(&spec)?;
    let reader = process.master.try_clone()?;
    let pane = Arc::new(Pane {
        id,
        child_pid: process.child_pid,
        writer: Arc::new(Mutex::new(process.master)),
        process: Mutex::new(PaneProcessStatus::Running {
            pid: process.child_pid,
        }),
        cwd: Mutex::new(spec.cwd.clone()),
        title: Mutex::new(String::new()),
        bell: AtomicBool::new(false),
        activity: AtomicBool::new(false),
        synchronized_output_redraw_pending: AtomicBool::new(false),
        size: Mutex::new(spec.size),
        raw_history: Arc::new(Mutex::new(Vec::new())),
        terminal: Arc::new(Mutex::new(TerminalState::new(
            spec.size.cols as usize,
            spec.size.rows as usize,
            10_000,
        ))),
        output_filter: Mutex::new(PtyOutputFilter::default()),
        clients: Arc::new(Mutex::new(Vec::new())),
        next_client_id: AtomicUsize::new(0),
        attach_events,
        session: Mutex::new(initial_session),
    });

    start_output_pump(reader, Arc::clone(&pane));
    Ok(pane)
}

fn handle_list(state: &Arc<ServerState>, stream: &mut UnixStream) -> io::Result<()> {
    let mut names = state
        .sessions
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();

    write_ok(stream)?;
    for name in names {
        writeln!(stream, "{name}")?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionDescription {
    name: String,
    windows: usize,
    attached_count: usize,
    created_at: usize,
}

fn session_descriptions(state: &Arc<ServerState>) -> Vec<SessionDescription> {
    let mut sessions = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .map(|session| SessionDescription {
            name: session.name(),
            windows: session.window_count(),
            attached_count: session.attached_count(),
            created_at: session.created_at,
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| left.name.cmp(&right.name));
    sessions
}

fn handle_list_sessions(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    format: Option<&str>,
) -> io::Result<()> {
    match format {
        Some(format) => {
            write_ok(stream)?;
            for session in session_descriptions(state) {
                writeln!(stream, "{}", format_session_line(format, &session))?;
            }
            Ok(())
        }
        None => handle_list(state, stream),
    }
}

fn format_session_line(format: &str, session: &SessionDescription) -> String {
    let windows = session.windows.to_string();
    let attached_count = session.attached_count.to_string();
    let attached = if session.attached_count > 0 { "1" } else { "0" };
    let created_at = session.created_at.to_string();
    let replacements = [
        ("#{session.name}", session.name.as_str()),
        ("#{session.windows}", windows.as_str()),
        ("#{session.window_count}", windows.as_str()),
        ("#{session.attached}", attached),
        ("#{session.attached_count}", attached_count.as_str()),
        ("#{session.created_at}", created_at.as_str()),
        ("#{client.count}", attached_count.as_str()),
    ];
    apply_replacements(format, &replacements)
}

fn validate_session_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("session name cannot be empty".to_string());
    }
    if name.chars().any(char::is_control) {
        return Err("session name cannot contain control characters".to_string());
    }
    if name.contains(':') {
        return Err("session name cannot contain ':'".to_string());
    }
    Ok(())
}

fn session_name_exists(state: &Arc<ServerState>, name: &str) -> bool {
    let alias_exists = state.session_aliases.lock().unwrap().contains_key(name);
    alias_exists || state.sessions.lock().unwrap().contains_key(name)
}

fn insert_session_if_name_available(
    state: &Arc<ServerState>,
    name: &str,
    session: Arc<Session>,
) -> bool {
    let aliases = state.session_aliases.lock().unwrap();
    let mut sessions = state.sessions.lock().unwrap();
    if aliases.contains_key(name) || sessions.contains_key(name) {
        return false;
    }
    sessions.insert(name.to_string(), session);
    true
}

fn resolve_session(state: &Arc<ServerState>, name: &str) -> Option<Arc<Session>> {
    let canonical_name = state.session_aliases.lock().unwrap().get(name).cloned();
    let sessions = state.sessions.lock().unwrap();
    sessions
        .get(name)
        .or_else(|| {
            canonical_name
                .as_ref()
                .and_then(|alias| sessions.get(alias))
        })
        .cloned()
}

fn remove_session(state: &Arc<ServerState>, name: &str) -> Option<Arc<Session>> {
    let alias_target = state.session_aliases.lock().unwrap().get(name).cloned();
    let canonical_name = if state.sessions.lock().unwrap().contains_key(name) {
        name.to_string()
    } else {
        alias_target?
    };
    let session = state.sessions.lock().unwrap().remove(&canonical_name);
    state
        .session_aliases
        .lock()
        .unwrap()
        .retain(|alias, target| {
            alias != name && alias != &canonical_name && target != &canonical_name
        });
    session
}

fn cleanup_rename_aliases_if_detached(state: &Arc<ServerState>, session: &Session) {
    if session.attached_count() > 0 {
        return;
    }

    let canonical_name = session.name();
    state
        .session_aliases
        .lock()
        .unwrap()
        .retain(|_, target| target != &canonical_name);
}

fn schedule_rename_alias_cleanup_if_detached(state: &Arc<ServerState>, session: &Arc<Session>) {
    let state = Arc::clone(state);
    let session = Arc::clone(session);
    std::thread::spawn(move || {
        std::thread::sleep(RAW_ATTACH_RECONNECT_ALIAS_GRACE);
        cleanup_rename_aliases_if_detached(&state, &session);
    });
}

fn handle_rename_session(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    old_name: &str,
    new_name: String,
) -> io::Result<()> {
    if let Err(message) = validate_session_name(&new_name) {
        write_err(stream, &message)?;
        return Ok(());
    }
    let requested_name = old_name.to_string();
    let session = {
        let mut aliases = state.session_aliases.lock().unwrap();
        let mut sessions = state.sessions.lock().unwrap();
        let canonical_old_name = if sessions.contains_key(old_name) {
            old_name.to_string()
        } else {
            let Some(alias_target) = aliases.get(old_name).cloned() else {
                write_err(stream, "missing session")?;
                return Ok(());
            };
            if !sessions.contains_key(&alias_target) {
                write_err(stream, "missing session")?;
                return Ok(());
            }
            alias_target
        };
        if aliases.contains_key(&new_name) || sessions.contains_key(&new_name) {
            write_err(stream, "session name already exists")?;
            return Ok(());
        }
        let session = sessions
            .remove(&canonical_old_name)
            .expect("checked canonical session exists");
        session.rename(new_name.clone());
        if session.attached_count() > 0 {
            for target in aliases.values_mut() {
                if target == &canonical_old_name {
                    *target = session.name();
                }
            }
            aliases.insert(canonical_old_name.clone(), session.name());
            aliases.insert(requested_name, session.name());
        } else {
            aliases.retain(|alias, target| {
                alias != &requested_name
                    && alias != &canonical_old_name
                    && target != &canonical_old_name
            });
        }
        sessions.insert(new_name, Arc::clone(&session));
        session
    };
    session.notify_attach_redraw_immediate();
    write_ok(stream)
}

fn client_descriptions(
    state: &Arc<ServerState>,
    session_name: Option<&str>,
) -> Result<Vec<ClientDescription>, String> {
    let mut clients = Vec::new();
    match session_name {
        Some(name) => {
            let Some(session) = resolve_session(state, name) else {
                return Err("missing session".to_string());
            };
            clients.extend(session.client_descriptions());
        }
        None => {
            let sessions = state.sessions.lock().unwrap();
            for session in sessions.values() {
                clients.extend(session.client_descriptions());
            }
        }
    }
    clients.sort_by_key(|client| client.id);
    Ok(clients)
}

fn handle_list_clients(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    session_name: Option<&str>,
    format: Option<&str>,
) -> io::Result<()> {
    let clients = match client_descriptions(state, session_name) {
        Ok(clients) => clients,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    let format = format.unwrap_or("#{client.id}\t#{client.session}\t#{client.type}\t#{client.attached}\t#{client.width}x#{client.height}");
    write_ok(stream)?;
    for client in clients {
        writeln!(stream, "{}", format_client_line(format, &client))?;
    }
    Ok(())
}

fn format_client_line(format: &str, client: &ClientDescription) -> String {
    let id = client.id.to_string();
    let attached = if client.attached { "1" } else { "0" };
    let width = client.width.to_string();
    let height = client.height.to_string();
    let replacements = [
        ("#{client.id}", id.as_str()),
        ("#{client.session}", client.session.as_str()),
        ("#{client.type}", client.client_type.as_str()),
        ("#{client.attached}", attached),
        ("#{client.width}", width.as_str()),
        ("#{client.height}", height.as_str()),
    ];
    apply_replacements(format, &replacements)
}

fn handle_detach_client(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    session_name: Option<&str>,
    client_id: Option<usize>,
) -> io::Result<()> {
    if session_name.is_none() && client_id.is_none() {
        write_err(
            stream,
            "detach-client requires -t <session> or -c <client-id>",
        )?;
        return Ok(());
    }
    let (detached, cleanup_session) = match (session_name, client_id) {
        (Some(name), Some(id)) => {
            let Some(session) = resolve_session(state, name) else {
                write_err(stream, "missing session")?;
                return Ok(());
            };
            (usize::from(session.detach_client(id)), Some(session))
        }
        (Some(name), None) => {
            let Some(session) = resolve_session(state, name) else {
                write_err(stream, "missing session")?;
                return Ok(());
            };
            (session.detach_all_clients(), Some(session))
        }
        (None, Some(id)) => {
            let matches = state
                .sessions
                .lock()
                .unwrap()
                .values()
                .filter(|session| {
                    session
                        .client_descriptions()
                        .iter()
                        .any(|client| client.id == id)
                })
                .cloned()
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                write_err(stream, "ambiguous client id")?;
                return Ok(());
            }
            let Some(session) = matches.first() else {
                write_err(stream, "missing client")?;
                return Ok(());
            };
            (
                usize::from(session.detach_client(id)),
                Some(Arc::clone(session)),
            )
        }
        (None, None) => (0, None),
    };
    if detached == 0 {
        write_err(stream, "missing client")?;
        return Ok(());
    }
    if let Some(session) = cleanup_session {
        session.notify_attach_redraw_immediate();
        cleanup_rename_aliases_if_detached(state, &session);
    }
    write_ok(stream)
}

fn handle_capture(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
    mode: CaptureMode,
    selection: &BufferSelection,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let pane = match session.target_pane(target) {
        Ok(pane) => pane,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };

    let captured = capture_pane_text(&pane, mode);
    let selected = match select_buffer_text(&captured, selection) {
        Ok(text) => text,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    write_ok(stream)?;
    stream.write_all(selected.as_bytes())
}

fn capture_pane_text(pane: &Pane, mode: CaptureMode) -> String {
    let terminal = pane.terminal.lock().unwrap();
    match mode {
        CaptureMode::Screen => terminal.capture_screen_text(),
        CaptureMode::History => terminal.capture_history_text(),
        CaptureMode::All => terminal.capture_text(),
    }
}

fn handle_save_buffer(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
    buffer: Option<&str>,
    mode: CaptureMode,
    selection: &BufferSelection,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let pane = match session.target_pane(target) {
        Ok(pane) => pane,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };

    let captured = capture_pane_text(&pane, mode);
    let selected = match select_buffer_text(&captured, selection) {
        Ok(text) => text,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    let saved_name = match state.buffers.lock().unwrap().save(buffer, selected) {
        Ok(name) => name,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    state.publish_buffer_status();

    write_ok(stream)?;
    writeln!(stream, "{saved_name}")
}

fn handle_save_buffer_text(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    buffer: Option<&str>,
    text: String,
) -> io::Result<()> {
    let session_exists = resolve_session(state, name).is_some();
    if !session_exists {
        write_err(stream, "missing session")?;
        return Ok(());
    }

    let saved_name = match state.buffers.lock().unwrap().save(buffer, text) {
        Ok(name) => name,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    state.publish_buffer_status();

    write_ok(stream)?;
    writeln!(stream, "{saved_name}")
}

fn handle_list_buffers(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    format: Option<&str>,
) -> io::Result<()> {
    let buffers = state.buffers.lock().unwrap().list();

    write_ok(stream)?;
    for buffer in buffers {
        if let Some(format) = format {
            writeln!(stream, "{}", format_buffer_description(format, &buffer))?;
        } else {
            writeln!(
                stream,
                "{}\t{}\t{}",
                buffer.name, buffer.bytes, buffer.preview
            )?;
        }
    }
    Ok(())
}

fn format_buffer_description(format: &str, buffer: &BufferDescription) -> String {
    let index = buffer.index.to_string();
    let bytes = buffer.bytes.to_string();
    let lines = buffer.lines.to_string();
    let latest = if buffer.latest { "1" } else { "0" };
    let replacements = [
        ("#{buffer.index}", index.as_str()),
        ("#{buffer.name}", buffer.name.as_str()),
        ("#{buffer.bytes}", bytes.as_str()),
        ("#{buffer.lines}", lines.as_str()),
        ("#{buffer.latest}", latest),
        ("#{buffer.preview}", buffer.preview.as_str()),
    ];
    apply_replacements(format, &replacements)
}

fn handle_copy_mode(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    mode: CaptureMode,
    search: Option<&str>,
    match_index: Option<usize>,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(pane) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };

    let captured = capture_pane_text(&pane, mode);
    let output = match format_copy_mode_lines(&captured, search, match_index) {
        Ok(output) => output,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    write_ok(stream)?;
    stream.write_all(output.as_bytes())
}

fn handle_paste_buffer(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
    buffer: Option<&str>,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(text) = state
        .buffers
        .lock()
        .unwrap()
        .resolve(buffer)
        .map(|buffer| buffer.text.clone())
    else {
        write_err(stream, "missing buffer")?;
        return Ok(());
    };
    if text.len() > MAX_BUFFER_BYTES {
        write_err(stream, "buffer exceeds maximum size")?;
        return Ok(());
    }
    let pane = match session.target_live_pane(target) {
        Ok(pane) => pane,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };

    let paste_bytes = paste_buffer_bytes(&pane, &text);
    pane.writer.lock().unwrap().write_all(&paste_bytes)?;
    write_ok(stream)
}

fn paste_buffer_bytes(pane: &Pane, text: &str) -> Vec<u8> {
    if !pane.terminal.lock().unwrap().bracketed_paste_enabled() {
        return text.as_bytes().to_vec();
    }

    let mut bytes = Vec::with_capacity(text.len() + "\x1b[200~".len() + "\x1b[201~".len());
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

fn handle_delete_buffer(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    buffer: &str,
) -> io::Result<()> {
    if !state.buffers.lock().unwrap().delete(buffer) {
        write_err(stream, "missing buffer")?;
        return Ok(());
    }
    state.publish_buffer_status();

    write_ok(stream)
}

fn handle_resize(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let size = match PtySize::new(cols, rows) {
        Ok(size) => size,
        Err(err) => {
            write_err(stream, &err.to_string())?;
            return Ok(());
        }
    };
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    if !session.resize_visible_panes(size)? {
        write_err(stream, "missing pane")?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    write_ok(stream)
}

fn handle_resize_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
    direction: PaneResizeDirection,
    amount: usize,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let pane_resizes = match session.resize_pane(target, direction, amount) {
        Ok(pane_resizes) => pane_resizes,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    if !resize_panes(pane_resizes)? {
        write_err(stream, "missing pane")?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    write_ok(stream)
}

fn handle_select_layout(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    session_name: &str,
    window: WindowTarget,
    preset: LayoutPreset,
) -> io::Result<()> {
    let session = resolve_session(state, session_name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let pane_resizes = match session.select_layout(window, preset) {
        Ok(pane_resizes) => pane_resizes,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    if !resize_panes(pane_resizes)? {
        write_err(stream, "missing pane")?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    write_ok(stream)
}

fn handle_send(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
    bytes: &[u8],
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let pane = match session.target_live_pane(target) {
        Ok(pane) => pane,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };

    pane.writer.lock().unwrap().write_all(bytes)?;
    write_ok(stream)
}

fn handle_split(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
    direction: SplitDirection,
    command: Vec<String>,
    cwd: Option<PathBuf>,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let size = match session.next_split_pane_size(target, direction) {
        Ok(size) => size,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    let close_raw_attach_streams = match session.target_is_active_window(target) {
        Ok(true) => session.attach_panes().len() == 1,
        Ok(false) => false,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    let cwd = match cwd {
        Some(cwd) => cwd,
        None => match session.target_pane(target) {
            Ok(pane) => pane.cwd(),
            Err(message) => {
                write_err(stream, &message)?;
                return Ok(());
            }
        },
    };
    let pane = spawn_pane(
        session.next_pane_id(),
        session.name(),
        command,
        cwd,
        size,
        session.attach_event_clients(),
        Some(Arc::downgrade(&session)),
    )?;
    pane.set_session(&session);
    if let Err(message) = session.add_pane(target, direction, Arc::clone(&pane)) {
        terminate_pane_if_running_async(&pane);
        write_err(stream, &message)?;
        return Ok(());
    }
    let pane_resizes = match session.visible_pane_resizes_for_window(target.window.clone()) {
        Ok(pane_resizes) => pane_resizes,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    if !resize_panes(pane_resizes)? {
        write_err(stream, "missing pane")?;
        return Ok(());
    }
    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    if close_raw_attach_streams {
        session.reconnect_raw_attach_streams();
    }
    result
}

fn handle_list_panes(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    window: WindowTarget,
    _format: Option<&str>,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let panes = match session.pane_descriptions_for_window(window) {
        Ok(panes) => panes,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    write_ok(stream)?;
    for pane in panes {
        match _format {
            Some(format) => writeln!(stream, "{}", format_pane_line(format, &pane))?,
            None => writeln!(stream, "{}", pane.index)?,
        }
    }
    Ok(())
}

fn format_pane_line(format: &str, pane: &PaneDescription) -> String {
    let pid = pane.process.pid().to_string();
    let exit_status = match pane.process {
        PaneProcessStatus::Exited {
            exit_status: Some(status),
            ..
        } => status.to_string(),
        _ => String::new(),
    };
    let exit_signal = match pane.process {
        PaneProcessStatus::Exited {
            exit_signal: Some(signal),
            ..
        } => signal.to_string(),
        _ => String::new(),
    };
    let pane_cwd = pane.cwd.to_string_lossy();
    format
        .replace("#{pane.id}", &pane.id.as_usize().to_string())
        .replace("#{pane.index}", &pane.index.to_string())
        .replace("#{pane.active}", if pane.active { "1" } else { "0" })
        .replace("#{pane.zoomed}", if pane.zoomed { "1" } else { "0" })
        .replace("#{pane.state}", pane.process.state())
        .replace("#{pane.pid}", &pid)
        .replace("#{pane.exit_status}", &exit_status)
        .replace("#{pane.exit_signal}", &exit_signal)
        .replace("#{pane.cwd}", pane_cwd.as_ref())
        .replace("#{pane.title}", &pane.title)
        .replace("#{pane.bell}", if pane.bell { "1" } else { "0" })
        .replace("#{pane.activity}", if pane.activity { "1" } else { "0" })
        .replace(
            "#{window.zoomed_flag}",
            if pane.window_zoomed { "1" } else { "0" },
        )
}

fn handle_zoom_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    match session.zoom_pane(target) {
        Ok(_) => {}
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    session.resize_current_visible_panes()?;

    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    session.reconnect_live_raw_pane_streams();
    result
}

fn handle_status_line(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    format: Option<&str>,
) -> io::Result<()> {
    let context = status_context(state, name);
    let Some(context) = context else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    let format = format.unwrap_or("#{session.name} #{window.list} pane #{pane.index}");
    writeln!(stream, "{}", format_status_line(format, &context))
}

fn handle_display_message(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    format: &str,
) -> io::Result<()> {
    let context = status_context(state, name);
    let Some(context) = context else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let rendered = format_status_line(format, &context);
    if let Some(session) = resolve_session(state, name) {
        session.set_transient_message(rendered.clone());
    }
    write_ok(stream)?;
    writeln!(stream, "{rendered}")
}

fn handle_list_keys(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    format: Option<&str>,
) -> io::Result<()> {
    let mut bindings = state
        .key_bindings
        .lock()
        .unwrap()
        .iter()
        .map(|(key, command)| KeyBinding {
            key: key.clone(),
            command: command.clone(),
        })
        .collect::<Vec<_>>();
    bindings.sort_by(|left, right| left.key.cmp(&right.key));
    let format = format.unwrap_or("#{key}\t#{command}");
    write_ok(stream)?;
    for binding in bindings {
        writeln!(stream, "{}", format_key_binding(format, &binding))?;
    }
    Ok(())
}

fn handle_bind_key(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    key: &str,
    command: &str,
) -> io::Result<()> {
    let key = match crate::config::canonical_key(key) {
        Ok(key) => key,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    let command = match crate::config::validate_binding_command(command) {
        Ok(command) => command,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    state.key_bindings.lock().unwrap().insert(key, command);
    notify_all_attach_redraws(state);
    write_ok(stream)
}

fn handle_unbind_key(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    key: &str,
) -> io::Result<()> {
    let key = match crate::config::canonical_key(key) {
        Ok(key) => key,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    state.key_bindings.lock().unwrap().remove(&key);
    notify_all_attach_redraws(state);
    write_ok(stream)
}

fn handle_show_options(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    format: Option<&str>,
) -> io::Result<()> {
    let mut options = state
        .options
        .lock()
        .unwrap()
        .iter()
        .map(|(name, value)| OptionEntry {
            name: name.clone(),
            value: value.clone(),
        })
        .collect::<Vec<_>>();
    options.sort_by(|left, right| left.name.cmp(&right.name));
    let format = format.unwrap_or("#{option.name}\t#{option.value}");
    write_ok(stream)?;
    for option in options {
        writeln!(stream, "{}", format_option(format, &option))?;
    }
    Ok(())
}

fn handle_set_option(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    value: &str,
) -> io::Result<()> {
    let old_prefix = if name == crate::config::OPTION_PREFIX {
        Some(current_runtime_options(state).0)
    } else {
        None
    };
    let value = match crate::config::validate_option_value(name, value) {
        Ok(value) => value,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    state
        .options
        .lock()
        .unwrap()
        .insert(name.to_string(), value);
    if let Some(old_prefix) = old_prefix {
        sync_send_prefix_binding(state, &old_prefix);
    }
    apply_runtime_options_to_sessions(state);
    write_ok(stream)
}

fn sync_send_prefix_binding(state: &Arc<ServerState>, old_prefix: &str) {
    let new_prefix = current_runtime_options(state).0;
    let mut bindings = state.key_bindings.lock().unwrap();
    if bindings
        .get(old_prefix)
        .is_some_and(|command| command == "send-prefix")
    {
        bindings.remove(old_prefix);
        bindings.insert(new_prefix, "send-prefix".to_string());
    }
}

fn notify_all_attach_redraws(state: &Arc<ServerState>) {
    let sessions = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for session in sessions {
        session.notify_attach_redraw_immediate();
    }
}

fn current_runtime_options(state: &Arc<ServerState>) -> (String, bool) {
    let options = state.options.lock().unwrap();
    let prefix = options
        .get(crate::config::OPTION_PREFIX)
        .cloned()
        .unwrap_or_else(|| crate::config::DEFAULT_PREFIX_KEY.to_string());
    let status_hints = options
        .get(crate::config::OPTION_STATUS_HINTS)
        .is_none_or(|value| value == "on");
    (prefix, status_hints)
}

fn apply_runtime_options_to_sessions(state: &Arc<ServerState>) {
    let (prefix, status_hints) = current_runtime_options(state);
    let sessions = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for session in sessions {
        session.set_runtime_options(prefix.clone(), status_hints);
        session.notify_attach_redraw_immediate();
    }
}

fn format_key_binding(format: &str, binding: &KeyBinding) -> String {
    apply_replacements(
        format,
        &[
            ("#{key}", binding.key.as_str()),
            ("#{command}", binding.command.as_str()),
        ],
    )
}

fn format_option(format: &str, option: &OptionEntry) -> String {
    apply_replacements(
        format,
        &[
            ("#{option.name}", option.name.as_str()),
            ("#{option.value}", option.value.as_str()),
        ],
    )
}

fn status_context(state: &Arc<ServerState>, name: &str) -> Option<StatusContext> {
    let session = resolve_session(state, name)?;
    let mut context = session.status_context()?;
    context.buffer_summary = state.buffer_status_summary();
    context.status_hints = state
        .options
        .lock()
        .unwrap()
        .get(crate::config::OPTION_STATUS_HINTS)
        .is_none_or(|value| value == "on");
    context.prefix_key = state
        .options
        .lock()
        .unwrap()
        .get(crate::config::OPTION_PREFIX)
        .cloned()
        .unwrap_or_else(|| crate::config::DEFAULT_PREFIX_KEY.to_string());
    Some(context)
}

fn format_status_line(format: &str, context: &StatusContext) -> String {
    let session_attached_count = context.session_attached_count.to_string();
    let session_attached = if context.session_attached_count > 0 {
        "1"
    } else {
        "0"
    };
    let session_created_at = context.session_created_at.to_string();
    let window_index = context.window_index.to_string();
    let window_id = context.window_id.as_usize().to_string();
    let window_count = context.window_count.to_string();
    let window_list = format_window_list(context);
    let pane_index = context.pane_index.to_string();
    let pane_id = context.pane_id.as_usize().to_string();
    let pane_zoomed = if context.pane_zoomed { "1" } else { "0" };
    let window_zoomed = if context.window_zoomed { "1" } else { "0" };
    let pane_cwd = context.pane_cwd.to_string_lossy();
    let pane_pid = context.pane_process.pid().to_string();
    let pane_bell = if context.pane_bell { "1" } else { "0" };
    let pane_activity = if context.pane_activity { "1" } else { "0" };
    let pane_exit_status = match context.pane_process {
        PaneProcessStatus::Exited {
            exit_status: Some(status),
            ..
        } => status.to_string(),
        _ => String::new(),
    };
    let pane_exit_signal = match context.pane_process {
        PaneProcessStatus::Exited {
            exit_signal: Some(signal),
            ..
        } => signal.to_string(),
        _ => String::new(),
    };
    let buffer_count = context.buffer_summary.count.to_string();
    let (buffer_index, buffer_name, buffer_bytes, buffer_lines, buffer_latest, buffer_preview) =
        if let Some(buffer) = &context.buffer_summary.latest {
            (
                buffer.index.to_string(),
                buffer.name.as_str(),
                buffer.bytes.to_string(),
                buffer.lines.to_string(),
                "1",
                buffer.preview.as_str(),
            )
        } else {
            (String::new(), "", String::new(), String::new(), "0", "")
        };
    let status_help = if context.status_hints {
        format!(
            "prefix {} | {} ? help | : command | [ copy",
            context.prefix_key, context.prefix_key
        )
    } else {
        String::new()
    };
    let replacements = [
        ("#{session.name}", context.session_name.as_str()),
        ("#{session.attached}", session_attached),
        ("#{session.attached_count}", session_attached_count.as_str()),
        ("#{session.created_at}", session_created_at.as_str()),
        ("#{client.count}", session_attached_count.as_str()),
        ("#{window.id}", window_id.as_str()),
        ("#{window.index}", window_index.as_str()),
        ("#{window.name}", context.window_name.as_str()),
        ("#{window.active}", "1"),
        ("#{window.count}", window_count.as_str()),
        ("#{window.list}", window_list.as_str()),
        ("#{tab.index}", window_index.as_str()),
        ("#{tab.id}", window_id.as_str()),
        ("#{tab.name}", context.window_name.as_str()),
        ("#{tab.active}", "1"),
        ("#{tab.count}", window_count.as_str()),
        ("#{tab.list}", window_list.as_str()),
        ("#{pane.id}", pane_id.as_str()),
        ("#{pane.index}", pane_index.as_str()),
        ("#{pane.zoomed}", pane_zoomed),
        ("#{pane.state}", context.pane_process.state()),
        ("#{pane.pid}", pane_pid.as_str()),
        ("#{pane.exit_status}", pane_exit_status.as_str()),
        ("#{pane.exit_signal}", pane_exit_signal.as_str()),
        ("#{pane.cwd}", pane_cwd.as_ref()),
        ("#{pane.title}", context.pane_title.as_str()),
        ("#{pane.bell}", pane_bell),
        ("#{pane.activity}", pane_activity),
        ("#{buffer.count}", buffer_count.as_str()),
        ("#{buffer.index}", buffer_index.as_str()),
        ("#{buffer.name}", buffer_name),
        ("#{buffer.bytes}", buffer_bytes.as_str()),
        ("#{buffer.lines}", buffer_lines.as_str()),
        ("#{buffer.latest}", buffer_latest),
        ("#{buffer.preview}", buffer_preview),
        ("#{window.zoomed_flag}", window_zoomed),
        ("#{status.help}", status_help.as_str()),
    ];
    apply_replacements(format, &replacements)
}

fn apply_replacements(format: &str, replacements: &[(&str, &str)]) -> String {
    let mut output = String::with_capacity(format.len());
    let mut remaining = format;

    while !remaining.is_empty() {
        if let Some((token, value)) = replacements
            .iter()
            .find(|(token, _)| remaining.starts_with(*token))
        {
            output.push_str(value);
            remaining = &remaining[token.len()..];
        } else {
            let ch = remaining
                .chars()
                .next()
                .expect("non-empty string must contain a character");
            output.push(ch);
            remaining = &remaining[ch.len_utf8()..];
        }
    }

    output
}

fn format_window_list(context: &StatusContext) -> String {
    (0..context.window_count)
        .map(|index| {
            if index == context.window_index {
                format!("[{index}]")
            } else {
                index.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn handle_select_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    window: WindowTarget,
    target: PaneSelectTarget,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    match session.select_pane(window, target) {
        Ok(_) => {}
        Err(error) => {
            write_err(stream, &error)?;
            return Ok(());
        }
    };
    session.resize_current_visible_panes()?;

    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    session.reconnect_live_raw_pane_streams();
    result
}

fn handle_kill_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let removed = match session.kill_pane(target) {
        Ok(removed) => removed,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    let pane_resizes = match session.visible_pane_resizes_for_window(target.window.clone()) {
        Ok(pane_resizes) => pane_resizes,
        Err(message) => {
            terminate_pane_if_running_async(&removed);
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    if let Err(error) = resize_panes(pane_resizes) {
        terminate_pane_if_running_async(&removed);
        write_err(stream, &error.to_string())?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    terminate_pane_if_running_async(&removed);
    write_ok(stream)
}

fn handle_swap_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    source: &Target,
    destination: &Target,
) -> io::Result<()> {
    if source.session != destination.session {
        write_err(
            stream,
            "swap-pane requires source and destination in the same session",
        )?;
        return Ok(());
    }
    let session = resolve_session(state, &source.session);
    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    if let Err(message) = session.swap_panes(source, destination) {
        write_err(stream, &message)?;
        return Ok(());
    }
    let pane_resizes = match session.visible_pane_resizes_for_window(source.window.clone()) {
        Ok(pane_resizes) => pane_resizes,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    if !resize_panes(pane_resizes)? {
        write_err(stream, "missing pane")?;
        return Ok(());
    }
    let swapped_active = match session.window_is_active(source.window.clone()) {
        Ok(swapped_active) => swapped_active,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };

    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    if swapped_active {
        session.reconnect_live_raw_pane_streams();
    }
    result
}

fn handle_move_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    source: &Target,
    destination: &Target,
    direction: SplitDirection,
) -> io::Result<()> {
    if source.session != destination.session {
        write_err(
            stream,
            "pane movement requires source and destination in the same session",
        )?;
        return Ok(());
    }
    let session = resolve_session(state, &source.session);
    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let pane_resizes = match session.move_pane(source, destination, direction) {
        Ok(pane_resizes) => pane_resizes,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    if let Err(error) = resize_panes(pane_resizes) {
        write_err(stream, &error.to_string())?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    session.reconnect_raw_attach_streams();
    result
}

fn handle_break_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);
    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let pane_resizes = match session.break_pane(target) {
        Ok(pane_resizes) => pane_resizes,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    if let Err(error) = resize_panes(pane_resizes) {
        write_err(stream, &error.to_string())?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    session.reconnect_raw_attach_streams();
    result
}

fn handle_respawn_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    target: &Target,
    force: bool,
    command: Vec<String>,
    cwd: Option<PathBuf>,
) -> io::Result<()> {
    let session = resolve_session(state, &target.session);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let old_pid = match session.respawn_pane(target, command, force, cwd) {
        Ok(old_pid) => old_pid,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    if let Some(pid) = old_pid {
        terminate_pane_async(pid);
    }
    write_ok(stream)
}

fn handle_new_window(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    command: Vec<String>,
    cwd: Option<PathBuf>,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(size) = session.active_window_size() else {
        write_err(stream, "missing window")?;
        return Ok(());
    };

    let cwd = match cwd {
        Some(cwd) => cwd,
        None => match session.active_pane() {
            Some(pane) => pane.cwd(),
            None => {
                write_err(stream, "missing pane")?;
                return Ok(());
            }
        },
    };
    let pane = spawn_pane(
        session.next_pane_id(),
        session.name(),
        command,
        cwd,
        size,
        session.attach_event_clients(),
        Some(Arc::downgrade(&session)),
    )?;
    pane.set_session(&session);
    session.add_window(pane);
    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    session.reconnect_raw_attach_streams();
    result
}

fn handle_list_windows(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    format: Option<&str>,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    for window in session.window_descriptions() {
        match format {
            Some(format) => writeln!(stream, "{}", format_window_line(format, &window))?,
            None => writeln!(stream, "{}", window.index)?,
        }
    }
    Ok(())
}

fn format_window_line(format: &str, window: &WindowDescription) -> String {
    let index = window.index.to_string();
    let id = window.id.as_usize().to_string();
    let active = if window.active { "1" } else { "0" };
    let panes = window.panes.to_string();
    let replacements = [
        ("#{window.index}", index.as_str()),
        ("#{window.id}", id.as_str()),
        ("#{window.name}", window.name.as_str()),
        ("#{window.active}", active),
        ("#{window.panes}", panes.as_str()),
        ("#{tab.index}", index.as_str()),
        ("#{tab.id}", id.as_str()),
        ("#{tab.name}", window.name.as_str()),
        ("#{tab.active}", active),
        ("#{tab.panes}", panes.as_str()),
    ];
    apply_replacements(format, &replacements)
}

fn handle_select_window(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    target: WindowTarget,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    if let Err(message) = session.select_window(target) {
        write_err(stream, &message)?;
        return Ok(());
    }
    if !session.resize_current_visible_panes()? {
        write_err(stream, "missing pane")?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    session.reconnect_raw_attach_streams();
    result
}

fn handle_rename_window(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    target: WindowTarget,
    new_name: String,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    if let Err(message) = session.rename_window(target, new_name) {
        write_err(stream, &message)?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    write_ok(stream)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowCycle {
    Next,
    Previous,
}

fn handle_cycle_window(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    cycle: WindowCycle,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let result = match cycle {
        WindowCycle::Next => session.select_next_window(),
        WindowCycle::Previous => session.select_previous_window(),
    };
    if let Err(message) = result {
        write_err(stream, &message)?;
        return Ok(());
    }
    if !session.resize_current_visible_panes()? {
        write_err(stream, "missing pane")?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    session.reconnect_raw_attach_streams();
    result
}

fn handle_kill_window(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    target: WindowTarget,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    let reconnect_live_raw = match session.window_is_active(target.clone()) {
        Ok(is_active) => is_active && !has_attach_pane_snapshot(&session),
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };

    if let Err(message) = session.kill_window(target) {
        write_err(stream, &message)?;
        return Ok(());
    }
    if !session.resize_current_visible_panes()? {
        write_err(stream, "missing pane")?;
        return Ok(());
    }

    session.notify_attach_redraw_immediate();
    let result = write_ok(stream);
    if reconnect_live_raw {
        session.reconnect_raw_attach_streams();
    }
    result
}

fn handle_kill(state: &Arc<ServerState>, stream: &mut UnixStream, name: &str) -> io::Result<()> {
    let session = remove_session(state, name);
    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    session.close_attach_streams();
    for pane in session.panes() {
        terminate_pane_if_running_async(&pane);
    }
    write_ok(stream)
}

fn handle_kill_server(state: &Arc<ServerState>, stream: &mut UnixStream) -> io::Result<()> {
    let sessions = state
        .sessions
        .lock()
        .unwrap()
        .drain()
        .map(|(_, session)| session)
        .collect::<Vec<_>>();
    let mut child_pids = Vec::new();
    for session in sessions {
        session.close_attach_streams();
        for pane in session.panes() {
            if pane.is_running() {
                child_pids.push(pane.child_pid);
            }
        }
    }

    let result = write_ok(stream).and_then(|_| stream.flush());
    let _ = std::fs::remove_file(&state.socket_path);
    exit_after_kill_server_cleanup(child_pids);
    result
}

fn exit_after_kill_server_cleanup(child_pids: Vec<i32>) {
    std::thread::spawn(move || {
        terminate_child_pids_concurrently(child_pids);
        std::process::exit(0);
    });
}

fn terminate_child_pids_concurrently(child_pids: Vec<i32>) {
    let handles = child_pids
        .into_iter()
        .map(|pid| {
            std::thread::spawn(move || {
                let _ = pty::terminate(pid);
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        let _ = handle.join();
    }
}

fn handle_attach(state: &Arc<ServerState>, mut stream: UnixStream, name: &str) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(&mut stream, "missing session")?;
        return Ok(());
    };
    if session.active_pane().is_none() {
        write_err(&mut stream, "missing pane")?;
        return Ok(());
    }

    let client_id = state.next_client_id.fetch_add(1, Ordering::SeqCst);
    let raw_layout_epoch = session.raw_attach_layout_epoch();
    let _attach_registration = session.register_attach_stream(&stream, client_id)?;
    session.notify_attach_redraw_immediate();
    let result = if has_attach_pane_snapshot(&session) {
        write_attach_snapshot_ok(&mut stream)?;
        forward_multi_pane_attach_input(&session, &mut stream)
    } else {
        let Some(pane) = session.active_live_pane() else {
            write_err(&mut stream, "pane is not running")?;
            return Ok(());
        };
        write_attach_live_ok(&mut stream, session.raw_attach_layout_epoch())?;
        {
            let history = pane.raw_history.lock().unwrap();
            stream.write_all(&history)?;
        }
        let _pane_client_registration = pane.register_client(&stream)?;

        let mut buf = [0_u8; 8192];
        loop {
            let n = stream.read(&mut buf)?;
            if n == 0 {
                break;
            }
            pane.writer.lock().unwrap().write_all(&buf[..n])?;
        }
        Ok(())
    };
    drop(_attach_registration);
    session.notify_attach_redraw_immediate();
    if session.raw_attach_layout_epoch() == raw_layout_epoch {
        cleanup_rename_aliases_if_detached(state, &session);
    } else {
        schedule_rename_alias_cleanup_if_detached(state, &session);
    }
    result
}

fn handle_attach_raw_state(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    writeln!(
        stream,
        "RAW_LAYOUT_EPOCH\t{}",
        session.raw_attach_layout_epoch()
    )
}

fn has_attach_pane_snapshot(session: &Session) -> bool {
    session.attach_panes().len() > 1 || session.active_live_pane().is_none()
}

fn write_attach_live_ok(stream: &mut UnixStream, raw_layout_epoch: u64) -> io::Result<()> {
    writeln!(stream, "OK\tLIVE\t{raw_layout_epoch}")
}

fn write_attach_snapshot_ok(stream: &mut UnixStream) -> io::Result<()> {
    stream.write_all(b"OK\tSNAPSHOT\n")
}

fn forward_multi_pane_attach_input(
    session: &Arc<Session>,
    stream: &mut UnixStream,
) -> io::Result<()> {
    let mut buf = [0_u8; 8192];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }

        let Some(pane) = session.active_live_pane() else {
            stream.write_all(b"pane is not running\r\n")?;
            continue;
        };
        pane.writer.lock().unwrap().write_all(&buf[..n])?;
    }

    Ok(())
}

fn handle_attach_snapshot(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    if let Some(snapshot) = attach_pane_snapshot(&session) {
        stream.write_all(snapshot.as_bytes())?;
    }
    Ok(())
}

fn handle_attach_layout_snapshot(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    if let Some(snapshot) = attach_pane_snapshot_with_regions(&session) {
        stream.write_all(&format_attach_layout_snapshot_body(&snapshot))?;
    }
    Ok(())
}

fn handle_attach_layout_frame(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    if let Some(frame) = attach_pane_frame_with_regions(&session) {
        stream.write_all(&format_attach_layout_snapshot_body(&frame))?;
    }
    Ok(())
}

fn handle_attach_events(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    let client_id = state.next_client_id.fetch_add(1, Ordering::SeqCst);
    let Some(_registration) = session.register_attach_event_stream(stream, client_id)? else {
        return Ok(());
    };
    wait_for_attach_event_client_close(stream);
    Ok(())
}

fn handle_attach_render(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
) -> io::Result<()> {
    let session = resolve_session(state, name);

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    stream.write_all(ATTACH_RENDER_RESPONSE)?;
    let client_id = state.next_client_id.fetch_add(1, Ordering::SeqCst);
    let Some(_registration) = session.register_attach_render_stream(stream, client_id)? else {
        return Ok(());
    };
    wait_for_attach_event_client_close(stream);
    Ok(())
}

fn attach_pane_snapshot(session: &Session) -> Option<String> {
    attach_pane_snapshot_with_regions(session).map(|snapshot| snapshot.text)
}

fn attach_pane_snapshot_with_regions(session: &Session) -> Option<RenderedAttachSnapshot> {
    let snapshot = session.attach_layout_snapshot();
    if snapshot.panes.is_empty() {
        return None;
    }

    let panes = snapshot
        .panes
        .into_iter()
        .map(|pane| PaneSnapshot {
            index: pane.index,
            screen: capture_pane_text(&pane.pane, CaptureMode::Screen),
        })
        .collect::<Vec<_>>();

    Some(render_attach_pane_snapshot_with_regions_for_size(
        &snapshot.layout,
        &panes,
        snapshot.size,
    ))
}

fn attach_pane_frame_with_regions(session: &Session) -> Option<RenderedAttachSnapshot> {
    let snapshot = session.attach_layout_snapshot();
    if snapshot.panes.is_empty() {
        return None;
    }

    let terminal_guards = snapshot
        .panes
        .iter()
        .map(|pane| (pane.index, pane.pane.terminal.lock().unwrap()))
        .collect::<Vec<_>>();
    let panes = terminal_guards
        .iter()
        .map(|(index, terminal)| PaneRenderSnapshot {
            index: *index,
            terminal,
        })
        .collect::<Vec<_>>();

    render_attach_frame_for_size_with_active(
        &snapshot.layout,
        &panes,
        snapshot.size,
        Some(snapshot.active_pane_index),
    )
}

#[allow(dead_code)]
fn render_attach_pane_snapshot(layout: &LayoutNode, panes: &[PaneSnapshot]) -> String {
    render_attach_pane_snapshot_with_regions(layout, panes).text
}

fn render_attach_pane_snapshot_with_regions(
    layout: &LayoutNode,
    panes: &[PaneSnapshot],
) -> RenderedAttachSnapshot {
    match render_attach_layout(layout, panes) {
        Some(rendered) => RenderedAttachSnapshot {
            text: render_client_lines(&rendered.lines),
            regions: rendered.regions,
            cursor: None,
        },
        None => RenderedAttachSnapshot {
            text: render_ordered_pane_sections(panes),
            regions: Vec::new(),
            cursor: None,
        },
    }
}

fn render_attach_pane_snapshot_with_regions_for_size(
    layout: &LayoutNode,
    panes: &[PaneSnapshot],
    size: PtySize,
) -> RenderedAttachSnapshot {
    match render_attach_layout_for_size(layout, panes, size) {
        Some(rendered) => RenderedAttachSnapshot {
            text: render_client_lines(&rendered.lines),
            regions: rendered.regions,
            cursor: None,
        },
        None => RenderedAttachSnapshot {
            text: render_ordered_pane_sections(panes),
            regions: Vec::new(),
            cursor: None,
        },
    }
}

fn format_attach_layout_snapshot_body(snapshot: &RenderedAttachSnapshot) -> Vec<u8> {
    let mut output = String::new();
    output.push_str("REGIONS\t");
    output.push_str(&snapshot.regions.len().to_string());
    output.push('\n');
    for region in &snapshot.regions {
        output.push_str("REGION\t");
        output.push_str(&region.pane.to_string());
        output.push('\t');
        output.push_str(&region.row_start.to_string());
        output.push('\t');
        output.push_str(&region.row_end.to_string());
        output.push('\t');
        output.push_str(&region.col_start.to_string());
        output.push('\t');
        output.push_str(&region.col_end.to_string());
        output.push('\n');
    }
    output.push_str("SNAPSHOT\t");
    output.push_str(&snapshot.text.as_bytes().len().to_string());
    output.push('\n');

    let mut bytes = output.into_bytes();
    bytes.extend_from_slice(snapshot.text.as_bytes());
    bytes
}

fn format_attach_render_stream_frame(session: &Session) -> Option<Vec<u8>> {
    let context = session.status_context()?;
    let status = format_status_line(ATTACH_RENDER_STATUS_FORMAT, &context);
    let message = context.transient_message.as_deref();
    let size = session.active_window_size();
    let width = size.map(|size| usize::from(size.cols));
    let max_header_rows = size.map(max_attach_render_header_rows);
    let header_lines = cap_attach_render_header_lines(
        attach_render_header_lines(&status, message, width),
        max_header_rows,
        width,
    );
    let header_rows = header_lines.len();
    let snapshot_rows = size
        .map(|size| usize::from(size.rows).saturating_sub(header_rows))
        .unwrap_or(usize::MAX);
    let snapshot = attach_pane_frame_with_regions(session)?;
    Some(format_attach_render_frame_body_with_header_lines(
        &header_lines,
        &snapshot,
        snapshot_rows,
    ))
}

#[cfg(test)]
fn format_attach_render_frame_body(
    status: &str,
    message: Option<&str>,
    snapshot: &RenderedAttachSnapshot,
    snapshot_rows: usize,
) -> Vec<u8> {
    let header_lines = attach_render_header_lines(status, message, None);
    format_attach_render_frame_body_with_header_lines(&header_lines, snapshot, snapshot_rows)
}

fn format_attach_render_frame_body_with_header_lines(
    header_lines: &[String],
    snapshot: &RenderedAttachSnapshot,
    snapshot_rows: usize,
) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(CURSOR_HOME);
    for line in header_lines {
        write_render_output_line(&mut output, line.as_bytes(), true);
    }
    write_render_output_rows(&mut output, snapshot.text.as_bytes(), snapshot_rows);
    if let Some(cursor) = snapshot.cursor {
        write_cursor_position(&mut output, cursor, header_lines.len(), snapshot_rows);
    }

    let mut header = String::new();
    header.push_str("HEADER_ROWS\t");
    header.push_str(&header_lines.len().to_string());
    header.push('\n');
    header.push_str("REGIONS\t");
    header.push_str(&snapshot.regions.len().to_string());
    header.push('\n');
    for region in &snapshot.regions {
        header.push_str("REGION\t");
        header.push_str(&region.pane.to_string());
        header.push('\t');
        header.push_str(&region.row_start.to_string());
        header.push('\t');
        header.push_str(&region.row_end.to_string());
        header.push('\t');
        header.push_str(&region.col_start.to_string());
        header.push('\t');
        header.push_str(&region.col_end.to_string());
        header.push('\n');
    }
    header.push_str("OUTPUT\t");
    header.push_str(&output.len().to_string());
    header.push('\n');

    let mut bytes = header.into_bytes();
    bytes.extend_from_slice(&output);
    bytes
}

fn max_attach_render_header_rows(size: PtySize) -> usize {
    let rows = usize::from(size.rows);
    if rows > 1 { rows - 1 } else { rows }
}

fn attach_render_header_lines(
    status: &str,
    message: Option<&str>,
    width: Option<usize>,
) -> Vec<String> {
    let mut lines = Vec::new();
    if !status.is_empty() {
        lines.push(truncate_attach_render_header_line(status, width));
    }
    if let Some(message) = message {
        for line in message.lines() {
            lines.push(truncate_attach_render_header_line(line, width));
        }
    }
    lines
}

fn cap_attach_render_header_lines(
    mut lines: Vec<String>,
    max_rows: Option<usize>,
    width: Option<usize>,
) -> Vec<String> {
    let Some(max_rows) = max_rows else {
        return lines;
    };
    if lines.len() <= max_rows {
        return lines;
    }
    if max_rows == 0 {
        return Vec::new();
    }

    let omitted = lines.len() - max_rows + 1;
    lines.truncate(max_rows);
    lines[max_rows - 1] =
        truncate_attach_render_header_line(&format!("... {omitted} more lines"), width);
    lines
}

fn truncate_attach_render_header_line(line: &str, width: Option<usize>) -> String {
    let line = line.trim_end_matches('\r');
    let Some(width) = width else {
        return line.to_string();
    };
    if width == 0 {
        return String::new();
    }
    if display_cell_width(line) <= width {
        return line.to_string();
    }
    if width <= 3 {
        return take_cells(line, width);
    }
    let mut output = take_cells(line, width - 3);
    output.push_str("...");
    output
}

fn display_cell_width(line: &str) -> usize {
    line.chars().map(char_cell_width).sum()
}

fn take_cells(line: &str, max_cells: usize) -> String {
    let mut output = String::new();
    let mut used = 0;
    for ch in line.chars() {
        let width = char_cell_width(ch);
        if used + width > max_cells {
            break;
        }
        output.push(ch);
        used += width;
    }
    output
}

fn char_cell_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(1).max(1)
}

fn write_cursor_position(
    output: &mut Vec<u8>,
    cursor: RenderedCursor,
    header_rows: usize,
    snapshot_rows: usize,
) {
    if snapshot_rows == 0 {
        return;
    }
    let row = header_rows + cursor.row.min(snapshot_rows - 1) + 1;
    let col = cursor.col + 1;
    output.extend_from_slice(if cursor.visible {
        SHOW_CURSOR
    } else {
        HIDE_CURSOR
    });
    output.extend_from_slice(format!("\x1b[{row};{col}H").as_bytes());
}

fn write_render_output_line(output: &mut Vec<u8>, line: &[u8], clear_line: bool) {
    if clear_line {
        output.extend_from_slice(CLEAR_LINE);
    }
    output.extend_from_slice(line);
    output.extend_from_slice(b"\r\n");
}

fn write_render_output_rows(output: &mut Vec<u8>, snapshot: &[u8], max_rows: usize) {
    if max_rows == 0 {
        return;
    }

    let mut rows = 0;
    let mut start = 0;
    while rows < max_rows {
        if rows > 0 {
            output.extend_from_slice(b"\r\n");
        }
        output.extend_from_slice(CLEAR_LINE);

        if start < snapshot.len() {
            let line_end = snapshot[start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(snapshot.len(), |offset| start + offset);
            let content_end = if line_end > start && snapshot[line_end - 1] == b'\r' {
                line_end - 1
            } else {
                line_end
            };
            output.extend_from_slice(&snapshot[start..content_end]);
            start = line_end.saturating_add(1);
        }
        rows += 1;
    }
}

fn write_attach_render_frame(stream: &mut UnixStream, body: &[u8]) -> bool {
    let header = format!("FRAME\t{}\n", body.len());
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(body))
        .is_ok()
}

fn render_attach_layout(
    layout: &LayoutNode,
    panes: &[PaneSnapshot],
) -> Option<RenderedAttachLayout> {
    if !layout_matches_panes(layout, panes) {
        return None;
    }

    let screens = panes
        .iter()
        .map(|pane| (pane.index, pane.screen.as_str()))
        .collect::<HashMap<_, _>>();

    render_layout(layout, &screens)
}

fn render_attach_layout_for_size(
    layout: &LayoutNode,
    panes: &[PaneSnapshot],
    size: PtySize,
) -> Option<RenderedAttachLayout> {
    if !layout_matches_panes(layout, panes) {
        return None;
    }

    let screens = panes
        .iter()
        .map(|pane| (pane.index, pane.screen.as_str()))
        .collect::<HashMap<_, _>>();

    render_sized_layout(
        layout,
        &screens,
        0,
        size.rows as usize,
        0,
        size.cols as usize,
    )
}

#[allow(dead_code)]
fn render_attach_frame_for_size(
    layout: &LayoutNode,
    panes: &[PaneRenderSnapshot<'_>],
    size: PtySize,
) -> Option<RenderedAttachSnapshot> {
    render_attach_frame_for_size_with_active(layout, panes, size, None)
}

fn render_attach_frame_for_size_with_active(
    layout: &LayoutNode,
    panes: &[PaneRenderSnapshot<'_>],
    size: PtySize,
    active_pane: Option<usize>,
) -> Option<RenderedAttachSnapshot> {
    if !layout_matches_render_panes(layout, panes) {
        return None;
    }

    let terminals = panes
        .iter()
        .map(|pane| (pane.index, pane.terminal))
        .collect::<HashMap<_, _>>();

    let rendered = render_styled_sized_layout(
        layout,
        &terminals,
        0,
        size.rows as usize,
        0,
        size.cols as usize,
    )?;
    let cursor = rendered_cursor_for_active_pane(&rendered.regions, &terminals, active_pane);
    Some(RenderedAttachSnapshot {
        text: render_client_lines(&rendered.lines),
        regions: rendered.regions,
        cursor,
    })
}

fn rendered_cursor_for_active_pane(
    regions: &[PaneRegion],
    terminals: &HashMap<usize, &TerminalState>,
    active_pane: Option<usize>,
) -> Option<RenderedCursor> {
    let active_pane = active_pane?;
    let region = regions.iter().find(|region| region.pane == active_pane)?;
    let width = region.col_end.checked_sub(region.col_start)?;
    let height = region.row_end.checked_sub(region.row_start)?;
    if width == 0 || height == 0 {
        return None;
    }

    let terminal = terminals.get(&active_pane)?;
    let (row, col) = terminal.cursor_position();
    Some(RenderedCursor {
        row: region.row_start + row.min(height - 1),
        col: region.col_start + col.min(width - 1),
        visible: terminal.cursor_visible(),
    })
}

fn layout_matches_panes(layout: &LayoutNode, panes: &[PaneSnapshot]) -> bool {
    let mut layout_indexes = Vec::new();
    collect_layout_pane_indexes(layout, &mut layout_indexes);
    layout_indexes.sort_unstable();

    let mut pane_indexes = panes.iter().map(|pane| pane.index).collect::<Vec<_>>();
    pane_indexes.sort_unstable();

    layout_indexes == pane_indexes
}

fn layout_matches_render_panes(layout: &LayoutNode, panes: &[PaneRenderSnapshot<'_>]) -> bool {
    let mut layout_indexes = Vec::new();
    collect_layout_pane_indexes(layout, &mut layout_indexes);
    layout_indexes.sort_unstable();

    let mut pane_indexes = panes.iter().map(|pane| pane.index).collect::<Vec<_>>();
    pane_indexes.sort_unstable();

    layout_indexes == pane_indexes
}

fn collect_layout_pane_indexes(layout: &LayoutNode, indexes: &mut Vec<usize>) {
    match layout {
        LayoutNode::Pane(index) => indexes.push(*index),
        LayoutNode::Split { first, second, .. } => {
            collect_layout_pane_indexes(first, indexes);
            collect_layout_pane_indexes(second, indexes);
        }
    }
}

fn render_layout(
    layout: &LayoutNode,
    screens: &HashMap<usize, &str>,
) -> Option<RenderedAttachLayout> {
    match layout {
        LayoutNode::Pane(index) => {
            let lines = screen_lines(screens.get(index)?);
            let width = max_line_width(&lines);
            let height = lines.len().max(1);
            Some(RenderedAttachLayout {
                lines,
                regions: vec![PaneRegion {
                    pane: *index,
                    row_start: 0,
                    row_end: height,
                    col_start: 0,
                    col_end: width,
                }],
            })
        }
        LayoutNode::Split {
            direction,
            first,
            second,
            ..
        } => {
            let first = render_layout(first, screens)?;
            let second = render_layout(second, screens)?;
            Some(match direction {
                SplitDirection::Horizontal => join_horizontal_layout(first, second),
                SplitDirection::Vertical => join_vertical_layout(first, second),
            })
        }
    }
}

fn render_styled_sized_layout(
    layout: &LayoutNode,
    terminals: &HashMap<usize, &TerminalState>,
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
) -> Option<RenderedAttachLayout> {
    match layout {
        LayoutNode::Pane(index) => {
            let width = col_end.saturating_sub(col_start);
            let height = row_end.saturating_sub(row_start);
            Some(RenderedAttachLayout {
                lines: render_terminal_ansi_region_lines(terminals.get(index)?, width, height),
                regions: vec![PaneRegion {
                    pane: *index,
                    row_start,
                    row_end,
                    col_start,
                    col_end,
                }],
            })
        }
        LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight,
            second_weight,
            first,
            second,
        } => {
            let ((first_start, first_end), (second_start, second_end)) =
                split_extent_weighted(col_start, col_end, 3, *first_weight, *second_weight);
            let left = render_styled_sized_layout(
                first,
                terminals,
                row_start,
                row_end,
                first_start,
                first_end,
            )?;
            let right = render_styled_sized_layout(
                second,
                terminals,
                row_start,
                row_end,
                second_start,
                second_end,
            )?;
            Some(join_styled_sized_horizontal_layout(left, right))
        }
        LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight,
            second_weight,
            first,
            second,
        } => {
            let ((first_start, first_end), (second_start, second_end)) =
                split_extent_weighted(row_start, row_end, 1, *first_weight, *second_weight);
            let top = render_styled_sized_layout(
                first,
                terminals,
                first_start,
                first_end,
                col_start,
                col_end,
            )?;
            let bottom = render_styled_sized_layout(
                second,
                terminals,
                second_start,
                second_end,
                col_start,
                col_end,
            )?;
            Some(join_styled_sized_vertical_layout(top, bottom))
        }
    }
}

fn render_terminal_ansi_region_lines(
    terminal: &TerminalState,
    width: usize,
    height: usize,
) -> Vec<String> {
    terminal.render_screen_ansi_lines(width, height)
}

fn join_styled_sized_horizontal_layout(
    left: RenderedAttachLayout,
    right: RenderedAttachLayout,
) -> RenderedAttachLayout {
    let separator_width = layout_column_gap(&left.regions, &right.regions).unwrap_or(0);
    let lines =
        join_styled_horizontal_with_separator_width(left.lines, right.lines, separator_width);
    let mut regions = left.regions;
    regions.extend(right.regions);
    RenderedAttachLayout { lines, regions }
}

fn join_styled_sized_vertical_layout(
    top: RenderedAttachLayout,
    bottom: RenderedAttachLayout,
) -> RenderedAttachLayout {
    let separator_height = layout_row_gap(&top.regions, &bottom.regions).unwrap_or(0);
    let separator_width = region_span_width(&top.regions)
        .max(region_span_width(&bottom.regions))
        .max(1);
    let lines = join_styled_vertical_with_separator_height(
        top.lines,
        bottom.lines,
        separator_height,
        separator_width,
    );
    let mut regions = top.regions;
    regions.extend(bottom.regions);
    RenderedAttachLayout { lines, regions }
}

fn join_styled_horizontal_with_separator_width(
    left: Vec<String>,
    right: Vec<String>,
    separator_width: usize,
) -> Vec<String> {
    let rows = left.len().max(right.len());
    (0..rows)
        .map(|index| {
            let mut line = left.get(index).cloned().unwrap_or_default();
            line.push_str(&horizontal_separator(separator_width));
            line.push_str(right.get(index).map_or("", String::as_str));
            line
        })
        .collect()
}

fn join_styled_vertical_with_separator_height(
    mut top: Vec<String>,
    bottom: Vec<String>,
    separator_height: usize,
    separator_width: usize,
) -> Vec<String> {
    top.extend(std::iter::repeat_n(
        "─".repeat(separator_width),
        separator_height,
    ));
    top.extend(bottom);
    top
}

fn region_span_width(regions: &[PaneRegion]) -> usize {
    let Some(start) = regions.iter().map(|region| region.col_start).min() else {
        return 0;
    };
    let Some(end) = regions.iter().map(|region| region.col_end).max() else {
        return 0;
    };
    end.saturating_sub(start)
}

fn render_sized_layout(
    layout: &LayoutNode,
    screens: &HashMap<usize, &str>,
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
) -> Option<RenderedAttachLayout> {
    match layout {
        LayoutNode::Pane(index) => {
            let width = col_end.saturating_sub(col_start);
            let height = row_end.saturating_sub(row_start);
            Some(RenderedAttachLayout {
                lines: fit_screen_lines_to_region(screens.get(index)?, width, height),
                regions: vec![PaneRegion {
                    pane: *index,
                    row_start,
                    row_end,
                    col_start,
                    col_end,
                }],
            })
        }
        LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight,
            second_weight,
            first,
            second,
        } => {
            let ((first_start, first_end), (second_start, second_end)) =
                split_extent_weighted(col_start, col_end, 3, *first_weight, *second_weight);
            let left =
                render_sized_layout(first, screens, row_start, row_end, first_start, first_end)?;
            let right = render_sized_layout(
                second,
                screens,
                row_start,
                row_end,
                second_start,
                second_end,
            )?;
            Some(join_sized_horizontal_layout(left, right))
        }
        LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight,
            second_weight,
            first,
            second,
        } => {
            let ((first_start, first_end), (second_start, second_end)) =
                split_extent_weighted(row_start, row_end, 1, *first_weight, *second_weight);
            let top =
                render_sized_layout(first, screens, first_start, first_end, col_start, col_end)?;
            let bottom = render_sized_layout(
                second,
                screens,
                second_start,
                second_end,
                col_start,
                col_end,
            )?;
            Some(join_sized_vertical_layout(top, bottom))
        }
    }
}

fn join_sized_horizontal_layout(
    left: RenderedAttachLayout,
    right: RenderedAttachLayout,
) -> RenderedAttachLayout {
    let separator_width = layout_column_gap(&left.regions, &right.regions).unwrap_or(0);
    let lines = join_horizontal_with_separator_width(left.lines, right.lines, separator_width);
    let mut regions = left.regions;
    regions.extend(right.regions);
    RenderedAttachLayout { lines, regions }
}

fn join_sized_vertical_layout(
    top: RenderedAttachLayout,
    bottom: RenderedAttachLayout,
) -> RenderedAttachLayout {
    let separator_height = layout_row_gap(&top.regions, &bottom.regions).unwrap_or(0);
    let lines = join_vertical_with_separator_height(top.lines, bottom.lines, separator_height);
    let mut regions = top.regions;
    regions.extend(bottom.regions);
    RenderedAttachLayout { lines, regions }
}

fn layout_column_gap(left: &[PaneRegion], right: &[PaneRegion]) -> Option<usize> {
    let left_end = left.iter().map(|region| region.col_end).max()?;
    let right_start = right.iter().map(|region| region.col_start).min()?;
    Some(right_start.saturating_sub(left_end))
}

fn layout_row_gap(top: &[PaneRegion], bottom: &[PaneRegion]) -> Option<usize> {
    let top_end = top.iter().map(|region| region.row_end).max()?;
    let bottom_start = bottom.iter().map(|region| region.row_start).min()?;
    Some(bottom_start.saturating_sub(top_end))
}

fn join_horizontal_layout(
    left: RenderedAttachLayout,
    right: RenderedAttachLayout,
) -> RenderedAttachLayout {
    let left_width = max_line_width(&left.lines);
    let rows = left.lines.len().max(right.lines.len()).max(1);
    let left_height = left.lines.len().max(1);
    let right_height = right.lines.len().max(1);
    let lines = join_horizontal(left.lines, right.lines);

    let mut regions = expand_boundary_region_rows(left.regions, left_height, rows);
    regions.extend(offset_regions(
        expand_boundary_region_rows(right.regions, right_height, rows),
        0,
        left_width + 3,
    ));
    RenderedAttachLayout { lines, regions }
}

fn join_vertical_layout(
    top: RenderedAttachLayout,
    bottom: RenderedAttachLayout,
) -> RenderedAttachLayout {
    let width = max_line_width(&top.lines)
        .max(max_line_width(&bottom.lines))
        .max(1);
    let top_height = top.lines.len().max(1);
    let top_width = max_line_width(&top.lines);
    let bottom_width = max_line_width(&bottom.lines);
    let lines = join_vertical(top.lines, bottom.lines);

    let mut regions = expand_boundary_region_cols(top.regions, top_width, width);
    regions.extend(offset_regions(
        expand_boundary_region_cols(bottom.regions, bottom_width, width),
        top_height + 1,
        0,
    ));

    RenderedAttachLayout { lines, regions }
}

fn expand_boundary_region_rows(
    mut regions: Vec<PaneRegion>,
    current_row_end: usize,
    target_row_end: usize,
) -> Vec<PaneRegion> {
    for region in &mut regions {
        if region.row_end == current_row_end {
            region.row_end = target_row_end;
        }
    }
    regions
}

fn expand_boundary_region_cols(
    mut regions: Vec<PaneRegion>,
    current_col_end: usize,
    target_col_end: usize,
) -> Vec<PaneRegion> {
    for region in &mut regions {
        if region.col_end == current_col_end {
            region.col_end = target_col_end;
        }
    }
    regions
}

fn offset_regions(
    mut regions: Vec<PaneRegion>,
    row_offset: usize,
    col_offset: usize,
) -> Vec<PaneRegion> {
    for region in &mut regions {
        region.row_start += row_offset;
        region.row_end += row_offset;
        region.col_start += col_offset;
        region.col_end += col_offset;
    }
    regions
}

fn resize_panes(pane_resizes: Vec<(Arc<Pane>, PtySize)>) -> io::Result<bool> {
    let resized_any = !pane_resizes.is_empty();
    for (pane, size) in pane_resizes {
        resize_pane(&pane, size)?;
    }
    Ok(resized_any)
}

fn resize_pane(pane: &Pane, size: PtySize) -> io::Result<()> {
    if pane.is_running() {
        let writer = pane.writer.lock().unwrap();
        pty::resize(&writer, size)?;
    }
    *pane.size.lock().unwrap() = size;
    pane.terminal
        .lock()
        .unwrap()
        .resize(size.cols as usize, size.rows as usize);
    Ok(())
}

fn screen_lines(screen: &str) -> Vec<String> {
    let lines = screen.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn fit_screen_lines_to_region(screen: &str, width: usize, height: usize) -> Vec<String> {
    let lines = screen_lines(screen);
    (0..height)
        .map(|row| fit_line_to_width(lines.get(row).map_or("", String::as_str), width))
        .collect()
}

fn fit_line_to_width(line: &str, width: usize) -> String {
    let truncated = line.chars().take(width).collect::<String>();
    pad_to_width(&truncated, width)
}

fn join_horizontal(left: Vec<String>, right: Vec<String>) -> Vec<String> {
    join_horizontal_with_separator_width(left, right, 3)
}

fn join_horizontal_with_separator_width(
    left: Vec<String>,
    right: Vec<String>,
    separator_width: usize,
) -> Vec<String> {
    let width = max_line_width(&left);
    let rows = left.len().max(right.len());
    (0..rows)
        .map(|index| {
            let mut line = pad_to_width(left.get(index).map_or("", String::as_str), width);
            line.push_str(&horizontal_separator(separator_width));
            line.push_str(right.get(index).map_or("", String::as_str));
            line
        })
        .collect()
}

fn horizontal_separator(width: usize) -> String {
    match width {
        0 => String::new(),
        1 => "│".to_string(),
        2 => " │".to_string(),
        _ => " │ ".to_string(),
    }
}

fn join_vertical(top: Vec<String>, bottom: Vec<String>) -> Vec<String> {
    join_vertical_with_separator_height(top, bottom, 1)
}

fn join_vertical_with_separator_height(
    mut top: Vec<String>,
    bottom: Vec<String>,
    separator_height: usize,
) -> Vec<String> {
    let width = max_line_width(&top).max(max_line_width(&bottom)).max(1);
    top.extend(std::iter::repeat_n("─".repeat(width), separator_height));
    top.extend(bottom);
    top
}

fn max_line_width(lines: &[String]) -> usize {
    lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
}

fn pad_to_width(line: &str, width: usize) -> String {
    let mut padded = line.to_string();
    let len = line.chars().count();
    if len < width {
        padded.push_str(&" ".repeat(width - len));
    }
    padded
}

fn render_client_lines(lines: &[String]) -> String {
    let mut output = String::new();
    for line in lines {
        output.push_str(line);
        output.push_str("\r\n");
    }
    output
}

fn render_ordered_pane_sections(panes: &[PaneSnapshot]) -> String {
    let mut output = String::new();
    for pane in panes {
        output.push_str("\r\n-- pane ");
        output.push_str(&pane.index.to_string());
        output.push_str(" --\r\n");
        for line in pane.screen.lines() {
            output.push_str(line);
            output.push_str("\r\n");
        }
    }
    output
}

fn start_output_pump(mut reader: File, pane: Arc<Pane>) {
    std::thread::spawn(move || {
        let mut buf = [0_u8; 8192];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            let bytes = &buf[..n];
            handle_pane_output(&pane, bytes);
        }
        flush_pane_output_filter(&pane);
        shutdown_tracked_clients(&pane.clients);
        mark_exited_pane_in_session(&pane);
    });
}

fn handle_pane_output(pane: &Arc<Pane>, bytes: &[u8]) {
    let filtered = pane.output_filter.lock().unwrap().filter(bytes);
    if !filtered.reply_bytes.is_empty() {
        let _ = pane.writer.lock().unwrap().write_all(&filtered.reply_bytes);
    }
    publish_pane_output(pane, &filtered.display_bytes);
}

fn flush_pane_output_filter(pane: &Arc<Pane>) {
    let pending = pane.output_filter.lock().unwrap().finish();
    publish_pane_output(pane, &pending);
}

fn publish_pane_output(pane: &Arc<Pane>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    append_history(&pane.raw_history, bytes);
    let terminal_changes = pane.terminal.lock().unwrap().apply_bytes(bytes);
    if let Some(cwd) = terminal_changes.cwd.as_ref().filter(|cwd| cwd.is_dir()) {
        pane.set_cwd(cwd.clone());
    }
    if let Some(title) = terminal_changes.title.as_ref() {
        pane.set_title(title.clone());
    }
    if terminal_changes.bell {
        pane.mark_bell();
    }
    if let Some(session) = pane.session() {
        if !session.is_active_pane(pane) {
            pane.mark_activity();
        }
        if terminal_changes.synchronized_output_active
            && !terminal_changes.synchronized_output_finished
        {
            schedule_synchronized_output_redraw_timeout(pane, &session);
        } else {
            pane.clear_synchronized_output_redraw_pending();
            session.notify_attach_redraw_for_pane_output(terminal_changes);
        }
    } else {
        notify_attach_redraw(&pane.attach_events);
    }
    broadcast(&pane.clients, bytes);
}

fn schedule_synchronized_output_redraw_timeout(pane: &Arc<Pane>, session: &Arc<Session>) {
    if !pane.mark_synchronized_output_redraw_pending() {
        return;
    }

    let pane = Arc::downgrade(pane);
    let session = Arc::downgrade(session);
    std::thread::spawn(move || {
        std::thread::sleep(SYNCHRONIZED_OUTPUT_REDRAW_TIMEOUT);
        let (Some(pane), Some(session)) = (pane.upgrade(), session.upgrade()) else {
            return;
        };
        if pane.take_synchronized_output_redraw_pending() {
            session.notify_attach_redraw_immediate();
        }
    });
}

fn mark_exited_pane_in_session(pane: &Arc<Pane>) {
    let status = pty::wait_exit_status(pane.child_pid).unwrap_or(pty::PtyExitStatus {
        code: None,
        signal: None,
    });
    if !pane.mark_exited(status) {
        return;
    }
    let Some(session) = pane.session() else {
        return;
    };
    session.mark_pane_exited(pane);
}

fn terminate_pane_async(child_pid: i32) {
    std::thread::spawn(move || {
        let _ = pty::terminate(child_pid);
    });
}

fn terminate_pane_if_running_async(pane: &Pane) {
    if pane.is_running() {
        terminate_pane_async(pane.child_pid);
    }
}

fn append_history(history: &Mutex<Vec<u8>>, bytes: &[u8]) {
    let mut history = history.lock().unwrap();
    history.extend_from_slice(bytes);
    if history.len() > MAX_HISTORY_BYTES {
        let excess = history.len() - MAX_HISTORY_BYTES;
        history.drain(..excess);
    }
}

fn broadcast(clients: &TrackedStreamClients, bytes: &[u8]) {
    let mut clients = clients.lock().unwrap();
    let mut live = Vec::with_capacity(clients.len());

    for mut client in clients.drain(..) {
        if client.stream.write_all(bytes).is_ok() {
            live.push(client);
        }
    }

    *clients = live;
}

fn notify_attach_redraw(clients: &AttachEventClients) {
    let mut clients = clients.lock().unwrap();
    let mut live = Vec::with_capacity(clients.len());

    for mut client in clients.drain(..) {
        if write_attach_redraw_event(&mut client.stream) {
            live.push(client);
        }
    }

    *clients = live;
}

fn run_attach_render_immediate_scheduler(session: Weak<Session>) {
    loop {
        let Some(session) = session.upgrade() else {
            return;
        };
        session.notify_attach_render_if_immediate_dirty();
        session
            .attach_render_immediate_pending
            .store(false, Ordering::SeqCst);
        if session.attach_render_rendered_epoch.load(Ordering::SeqCst)
            >= session.attach_render_immediate_epoch.load(Ordering::SeqCst)
        {
            return;
        }
        if session
            .attach_render_immediate_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
    }
}

fn notify_attach_render(
    clients: &AttachRenderClients,
    render_frame: impl FnOnce() -> Option<Vec<u8>>,
) {
    if clients.lock().unwrap().is_empty() {
        return;
    }
    let frame = render_frame();
    let Some(frame) = frame else {
        return;
    };
    let mut clients = clients.lock().unwrap();
    let mut live = Vec::with_capacity(clients.len());

    for mut client in clients.drain(..) {
        if write_attach_render_frame(&mut client.stream, &frame) {
            live.push(client);
        } else {
            let _ = client.stream.shutdown(std::net::Shutdown::Both);
        }
    }

    *clients = live;
}

fn shutdown_tracked_clients(clients: &TrackedStreamClients) {
    let mut clients = clients.lock().unwrap();
    for client in clients.drain(..) {
        let _ = client.stream.shutdown(std::net::Shutdown::Both);
    }
}

fn detach_all_tracked_clients(clients: &TrackedStreamClients) -> usize {
    let mut clients = clients.lock().unwrap();
    let count = clients.len();
    for client in clients.drain(..) {
        let _ = client.stream.shutdown(std::net::Shutdown::Both);
    }
    count
}

fn detach_tracked_client(clients: &TrackedStreamClients, client_id: usize) -> bool {
    let mut clients = clients.lock().unwrap();
    let Some(index) = clients
        .iter()
        .position(|client| client.client_id == client_id)
    else {
        return false;
    };
    let client = clients.remove(index);
    let _ = client.stream.shutdown(std::net::Shutdown::Both);
    true
}

fn collect_client_descriptions(
    output: &mut Vec<ClientDescription>,
    clients: &TrackedStreamClients,
    session: &str,
    size: PtySize,
) {
    output.extend(
        clients
            .lock()
            .unwrap()
            .iter()
            .map(|client| ClientDescription {
                id: client.client_id,
                session: session.to_string(),
                client_type: client.client_type,
                attached: true,
                width: size.cols,
                height: size.rows,
            }),
    );
}

fn register_tracked_stream(
    clients: &TrackedStreamClients,
    next_id: &AtomicUsize,
    stream: &UnixStream,
    client_id: usize,
    client_type: ClientType,
) -> io::Result<StreamRegistration> {
    let stream = stream.try_clone()?;
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    clients.lock().unwrap().push(TrackedStream {
        id,
        client_id,
        client_type,
        stream,
    });
    Ok(StreamRegistration {
        clients: Arc::clone(clients),
        id,
    })
}

fn register_attach_event_client(
    clients: &AttachEventClients,
    next_id: &AtomicUsize,
    stream: &UnixStream,
    client_id: usize,
) -> io::Result<Option<StreamRegistration>> {
    let mut client = stream.try_clone()?;
    client.set_nonblocking(true)?;
    if !write_attach_redraw_event(&mut client) {
        return Ok(None);
    }

    let id = next_id.fetch_add(1, Ordering::Relaxed);
    clients.lock().unwrap().push(TrackedStream {
        id,
        client_id,
        client_type: ClientType::Event,
        stream: client,
    });
    Ok(Some(StreamRegistration {
        clients: Arc::clone(clients),
        id,
    }))
}

fn register_attach_render_client(
    clients: &AttachRenderClients,
    next_id: &AtomicUsize,
    stream: &UnixStream,
    client_id: usize,
    frame: Option<Vec<u8>>,
) -> io::Result<Option<StreamRegistration>> {
    let Some(frame) = frame else {
        return Ok(None);
    };

    let mut client = stream.try_clone()?;
    client.set_write_timeout(Some(std::time::Duration::from_millis(100)))?;
    if !write_attach_render_frame(&mut client, &frame) {
        let _ = client.shutdown(std::net::Shutdown::Both);
        return Ok(None);
    }

    let id = next_id.fetch_add(1, Ordering::Relaxed);
    clients.lock().unwrap().push(TrackedStream {
        id,
        client_id,
        client_type: ClientType::Render,
        stream: client,
    });
    Ok(Some(StreamRegistration {
        clients: Arc::clone(clients),
        id,
    }))
}

fn write_attach_redraw_event(stream: &mut UnixStream) -> bool {
    matches!(stream.write(ATTACH_REDRAW_EVENT), Ok(n) if n == ATTACH_REDRAW_EVENT.len())
}

fn wait_for_attach_event_client_close(stream: &mut UnixStream) {
    let mut buf = [0_u8; 1];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return,
        }
    }
}

fn write_ok(stream: &mut UnixStream) -> io::Result<()> {
    stream.write_all(b"OK\n")
}

fn write_err(stream: &mut UnixStream, message: &str) -> io::Result<()> {
    writeln!(stream, "ERR {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_socket_line(stream: &mut UnixStream) -> String {
        let mut bytes = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            let n = stream.read(&mut byte).unwrap();
            if n == 0 {
                break;
            }
            bytes.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    fn read_attach_render_frame_body(stream: &mut UnixStream) -> Vec<u8> {
        let line = read_socket_line(stream);
        let Some(len) = line
            .strip_prefix("FRAME\t")
            .and_then(|line| line.strip_suffix('\n'))
        else {
            panic!("invalid render frame header: {line:?}");
        };
        let len = len.parse::<usize>().expect("parse render frame length");
        let mut body = vec![0_u8; len];
        stream
            .read_exact(&mut body)
            .expect("read attach render frame body");
        body
    }

    fn assert_no_attach_render_frame(stream: &mut UnixStream) {
        let mut byte = [0_u8; 1];
        match stream.read(&mut byte) {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Ok(0) => panic!("attach render stream closed while waiting for no frame"),
            Ok(n) => panic!("unexpected attach render frame byte count: {n}"),
            Err(error) => panic!("unexpected attach render read error: {error}"),
        }
    }

    fn test_pane() -> (Arc<Pane>, PathBuf) {
        test_pane_with_id(0)
    }

    fn test_pane_with_id(id: usize) -> (Arc<Pane>, PathBuf) {
        let writer_path = std::env::current_dir().unwrap().join(format!(
            ".dmux-test-pane-{id}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let writer = File::create(&writer_path).unwrap();
        let pane = Arc::new(Pane {
            id: PaneId::new(id),
            child_pid: 0,
            writer: Arc::new(Mutex::new(writer)),
            process: Mutex::new(PaneProcessStatus::Running { pid: 0 }),
            cwd: Mutex::new(std::env::current_dir().unwrap()),
            title: Mutex::new(String::new()),
            bell: AtomicBool::new(false),
            activity: AtomicBool::new(false),
            synchronized_output_redraw_pending: AtomicBool::new(false),
            size: Mutex::new(PtySize { cols: 80, rows: 24 }),
            raw_history: Arc::new(Mutex::new(Vec::new())),
            terminal: Arc::new(Mutex::new(TerminalState::new(80, 24, 100))),
            output_filter: Mutex::new(PtyOutputFilter::default()),
            clients: Arc::new(Mutex::new(Vec::new())),
            next_client_id: AtomicUsize::new(0),
            attach_events: Arc::new(Mutex::new(Vec::new())),
            session: Mutex::new(None),
        });
        (pane, writer_path)
    }

    fn wait_for_tracked_stream_count(clients: &TrackedStreamClients, expected: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let len = clients.lock().unwrap().len();
            if len == expected {
                return;
            }
            if std::time::Instant::now() >= deadline {
                assert_eq!(len, expected);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn default_key_bindings_and_options_are_inspectable_and_mutable() {
        let mut bindings = default_key_binding_map();
        assert_eq!(bindings.get("d").map(String::as_str), Some("detach-client"));
        bindings.remove("d");
        assert!(!bindings.contains_key("d"));
        bindings.insert("m".to_string(), "copy-mode".to_string());
        assert_eq!(bindings.get("m").map(String::as_str), Some("copy-mode"));

        let mut options = default_option_map();
        assert_eq!(
            options
                .get(crate::config::OPTION_PREFIX)
                .map(String::as_str),
            Some(crate::config::DEFAULT_PREFIX_KEY)
        );
        let value = crate::config::validate_option_value(crate::config::OPTION_PREFIX, "C-a")
            .expect("prefix option validates");
        options.insert(crate::config::OPTION_PREFIX.to_string(), value);
        assert_eq!(
            options
                .get(crate::config::OPTION_PREFIX)
                .map(String::as_str),
            Some("C-a")
        );
        assert!(crate::config::validate_option_value("unknown", "x").is_err());
    }

    fn tracked_stream(id: usize, stream: UnixStream) -> TrackedStream {
        TrackedStream {
            id,
            client_id: id,
            client_type: ClientType::Event,
            stream,
        }
    }

    #[test]
    fn validates_session_names_for_rename() {
        assert_eq!(
            validate_session_name(""),
            Err("session name cannot be empty".to_string())
        );
        assert_eq!(
            validate_session_name("bad\u{1}name"),
            Err("session name cannot contain control characters".to_string())
        );
        assert_eq!(
            validate_session_name("bad:target"),
            Err("session name cannot contain ':'".to_string())
        );
        assert!(validate_session_name("good").is_ok());
    }

    #[test]
    fn formats_session_and_client_lifecycle_tokens() {
        let session = SessionDescription {
            name: "dev".to_string(),
            windows: 2,
            attached_count: 3,
            created_at: 7,
        };
        assert_eq!(
            format_session_line(
                "#{session.name} #{session.window_count} #{session.attached} #{client.count} #{session.created_at}",
                &session,
            ),
            "dev 2 1 3 7"
        );
        let client = ClientDescription {
            id: 9,
            session: "dev".to_string(),
            client_type: ClientType::Raw,
            attached: true,
            width: 80,
            height: 24,
        };
        assert_eq!(
            format_client_line(
                "#{client.id} #{client.session} #{client.type} #{client.attached} #{client.width}x#{client.height}",
                &client
            ),
            "9 dev raw 1 80x24"
        );
    }

    #[test]
    fn notify_attach_redraw_writes_redraw_event() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (server, mut client) = UnixStream::pair().unwrap();
        events.lock().unwrap().push(tracked_stream(0, server));

        notify_attach_redraw(&events);

        let mut buf = [0_u8; 7];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"REDRAW\n");
    }

    #[test]
    fn notify_attach_redraw_drops_dead_clients() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (dead_server, dead_client) = UnixStream::pair().unwrap();
        drop(dead_client);
        let (live_server, mut live_client) = UnixStream::pair().unwrap();
        {
            let mut events = events.lock().unwrap();
            events.push(tracked_stream(0, dead_server));
            events.push(tracked_stream(1, live_server));
        }

        notify_attach_redraw(&events);

        let mut buf = [0_u8; 7];
        live_client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"REDRAW\n");
        assert_eq!(events.lock().unwrap().len(), 1);
    }

    #[test]
    fn notify_attach_render_does_not_build_frame_without_clients() {
        let clients = Arc::new(Mutex::new(Vec::new()));
        let called = std::sync::atomic::AtomicBool::new(false);

        notify_attach_render(&clients, || {
            called.store(true, Ordering::SeqCst);
            Some(Vec::new())
        });

        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn attach_render_debounce_interval_stays_within_frame_budget() {
        assert!(ATTACH_RENDER_DEBOUNCE_INTERVAL <= Duration::from_millis(16));
    }

    #[test]
    fn notify_attach_redraw_immediate_sends_render_frame() {
        let (pane, writer_path) = test_pane();
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::new(Mutex::new(Vec::new())),
        ));
        let (server, mut client) = UnixStream::pair().unwrap();
        let _registration = session
            .register_attach_render_stream(&server, 1)
            .unwrap()
            .unwrap();
        let _initial = read_attach_render_frame_body(&mut client);
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        session.notify_attach_redraw_immediate();

        let body = String::from_utf8(read_attach_render_frame_body(&mut client))
            .expect("render frame utf8");
        assert!(body.contains("OUTPUT\t"), "{body:?}");
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn pane_output_alternate_screen_change_bypasses_render_debounce() {
        let (pane, writer_path) = test_pane();
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::new(Mutex::new(Vec::new())),
        ));
        let (server, mut client) = UnixStream::pair().unwrap();
        let _registration = session
            .register_attach_render_stream(&server, 1)
            .unwrap()
            .unwrap();
        let _initial = read_attach_render_frame_body(&mut client);
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        session.notify_attach_redraw_for_pane_output(TerminalChanges {
            alternate_screen: true,
            ..TerminalChanges::default()
        });

        assert!(
            !session.attach_render_pending.load(Ordering::SeqCst),
            "alternate-screen transitions should use the immediate scheduler, not the debounced scheduler"
        );
        let body = String::from_utf8(read_attach_render_frame_body(&mut client))
            .expect("render frame utf8");
        assert!(body.contains("OUTPUT\t"), "{body:?}");
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn pane_output_after_alternate_screen_exit_bypasses_render_debounce() {
        let (pane, writer_path) = test_pane();
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::new(Mutex::new(Vec::new())),
        ));
        let (server, mut client) = UnixStream::pair().unwrap();
        let _registration = session
            .register_attach_render_stream(&server, 1)
            .unwrap()
            .unwrap();
        let _initial = read_attach_render_frame_body(&mut client);
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        session.notify_attach_redraw_for_pane_output(TerminalChanges {
            post_alternate_screen_exit: true,
            ..TerminalChanges::default()
        });

        assert!(
            !session.attach_render_pending.load(Ordering::SeqCst),
            "primary output immediately after alternate-screen exit should use the immediate scheduler"
        );
        let body = String::from_utf8(read_attach_render_frame_body(&mut client))
            .expect("render frame utf8");
        assert!(body.contains("OUTPUT\t"), "{body:?}");
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn ordinary_pane_output_keeps_render_debounce() {
        let (pane, writer_path) = test_pane();
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::new(Mutex::new(Vec::new())),
        ));
        let (server, mut client) = UnixStream::pair().unwrap();
        let _registration = session
            .register_attach_render_stream(&server, 1)
            .unwrap()
            .unwrap();
        let _initial = read_attach_render_frame_body(&mut client);
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        session.notify_attach_redraw_for_pane_output(TerminalChanges::default());

        assert!(
            session.attach_render_pending.load(Ordering::SeqCst),
            "ordinary pane output should keep the burst-coalescing debounce"
        );
        let body = String::from_utf8(read_attach_render_frame_body(&mut client))
            .expect("render frame utf8");
        assert!(body.contains("OUTPUT\t"), "{body:?}");
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn synchronized_output_suppresses_attach_render_until_finish() {
        let (pane, writer_path) = test_pane();
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::new(Mutex::new(Vec::new())),
        ));
        pane.set_session(&session);
        let (server, mut client) = UnixStream::pair().unwrap();
        let _registration = session
            .register_attach_render_stream(&server, 1)
            .unwrap()
            .unwrap();
        let _initial = read_attach_render_frame_body(&mut client);
        client
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();

        publish_pane_output(&pane, b"\x1b[?2026hfirst");

        assert_no_attach_render_frame(&mut client);

        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        publish_pane_output(&pane, b"second\x1b[?2026l");

        let body = String::from_utf8(read_attach_render_frame_body(&mut client))
            .expect("render frame utf8");
        assert!(body.contains("OUTPUT\t"), "{body:?}");
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn synchronized_output_timeout_flushes_unterminated_block() {
        let (pane, writer_path) = test_pane();
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::new(Mutex::new(Vec::new())),
        ));
        pane.set_session(&session);
        let (server, mut client) = UnixStream::pair().unwrap();
        let _registration = session
            .register_attach_render_stream(&server, 1)
            .unwrap()
            .unwrap();
        let _initial = read_attach_render_frame_body(&mut client);
        client
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();

        publish_pane_output(&pane, b"\x1b[?2026hunterminated");

        assert_no_attach_render_frame(&mut client);

        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let body = String::from_utf8(read_attach_render_frame_body(&mut client))
            .expect("render frame utf8");
        assert!(body.contains("OUTPUT\t"), "{body:?}");
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn immediate_scheduler_does_not_drain_later_debounced_epoch() {
        let (pane, writer_path) = test_pane();
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::new(Mutex::new(Vec::new())),
        ));

        let immediate_epoch = session.mark_attach_render_dirty();
        session
            .attach_render_immediate_epoch
            .store(immediate_epoch, Ordering::SeqCst);
        let debounced_epoch = session.mark_attach_render_dirty();

        run_attach_render_immediate_scheduler(Arc::downgrade(&session));

        assert_eq!(
            session.attach_render_rendered_epoch.load(Ordering::SeqCst),
            immediate_epoch
        );
        assert_eq!(
            session.attach_render_epoch.load(Ordering::SeqCst),
            debounced_epoch
        );
        assert!(
            session.attach_render_rendered_epoch.load(Ordering::SeqCst)
                < session.attach_render_epoch.load(Ordering::SeqCst)
        );
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn transient_message_expiry_keeps_newer_message() {
        let (pane, writer_path) = test_pane();
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::new(Mutex::new(Vec::new())),
        ));
        session.transient_message_epoch.store(2, Ordering::SeqCst);
        *session.transient_message.lock().unwrap() = Some("newer".to_string());

        assert!(!session.clear_transient_message_if_epoch(1));
        assert_eq!(
            session.transient_message.lock().unwrap().as_deref(),
            Some("newer")
        );

        assert!(session.clear_transient_message_if_epoch(2));
        assert!(session.transient_message.lock().unwrap().is_none());
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn notify_attach_render_shuts_down_failed_clients() {
        let clients = Arc::new(Mutex::new(Vec::new()));
        let (handler, mut client) = UnixStream::pair().unwrap();
        let tracked = handler.try_clone().unwrap();
        tracked.set_nonblocking(true).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .unwrap();
        clients.lock().unwrap().push(tracked_stream(0, tracked));

        notify_attach_render(&clients, || Some(vec![b'x'; 8 * 1024 * 1024]));

        assert!(clients.lock().unwrap().is_empty());
        let mut saw_eof = false;
        let mut buf = [0_u8; 8192];
        loop {
            match client.read(&mut buf) {
                Ok(0) => {
                    saw_eof = true;
                    break;
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("unexpected client read error: {error:?}"),
            }
        }
        assert!(
            saw_eof,
            "failed render stream stayed open after write failure"
        );
        drop(handler);
    }

    #[test]
    fn notify_attach_redraw_registered_clients_are_nonblocking() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let next_id = AtomicUsize::new(0);
        let (server, _client) = UnixStream::pair().unwrap();

        let _registration = register_attach_event_client(&events, &next_id, &server, 1)
            .unwrap()
            .unwrap();

        let mut registered = events.lock().unwrap().pop().unwrap();
        let mut buf = [0_u8; 1];
        let err = registered.stream.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn register_attach_event_client_sends_initial_redraw_to_new_client() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let next_id = AtomicUsize::new(0);
        let (server, mut client) = UnixStream::pair().unwrap();

        let _registration = register_attach_event_client(&events, &next_id, &server, 1)
            .unwrap()
            .unwrap();

        let mut buf = [0_u8; 7];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"REDRAW\n");
        assert_eq!(events.lock().unwrap().len(), 1);
    }

    #[test]
    fn handle_attach_events_removes_event_stream_after_client_disconnect() {
        let attach_events = Arc::new(Mutex::new(Vec::new()));
        let dir = std::env::temp_dir();
        let writer_path = dir.join(format!(
            "dmux-attach-events-lifetime-{}",
            std::process::id()
        ));
        let writer = File::create(&writer_path).unwrap();
        let pane = Arc::new(Pane {
            id: PaneId::new(0),
            child_pid: 0,
            writer: Arc::new(Mutex::new(writer)),
            process: Mutex::new(PaneProcessStatus::Running { pid: 0 }),
            cwd: Mutex::new(dir.clone()),
            title: Mutex::new(String::new()),
            bell: AtomicBool::new(false),
            activity: AtomicBool::new(false),
            synchronized_output_redraw_pending: AtomicBool::new(false),
            size: Mutex::new(PtySize { cols: 80, rows: 24 }),
            raw_history: Arc::new(Mutex::new(Vec::new())),
            terminal: Arc::new(Mutex::new(TerminalState::new(80, 24, 100))),
            output_filter: Mutex::new(PtyOutputFilter::default()),
            clients: Arc::new(Mutex::new(Vec::new())),
            next_client_id: AtomicUsize::new(0),
            attach_events: Arc::clone(&attach_events),
            session: Mutex::new(None),
        });
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            Arc::clone(&attach_events),
        ));
        let state = Arc::new(ServerState {
            sessions: Mutex::new(HashMap::from([("test".to_string(), session)])),
            session_aliases: Mutex::new(HashMap::new()),
            buffers: Mutex::new(BufferStore::new()),
            key_bindings: Mutex::new(default_key_binding_map()),
            options: Mutex::new(default_option_map()),
            socket_path: writer_path.with_extension("sock"),
            next_session_created_id: AtomicUsize::new(1),
            next_client_id: AtomicUsize::new(1),
        });
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let handle =
            std::thread::spawn(move || handle_attach_events(&state, &mut server, "test").unwrap());

        assert_eq!(read_socket_line(&mut client), "OK\n");
        assert_eq!(read_socket_line(&mut client), "REDRAW\n");
        wait_for_tracked_stream_count(&attach_events, 1);

        drop(client);
        handle.join().unwrap();

        wait_for_tracked_stream_count(&attach_events, 0);
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn handle_attach_removes_lifetime_stream_after_normal_detach() {
        let attach_events = Arc::new(Mutex::new(Vec::new()));
        let dir = std::env::temp_dir();
        let writer_path = dir.join(format!("dmux-attach-lifetime-{}", std::process::id()));
        let writer = File::create(&writer_path).unwrap();
        let pane = Arc::new(Pane {
            id: PaneId::new(0),
            child_pid: 0,
            writer: Arc::new(Mutex::new(writer)),
            process: Mutex::new(PaneProcessStatus::Running { pid: 0 }),
            cwd: Mutex::new(dir.clone()),
            title: Mutex::new(String::new()),
            bell: AtomicBool::new(false),
            activity: AtomicBool::new(false),
            synchronized_output_redraw_pending: AtomicBool::new(false),
            size: Mutex::new(PtySize { cols: 80, rows: 24 }),
            raw_history: Arc::new(Mutex::new(Vec::new())),
            terminal: Arc::new(Mutex::new(TerminalState::new(80, 24, 100))),
            output_filter: Mutex::new(PtyOutputFilter::default()),
            clients: Arc::new(Mutex::new(Vec::new())),
            next_client_id: AtomicUsize::new(0),
            attach_events: Arc::clone(&attach_events),
            session: Mutex::new(None),
        });
        let session = Arc::new(Session::new(
            "test".to_string(),
            1,
            Arc::clone(&pane),
            attach_events,
        ));
        let state = Arc::new(ServerState {
            sessions: Mutex::new(HashMap::from([("test".to_string(), Arc::clone(&session))])),
            session_aliases: Mutex::new(HashMap::new()),
            buffers: Mutex::new(BufferStore::new()),
            key_bindings: Mutex::new(default_key_binding_map()),
            options: Mutex::new(default_option_map()),
            socket_path: writer_path.with_extension("sock"),
            next_session_created_id: AtomicUsize::new(1),
            next_client_id: AtomicUsize::new(1),
        });
        let (server, mut client) = UnixStream::pair().unwrap();
        let handle = std::thread::spawn(move || handle_attach(&state, server, "test").unwrap());

        assert_eq!(read_socket_line(&mut client), "OK\tLIVE\t0\n");
        drop(client);
        handle.join().unwrap();

        assert_eq!(session.attach_streams.lock().unwrap().len(), 0);
        assert_eq!(pane.clients.lock().unwrap().len(), 0);
        let _ = std::fs::remove_file(writer_path);
    }

    #[test]
    fn pane_set_removes_active_and_selects_previous_pane() {
        let mut panes = PaneSet::new("base");
        panes.add("split");

        let removed = panes.kill(None).unwrap();

        assert_eq!(removed, "split");
        assert_eq!(panes.active(), Some("base"));
        assert_eq!(panes.len(), 1);
    }

    #[test]
    fn pane_set_rejects_last_pane_removal() {
        let mut panes = PaneSet::new("base");

        assert_eq!(
            panes.kill(None),
            Err("cannot kill last pane; use kill-session")
        );
        assert_eq!(panes.active(), Some("base"));
    }

    #[test]
    fn directional_target_selects_adjacent_in_simple_geometry() {
        let regions = vec![
            PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 10,
                col_start: 0,
                col_end: 10,
            },
            PaneRegion {
                pane: 1,
                row_start: 0,
                row_end: 10,
                col_start: 13,
                col_end: 20,
            },
        ];

        assert_eq!(
            directional_pane_target_from_regions(&regions, 0, PaneDirection::Right),
            Some(1)
        );
        assert_eq!(
            directional_pane_target_from_regions(&regions, 1, PaneDirection::Left),
            Some(0)
        );
    }

    #[test]
    fn directional_target_uses_distance_overlap_and_index_tiebreaks_for_nested_geometry() {
        let regions = vec![
            PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 10,
                col_start: 10,
                col_end: 20,
            },
            PaneRegion {
                pane: 1,
                row_start: 0,
                row_end: 4,
                col_start: 0,
                col_end: 8,
            },
            PaneRegion {
                pane: 2,
                row_start: 4,
                row_end: 10,
                col_start: 0,
                col_end: 8,
            },
            PaneRegion {
                pane: 3,
                row_start: 0,
                row_end: 10,
                col_start: 22,
                col_end: 30,
            },
        ];

        assert_eq!(
            directional_pane_target_from_regions(&regions, 0, PaneDirection::Left),
            Some(2)
        );
        assert_eq!(
            directional_pane_target_from_regions(&regions, 0, PaneDirection::Right),
            Some(3)
        );
    }

    #[test]
    fn directional_target_rejects_non_overlapping_candidates() {
        let regions = vec![
            PaneRegion {
                pane: 0,
                row_start: 10,
                row_end: 20,
                col_start: 10,
                col_end: 20,
            },
            PaneRegion {
                pane: 1,
                row_start: 0,
                row_end: 5,
                col_start: 0,
                col_end: 5,
            },
        ];

        assert_eq!(
            directional_pane_target_from_regions(&regions, 0, PaneDirection::Left),
            None
        );
    }

    #[test]
    fn window_directional_select_without_adjacent_preserves_active_pane() {
        let (pane0, path0) = test_pane_with_id(0);
        let (pane1, path1) = test_pane_with_id(1);
        let mut window = Window::new(TabId::new(0), "0".to_string(), pane0);
        window.add_pane(SplitDirection::Horizontal, pane1);
        window.select_pane(PaneSelectTarget::Index(0)).unwrap();

        let error = window
            .select_pane(PaneSelectTarget::Direction(PaneDirection::Up))
            .unwrap_err();

        assert_eq!(error, "missing adjacent pane");
        assert_eq!(window.active_pane_index(), 0);
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
    }

    #[test]
    fn window_selects_pane_by_stable_id_after_index_changes() {
        let (pane0, path0) = test_pane_with_id(10);
        let (pane1, path1) = test_pane_with_id(11);
        let (pane2, path2) = test_pane_with_id(12);
        let mut window = Window::new(TabId::new(0), "0".to_string(), pane0);
        window.add_pane(SplitDirection::Horizontal, pane1);
        window.add_pane(SplitDirection::Vertical, pane2);
        window.remove_pane_at(1);

        window.select_pane(PaneSelectTarget::Id(12)).unwrap();

        assert_eq!(window.active_pane_index(), 1);
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
        let _ = std::fs::remove_file(path2);
    }

    #[test]
    fn window_select_layout_preserves_active_pane_and_pane_identities() {
        let (pane0, path0) = test_pane_with_id(10);
        let (pane1, path1) = test_pane_with_id(11);
        let (pane2, path2) = test_pane_with_id(12);
        let (pane3, path3) = test_pane_with_id(13);
        let mut window = Window::new(TabId::new(0), "0".to_string(), Arc::clone(&pane0));
        window.add_pane(SplitDirection::Horizontal, Arc::clone(&pane1));
        window.add_pane(SplitDirection::Vertical, Arc::clone(&pane2));
        window.add_pane(SplitDirection::Horizontal, Arc::clone(&pane3));
        window.select_pane(PaneSelectTarget::Index(2)).unwrap();

        let resizes = window
            .apply_layout_preset(LayoutPreset::EvenVertical, PtySize { cols: 80, rows: 27 })
            .unwrap();

        assert_eq!(window.active_pane_index(), 2);
        assert_eq!(
            window
                .panes()
                .iter()
                .map(|pane| pane.id.as_usize())
                .collect::<Vec<_>>(),
            vec![10, 11, 12, 13]
        );
        assert_eq!(resizes.len(), 4);
        assert!(
            resizes
                .iter()
                .all(|(_, size)| size.cols == 80 && size.rows >= 6)
        );
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
        let _ = std::fs::remove_file(path2);
        let _ = std::fs::remove_file(path3);
    }

    #[test]
    fn window_swap_panes_preserves_ids_and_active_pane_identity() {
        let (pane0, path0) = test_pane_with_id(10);
        let (pane1, path1) = test_pane_with_id(11);
        let (pane2, path2) = test_pane_with_id(12);
        let mut window = Window::new(TabId::new(0), "0".to_string(), Arc::clone(&pane0));
        window.add_pane(SplitDirection::Horizontal, Arc::clone(&pane1));
        window.add_pane(SplitDirection::Vertical, Arc::clone(&pane2));
        window.select_pane(PaneSelectTarget::Id(11)).unwrap();

        window
            .swap_panes(PaneTarget::Index(0), PaneTarget::Index(1))
            .unwrap();

        assert_eq!(
            window
                .panes()
                .iter()
                .map(|pane| pane.id.as_usize())
                .collect::<Vec<_>>(),
            vec![11, 10, 12]
        );
        assert_eq!(window.active_pane_id().unwrap().as_usize(), 11);
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
        let _ = std::fs::remove_file(path2);
    }

    #[test]
    fn window_set_selects_by_id_and_name_and_cycles_with_wraparound() {
        let (pane0, path0) = test_pane_with_id(0);
        let (pane1, path1) = test_pane_with_id(1);
        let (pane2, path2) = test_pane_with_id(2);
        let mut windows = WindowSet::new(Window::new(TabId::new(10), "base".to_string(), pane0));
        windows.add(Window::new(TabId::new(11), "editor".to_string(), pane1));
        windows.add(Window::new(TabId::new(12), "logs".to_string(), pane2));

        windows
            .select(WindowTarget::Name("base".to_string()))
            .unwrap();
        assert_eq!(windows.status_context("dev").unwrap().window_index, 0);
        windows.select_previous().unwrap();
        assert_eq!(windows.status_context("dev").unwrap().window_index, 2);
        windows.select_next().unwrap();
        assert_eq!(windows.status_context("dev").unwrap().window_index, 0);
        windows.select(WindowTarget::Id(11)).unwrap();
        assert_eq!(windows.status_context("dev").unwrap().window_name, "editor");

        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
        let _ = std::fs::remove_file(path2);
    }

    #[test]
    fn window_set_swap_panes_in_non_active_window_preserves_active_window() {
        let (pane0, path0) = test_pane_with_id(10);
        let (pane1, path1) = test_pane_with_id(11);
        let (pane2, path2) = test_pane_with_id(12);
        let (pane3, path3) = test_pane_with_id(13);
        let mut base = Window::new(TabId::new(10), "base".to_string(), Arc::clone(&pane0));
        base.add_pane(SplitDirection::Horizontal, Arc::clone(&pane1));
        let mut windows = WindowSet::new(base);
        windows.add(Window::new(
            TabId::new(11),
            "logs".to_string(),
            Arc::clone(&pane2),
        ));
        windows.add(Window::new(
            TabId::new(12),
            "shell".to_string(),
            Arc::clone(&pane3),
        ));

        windows
            .swap_panes(
                WindowTarget::Name("base".to_string()),
                PaneTarget::Index(0),
                WindowTarget::Name("base".to_string()),
                PaneTarget::Index(1),
            )
            .unwrap();

        assert_eq!(windows.status_context("dev").unwrap().window_name, "shell");
        assert_eq!(
            windows
                .pane_descriptions_for_window(WindowTarget::Name("base".to_string()))
                .unwrap()
                .iter()
                .map(|pane| pane.id.as_usize())
                .collect::<Vec<_>>(),
            vec![11, 10]
        );
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
        let _ = std::fs::remove_file(path2);
        let _ = std::fs::remove_file(path3);
    }

    #[test]
    fn window_set_moves_pane_between_windows_and_removes_empty_source_window() {
        let (pane0, path0) = test_pane_with_id(10);
        let (pane1, path1) = test_pane_with_id(11);
        let (pane2, path2) = test_pane_with_id(12);
        let mut windows = WindowSet::new(Window::new(
            TabId::new(10),
            "base".to_string(),
            Arc::clone(&pane0),
        ));
        windows.add(Window::new(
            TabId::new(11),
            "logs".to_string(),
            Arc::clone(&pane1),
        ));
        windows.add(Window::new(
            TabId::new(12),
            "shell".to_string(),
            Arc::clone(&pane2),
        ));

        let resizes = windows
            .move_pane(
                WindowTarget::Index(1),
                PaneTarget::Id(11),
                WindowTarget::Index(0),
                PaneTarget::Index(0),
                SplitDirection::Horizontal,
                PtySize { cols: 80, rows: 24 },
            )
            .unwrap();

        assert_eq!(windows.window_count(), 2);
        assert_eq!(
            windows
                .pane_descriptions_for_window(WindowTarget::Index(0))
                .unwrap()
                .iter()
                .map(|pane| pane.id.as_usize())
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
        assert_eq!(windows.status_context("dev").unwrap().window_name, "shell");
        assert_eq!(
            windows.status_context("dev").unwrap().pane_id.as_usize(),
            12
        );
        assert_eq!(resizes.len(), 2);
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
        let _ = std::fs::remove_file(path2);
    }

    #[test]
    fn window_set_rejects_move_that_would_create_zero_sized_destination_pane() {
        let (pane0, path0) = test_pane_with_id(10);
        let (pane1, path1) = test_pane_with_id(11);
        let mut windows = WindowSet::new(Window::new(
            TabId::new(10),
            "base".to_string(),
            Arc::clone(&pane0),
        ));
        windows.add(Window::new(
            TabId::new(11),
            "logs".to_string(),
            Arc::clone(&pane1),
        ));

        let error = match windows.move_pane(
            WindowTarget::Name("logs".to_string()),
            PaneTarget::Index(0),
            WindowTarget::Name("base".to_string()),
            PaneTarget::Index(0),
            SplitDirection::Horizontal,
            PtySize { cols: 1, rows: 1 },
        ) {
            Ok(_) => panic!("move-pane unexpectedly succeeded"),
            Err(error) => error,
        };

        assert_eq!(error, "resize would exceed minimum pane size");
        assert_eq!(windows.window_count(), 2);
        assert_eq!(
            windows
                .pane_descriptions_for_window(WindowTarget::Name("base".to_string()))
                .unwrap()
                .iter()
                .map(|pane| pane.id.as_usize())
                .collect::<Vec<_>>(),
            vec![10]
        );
        assert_eq!(
            windows
                .pane_descriptions_for_window(WindowTarget::Name("logs".to_string()))
                .unwrap()
                .iter()
                .map(|pane| pane.id.as_usize())
                .collect::<Vec<_>>(),
            vec![11]
        );
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
    }

    #[test]
    fn window_set_break_pane_from_non_active_window_preserves_active_window() {
        let (pane0, path0) = test_pane_with_id(10);
        let (pane1, path1) = test_pane_with_id(11);
        let (pane2, path2) = test_pane_with_id(12);
        let mut base = Window::new(TabId::new(10), "base".to_string(), Arc::clone(&pane0));
        base.add_pane(SplitDirection::Horizontal, Arc::clone(&pane1));
        let mut windows = WindowSet::new(base);
        windows.add(Window::new(
            TabId::new(11),
            "shell".to_string(),
            Arc::clone(&pane2),
        ));

        let resizes = windows
            .break_pane(
                WindowTarget::Name("base".to_string()),
                PaneTarget::Id(11),
                TabId::new(12),
                PtySize { cols: 80, rows: 24 },
            )
            .unwrap();

        assert_eq!(windows.window_count(), 3);
        assert_eq!(windows.status_context("dev").unwrap().window_name, "shell");
        assert_eq!(
            windows
                .pane_descriptions_for_window(WindowTarget::Name("base".to_string()))
                .unwrap()
                .iter()
                .map(|pane| pane.id.as_usize())
                .collect::<Vec<_>>(),
            vec![10]
        );
        assert_eq!(
            windows
                .pane_descriptions_for_window(WindowTarget::Name("12".to_string()))
                .unwrap()
                .iter()
                .map(|pane| pane.id.as_usize())
                .collect::<Vec<_>>(),
            vec![11]
        );
        assert_eq!(resizes.len(), 2);
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
        let _ = std::fs::remove_file(path2);
    }

    #[test]
    fn window_set_break_pane_from_active_window_selects_new_window() {
        let (pane0, path0) = test_pane_with_id(10);
        let (pane1, path1) = test_pane_with_id(11);
        let (pane2, path2) = test_pane_with_id(12);
        let mut base = Window::new(TabId::new(10), "base".to_string(), Arc::clone(&pane0));
        base.add_pane(SplitDirection::Horizontal, Arc::clone(&pane1));
        let mut windows = WindowSet::new(base);
        windows.add(Window::new(
            TabId::new(11),
            "shell".to_string(),
            Arc::clone(&pane2),
        ));
        windows
            .select(WindowTarget::Name("base".to_string()))
            .unwrap();

        windows
            .break_pane(
                WindowTarget::Active,
                PaneTarget::Id(11),
                TabId::new(12),
                PtySize { cols: 80, rows: 24 },
            )
            .unwrap();

        assert_eq!(windows.status_context("dev").unwrap().window_name, "12");
        assert_eq!(
            windows.status_context("dev").unwrap().pane_id.as_usize(),
            11
        );
        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
        let _ = std::fs::remove_file(path2);
    }

    #[test]
    fn window_set_rename_rejects_duplicates_and_empty_names() {
        let (pane0, path0) = test_pane_with_id(0);
        let (pane1, path1) = test_pane_with_id(1);
        let mut windows = WindowSet::new(Window::new(TabId::new(0), "base".to_string(), pane0));
        windows.add(Window::new(TabId::new(1), "editor".to_string(), pane1));

        assert_eq!(
            windows
                .rename_window(WindowTarget::Active, "base".to_string())
                .unwrap_err(),
            "window name already exists"
        );
        assert_eq!(
            windows
                .rename_window(WindowTarget::Active, "".to_string())
                .unwrap_err(),
            "window name cannot be empty"
        );
        assert_eq!(
            windows
                .rename_window(WindowTarget::Active, "bad\nname".to_string())
                .unwrap_err(),
            "window name cannot contain control characters"
        );
        windows
            .rename_window(WindowTarget::Name("editor".to_string()), "logs".to_string())
            .unwrap();
        assert_eq!(windows.status_context("dev").unwrap().window_name, "logs");

        let _ = std::fs::remove_file(path0);
        let _ = std::fs::remove_file(path1);
    }

    #[test]
    fn layout_node_splits_active_leaf_horizontally() {
        let mut layout = LayoutNode::Pane(0);

        assert!(layout.split_pane(0, SplitDirection::Horizontal, 1));

        assert_eq!(
            layout,
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }
        );
    }

    #[test]
    fn layout_node_removes_pane_and_shifts_remaining_indexes() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(1)),
                second: Box::new(LayoutNode::Pane(2)),
            }),
        };

        assert!(layout.remove_pane(1));

        assert_eq!(
            layout,
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }
        );
    }

    #[test]
    fn render_attach_layout_joins_horizontal_panes() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "base-ready\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "split-ready\n".to_string(),
            },
        ];

        let rendered = render_attach_pane_snapshot(&layout, &panes);

        assert_eq!(rendered, "base-ready │ split-ready\r\n");
    }

    #[test]
    fn render_attach_layout_maps_horizontal_pane_regions() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "left\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "right\n".to_string(),
            },
        ];

        let rendered = render_attach_layout(&layout, &panes).unwrap();

        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 4,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 7,
                    col_end: 12,
                },
            ]
        );
    }

    #[test]
    fn render_attach_layout_for_size_uses_allocated_regions_and_padding() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "left\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "right\n".to_string(),
            },
        ];

        let rendered =
            render_attach_layout_for_size(&layout, &panes, PtySize { cols: 20, rows: 3 }).unwrap();

        assert_eq!(
            rendered.lines,
            vec![
                "left     │ right    ".to_string(),
                "         │          ".to_string(),
                "         │          ".to_string(),
            ]
        );
        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 3,
                    col_start: 0,
                    col_end: 8,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 3,
                    col_start: 11,
                    col_end: 20,
                },
            ]
        );
    }

    #[test]
    fn render_attach_layout_for_size_omits_horizontal_separator_when_space_is_too_narrow() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "left\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "right\n".to_string(),
            },
        ];

        let rendered =
            render_attach_layout_for_size(&layout, &panes, PtySize { cols: 4, rows: 1 }).unwrap();

        assert_eq!(rendered.lines, vec!["leri".to_string()]);
        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 2,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 2,
                    col_end: 4,
                },
            ]
        );
    }

    #[test]
    fn render_attach_layout_for_size_omits_vertical_separator_when_space_is_too_short() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "top\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "bottom\n".to_string(),
            },
        ];

        let one_row =
            render_attach_layout_for_size(&layout, &panes, PtySize { cols: 10, rows: 1 }).unwrap();
        assert_eq!(one_row.lines, vec!["bottom    ".to_string()]);
        assert_eq!(
            one_row.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 0,
                    col_start: 0,
                    col_end: 10,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 10,
                },
            ]
        );

        let two_rows =
            render_attach_layout_for_size(&layout, &panes, PtySize { cols: 10, rows: 2 }).unwrap();
        assert_eq!(
            two_rows.lines,
            vec!["top       ".to_string(), "bottom    ".to_string()]
        );
        assert_eq!(
            two_rows.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 10,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 1,
                    row_end: 2,
                    col_start: 0,
                    col_end: 10,
                },
            ]
        );
    }

    #[test]
    fn sized_layout_regions_do_not_overlap_when_horizontal_space_is_one_column() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        assert_eq!(
            layout_regions_for_size(&layout, PtySize { cols: 1, rows: 1 }),
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 0,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 1,
                },
            ]
        );
    }

    #[test]
    fn sized_layout_regions_do_not_overlap_when_vertical_space_is_one_row() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        assert_eq!(
            layout_regions_for_size(&layout, PtySize { cols: 1, rows: 1 }),
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 0,
                    col_start: 0,
                    col_end: 1,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 1,
                },
            ]
        );
    }

    #[test]
    fn sized_layout_regions_split_horizontal_panes_around_separator() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        assert_eq!(
            layout_regions_for_size(&layout, PtySize { cols: 83, rows: 24 }),
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 24,
                    col_start: 0,
                    col_end: 40,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 24,
                    col_start: 43,
                    col_end: 83,
                },
            ]
        );
    }

    #[test]
    fn sized_layout_regions_split_vertical_panes_around_separator() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        assert_eq!(
            layout_regions_for_size(&layout, PtySize { cols: 80, rows: 24 }),
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 11,
                    col_start: 0,
                    col_end: 80,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 12,
                    row_end: 24,
                    col_start: 0,
                    col_end: 80,
                },
            ]
        );
    }

    #[test]
    fn format_attach_layout_snapshot_response_includes_regions_and_snapshot() {
        let snapshot = RenderedAttachSnapshot {
            text: "left | right\r\n".to_string(),
            regions: vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 4,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 7,
                    col_end: 12,
                },
            ],
            cursor: None,
        };

        let body = String::from_utf8(format_attach_layout_snapshot_body(&snapshot)).unwrap();

        assert_eq!(
            body,
            "REGIONS\t2\n\
REGION\t0\t0\t1\t0\t4\n\
REGION\t1\t0\t1\t7\t12\n\
SNAPSHOT\t14\n\
left | right\r\n"
        );
    }

    #[test]
    fn format_attach_render_frame_includes_regions_and_output_bytes() {
        let snapshot = RenderedAttachSnapshot {
            text: "left | right\r\n".to_string(),
            regions: vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 4,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 7,
                    col_end: 12,
                },
            ],
            cursor: None,
        };

        let body = String::from_utf8(format_attach_render_frame_body(
            "status", None, &snapshot, 10,
        ))
        .expect("render frame utf8");

        assert!(body.starts_with("HEADER_ROWS\t1\nREGIONS\t2\n"), "{body:?}");
        assert!(
            body.contains("REGION\t0\t0\t1\t0\t4\nREGION\t1\t0\t1\t7\t12\n"),
            "{body:?}"
        );
        assert!(body.contains("\nOUTPUT\t"), "{body:?}");
        assert!(body.contains("\x1b[H\x1b[2Kstatus\r\n"), "{body:?}");
        assert!(body.contains("\x1b[2Kleft | right"), "{body:?}");
        assert!(!body.contains("SNAPSHOT\t"), "{body:?}");
    }

    #[test]
    fn attach_render_header_lines_truncate_and_count_message_rows() {
        let lines = attach_render_header_lines(
            "session-with-a-very-long-status-line",
            Some("first long line\nsecond"),
            Some(12),
        );

        assert_eq!(lines, vec!["session-w...", "first lon...", "second"]);
    }

    #[test]
    fn attach_render_header_lines_truncate_wide_chars_to_terminal_cells() {
        let lines = attach_render_header_lines("界界界界", None, Some(5));

        assert_eq!(lines, vec!["界..."]);
        assert!(lines.iter().all(|line| display_cell_width(line) <= 5));
    }

    #[test]
    fn attach_render_header_lines_cap_to_terminal_height() {
        let lines = attach_render_header_lines("status", Some("one\ntwo\nthree\nfour"), Some(12));

        let capped = cap_attach_render_header_lines(
            lines,
            Some(max_attach_render_header_rows(PtySize { cols: 12, rows: 4 })),
            Some(12),
        );

        assert_eq!(capped, vec!["status", "one", "... 3 mor..."]);
    }

    #[test]
    fn format_attach_render_frame_uses_capped_header_rows_for_cursor() {
        let snapshot = RenderedAttachSnapshot {
            text: "pane\r\n".to_string(),
            regions: vec![PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 10,
            }],
            cursor: Some(RenderedCursor {
                row: 0,
                col: 1,
                visible: true,
            }),
        };
        let header_lines = cap_attach_render_header_lines(
            attach_render_header_lines("status", Some("one\ntwo\nthree\nfour"), Some(12)),
            Some(3),
            Some(12),
        );

        let body = String::from_utf8(format_attach_render_frame_body_with_header_lines(
            &header_lines,
            &snapshot,
            1,
        ))
        .expect("render frame utf8");

        assert!(body.starts_with("HEADER_ROWS\t3\n"), "{body:?}");
        assert!(body.contains("\x1b[2K... 3 mor...\r\n"), "{body:?}");
        assert!(body.ends_with("\x1b[?25h\x1b[4;2H"), "{body:?}");
        assert!(!body.contains("\x1b[2Ktwo\r\n"), "{body:?}");
    }

    #[test]
    fn format_attach_render_frame_clears_rows_below_short_snapshot() {
        let snapshot = RenderedAttachSnapshot {
            text: "short\r\n".to_string(),
            regions: vec![PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 10,
            }],
            cursor: None,
        };

        let body = String::from_utf8(format_attach_render_frame_body("", None, &snapshot, 3))
            .expect("render frame utf8");

        assert!(
            body.contains("\x1b[H\x1b[2Kshort\r\n\x1b[2K\r\n\x1b[2K"),
            "{body:?}"
        );
    }

    #[test]
    fn format_attach_render_frame_places_cursor_after_output() {
        let snapshot = RenderedAttachSnapshot {
            text: "left │ right\r\n".to_string(),
            regions: vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 4,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 7,
                    col_end: 12,
                },
            ],
            cursor: Some(RenderedCursor {
                row: 0,
                col: 9,
                visible: true,
            }),
        };

        let body = String::from_utf8(format_attach_render_frame_body(
            "status", None, &snapshot, 10,
        ))
        .expect("render frame utf8");

        assert!(body.contains("\x1b[2Kleft │ right\r\n"), "{body:?}");
        assert!(body.ends_with("\x1b[?25h\x1b[2;10H"), "{body:?}");
    }

    #[test]
    fn format_attach_render_frame_preserves_hidden_cursor_state() {
        let snapshot = RenderedAttachSnapshot {
            text: "hidden\r\n".to_string(),
            regions: vec![PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 10,
            }],
            cursor: Some(RenderedCursor {
                row: 0,
                col: 6,
                visible: false,
            }),
        };

        let body = String::from_utf8(format_attach_render_frame_body(
            "status", None, &snapshot, 10,
        ))
        .expect("render frame utf8");

        assert!(body.contains("\x1b[2Khidden\r\n"), "{body:?}");
        assert!(body.ends_with("\x1b[?25l\x1b[2;7H"), "{body:?}");
        assert!(!body.contains("\x1b[?25h\x1b[2;7H"), "{body:?}");
    }

    #[test]
    fn format_attach_render_frame_clamps_cursor_to_visible_snapshot_rows() {
        let snapshot = RenderedAttachSnapshot {
            text: "top\r\nbottom\r\n".to_string(),
            regions: vec![PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 2,
                col_start: 0,
                col_end: 10,
            }],
            cursor: Some(RenderedCursor {
                row: 1,
                col: 2,
                visible: true,
            }),
        };

        let body = String::from_utf8(format_attach_render_frame_body(
            "status", None, &snapshot, 1,
        ))
        .expect("render frame utf8");

        assert!(body.contains("\x1b[?25h\x1b[2;3H"), "{body:?}");
        assert!(!body.contains("\x1b[3;3H"), "{body:?}");
    }

    #[test]
    fn render_attach_pane_snapshot_with_regions_returns_fallback_without_regions() {
        let layout = LayoutNode::Pane(0);
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "base-ready\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "split-ready\n".to_string(),
            },
        ];

        let snapshot = render_attach_pane_snapshot_with_regions(&layout, &panes);

        assert!(snapshot.regions.is_empty());
        assert!(
            snapshot.text.contains("-- pane 0 --"),
            "{:?}",
            snapshot.text
        );
        assert!(
            snapshot.text.contains("-- pane 1 --"),
            "{:?}",
            snapshot.text
        );
    }

    #[test]
    fn render_attach_layout_stacks_vertical_panes() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "base-ready\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "split-ready\n".to_string(),
            },
        ];

        let rendered = render_attach_pane_snapshot(&layout, &panes);

        assert_eq!(rendered, "base-ready\r\n───────────\r\nsplit-ready\r\n");
    }

    #[test]
    fn render_attach_layout_maps_vertical_pane_regions() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "top\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "bottom\n".to_string(),
            },
        ];

        let rendered = render_attach_layout(&layout, &panes).unwrap();
        let top = rendered
            .regions
            .iter()
            .find(|region| region.pane == 0)
            .unwrap();
        let bottom = rendered
            .regions
            .iter()
            .find(|region| region.pane == 1)
            .unwrap();

        assert!(top.row_end <= bottom.row_start, "{rendered:?}");

        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 6,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 2,
                    row_end: 3,
                    col_start: 0,
                    col_end: 6,
                },
            ]
        );
    }

    #[test]
    fn render_attach_layout_offsets_nested_pane_regions() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(1)),
                second: Box::new(LayoutNode::Pane(2)),
            }),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "left\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "top\n".to_string(),
            },
            PaneSnapshot {
                index: 2,
                screen: "bottom\n".to_string(),
            },
        ];

        let rendered = render_attach_layout(&layout, &panes).unwrap();

        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 3,
                    col_start: 0,
                    col_end: 4,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 7,
                    col_end: 13,
                },
                PaneRegion {
                    pane: 2,
                    row_start: 2,
                    row_end: 3,
                    col_start: 7,
                    col_end: 13,
                },
            ]
        );
    }

    #[test]
    fn render_attach_layout_expands_only_bottom_nested_region_rows() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }),
            second: Box::new(LayoutNode::Pane(2)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "top\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "bottom\n".to_string(),
            },
            PaneSnapshot {
                index: 2,
                screen: "right0\nright1\nright2\nright3\n".to_string(),
            },
        ];

        let rendered = render_attach_layout(&layout, &panes).unwrap();

        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 6,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 2,
                    row_end: 4,
                    col_start: 0,
                    col_end: 6,
                },
                PaneRegion {
                    pane: 2,
                    row_start: 0,
                    row_end: 4,
                    col_start: 9,
                    col_end: 15,
                },
            ]
        );
    }

    #[test]
    fn render_attach_layout_expands_right_region_rows_for_left_padding() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "left0\nleft1\nleft2\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "right\n".to_string(),
            },
        ];

        let rendered = render_attach_layout(&layout, &panes).unwrap();

        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 3,
                    col_start: 0,
                    col_end: 5,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 3,
                    col_start: 8,
                    col_end: 13,
                },
            ]
        );
    }

    #[test]
    fn render_attach_layout_returns_none_when_layout_omits_visible_pane() {
        let layout = LayoutNode::Pane(0);
        let panes = vec![
            PaneSnapshot {
                index: 0,
                screen: "base-ready\n".to_string(),
            },
            PaneSnapshot {
                index: 1,
                screen: "split-ready\n".to_string(),
            },
        ];

        assert!(render_attach_layout(&layout, &panes).is_none());

        let rendered = render_attach_pane_snapshot(&layout, &panes);

        assert!(rendered.contains("-- pane 0 --"), "{rendered:?}");
        assert!(rendered.contains("base-ready"), "{rendered:?}");
        assert!(rendered.contains("-- pane 1 --"), "{rendered:?}");
        assert!(rendered.contains("split-ready"), "{rendered:?}");
    }

    #[test]
    fn render_attach_frame_for_size_preserves_styled_pane_output() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let mut left = TerminalState::new(20, 1, 100);
        left.apply_bytes(b"\x1b[31mleft\x1b[0m");
        let mut right = TerminalState::new(20, 1, 100);
        right.apply_bytes(b"\x1b[1;38;2;1;2;3mright\x1b[0m");
        let panes = vec![
            PaneRenderSnapshot {
                index: 0,
                terminal: &left,
            },
            PaneRenderSnapshot {
                index: 1,
                terminal: &right,
            },
        ];

        let rendered =
            render_attach_frame_for_size(&layout, &panes, PtySize { cols: 20, rows: 1 }).unwrap();

        assert_eq!(
            rendered.text,
            "\x1b[31mleft\x1b[0m     │ \x1b[1;38;2;1;2;3mright\x1b[0m    \r\n"
        );
        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 8,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 11,
                    col_end: 20,
                },
            ]
        );
    }

    #[test]
    fn render_attach_frame_for_size_clips_styled_content_to_region_width() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let mut left = TerminalState::new(20, 1, 100);
        left.apply_bytes(b"\x1b[31mabcdef\x1b[0m");
        let mut right = TerminalState::new(20, 1, 100);
        right.apply_bytes(b"right");
        let panes = vec![
            PaneRenderSnapshot {
                index: 0,
                terminal: &left,
            },
            PaneRenderSnapshot {
                index: 1,
                terminal: &right,
            },
        ];

        let rendered =
            render_attach_frame_for_size(&layout, &panes, PtySize { cols: 10, rows: 1 }).unwrap();

        assert_eq!(rendered.text, "\x1b[31mabc\x1b[0m │ righ\r\n");
        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 3,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 1,
                    col_start: 6,
                    col_end: 10,
                },
            ]
        );
    }

    #[test]
    fn render_attach_frame_for_size_clips_wide_utf8_to_region_width() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let mut left = TerminalState::new(20, 1, 100);
        left.apply_bytes("한글".as_bytes());
        let mut right = TerminalState::new(20, 1, 100);
        right.apply_bytes(b"right");
        let panes = vec![
            PaneRenderSnapshot {
                index: 0,
                terminal: &left,
            },
            PaneRenderSnapshot {
                index: 1,
                terminal: &right,
            },
        ];

        let rendered =
            render_attach_frame_for_size(&layout, &panes, PtySize { cols: 10, rows: 1 }).unwrap();

        assert_eq!(rendered.text, "한  │ righ\r\n");
    }

    #[test]
    fn render_attach_frame_for_size_maps_active_pane_cursor() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let mut left = TerminalState::new(20, 1, 100);
        left.apply_bytes(b"left");
        let mut right = TerminalState::new(20, 1, 100);
        right.apply_bytes(b"ab");
        let panes = vec![
            PaneRenderSnapshot {
                index: 0,
                terminal: &left,
            },
            PaneRenderSnapshot {
                index: 1,
                terminal: &right,
            },
        ];

        let rendered = render_attach_frame_for_size_with_active(
            &layout,
            &panes,
            PtySize { cols: 20, rows: 1 },
            Some(1),
        )
        .unwrap();

        assert_eq!(
            rendered.cursor,
            Some(RenderedCursor {
                row: 0,
                col: 13,
                visible: true,
            })
        );
    }

    #[test]
    fn render_attach_frame_for_size_uses_visible_width_for_vertical_separator() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };
        let mut top = TerminalState::new(20, 1, 100);
        top.apply_bytes(b"\x1b[31mt\x1b[0m");
        let mut bottom = TerminalState::new(20, 1, 100);
        bottom.apply_bytes(b"bottom");
        let panes = vec![
            PaneRenderSnapshot {
                index: 0,
                terminal: &top,
            },
            PaneRenderSnapshot {
                index: 1,
                terminal: &bottom,
            },
        ];

        let rendered =
            render_attach_frame_for_size(&layout, &panes, PtySize { cols: 6, rows: 3 }).unwrap();

        assert_eq!(
            rendered.text,
            "\x1b[31mt\x1b[0m     \r\n──────\r\nbottom\r\n"
        );
        assert_eq!(
            rendered.regions,
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 1,
                    col_start: 0,
                    col_end: 6,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 2,
                    row_end: 3,
                    col_start: 0,
                    col_end: 6,
                },
            ]
        );
    }

    #[test]
    fn buffer_store_evicts_oldest_buffer_at_capacity() {
        let mut store = BufferStore::new();

        for index in 0..=MAX_BUFFERS {
            store
                .save(Some(&format!("buffer-{index}")), format!("text-{index}"))
                .unwrap();
        }

        assert!(store.resolve(Some("buffer-0")).is_none());
        assert!(
            store
                .resolve(Some(&format!("buffer-{MAX_BUFFERS}")))
                .is_some()
        );
        assert_eq!(store.list().len(), MAX_BUFFERS);
    }

    #[test]
    fn read_control_line_rejects_oversized_line() {
        let bytes = vec![b'a'; MAX_CONTROL_LINE_BYTES + 1];
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(bytes));

        let err = read_control_line(&mut reader).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "request line too long");
    }

    #[test]
    fn selected_buffer_text_returns_line_range() {
        let text = select_buffer_text(
            "first\nkeep-one\nkeep-two\nlast\n",
            &BufferSelection::LineRange { start: 2, end: 3 },
        )
        .unwrap();
        assert_eq!(text, "keep-one\nkeep-two\n");
    }

    #[test]
    fn selected_buffer_text_returns_first_search_match() {
        let text = select_buffer_text(
            "first\nneedle-one\nneedle-two\n",
            &BufferSelection::Search {
                needle: "needle".to_string(),
                match_index: 1,
            },
        )
        .unwrap();
        assert_eq!(text, "needle-one\n");
    }

    #[test]
    fn selected_buffer_text_returns_requested_search_match() {
        let text = select_buffer_text(
            "first\nneedle-one\nneedle-two\n",
            &BufferSelection::Search {
                needle: "needle".to_string(),
                match_index: 2,
            },
        )
        .unwrap();
        assert_eq!(text, "needle-two\n");
    }

    #[test]
    fn selected_buffer_text_supports_negative_tail_line_range() {
        let text = select_buffer_text(
            "first\nmiddle\ntail-one\ntail-two\n",
            &BufferSelection::LineRange { start: -2, end: -1 },
        )
        .unwrap();
        assert_eq!(text, "tail-one\ntail-two\n");
    }

    #[test]
    fn selected_buffer_text_rejects_missing_line_range() {
        let err = select_buffer_text(
            "first\nsecond\n",
            &BufferSelection::LineRange { start: 2, end: 3 },
        )
        .unwrap_err();
        assert_eq!(err, "missing line");
    }

    #[test]
    fn selected_buffer_text_rejects_missing_search_match() {
        let err = select_buffer_text(
            "first\nsecond\n",
            &BufferSelection::Search {
                needle: "needle".to_string(),
                match_index: 1,
            },
        )
        .unwrap_err();
        assert_eq!(err, "missing match");
    }

    #[test]
    fn format_copy_mode_lines_numbers_all_lines() {
        let text = format_copy_mode_lines("first\nsecond\n", None, None).unwrap();
        assert_eq!(text, "1\tfirst\n2\tsecond\n");
    }

    #[test]
    fn format_copy_mode_lines_filters_search_matches() {
        let text = format_copy_mode_lines(
            "first\nneedle-one\nlast\nneedle-two\n",
            Some("needle"),
            None,
        )
        .unwrap();
        assert_eq!(text, "2\tneedle-one\n4\tneedle-two\n");
    }

    #[test]
    fn format_copy_mode_lines_filters_search_match_index() {
        let text = format_copy_mode_lines(
            "first\nneedle-one\nlast\nneedle-two\n",
            Some("needle"),
            Some(2),
        )
        .unwrap();
        assert_eq!(text, "4\tneedle-two\n");
    }
}
