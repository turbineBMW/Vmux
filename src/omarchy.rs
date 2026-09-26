//! Follow the Omarchy desktop theme.
//!
//! Omarchy keeps the active theme at `~/.local/state/omarchy/current/theme`
//! and swaps that directory wholesale on every `omarchy theme set`. The
//! palette lives in `colors.toml`; every themed app derives its own config
//! from those keys. Vmux does the same, in-process: [`load`] resolves the
//! palette and renders it as the CSS vmux already speaks (the
//! `vmux_terminal_*` named colors plus the libadwaita tokens), which
//! [`crate::app::App::reload_omarchy_css`] hands to a provider stacked above
//! style.css. No omarchy binary is invoked, so a theme switch costs a file
//! read.
//!
//! A theme may also ship its own `vmux.css` — either by hand or from a
//! `~/.config/omarchy/themed/vmux.css.tpl` template — and that file is used
//! verbatim in place of everything derived here. Such a file may carry a
//! `vmux-mode: dark` (or `light`) marker, which then decides the libadwaita
//! color scheme instead of `colors.toml`: a theme keeping vmux dark under a
//! light desktop needs both halves to agree.
//!
//! The resolution cascade below mirrors `omarchy-theme-color` so vmux and the
//! rest of the desktop read an identical palette out of the same file,
//! including the legacy themes that only define `color0`..`color15`.

use gtk4::glib;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The state directory omarchy swaps on a theme change. `theme` is replaced
/// by a rename, so this stable parent is what a file monitor can watch.
pub fn state_dir() -> PathBuf {
    glib::home_dir()
        .join(".local")
        .join("state")
        .join("omarchy")
        .join("current")
}

pub fn theme_dir() -> PathBuf {
    state_dir().join("theme")
}

/// Whether this machine has an Omarchy theme to follow at all.
pub fn detected() -> bool {
    theme_dir().is_dir()
}

/// The active theme's slug, for display in Preferences.
pub fn theme_name() -> Option<String> {
    let name = fs::read_to_string(state_dir().join("theme.name")).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub struct Theme {
    /// CSS ready for a `gtk::CssProvider`.
    pub css: String,
    /// The theme's `mode`, which decides the libadwaita color scheme.
    pub light: bool,
}

/// The color scheme a theme's own `vmux.css` asks for, from a
/// `vmux-mode: dark` (or `light`) marker anywhere in it.
///
/// `colors.toml`'s `mode` describes the rest of the desktop, and a theme that
/// ships its own vmux.css is opting vmux out of exactly that — a dark vmux
/// under a light desktop would otherwise get dark colors and a light
/// libadwaita scheme. The marker is the theme saying which one it meant.
fn css_mode(css: &str) -> Option<bool> {
    const MARKER: &str = "vmux-mode:";

    let rest = &css[css.find(MARKER)? + MARKER.len()..];
    match rest
        .split(|c: char| !c.is_ascii_alphabetic())
        .find(|word| !word.is_empty())?
        .to_ascii_lowercase()
        .as_str()
    {
        "light" => Some(true),
        "dark" => Some(false),
        _ => None,
    }
}

/// Resolve the active theme into CSS, or `None` when it has neither a palette
/// nor its own vmux.css.
///
/// `background_alpha` is carried over from the user's own terminal background
/// so enabling this doesn't silently turn a transparent terminal opaque: an
/// omarchy palette is always opaque hex, and the alpha is the one piece of
/// the color the user set in vmux rather than in the theme.
pub fn load(background_alpha: f32) -> Option<Theme> {
    load_in(&theme_dir(), background_alpha)
}

fn load_in(dir: &Path, background_alpha: f32) -> Option<Theme> {
    let colors = fs::read_to_string(dir.join("colors.toml")).ok();
    let palette = colors
        .as_deref()
        .map(|colors| Palette::resolve(colors, dir.join("light.mode").exists()));

    // A theme that speaks vmux directly outranks anything derived here, and
    // that includes the color scheme when it names one.
    let (css, mode) = match fs::read_to_string(dir.join("vmux.css")) {
        Ok(css) => {
            let mode = css_mode(&css);
            (css, mode)
        }
        Err(_) => (palette.as_ref()?.to_css(background_alpha), None),
    };
    Some(Theme {
        css,
        light: mode
            .or_else(|| palette.as_ref().map(|palette| palette.light))
            .unwrap_or(false),
    })
}

struct Palette {
    colors: HashMap<String, String>,
    light: bool,
}

impl Palette {
    /// Port of `omarchy-theme-color`'s parse-and-resolve. Keep the two in
    /// step: a theme that renders correctly in Alacritty must render the same
    /// here, and the fallbacks are the only thing standing between a
    /// pre-`colors.toml` theme and a half-empty palette.
    fn resolve(toml: &str, light_mode_file: bool) -> Self {
        let mut colors = parse(toml);
        let mut palette = Palette {
            light: false,
            colors: HashMap::new(),
        };

        // The short palette names an older theme may use instead.
        for (canonical, legacy) in [
            ("background", "bg"),
            ("dark_background", "dark_bg"),
            ("darker_background", "darker_bg"),
            ("lighter_background", "lighter_bg"),
            ("foreground", "fg"),
            ("dark_foreground", "dark_fg"),
            ("light_foreground", "light_fg"),
            ("bright_foreground", "bright_fg"),
        ] {
            alias(&mut colors, canonical, legacy);
        }

        // Themes predating the semantic palette define only ANSI names.
        alias(&mut colors, "background", "color0");
        alias(&mut colors, "foreground", "color7");
        mirror(&mut colors, "color0", "background");
        mirror(&mut colors, "color7", "foreground");

        for (name, ansi) in [
            ("red", "color1"),
            ("green", "color2"),
            ("yellow", "color3"),
            ("blue", "color4"),
            ("magenta", "color5"),
            ("cyan", "color6"),
            ("bright_red", "color9"),
            ("bright_green", "color10"),
            ("bright_yellow", "color11"),
            ("bright_blue", "color12"),
            ("bright_magenta", "color13"),
            ("bright_cyan", "color14"),
        ] {
            alias(&mut colors, name, ansi);
        }
        alias(&mut colors, "magenta", "purple");
        alias(&mut colors, "bright_magenta", "bright_purple");

        alias_any(&mut colors, "light_foreground", &["color7", "foreground"]);
        alias_any(&mut colors, "bright_foreground", &["color15", "foreground"]);
        // Unconditional, exactly as omarchy resolves it: the cursor is the
        // brightest foreground, not a key a theme sets on its own.
        mirror(&mut colors, "cursor", "bright_foreground");
        alias_any(&mut colors, "lighter_background", &["color0", "background"]);
        alias_any(&mut colors, "dark_foreground", &["color8", "foreground"]);
        alias_any(&mut colors, "muted", &["color8", "dark_foreground"]);
        alias_any(
            &mut colors,
            "selection",
            &["selection_background", "color8", "color0", "background"],
        );
        alias(&mut colors, "selection_background", "selection");
        alias(&mut colors, "selection_foreground", "bright_foreground");
        alias(&mut colors, "orange", "yellow");
        derive(&mut colors, "brown", "orange", "#000000", 0.5);

        derive(
            &mut colors,
            "dark_background",
            "background",
            "#000000",
            0.25,
        );
        derive(
            &mut colors,
            "darker_background",
            "background",
            "#000000",
            0.5,
        );
        for (bright, base) in [
            ("bright_red", "red"),
            ("bright_yellow", "yellow"),
            ("bright_green", "green"),
            ("bright_cyan", "cyan"),
            ("bright_blue", "blue"),
            ("bright_magenta", "magenta"),
        ] {
            derive(&mut colors, bright, base, "#ffffff", 0.2);
        }

        for (ansi, name) in [
            ("color0", "background"),
            ("color1", "red"),
            ("color2", "green"),
            ("color3", "yellow"),
            ("color4", "blue"),
            ("color5", "magenta"),
            ("color6", "cyan"),
            ("color7", "foreground"),
            ("color8", "muted"),
            ("color9", "bright_red"),
            ("color10", "bright_green"),
            ("color11", "bright_yellow"),
            ("color12", "bright_blue"),
            ("color13", "bright_magenta"),
            ("color14", "bright_cyan"),
            ("color15", "bright_foreground"),
        ] {
            alias(&mut colors, ansi, name);
        }

        palette.light = resolve_mode(&colors, light_mode_file);
        palette.colors = colors;
        palette
    }

    /// A palette key, or the value of `fallback` when the theme is missing it
    /// even after the cascade above.
    fn get(&self, key: &str, fallback: &str) -> String {
        self.colors
            .get(key)
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| {
                self.colors
                    .get(fallback)
                    .cloned()
                    .unwrap_or_else(|| fallback.to_string())
            })
    }

    fn mixed(&self, key: &str, fallback: &str, toward: &str, amount: f64) -> String {
        let base = self.get(key, fallback);
        let toward = self.get(toward, toward);
        mix(&base, &toward, amount).unwrap_or(base)
    }

    /// Render the palette as the declarations vmux's own stylesheet defines,
    /// so every widget and the VTE palette pick it up through the paths that
    /// already exist.
    fn to_css(&self, background_alpha: f32) -> String {
        let c = |key: &str, fallback: &str| self.get(key, fallback);
        let bg = c("background", "#1d1d20");
        let fg = c("foreground", "#d0cfcc");
        let accent = c("accent", "blue");
        // Chrome sits one step off the content background in both modes:
        // omarchy's dark_* shades are darker than `background` for a dark
        // theme and for a light one alike.
        let chrome = c("dark_background", "background");
        let backdrop = c("darker_background", "background");
        // The hairline where chrome meets content: the sidebar's edge and the
        // tab bar's bottom. lighter_background reads as a highlight against a
        // dark theme's chrome, but a light theme's is lighter still than the
        // chrome it draws on, so the seam vanishes; darken the chrome toward
        // the theme's own foreground instead.
        let border = if self.light {
            self.mixed("dark_background", "background", "foreground", 0.18)
        } else {
            c("lighter_background", "muted")
        };
        let raised = c("lighter_background", "background");

        let mut css = String::from(
            "/* Generated by vmux from the active Omarchy theme. Not a file to\n\
             \x20* edit: it is rebuilt on every theme change. Override any of it in\n\
             \x20* ~/.config/vmux/style.css, or ship a vmux.css in the theme. */\n\n",
        );

        let mut color = |name: &str, value: &str| {
            css.push_str(&format!("@define-color {name} {value};\n"));
        };
        color("vmux_terminal_foreground", &fg);
        color(
            "vmux_terminal_background",
            &with_alpha(&bg, background_alpha),
        );
        color("vmux_terminal_cursor", &c("cursor", "bright_foreground"));
        for (name, key) in [
            ("black", "background"),
            ("red", "red"),
            ("green", "green"),
            ("yellow", "yellow"),
            ("blue", "blue"),
            ("magenta", "magenta"),
            ("cyan", "cyan"),
            ("white", "foreground"),
            ("bright_black", "muted"),
            ("bright_red", "bright_red"),
            ("bright_green", "bright_green"),
            ("bright_yellow", "bright_yellow"),
            ("bright_blue", "bright_blue"),
            ("bright_magenta", "bright_magenta"),
            ("bright_cyan", "bright_cyan"),
            ("bright_white", "bright_foreground"),
        ] {
            color(&format!("vmux_terminal_{name}"), &c(key, "foreground"));
        }

        // The focused pane's selected tab, and the Git summary tokens.
        color("vmux_focused_tab_background", &c("selection", "background"));
        color("vmux_focused_tab_foreground", &accent);
        color(
            "vmux_focused_tab_hover",
            &self.mixed("selection", "background", "foreground", 0.1),
        );
        color(
            "vmux_focused_tab_pressed",
            &self.mixed("selection", "background", "foreground", 0.2),
        );
        color("vmux_git_added", &c("green", "green"));
        color("vmux_git_modified", &c("yellow", "yellow"));
        color("vmux_git_deleted", &c("red", "red"));
        color("vmux_git_lines_added", &c("bright_green", "green"));
        color("vmux_git_lines_deleted", &c("bright_red", "red"));
        color("vmux_git_ahead", &c("blue", "blue"));

        css.push_str("\n:root {\n");
        let mut property = |name: &str, value: &str| {
            css.push_str(&format!("  {name}: {value};\n"));
        };
        let on_accent = readable_on(&accent);
        property("--accent-bg-color", &accent);
        property("--accent-fg-color", on_accent);
        property("--accent-color", &accent);
        property("--window-bg-color", &bg);
        property("--window-fg-color", &fg);
        property("--view-bg-color", &bg);
        property("--view-fg-color", &fg);
        for prefix in ["--headerbar", "--sidebar", "--secondary-sidebar"] {
            property(&format!("{prefix}-bg-color"), &chrome);
            property(&format!("{prefix}-fg-color"), &fg);
            property(&format!("{prefix}-backdrop-color"), &backdrop);
            property(&format!("{prefix}-border-color"), &border);
        }
        // libadwaita draws a tab bar's bottom edge from the headerbar *shade*,
        // not from any -border-color, so leaving it stock left that seam a
        // translucent black while the sidebar's edge took the palette color.
        // `tabbar .box` is the only rule this reaches in vmux — headerbars
        // inside an AdwToolbarView drop their own shadow — so it just lines
        // the two seams up.
        property("--headerbar-shade-color", &border);
        for prefix in ["--card", "--popover", "--dialog"] {
            property(
                &format!("{prefix}-bg-color"),
                if prefix == "--card" { &raised } else { &chrome },
            );
            property(&format!("{prefix}-fg-color"), &fg);
        }
        for (prefix, base, bright) in [
            ("--success", "green", "bright_green"),
            ("--warning", "yellow", "bright_yellow"),
            ("--error", "red", "bright_red"),
            ("--destructive", "red", "bright_red"),
        ] {
            let base = c(base, base);
            property(&format!("{prefix}-bg-color"), &base);
            property(&format!("{prefix}-fg-color"), readable_on(&base));
            property(&format!("{prefix}-color"), &c(bright, base.as_str()));
        }
        // vmux's own tokens from the built-in stylesheet.
        property("--vmux-agent-color", &accent);
        property("--vmux-agent-attention-color", &c("red", "red"));
        property("--vmux-root-color", &c("red", "red"));
        property("--vmux-remote-color", &c("magenta", "magenta"));
        css.push_str("}\n");
        css
    }
}

/// `key = "value"` pairs, quotes and inline comments stripped. Keys and
/// values outside the charset omarchy accepts are dropped rather than
/// smuggled into CSS.
fn parse(toml: &str) -> HashMap<String, String> {
    let mut colors = HashMap::new();
    for line in toml.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key: String = key
            .chars()
            .filter(|ch| !matches!(ch, '"' | '\'' | ' ' | '\t'))
            .collect();
        if key.is_empty() || key.starts_with('#') {
            continue;
        }
        let value = match value.split_once(['"', '\'']) {
            Some((_, rest)) => rest
                .split(['"', '\''])
                .next()
                .unwrap_or_default()
                .to_string(),
            None => value.trim().to_string(),
        };
        if !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            continue;
        }
        if !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "#(),._+/% -".contains(ch))
        {
            continue;
        }
        colors.insert(key, value);
    }
    colors
}

/// Fill `key` from `from` only when the theme left it unset.
fn alias(colors: &mut HashMap<String, String>, key: &str, from: &str) {
    alias_any(colors, key, &[from]);
}

fn alias_any(colors: &mut HashMap<String, String>, key: &str, from: &[&str]) {
    if colors.get(key).is_some_and(|value| !value.is_empty()) {
        return;
    }
    for candidate in from {
        if let Some(value) = colors.get(*candidate).filter(|value| !value.is_empty()) {
            let value = value.clone();
            colors.insert(key.to_string(), value);
            return;
        }
    }
}

/// Overwrite `key` with `from` whenever `from` is set.
fn mirror(colors: &mut HashMap<String, String>, key: &str, from: &str) {
    if let Some(value) = colors.get(from).filter(|value| !value.is_empty()) {
        let value = value.clone();
        colors.insert(key.to_string(), value);
    }
}

/// Fill `key` by blending `base` toward `toward`, when the theme left it unset.
fn derive(colors: &mut HashMap<String, String>, key: &str, base: &str, toward: &str, amount: f64) {
    if colors.get(key).is_some_and(|value| !value.is_empty()) {
        return;
    }
    let Some(base) = colors.get(base).cloned() else {
        return;
    };
    if let Some(value) = mix(&base, toward, amount) {
        colors.insert(key.to_string(), value);
    }
}

fn rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

/// Blend two hex colors, `amount` of the way from `start` to `end`. A
/// non-hex value (an `rgba()` a theme wrote by hand) has nothing to blend.
fn mix(start: &str, end: &str, amount: f64) -> Option<String> {
    let (sr, sg, sb) = rgb(start)?;
    let (er, eg, eb) = rgb(end)?;
    let amount = amount.clamp(0.0, 1.0);
    let blend = |s: u8, e: u8| (s as f64 * (1.0 - amount) + e as f64 * amount + 0.5) as u8;
    Some(format!(
        "#{:02x}{:02x}{:02x}",
        blend(sr, er),
        blend(sg, eg),
        blend(sb, eb)
    ))
}

/// Black or white, whichever stays legible on `color`. Used for text on the
/// saturated accent and status fills, where no palette key is guaranteed to
/// contrast with them in both light and dark themes.
fn readable_on(color: &str) -> &'static str {
    match rgb(color) {
        // Rec. 601 luma, the same threshold GTK uses to pick icon shades.
        Some((r, g, b)) if 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64 > 140.0 => {
            "#000000"
        }
        _ => "#ffffff",
    }
}

fn with_alpha(color: &str, alpha: f32) -> String {
    match rgb(color) {
        Some((r, g, b)) if alpha < 0.999 => {
            let alpha = format!("{:.3}", alpha.clamp(0.0, 1.0));
            let alpha = alpha.trim_end_matches('0').trim_end_matches('.');
            format!("rgba({r}, {g}, {b}, {alpha})")
        }
        _ => color.to_string(),
    }
}

/// `mode`, then the legacy `theme_type`, then a `light.mode` file beside
/// colors.toml, then the background's brightness.
fn resolve_mode(colors: &HashMap<String, String>, light_mode_file: bool) -> bool {
    for key in ["mode", "theme_type"] {
        if let Some(mode) = colors.get(key).filter(|mode| !mode.is_empty()) {
            return mode.eq_ignore_ascii_case("light");
        }
    }
    if light_mode_file {
        return true;
    }
    match colors.get("background").and_then(|bg| rgb(bg)) {
        Some((r, g, b)) => r as u32 + g as u32 + b as u32 > 382,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OSAKA_JADE: &str = r##"
mode = "dark"
accent = "#509475"
selection = "#32473B"
muted = "#53685B"
background = "#111c18"
foreground = "#C1C497"
bright_foreground = "#F7E8B2"
red = "#FF5345"
yellow = "#459451"
green = "#549e6a"
cyan = "#2DD5B7"
blue = "#509475"
magenta = "#D2689C"
bright_red = "#db9f9c"
"##;

    #[test]
    fn semantic_keys_reach_the_terminal_palette() {
        let palette = Palette::resolve(OSAKA_JADE, false);
        let css = palette.to_css(1.0);
        assert!(css.contains("@define-color vmux_terminal_background #111c18;"));
        assert!(css.contains("@define-color vmux_terminal_foreground #C1C497;"));
        // The cursor is the brightest foreground, never a key of its own.
        assert!(css.contains("@define-color vmux_terminal_cursor #F7E8B2;"));
        // ANSI 0 and 7 are the background and foreground, as in every other
        // omarchy-themed terminal.
        assert!(css.contains("@define-color vmux_terminal_black #111c18;"));
        assert!(css.contains("@define-color vmux_terminal_white #C1C497;"));
        assert!(css.contains("@define-color vmux_terminal_bright_black #53685B;"));
        assert!(css.contains("--accent-bg-color: #509475;"));
        assert!(!palette.light);
    }

    /// A theme that predates the semantic palette only has color0..color15.
    #[test]
    fn legacy_ansi_only_themes_still_resolve() {
        let css = Palette::resolve(
            "color0 = \"#101010\"\n\
             color1 = \"#ff0000\"\n\
             color7 = \"#e0e0e0\"\n\
             color8 = \"#505050\"\n",
            false,
        )
        .to_css(1.0);
        assert!(css.contains("@define-color vmux_terminal_background #101010;"));
        assert!(css.contains("@define-color vmux_terminal_foreground #e0e0e0;"));
        assert!(css.contains("@define-color vmux_terminal_red #ff0000;"));
        assert!(css.contains("@define-color vmux_terminal_bright_black #505050;"));
        // bright_red is derived from red when the theme omits it.
        assert!(css.contains("@define-color vmux_terminal_bright_red #ff3333;"));
    }

    #[test]
    fn light_themes_are_detected_by_mode_then_by_luminance() {
        assert!(Palette::resolve("mode = \"light\"\n", false).light);
        assert!(Palette::resolve("theme_type = \"light\"\n", false).light);
        assert!(Palette::resolve("background = \"#eff1f5\"\n", false).light);
        assert!(!Palette::resolve("background = \"#111c18\"\n", false).light);
        // A light.mode file decides only when the theme states no mode.
        assert!(Palette::resolve("background = \"#111c18\"\n", true).light);
        assert!(!Palette::resolve("mode = \"dark\"\n", true).light);
    }

    /// A theme keeping vmux dark under a light desktop: the marker in its own
    /// vmux.css has to beat `mode = "light"`, or the colors go dark and the
    /// libadwaita scheme stays light.
    #[test]
    fn a_themes_own_css_can_pin_the_color_scheme() {
        assert_eq!(css_mode("/* vmux-mode: dark */\n"), Some(false));
        assert_eq!(css_mode("/* vmux-mode:light */\n"), Some(true));
        assert_eq!(css_mode("/* VMUX-MODE: Dark */\n"), None); // marker is literal
        assert_eq!(css_mode("@define-color x #fff;\n"), None);
        // A marker naming nothing usable leaves colors.toml in charge.
        assert_eq!(css_mode("/* vmux-mode: */"), None);
        assert_eq!(css_mode("/* vmux-mode: sepia */"), None);
    }

    #[test]
    fn a_theme_can_supply_only_its_own_css() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("vmux.css"),
            "/* vmux-mode: light */\n@define-color vmux_terminal_background #eeeeee;\n",
        )
        .unwrap();
        let theme = load_in(dir.path(), 0.5).unwrap();
        assert!(theme.light);
        assert!(theme.css.contains("#eeeeee"));
    }

    /// Transparency is the user's setting, not the theme's, so it survives.
    #[test]
    fn the_terminal_background_keeps_the_users_alpha() {
        let css = Palette::resolve(OSAKA_JADE, false).to_css(0.85);
        assert!(css.contains("@define-color vmux_terminal_background rgba(17, 28, 24, 0.85);"));
        // Only the terminal canvas is translucent; the window is not.
        assert!(css.contains("--window-bg-color: #111c18;"));
    }

    /// The sidebar's right edge and a tab bar's bottom are the same seam, so
    /// they take the same color — libadwaita draws the latter from the
    /// headerbar shade, which would otherwise stay a stock translucent black.
    #[test]
    fn the_sidebar_edge_and_the_tab_bar_share_one_seam_color() {
        let css = Palette::resolve(
            "mode = \"dark\"\n\
             background = \"#111c18\"\n\
             dark_background = \"#0d1713\"\n\
             lighter_background = \"#22362a\"\n",
            false,
        )
        .to_css(1.0);
        assert!(css.contains("--sidebar-border-color: #22362a;"));
        assert!(css.contains("--headerbar-shade-color: #22362a;"));
    }

    /// A light theme's lighter_background sits above the chrome it draws on,
    /// leaving no seam at all, so the seam darkens toward the foreground.
    #[test]
    fn a_light_themes_seam_darkens_instead_of_lightening() {
        let css = Palette::resolve(
            "mode = \"light\"\n\
             background = \"#e1e2e7\"\n\
             dark_background = \"#d4d6e0\"\n\
             lighter_background = \"#d0d5e3\"\n\
             foreground = \"#3760bf\"\n",
            false,
        )
        .to_css(1.0);
        // 18% of the way from the chrome (#d4d6e0) toward the foreground.
        assert!(css.contains("--sidebar-border-color: #b8c1da;"));
        assert!(css.contains("--headerbar-shade-color: #b8c1da;"));
    }

    #[test]
    fn accent_text_flips_to_stay_legible() {
        assert_eq!(readable_on("#1e66f5"), "#ffffff");
        assert_eq!(readable_on("#e9ad0c"), "#000000");
        assert_eq!(readable_on("not-a-hex-color"), "#ffffff");
    }

    /// Values omarchy would refuse are not smuggled into a CSS declaration.
    /// A theme is a file from the internet: `omarchy theme install` clones
    /// one from any repo, and its colors.toml is kept verbatim.
    #[test]
    fn unsupported_keys_and_values_are_dropped() {
        let colors = parse(
            "background = \"#101010\"\n\
             evil = \"red; } * { color: blue\"\n\
             bad$key = \"#ffffff\"\n\
             # comment = \"#000000\"\n",
        );
        assert_eq!(
            colors.get("background").map(String::as_str),
            Some("#101010")
        );
        assert!(!colors.contains_key("evil"));
        assert!(!colors.contains_key("bad$key"));
        // Spaces around a key are stripped rather than rejected, as omarchy
        // does: `  accent = ...` is an ordinary way to write the file.
        assert_eq!(
            parse("  accent  =  \"#509475\"\n")
                .get("accent")
                .map(String::as_str),
            Some("#509475")
        );
    }
}
