use gtk4::glib;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const SCHEMA_MARKER: &str = "vmux-appearance-schema: 1";
const DEFAULT_FOREGROUND: &str = "#d0cfcc";
const DEFAULT_BACKGROUND: &str = "#1d1d20";
const DEFAULT_PALETTE: [&str; 16] = [
    "#171421", "#c01c28", "#26a269", "#a2734c", "#12488b", "#a347ba", "#2aa1b3", "#d0cfcc",
    "#5e5c64", "#f66151", "#33d17a", "#e9ad0c", "#2a7bde", "#c061cb", "#33c7de", "#ffffff",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Declaration {
    NamedColor(&'static str),
    CustomProperty(&'static str),
    TerminalProperty(&'static str),
}

pub const TERMINAL_FOREGROUND: &str = "vmux_terminal_foreground";
pub const TERMINAL_BACKGROUND: &str = "vmux_terminal_background";
pub const TERMINAL_CURSOR: &str = "vmux_terminal_cursor";
pub const TERMINAL_PALETTE: [&str; 16] = [
    "vmux_terminal_black",
    "vmux_terminal_red",
    "vmux_terminal_green",
    "vmux_terminal_yellow",
    "vmux_terminal_blue",
    "vmux_terminal_magenta",
    "vmux_terminal_cyan",
    "vmux_terminal_white",
    "vmux_terminal_bright_black",
    "vmux_terminal_bright_red",
    "vmux_terminal_bright_green",
    "vmux_terminal_bright_yellow",
    "vmux_terminal_bright_blue",
    "vmux_terminal_bright_magenta",
    "vmux_terminal_bright_cyan",
    "vmux_terminal_bright_white",
];

/// Written to style.css on first run; also the fallback when the file can't
/// be read. Appearance lives here rather than in state.json, and every
/// declaration in the managed block is also exposed in the settings GUI.
pub const TEMPLATE: &str = r#"/* vmux appearance. vmux-appearance-schema: 1
 *
 * Plain GTK4 / libadwaita CSS, loaded at the USER priority so any rule here
 * wins over both the Adwaita theme and vmux's own built-in styles. Edits
 * apply live: save the file and the window restyles immediately. Delete the
 * file to restore defaults (it is regenerated from this template).
 */

/* Terminal font and colors. The background may use rgba() for transparency. */
@define-color vmux_terminal_foreground #d0cfcc;
@define-color vmux_terminal_background #1d1d20;
@define-color vmux_terminal_cursor #d0cfcc;

/* ANSI colors 0–15: normal, then bright. */
@define-color vmux_terminal_black #171421;
@define-color vmux_terminal_red #c01c28;
@define-color vmux_terminal_green #26a269;
@define-color vmux_terminal_yellow #a2734c;
@define-color vmux_terminal_blue #12488b;
@define-color vmux_terminal_magenta #a347ba;
@define-color vmux_terminal_cyan #2aa1b3;
@define-color vmux_terminal_white #d0cfcc;
@define-color vmux_terminal_bright_black #5e5c64;
@define-color vmux_terminal_bright_red #f66151;
@define-color vmux_terminal_bright_green #33d17a;
@define-color vmux_terminal_bright_yellow #e9ad0c;
@define-color vmux_terminal_bright_blue #2a7bde;
@define-color vmux_terminal_bright_magenta #c061cb;
@define-color vmux_terminal_bright_cyan #33c7de;
@define-color vmux_terminal_bright_white #ffffff;

.vmux-terminal {
  font-family: monospace;
  font-size: 11pt;
}

/* Focused pane tab states and zone Git summary. */
@define-color vmux_focused_tab_background #243247;
@define-color vmux_focused_tab_foreground #62a0ea;
@define-color vmux_focused_tab_hover #293952;
@define-color vmux_focused_tab_pressed #304563;
@define-color vmux_git_added #26a269;
@define-color vmux_git_modified #e9ad0c;
@define-color vmux_git_deleted #c01c28;
@define-color vmux_git_lines_added #26a269;
@define-color vmux_git_lines_deleted #c01c28;
@define-color vmux_git_ahead #2a7bde;

/* Window chrome and semantic colors. Every value in this managed block is
 * also exposed in Preferences > Appearance. */
:root {
  --accent-bg-color: #3584e4;
  --accent-fg-color: #ffffff;
  --accent-color: #62a0ea;
  --window-bg-color: #1d1d20;
  --window-fg-color: #ffffff;
  --view-bg-color: #1d1d20;
  --view-fg-color: #ffffff;
  --headerbar-bg-color: #303030;
  --headerbar-fg-color: #ffffff;
  --headerbar-backdrop-color: #242424;
  --headerbar-border-color: #3d3d3d;
  --sidebar-bg-color: #242424;
  --sidebar-fg-color: #ffffff;
  --sidebar-backdrop-color: #202020;
  --sidebar-border-color: #3d3d3d;
  --secondary-sidebar-bg-color: #242424;
  --secondary-sidebar-fg-color: #ffffff;
  --secondary-sidebar-backdrop-color: #202020;
  --secondary-sidebar-border-color: #3d3d3d;
  --card-bg-color: #303030;
  --card-fg-color: #ffffff;
  --popover-bg-color: #303030;
  --popover-fg-color: #ffffff;
  --dialog-bg-color: #242424;
  --dialog-fg-color: #ffffff;
  --destructive-bg-color: #c01c28;
  --destructive-fg-color: #ffffff;
  --destructive-color: #f66151;
  --success-bg-color: #26a269;
  --success-fg-color: #ffffff;
  --success-color: #33d17a;
  --warning-bg-color: #cd9309;
  --warning-fg-color: #000000;
  --warning-color: #e9ad0c;
  --error-bg-color: #c01c28;
  --error-fg-color: #ffffff;
  --error-color: #f66151;
  --vmux-root-color: #c01c28;
  --vmux-remote-color: #9141ac;
  --vmux-git-clean-opacity: 0.55;
  --vmux-zone-path-opacity: 0.55;
}

.vmux-pane:focus-within tabbar tab:selected {
  background-color: @vmux_focused_tab_background;
  color: @vmux_focused_tab_foreground;
}
.vmux-pane:focus-within tabbar tab:selected:hover {
  background-color: @vmux_focused_tab_hover;
}
.vmux-pane:focus-within tabbar tab:selected:active {
  background-color: @vmux_focused_tab_pressed;
}
.zone-path .git-added       { color: @vmux_git_added; }
.zone-path .git-modified    { color: @vmux_git_modified; }
.zone-path .git-deleted     { color: @vmux_git_deleted; }
.zone-path .git-lines-added { color: @vmux_git_lines_added; }
.zone-path .git-lines-del   { color: @vmux_git_lines_deleted; }
.zone-path .git-ahead       { color: @vmux_git_ahead; }
.zone-path .git-clean       { opacity: var(--vmux-git-clean-opacity); }
.zone-path .zone-path-dir   { opacity: var(--vmux-zone-path-opacity); }

/* All declarations above are mapped in Preferences > Appearance. You can
 * still add arbitrary GTK CSS below for effects that do not fit those fields.
 * Useful vmux selectors:
 *
 *   .zone-row    - sidebar zone entries, e.g.  .zone-row { padding: 8px 10px; }
 *   .zone-path   - the per-zone secondary line, e.g.  .zone-path { font-size: 0.9em; }
 *   .zone-number - the position chip before the zone name
 *   .zone-avatar - the picture/initials chip
 *   tabbar tab   - pane tab labels, e.g.  tabbar tab { font-weight: bold; }
 *
 * Root and remote classes can be restyled beyond their mapped indicator color:
 *
 *   .vmux-pane tabbar tab.vmux-root   { ... }   any tab running as root
 *   .vmux-pane tabbar tab.vmux-remote:selected { ... }   only when selected
 */
"#;

pub fn path() -> PathBuf {
    glib::user_config_dir().join("vmux").join("style.css")
}

pub fn themes_path() -> PathBuf {
    glib::user_config_dir().join("vmux").join("themes")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Theme {
    name: String,
    path: PathBuf,
}

impl Theme {
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// List saved CSS theme snapshots, sorted case-insensitively by display name.
/// Legacy companion JSON files are intentionally ignored: style.css remains
/// the only appearance format.
pub fn themes() -> std::io::Result<Vec<Theme>> {
    themes_in(&themes_path())
}

/// Snapshot the complete active stylesheet under a user-facing theme name.
/// Saving the same name again replaces that snapshot.
pub fn save_theme(name: &str) -> std::io::Result<Theme> {
    let css = fs::read_to_string(path())?;
    save_theme_in(&themes_path(), name, &css)
}

/// Make a saved snapshot the active stylesheet. The caller reloads the app's
/// CSS provider after this succeeds.
pub fn apply_theme(theme: &Theme) -> std::io::Result<()> {
    let css = fs::read_to_string(&theme.path)?;
    write_css(&path(), &css)
}

/// Index of the saved theme whose bytes exactly match the active stylesheet.
pub fn current_theme(themes: &[Theme]) -> Option<usize> {
    let active = fs::read(path()).ok()?;
    themes
        .iter()
        .position(|theme| fs::read(&theme.path).is_ok_and(|css| css == active))
}

fn themes_in(dir: &Path) -> std::io::Result<Vec<Theme>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut themes = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("css") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        themes.push(Theme {
            name: name.to_string(),
            path,
        });
    }
    themes.sort_unstable_by_key(|theme| theme.name.to_lowercase());
    Ok(themes)
}

fn save_theme_in(dir: &Path, name: &str, css: &str) -> std::io::Result<Theme> {
    let name = valid_theme_name(name)?;
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{name}.css"));
    write_css(&path, css)?;
    Ok(Theme { name, path })
}

fn valid_theme_name(name: &str) -> std::io::Result<String> {
    let name = name.trim();
    let name = name.strip_suffix(".css").unwrap_or(name).trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.chars().count() > 80
        || name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
    {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "theme names must be 1–80 characters and cannot contain / or \\",
        ));
    }
    Ok(name.to_string())
}

fn write_css(path: &Path, css: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map_or_else(|| "tmp".to_string(), |ext| format!("{ext}.tmp")),
    );
    fs::write(&tmp, css)?;
    fs::rename(tmp, path)
}

/// Read style.css, creating it from the template on first run. A missing file
/// is seeded with the template; any other read error logs and yields empty
/// CSS (vmux's built-in styles stay in effect).
pub fn load() -> String {
    let path = path();
    match fs::read_to_string(&path) {
        Ok(text) if !text.contains(SCHEMA_MARKER) => {
            // Before appearance moved into CSS, vmux still created a
            // comment-only style.css and stored terminal colors/font in
            // state.json. Prepend the new managed defaults once, customized
            // with those legacy values, and leave the old stylesheet last so
            // all of the user's existing overrides keep winning.
            let legacy_state = read_legacy_state();
            let upgraded = upgrade_stylesheet(&text, legacy_state.as_deref());
            if let Err(e) = write_css(&path, &upgraded) {
                eprintln!("vmux: cannot upgrade {}: {e}", path.display());
            }
            upgraded
        }
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let legacy_state = read_legacy_state();
            let initial = if legacy_state.as_deref().is_some_and(has_legacy_appearance) {
                upgrade_stylesheet("", legacy_state.as_deref())
            } else {
                TEMPLATE.to_string()
            };
            if let Err(e) = fs::write(&path, &initial) {
                eprintln!("vmux: cannot write {}: {e}", path.display());
            }
            initial
        }
        Err(e) => {
            eprintln!("vmux: cannot read {}: {e}", path.display());
            String::new()
        }
    }
}

fn read_legacy_state() -> Option<String> {
    fs::read_to_string(glib::user_config_dir().join("vmux").join("state.json")).ok()
}

fn has_legacy_appearance(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|state| state.get("config").cloned())
        .is_some_and(|config| {
            ["font", "background_opacity", "theme"]
                .iter()
                .any(|key| config.get(key).is_some())
        })
}

/// Build the one-time upgrade for a pre-appearance-schema stylesheet. The
/// state parameter is deliberately raw JSON: current Config no longer owns
/// visual fields, but an older state file may still contain them at the exact
/// moment this migration runs.
fn upgrade_stylesheet(existing: &str, legacy_state: Option<&str>) -> String {
    let mut managed = TEMPLATE.to_string();
    if let Some(config) = legacy_state
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|state| state.get("config").cloned())
    {
        let theme = config.get("theme");
        let foreground = legacy_color(
            theme.and_then(|value| value.get("foreground")),
            DEFAULT_FOREGROUND,
        );
        let background = legacy_color(
            theme.and_then(|value| value.get("background")),
            DEFAULT_BACKGROUND,
        );
        let cursor = legacy_color(
            theme.and_then(|value| value.get("cursor")),
            DEFAULT_FOREGROUND,
        );
        managed = replace_value(
            &managed,
            Declaration::NamedColor(TERMINAL_FOREGROUND),
            &foreground,
        );
        managed = replace_value(&managed, Declaration::NamedColor(TERMINAL_CURSOR), &cursor);

        let opacity = config
            .get("background_opacity")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0) as f32;
        let mut background = gtk4::gdk::RGBA::parse(&background)
            .unwrap_or_else(|_| gtk4::gdk::RGBA::parse(DEFAULT_BACKGROUND).unwrap());
        background.set_alpha(opacity);
        managed = replace_value(
            &managed,
            Declaration::NamedColor(TERMINAL_BACKGROUND),
            &css_color(&background),
        );

        for (index, name) in TERMINAL_PALETTE.iter().enumerate() {
            let color = legacy_color(
                theme
                    .and_then(|value| value.get("palette"))
                    .and_then(|value| value.get(index)),
                DEFAULT_PALETTE[index],
            );
            managed = replace_value(&managed, Declaration::NamedColor(name), &color);
        }

        if let Some(font) = config.get("font").and_then(serde_json::Value::as_str) {
            let description = gtk4::pango::FontDescription::from_string(font);
            if let Some(family) = description.family() {
                let family: String = family.chars().filter(|ch| !ch.is_control()).collect();
                let family = family.replace('\\', "\\\\").replace('"', "\\\"");
                managed = replace_value(
                    &managed,
                    Declaration::TerminalProperty("font-family"),
                    &format!("\"{family}\""),
                );
            }
            if description.size() > 0 {
                let size = description.size() as f64 / gtk4::pango::SCALE as f64;
                managed = replace_value(
                    &managed,
                    Declaration::TerminalProperty("font-size"),
                    &format!("{size}pt"),
                );
            }
        }
    }

    if existing.trim().is_empty() {
        managed
    } else {
        format!(
            "{}\n\n/* Preserved rules from the previous style.css follow. */\n{}",
            managed.trim_end(),
            existing.trim_start()
        )
    }
}

fn legacy_color(value: Option<&serde_json::Value>, fallback: &str) -> String {
    let value = value
        .and_then(serde_json::Value::as_str)
        .unwrap_or(fallback);
    gtk4::gdk::RGBA::parse(value)
        .or_else(|_| gtk4::gdk::RGBA::parse(fallback))
        .map(|color| css_color(&color))
        .expect("built-in appearance colors are valid")
}

fn css_color(color: &gtk4::gdk::RGBA) -> String {
    let channel = |value: f32| (value * 255.0).round() as u8;
    if color.alpha() >= 0.999 {
        format!(
            "#{:02x}{:02x}{:02x}",
            channel(color.red()),
            channel(color.green()),
            channel(color.blue())
        )
    } else {
        let alpha = format!("{:.3}", color.alpha());
        format!(
            "rgba({}, {}, {}, {})",
            channel(color.red()),
            channel(color.green()),
            channel(color.blue()),
            alpha.trim_end_matches('0').trim_end_matches('.')
        )
    }
}

/// Read one GUI-managed value from the active stylesheet. Commented examples
/// are deliberately ignored.
pub fn value(declaration: Declaration) -> Option<String> {
    value_in(&load(), declaration)
}

/// Replace one GUI-managed declaration without disturbing the rest of the
/// hand-editable stylesheet. The write is atomic so the live file monitor
/// never observes a partially written CSS document.
pub fn set_value(declaration: Declaration, value: &str) -> std::io::Result<()> {
    if value.contains(['\n', '\r', ';', '{', '}']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid CSS declaration value",
        ));
    }
    let path = path();
    let css = match fs::read_to_string(&path) {
        Ok(css) => css,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let _ = load();
            fs::read_to_string(&path)?
        }
        Err(e) => return Err(e),
    };
    let updated = replace_value(&css, declaration, value);
    let tmp = path.with_extension("css.tmp");
    fs::write(&tmp, updated)?;
    fs::rename(tmp, path)
}

fn value_in(css: &str, declaration: Declaration) -> Option<String> {
    declaration_range(css, declaration).map(|range| css[range].trim().to_string())
}

fn replace_value(css: &str, declaration: Declaration, value: &str) -> String {
    if let Some(range) = declaration_range(css, declaration) {
        let mut updated = css.to_string();
        updated.replace_range(range, value);
        return updated;
    }

    let mut updated = css.trim_end().to_string();
    match declaration {
        Declaration::NamedColor(name) => {
            updated.push_str(&format!("\n\n@define-color {name} {value};\n"));
        }
        Declaration::CustomProperty(name) => {
            updated.push_str(&format!("\n\n:root {{\n  {name}: {value};\n}}\n"));
        }
        Declaration::TerminalProperty(name) => {
            updated.push_str(&format!("\n\n.vmux-terminal {{\n  {name}: {value};\n}}\n"));
        }
    }
    updated
}

fn declaration_range(css: &str, declaration: Declaration) -> Option<std::ops::Range<usize>> {
    let mut offset = 0;
    let mut in_comment = false;
    let target_selector = match declaration {
        Declaration::NamedColor(_) => None,
        Declaration::CustomProperty(_) => Some(":root"),
        Declaration::TerminalProperty(_) => Some(".vmux-terminal"),
    };
    let mut in_target = target_selector.is_none();
    for line in css.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if in_comment {
            if trimmed.contains("*/") {
                in_comment = false;
            }
            offset += line.len();
            continue;
        }
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_comment = true;
            }
            offset += line.len();
            continue;
        }

        if let Some(selector) = target_selector {
            if !in_target {
                if trimmed
                    .strip_prefix(selector)
                    .is_some_and(|rest| rest.trim_start().starts_with('{'))
                {
                    in_target = true;
                }
                offset += line.len();
                continue;
            }
            if trimmed.starts_with('}') {
                in_target = false;
                offset += line.len();
                continue;
            }
        }

        let marker = match declaration {
            Declaration::NamedColor(name) => format!("@define-color {name}"),
            Declaration::CustomProperty(name) | Declaration::TerminalProperty(name) => {
                name.to_string()
            }
        };
        let Some(rest) = trimmed.strip_prefix(&marker) else {
            offset += line.len();
            continue;
        };
        let rest = match declaration {
            Declaration::NamedColor(_) => {
                if !rest.starts_with(char::is_whitespace) {
                    offset += line.len();
                    continue;
                }
                rest
            }
            Declaration::CustomProperty(_) | Declaration::TerminalProperty(_) => {
                let rest = rest.trim_start();
                let Some(rest) = rest.strip_prefix(':') else {
                    offset += line.len();
                    continue;
                };
                rest
            }
        };
        let whitespace = rest.len() - rest.trim_start().len();
        let value_start_in_trimmed = trimmed.len() - rest.len() + whitespace;
        let rest = rest.trim_start();
        let semicolon = rest.find(';')?;
        let line_start = offset + line.len() - trimmed.len();
        return Some(
            line_start + value_start_in_trimmed..line_start + value_start_in_trimmed + semicolon,
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_defines_the_complete_terminal_appearance() {
        for name in [TERMINAL_FOREGROUND, TERMINAL_BACKGROUND, TERMINAL_CURSOR]
            .into_iter()
            .chain(TERMINAL_PALETTE)
        {
            assert!(TEMPLATE.contains(&format!("@define-color {name} ")));
        }
        assert!(TEMPLATE.contains(".vmux-terminal"));
        assert!(TEMPLATE.contains("font-family:"));
        assert!(TEMPLATE.contains("font-size:"));
    }

    #[test]
    fn declarations_ignore_comments_and_preserve_surrounding_css() {
        let css = "/* @define-color tone #000000; */\n\
                   @define-color tone #112233;\n\
                   :root {\n  --surface: #445566;\n}\n\
                   .vmux-terminal {\n  font-size: 12pt;\n}\n";
        assert_eq!(
            value_in(css, Declaration::NamedColor("tone")).as_deref(),
            Some("#112233")
        );
        assert_eq!(
            value_in(css, Declaration::CustomProperty("--surface")).as_deref(),
            Some("#445566")
        );
        let updated = replace_value(css, Declaration::TerminalProperty("font-size"), "14pt");
        assert!(updated.contains("font-size: 14pt;"));
        assert!(updated.contains("@define-color tone #112233;"));
    }

    #[test]
    fn declarations_are_updated_only_in_their_managed_selector() {
        let css = ".other {\n  font-size: 99pt;\n  --surface: #000000;\n}\n\
                   :root {\n  --surface: #112233;\n}\n\
                   .vmux-terminal {\n  font-size: 11pt;\n}\n";
        let updated = replace_value(css, Declaration::TerminalProperty("font-size"), "13pt");
        assert!(updated.contains(".other {\n  font-size: 99pt;"));
        assert!(updated.contains(".vmux-terminal {\n  font-size: 13pt;"));
        let updated = replace_value(
            &updated,
            Declaration::CustomProperty("--surface"),
            "#abcdef",
        );
        assert!(updated.contains(".other {\n  font-size: 99pt;\n  --surface: #000000;"));
        assert!(updated.contains(":root {\n  --surface: #abcdef;"));
    }

    #[test]
    fn absent_declarations_are_appended_in_valid_blocks() {
        let named = replace_value("/* custom */\n", Declaration::NamedColor("tone"), "#abcdef");
        assert!(named.contains("@define-color tone #abcdef;"));
        let custom = replace_value(&named, Declaration::CustomProperty("--surface"), "#123456");
        assert!(custom.contains(":root {\n  --surface: #123456;\n}"));
        let font = replace_value(
            &custom,
            Declaration::TerminalProperty("font-family"),
            "monospace",
        );
        assert!(font.contains(".vmux-terminal {\n  font-family: monospace;\n}"));
    }

    #[test]
    fn theme_snapshots_are_named_sorted_and_replaceable() {
        let temp = tempfile::tempdir().unwrap();
        save_theme_in(temp.path(), "Zulu", "z").unwrap();
        save_theme_in(temp.path(), "alpha.css", "a").unwrap();
        save_theme_in(temp.path(), "Zulu", "new z").unwrap();
        fs::write(temp.path().join("ignored.json"), "{}").unwrap();
        let themes = themes_in(temp.path()).unwrap();
        assert_eq!(
            themes.iter().map(Theme::name).collect::<Vec<_>>(),
            ["alpha", "Zulu"]
        );
        assert_eq!(fs::read_to_string(&themes[1].path).unwrap(), "new z");
    }

    #[test]
    fn theme_names_cannot_escape_the_theme_directory() {
        for name in ["", ".", "..", "../escape", "nested/name", "bad\\name"] {
            assert!(valid_theme_name(name).is_err(), "accepted {name:?}");
        }
        assert_eq!(valid_theme_name("  My Theme.css  ").unwrap(), "My Theme");
        assert!(valid_theme_name(&"é".repeat(80)).is_ok());
        assert!(valid_theme_name(&"é".repeat(81)).is_err());
    }

    #[test]
    fn old_stylesheet_preserves_rules_and_migrates_terminal_config() {
        let old = ":root { --window-bg-color: #abcdef; }\n";
        let state = r##"{
            "config": {
                "font": "Legacy Mono 13",
                "background_opacity": 0.5,
                "theme": {
                    "foreground": "#AABBCC",
                    "background": "#223344",
                    "cursor": "#DDEEFF",
                    "palette": ["#010203"]
                }
            }
        }"##;
        let upgraded = upgrade_stylesheet(old, Some(state));
        assert!(upgraded.contains(SCHEMA_MARKER));
        assert_eq!(
            value_in(&upgraded, Declaration::NamedColor(TERMINAL_FOREGROUND)).as_deref(),
            Some("#aabbcc")
        );
        assert_eq!(
            value_in(&upgraded, Declaration::NamedColor(TERMINAL_BACKGROUND)).as_deref(),
            Some("rgba(34, 51, 68, 0.5)")
        );
        assert_eq!(
            value_in(&upgraded, Declaration::TerminalProperty("font-family")).as_deref(),
            Some("\"Legacy Mono\"")
        );
        assert_eq!(
            value_in(&upgraded, Declaration::TerminalProperty("font-size")).as_deref(),
            Some("13pt")
        );
        assert_eq!(
            value_in(&upgraded, Declaration::NamedColor(TERMINAL_PALETTE[0])).as_deref(),
            Some("#010203")
        );
        assert!(upgraded.ends_with(old));
    }
}
