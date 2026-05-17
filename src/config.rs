use crate::protocol::{KeyBinding, OptionEntry};

pub const DEFAULT_PREFIX_KEY: &str = "C-b";
pub const OPTION_PREFIX: &str = "prefix";
pub const OPTION_STATUS_HINTS: &str = "status-hints";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Byte(u8),
    Left,
    Down,
    Up,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyStroke {
    pub fn is_global_binding(self) -> bool {
        self.alt
    }
}

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
        ("Left", "select-pane -L"),
        ("Down", "select-pane -D"),
        ("Up", "select-pane -U"),
        ("Right", "select-pane -R"),
        ("M-h", "select-pane -L"),
        ("M-j", "select-pane -D"),
        ("M-k", "select-pane -U"),
        ("M-l", "select-pane -R"),
        ("M-Left", "select-pane -L"),
        ("M-Down", "select-pane -D"),
        ("M-Up", "select-pane -U"),
        ("M-Right", "select-pane -R"),
        ("H", "resize-pane -L"),
        ("J", "resize-pane -D"),
        ("K", "resize-pane -U"),
        ("L", "resize-pane -R"),
        ("C-Left", "resize-pane -L 1"),
        ("C-Down", "resize-pane -D 1"),
        ("C-Up", "resize-pane -U 1"),
        ("C-Right", "resize-pane -R 1"),
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
    parse_key_stroke(input).map(canonical_key_stroke)
}

pub fn parse_key_stroke(input: &str) -> Result<KeyStroke, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("key must not be empty".to_string());
    }

    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let parts = input.split('-').collect::<Vec<_>>();
    let (modifiers, key) = parts.split_at(parts.len().saturating_sub(1));
    let key = key.first().copied().unwrap_or("");

    for modifier in modifiers {
        match modifier.to_ascii_lowercase().as_str() {
            "c" | "ctrl" | "control" => ctrl = true,
            "m" | "meta" | "alt" => alt = true,
            "s" | "shift" => shift = true,
            _ => return invalid_key(input),
        }
    }

    let mut code = match key.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Byte(b' '),
        "tab" => KeyCode::Byte(b'\t'),
        "enter" => KeyCode::Byte(b'\r'),
        "escape" | "esc" => KeyCode::Byte(0x1b),
        "left" => KeyCode::Left,
        "down" => KeyCode::Down,
        "up" => KeyCode::Up,
        "right" => KeyCode::Right,
        _ if key.len() == 1 => {
            let byte = key.as_bytes()[0];
            if !byte.is_ascii_graphic() {
                return invalid_key(input);
            }
            KeyCode::Byte(byte)
        }
        _ => return invalid_key(input),
    };

    if ctrl {
        match code {
            KeyCode::Byte(byte) if byte.is_ascii_alphabetic() => {
                code = KeyCode::Byte(byte.to_ascii_lowercase() - b'a' + 1);
            }
            KeyCode::Left | KeyCode::Down | KeyCode::Up | KeyCode::Right => {}
            _ => return invalid_key(input),
        }
    }

    Ok(KeyStroke {
        code,
        ctrl,
        alt,
        shift,
    })
}

fn invalid_key<T>(input: &str) -> Result<T, String> {
    Err(format!(
        "invalid key {input:?}; use a printable key, Space, Tab, Enter, Escape, Left, Down, Up, Right, C-<letter>, C-<arrow>, or M-<key>"
    ))
}

pub fn canonical_key_stroke(key: KeyStroke) -> String {
    let mut output = String::new();
    if key.ctrl {
        output.push_str("C-");
    }
    if key.alt {
        output.push_str("M-");
    }
    if key.shift {
        output.push_str("S-");
    }
    output.push_str(match key.code {
        KeyCode::Byte(b' ') => "Space",
        KeyCode::Byte(b'\t') => "Tab",
        KeyCode::Byte(b'\r') => "Enter",
        KeyCode::Byte(0x1b) => "Escape",
        KeyCode::Byte(byte) if byte.is_ascii_control() && key.ctrl => {
            let letter = (byte + b'a' - 1) as char;
            return format!("{}{}", output, letter);
        }
        KeyCode::Byte(byte) => return format!("{}{}", output, byte as char),
        KeyCode::Left => "Left",
        KeyCode::Down => "Down",
        KeyCode::Up => "Up",
        KeyCode::Right => "Right",
    });
    output
}

pub fn key_name_to_byte(key: &str) -> Result<u8, String> {
    match parse_key_stroke(key)? {
        KeyStroke {
            code: KeyCode::Byte(byte),
            alt: false,
            shift: false,
            ..
        } => Ok(byte),
        _ => Err(format!(
            "invalid key {key:?}; prefix must be a byte-backed key"
        )),
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
    if let Some((base, amount)) = normalized.rsplit_once(' ') {
        if SUPPORTED_BINDING_COMMANDS.contains(&base)
            && base.starts_with("resize-pane ")
            && amount.parse::<usize>().is_ok_and(|amount| amount > 0)
        {
            return Ok(normalized);
        }
    }
    Err(format!(
        "unsupported binding command {command:?}; supported live actions include send-prefix, detach-client, copy-mode, command-prompt, split-window -h|-v, select-pane -L|-D|-U|-R, resize-pane -L|-D|-U|-R [amount], next-pane, new-window, next-window, previous-window, kill-pane, and zoom-pane"
    ))
}

pub fn validate_option_value(name: &str, value: &str) -> Result<String, String> {
    match name {
        OPTION_PREFIX => {
            key_name_to_byte(value)?;
            canonical_key(value)
        }
        OPTION_STATUS_HINTS => match value {
            "on" | "off" => Ok(value.to_string()),
            _ => Err("status-hints must be on or off".to_string()),
        },
        _ => Err(format!("unknown option {name:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_accepts_directional_and_modified_keys() {
        assert_eq!(canonical_key("Left").unwrap(), "Left");
        assert_eq!(canonical_key("right").unwrap(), "Right");
        assert_eq!(canonical_key("M-h").unwrap(), "M-h");
        assert_eq!(canonical_key("Alt-Left").unwrap(), "M-Left");
        assert_eq!(canonical_key("C-Left").unwrap(), "C-Left");
        assert_eq!(canonical_key("Ctrl-Right").unwrap(), "C-Right");
    }

    #[test]
    fn prefix_option_stays_byte_backed() {
        assert!(validate_option_value(OPTION_PREFIX, "Left").is_err());
        assert!(validate_option_value(OPTION_PREFIX, "M-h").is_err());
        assert_eq!(validate_option_value(OPTION_PREFIX, "C-a").unwrap(), "C-a");
    }

    #[test]
    fn binding_command_accepts_resize_amounts() {
        assert_eq!(
            validate_binding_command("resize-pane -L 1").unwrap(),
            "resize-pane -L 1"
        );
        assert!(validate_binding_command("resize-pane -L 0").is_err());
    }
}
