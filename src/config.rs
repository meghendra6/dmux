use crate::protocol::{KeyBinding, OptionEntry};

pub const DEFAULT_PREFIX_KEY: &str = "C-b";
pub const OPTION_PREFIX: &str = "prefix";
pub const OPTION_STATUS_HINTS: &str = "status-hints";

const SUPPORTED_BINDING_COMMANDS: &[&str] = &[
    "detach-client",
    "copy-mode",
    "display-panes",
    "show-help",
    "command-prompt",
    "next-pane",
    "new-window",
    "next-window",
    "previous-window",
    "split-window -h",
    "split-window -v",
    "select-pane -L",
    "select-pane -D",
    "select-pane -U",
    "select-pane -R",
    "resize-pane -L",
    "resize-pane -D",
    "resize-pane -U",
    "resize-pane -R",
    "kill-pane",
    "zoom-pane",
];

pub fn default_key_bindings() -> Vec<KeyBinding> {
    [
        ("C-b", "send-prefix"),
        ("d", "detach-client"),
        ("D", "detach-client"),
        ("[", "copy-mode"),
        ("o", "next-pane"),
        ("q", "display-panes"),
        ("?", "show-help"),
        (":", "command-prompt"),
        ("%", "split-window -h"),
        ("c", "new-window"),
        ("n", "next-window"),
        ("p", "previous-window"),
        ("\"", "split-window -v"),
        ("h", "select-pane -L"),
        ("j", "select-pane -D"),
        ("k", "select-pane -U"),
        ("l", "select-pane -R"),
        ("H", "resize-pane -L"),
        ("J", "resize-pane -D"),
        ("K", "resize-pane -U"),
        ("L", "resize-pane -R"),
        ("x", "kill-pane"),
        ("z", "zoom-pane"),
    ]
    .into_iter()
    .map(|(key, command)| KeyBinding {
        key: key.to_string(),
        command: command.to_string(),
    })
    .collect()
}

pub fn default_options() -> Vec<OptionEntry> {
    vec![
        OptionEntry {
            name: OPTION_PREFIX.to_string(),
            value: DEFAULT_PREFIX_KEY.to_string(),
        },
        OptionEntry {
            name: OPTION_STATUS_HINTS.to_string(),
            value: "on".to_string(),
        },
    ]
}

pub fn canonical_key(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("key must not be empty".to_string());
    }
    if input.eq_ignore_ascii_case("space") {
        return Ok("Space".to_string());
    }
    if input.eq_ignore_ascii_case("tab") {
        return Ok("Tab".to_string());
    }
    if input.eq_ignore_ascii_case("enter") {
        return Ok("Enter".to_string());
    }
    if input.eq_ignore_ascii_case("escape") || input.eq_ignore_ascii_case("esc") {
        return Ok("Escape".to_string());
    }
    if input.len() == 1 {
        let byte = input.as_bytes()[0];
        if byte.is_ascii_graphic() {
            return Ok(input.to_string());
        }
    }
    if input.len() == 3
        && input.as_bytes()[0].eq_ignore_ascii_case(&b'C')
        && input.as_bytes()[1] == b'-'
    {
        let letter = input.as_bytes()[2];
        if letter.is_ascii_alphabetic() {
            return Ok(format!("C-{}", (letter as char).to_ascii_lowercase()));
        }
    }
    Err(format!(
        "invalid key {input:?}; use a printable key, Space, Tab, Enter, Escape, or C-<letter>"
    ))
}

pub fn key_name_to_byte(key: &str) -> Result<u8, String> {
    match canonical_key(key)?.as_str() {
        "Space" => Ok(b' '),
        "Tab" => Ok(b'\t'),
        "Enter" => Ok(b'\r'),
        "Escape" => Ok(0x1b),
        key if key.starts_with("C-") => {
            let letter = key.as_bytes()[2];
            Ok(letter.to_ascii_lowercase() - b'a' + 1)
        }
        key if key.len() == 1 => Ok(key.as_bytes()[0]),
        _ => Err(format!("invalid key {key:?}")),
    }
}

pub fn validate_binding_command(command: &str) -> Result<String, String> {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return Err("binding command must not be empty".to_string());
    }
    if normalized == "send-prefix" || SUPPORTED_BINDING_COMMANDS.contains(&normalized.as_str()) {
        return Ok(normalized);
    }
    Err(format!(
        "unsupported binding command {command:?}; supported live actions include send-prefix, detach-client, copy-mode, command-prompt, split-window -h|-v, select-pane -L|-D|-U|-R, resize-pane -L|-D|-U|-R, next-pane, new-window, next-window, previous-window, kill-pane, and zoom-pane"
    ))
}

pub fn validate_option_value(name: &str, value: &str) -> Result<String, String> {
    match name {
        OPTION_PREFIX => canonical_key(value),
        OPTION_STATUS_HINTS => match value {
            "on" | "off" => Ok(value.to_string()),
            _ => Err("status-hints must be on or off".to_string()),
        },
        _ => Err(format!("unknown option {name:?}")),
    }
}
