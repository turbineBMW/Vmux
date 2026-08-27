use gtk4::glib;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Render the whole app at an integer scale (2×) and let the compositor
    /// downscale, instead of GTK rasterizing at a fractional scale directly.
    /// On fractional-scaled HiDPI displays this is what keeps diagonal glyphs
    /// (the powerline separators U+E0B0+) from stair-stepping — GTK otherwise
    /// rasterizes the font glyph at too low a resolution and scales it up,
    /// which native terminals like foot avoid. Costs extra GPU/memory (4× the
    /// pixels); turn off if you're on an integer-scale (1×/2×) display where it
    /// only softens text. Implemented by exporting GDK_SCALE=2 before GTK init.
    pub force_integer_scale: bool,
    pub scrollback_lines: i64,
    pub shell: Option<String>,
    /// Hide the window titlebar (the sidebar header keeps the app menu).
    pub hide_titlebar: bool,
    /// Whether the zone sidebar is shown; restored across restarts.
    pub show_sidebar: bool,
    /// Desktop notifications when a background terminal emits a
    /// notification escape (OSC 9 / 777 / kitty 99).
    pub desktop_notifications: bool,
    /// Also raise a desktop notification on the terminal bell.
    pub notify_on_bell: bool,
    /// Which sound the notification daemon is asked to play.
    pub notification_sound: Sound,
    /// Overrides of the default keybindings, action id -> accelerator
    /// ("" disables the binding). Defaults live in keybinds::ACTIONS.
    pub keybindings: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            force_integer_scale: true,
            scrollback_lines: 10_000,
            shell: None,
            hide_titlebar: false,
            show_sidebar: true,
            desktop_notifications: true,
            notify_on_bell: false,
            notification_sound: Sound::default(),
            keybindings: BTreeMap::new(),
        }
    }
}

/// Which sound the notification daemon is asked to play. Vmux never plays
/// audio itself: the choice travels as a hint on the notification so the
/// shell (and its do-not-disturb logic) stays in charge of whether anything
/// is actually heard.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(tag = "kind", content = "path", rename_all = "kebab-case")]
pub enum Sound {
    /// `sound-name = message-new-instant`, resolved from the system sound theme.
    #[default]
    SystemDefault,
    /// `sound-file = <path>`.
    File(PathBuf),
    /// `suppress-sound = true`.
    None,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct TabState {
    pub cwd: String,
}

/// A zone's layout: a binary split tree whose leaves are tabbed panes.
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeState {
    Pane {
        #[serde(default)]
        tabs: Vec<TabState>,
        #[serde(default)]
        active_tab: usize,
    },
    Split {
        orientation: String, // "horizontal" | "vertical"
        #[serde(default = "default_ratio")]
        ratio: f64,
        first: Box<NodeState>,
        second: Box<NodeState>,
    },
}

fn default_ratio() -> f64 {
    0.5
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct ZoneState {
    pub name: String,
    pub cwd: String,
    pub root: Option<NodeState>,
    /// Legacy (pre-pane-tree) tab list; consumed by effective_root().
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<TabState>,
}

impl Default for ZoneState {
    fn default() -> Self {
        Self {
            name: "main".into(),
            cwd: home_dir(),
            root: None,
            tabs: Vec::new(),
        }
    }
}

impl ZoneState {
    pub fn effective_root(&self) -> NodeState {
        if let Some(root) = &self.root {
            return root.clone();
        }
        let tabs = if self.tabs.is_empty() {
            vec![TabState { cwd: self.cwd.clone() }]
        } else {
            self.tabs.clone()
        };
        NodeState::Pane { tabs, active_tab: 0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct AppState {
    pub config: Config,
    pub zones: Vec<ZoneState>,
    pub active_zone: usize,
}

pub fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".into())
}

pub fn state_path() -> PathBuf {
    glib::user_config_dir().join("vmux").join("state.json")
}

pub fn load() -> AppState {
    let mut st: AppState = fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if st.zones.is_empty() {
        st.zones.push(ZoneState::default());
    }
    st.active_zone = st.active_zone.min(st.zones.len() - 1);
    st
}

pub fn save(st: &AppState) {
    let path = state_path();
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(st).unwrap_or_default())?;
        fs::rename(&tmp, &path)?;
        Ok(())
    };
    if let Err(e) = write() {
        eprintln!("vmux: failed to save state: {e}");
    }
}

pub fn default_shell(cfg: &Config) -> String {
    cfg.shell
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/bash".into())
}

pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        home_dir()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{rest}", home_dir())
    } else {
        path.to_string()
    }
}

pub fn display_name(cwd: &str) -> String {
    let home = home_dir();
    if cwd == home {
        return "~".into();
    }
    std::path::Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_appearance_fields_are_ignored() {
        let cfg: Config = serde_json::from_str(
            r##"{
                "font": "Legacy Mono 12",
                "background_opacity": 0.8,
                "theme": {
                    "foreground": "#FFFFFF",
                    "background": "#000000",
                    "cursor": "#FFFFFF",
                    "palette": []
                },
                "scrollback_lines": 5000
            }"##,
        )
        .unwrap();
        assert_eq!(cfg.scrollback_lines, 5000);
        assert!(cfg.desktop_notifications);
        assert!(!cfg.notify_on_bell);
        assert_eq!(cfg.notification_sound, Sound::SystemDefault);
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("font"));
        assert!(!json.contains("background_opacity"));
        assert!(!json.contains("theme"));
    }
}
