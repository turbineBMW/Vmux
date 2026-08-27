use gtk4::{gdk, glib};
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

/// Written to bindings.conf on first run; also the built-in defaults when the
/// file cannot be read.
pub const TEMPLATE: &str = r#"# vmux terminal key bindings.
#
# Each line sends bytes to the foreground program when a key is pressed:
#
#     key combo = bytes
#
# Key combos use foot-style "Control+Shift+c" or GTK-style
# "<Control><Shift>c" syntax; key names are XKB keysym names (Return,
# BackSpace, Left, Home, Page_Up, plus, ...). List Shift explicitly.
# Bytes are literal text plus the escapes \xHH, \e (ESC), \n, \r, \t, \\.
# Comments start with # on their own line. Edits apply live; application
# shortcuts (Preferences > Keybindings) take precedence over these.

# Send Shift+Enter as a CSI-u modified key so tmux and other CSI-u aware
# programs can tell it apart from plain Enter.
Shift+Return = \x1b[13;2u

# Examples (readline-style word navigation):
#Control+Left = \eb
#Control+Right = \ef
#Control+BackSpace = \x17
"#;

/// One parsed line: pressing `key`+`mods` feeds `bytes` to the child.
pub struct TextBinding {
    key: gdk::Key,
    mods: gdk::ModifierType,
    pub bytes: Vec<u8>,
}

impl TextBinding {
    /// `mods` must already be masked with accelerator_get_default_mod_mask().
    pub fn matches(&self, keyval: gdk::Key, mods: gdk::ModifierType) -> bool {
        mods == self.mods && (keyval == self.key || keyval.to_lower() == self.key)
    }
}

pub fn path() -> PathBuf {
    glib::user_config_dir().join("vmux").join("bindings.conf")
}

/// Read bindings.conf, creating it from the template on first run. Any other
/// read failure falls back to the template's built-in defaults.
pub fn load() -> Vec<TextBinding> {
    let path = path();
    match fs::read_to_string(&path) {
        Ok(text) => parse_str(&text),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Err(e) = fs::write(&path, TEMPLATE) {
                eprintln!("vmux: cannot write {}: {e}", path.display());
            }
            parse_str(TEMPLATE)
        }
        Err(e) => {
            eprintln!("vmux: cannot read {}: {e}", path.display());
            parse_str(TEMPLATE)
        }
    }
}

pub fn parse_str(text: &str) -> Vec<TextBinding> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((combo, value)) = line.split_once('=') else {
            warn(i, "expected \"key combo = bytes\"");
            continue;
        };
        let Some((key, mods)) = parse_combo(combo.trim()) else {
            warn(i, &format!("unrecognized key combo {:?}", combo.trim()));
            continue;
        };
        out.push(TextBinding {
            key,
            mods,
            bytes: unescape(value.trim()),
        });
    }
    out
}

fn warn(line_idx: usize, msg: &str) {
    eprintln!("vmux: bindings.conf line {}: {msg}", line_idx + 1);
}

/// Accepts foot-style "Control+Shift+c" and GTK-style "<Control><Shift>c".
/// Key names are XKB keysym names, as in both source formats.
fn parse_combo(s: &str) -> Option<(gdk::Key, gdk::ModifierType)> {
    let (mod_names, key_name) = if s.starts_with('<') {
        let mut names = Vec::new();
        let mut rest = s;
        while let Some(r) = rest.strip_prefix('<') {
            let (name, after) = r.split_once('>')?;
            names.push(name);
            rest = after;
        }
        (names, rest)
    } else {
        let mut parts: Vec<&str> = s.split('+').collect();
        let key = parts.pop()?;
        (parts, key)
    };
    let mut mods = gdk::ModifierType::empty();
    for name in mod_names {
        mods |= match name.to_ascii_lowercase().as_str() {
            "control" | "ctrl" | "primary" => gdk::ModifierType::CONTROL_MASK,
            "shift" => gdk::ModifierType::SHIFT_MASK,
            "alt" | "mod1" => gdk::ModifierType::ALT_MASK,
            "super" | "mod4" => gdk::ModifierType::SUPER_MASK,
            "meta" => gdk::ModifierType::META_MASK,
            "hyper" => gdk::ModifierType::HYPER_MASK,
            _ => return None,
        };
    }
    if key_name.is_empty() {
        return None;
    }
    // Events deliver shifted keyvals ("C"); store the lowered form to match.
    Some((gdk::Key::from_name(key_name)?.to_lower(), mods))
}

fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut it = s.chars();
    let mut buf = [0u8; 4];
    while let Some(c) = it.next() {
        if c != '\\' {
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match it.next() {
            Some('x') => {
                let hi = it.next().and_then(|c| c.to_digit(16));
                let lo = it.next().and_then(|c| c.to_digit(16));
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                }
            }
            Some('e') => out.push(0x1b),
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some(other) => {
                out.push(b'\\');
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foot_style_combo() {
        let (key, mods) = parse_combo("Control+Shift+c").unwrap();
        assert_eq!(key, gdk::Key::c);
        assert_eq!(
            mods,
            gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK
        );
    }

    #[test]
    fn gtk_style_combo() {
        let (key, mods) = parse_combo("<Shift>Return").unwrap();
        assert_eq!(key, gdk::Key::Return);
        assert_eq!(mods, gdk::ModifierType::SHIFT_MASK);
    }

    #[test]
    fn bare_key_combo() {
        let (key, mods) = parse_combo("Home").unwrap();
        assert_eq!(key, gdk::Key::Home);
        assert!(mods.is_empty());
    }

    #[test]
    fn unknown_modifier_rejected() {
        assert!(parse_combo("Bogus+x").is_none());
        assert!(parse_combo("").is_none());
    }

    #[test]
    fn shifted_letter_matches_uppercase_keyval() {
        let b = &parse_str(r"Control+Shift+c = \x03")[0];
        let mods = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK;
        assert!(b.matches(gdk::Key::C, mods)); // events deliver the shifted keyval
        assert!(!b.matches(gdk::Key::C, gdk::ModifierType::CONTROL_MASK));
    }

    #[test]
    fn unescape_sequences() {
        assert_eq!(unescape(r"\x1b[13;2u"), b"\x1b[13;2u");
        assert_eq!(unescape(r"\eb"), b"\x1bb");
        assert_eq!(unescape(r"\n\r\t\\"), b"\n\r\t\\");
        assert_eq!(unescape("plain"), b"plain");
    }

    #[test]
    fn parser_skips_comments_and_bad_lines() {
        let parsed = parse_str("# comment\n\nnot a binding\nBogus+x = y\nShift+Return = \\r");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].bytes, b"\r");
    }

    #[test]
    fn template_has_shift_return_default() {
        let parsed = parse_str(TEMPLATE);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].matches(gdk::Key::Return, gdk::ModifierType::SHIFT_MASK));
        assert_eq!(parsed[0].bytes, b"\x1b[13;2u");
    }
}
