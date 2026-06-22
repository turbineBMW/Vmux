use gtk4::glib;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

/// Written to style.css on first run; also the fallback when the file can't
/// be read. It is entirely comments: vmux ships its built-in chrome styles in
/// `main.rs`, and this file only *overrides* them, so an untouched file is a
/// no-op. (GTK CSS only understands `/* */` comments — `#` and `//` are
/// parse errors.)
pub const TEMPLATE: &str = r#"/* vmux GTK theme overrides.
 *
 * Plain GTK4 / libadwaita CSS, loaded at the USER priority so any rule here
 * wins over both the Adwaita theme and vmux's own built-in styles. Edits
 * apply live: save the file and the window restyles immediately. Delete the
 * file to restore defaults (it is regenerated from this template).
 *
 * Recolor the chrome to match your terminal by overriding libadwaita's named
 * colors on :root. Uncomment and tweak the block below — these values are
 * Catppuccin Mocha as an example.
 *
 *   :root {
 *     --window-bg-color:    #1e1e2e;
 *     --window-fg-color:    #cdd6f4;
 *     --headerbar-bg-color: #181825;
 *     --headerbar-fg-color: #cdd6f4;
 *     --sidebar-bg-color:   #181825;
 *     --sidebar-fg-color:   #cdd6f4;
 *     --popover-bg-color:   #1e1e2e;
 *     --popover-fg-color:   #cdd6f4;
 *     --accent-bg-color:    #89b4fa;
 *     --accent-fg-color:    #11111b;
 *     --accent-color:       #89b4fa;
 *   }
 *
 * The legacy syntax also works, e.g. `@define-color window_bg_color #1e1e2e;`.
 * Full list of named colors:
 * https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/named-colors.html
 *
 * You can also target vmux's own widgets directly. Useful selectors:
 *
 *   .zone-row    - sidebar zone entries, e.g.  .zone-row { padding: 8px 10px; }
 *   .zone-path   - the per-zone secondary line, e.g.  .zone-path { opacity: 0.7; }
 *   tabbar tab   - pane tab labels, e.g.  tabbar tab { font-weight: bold; }
 */
"#;

pub fn path() -> PathBuf {
    glib::user_config_dir().join("vmux").join("style.css")
}

/// Read style.css, creating it from the template on first run. A missing file
/// is seeded with the template; any other read error logs and yields empty
/// CSS (vmux's built-in styles stay in effect).
pub fn load() -> String {
    let path = path();
    match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Err(e) = fs::write(&path, TEMPLATE) {
                eprintln!("vmux: cannot write {}: {e}", path.display());
            }
            TEMPLATE.to_string()
        }
        Err(e) => {
            eprintln!("vmux: cannot read {}: {e}", path.display());
            String::new()
        }
    }
}
