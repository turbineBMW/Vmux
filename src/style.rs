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
 *   .zone-path   - the per-zone secondary line, e.g.  .zone-path { font-size: 0.9em; }
 *   tabbar tab   - pane tab labels, e.g.  tabbar tab { font-weight: bold; }
 *
 * The secondary line colors git status. Recolor any piece (defaults shown):
 *
 *   .zone-path .git-added       { color: #26a269; }   files added
 *   .zone-path .git-modified    { color: #e9ad0c; }   files modified
 *   .zone-path .git-deleted     { color: #c01c28; }   files deleted
 *   .zone-path .git-lines-added { color: #26a269; }   lines inserted
 *   .zone-path .git-lines-del   { color: #c01c28; }   lines deleted
 *   .zone-path .git-ahead       { color: #2a7bde; }   unpushed commits
 *   .zone-path .git-clean       { opacity: 0.55; }    the clean (check) marker
 *   .zone-path .zone-path-dir   { opacity: 0.55; }    directory name (no repo)
 *
 * A tab recolors when its foreground process runs as root or is a remote
 * session (ssh / mosh / telnet / et), whether or not it is the selected tab.
 * Change the colors (defaults shown):
 *
 *   :root {
 *     --vmux-root-color:   #c01c28;
 *     --vmux-remote-color: #9141ac;
 *   }
 *
 * or restyle the tabs fully:
 *
 *   .vmux-pane tabbar tab.vmux-root   { ... }   any tab running as root
 *   .vmux-pane tabbar tab.vmux-remote:selected { ... }   only when selected
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
