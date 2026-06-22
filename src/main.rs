mod app;
mod appearance;
mod git;
mod keybinds;
mod open_path_dialog;
mod pane;
mod splits;
mod state;
mod style;
mod term;
mod text_bindings;
mod window;
mod zone;

use gtk4 as gtk;
use gtk4::glib;
use gtk::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "dev.vmux.Vmux";

const CSS: &str = "
.attention-dot { color: @accent_bg_color; }
.zone-path { font-size: 0.85em; }
/* Secondary line: dim the directory name and the clean marker; color each
   git-status token. Override any of these in ~/.config/vmux/style.css. */
.zone-path .zone-path-dir,
.zone-path .git-clean        { opacity: 0.55; }
.zone-path .git-added,
.zone-path .git-lines-added  { color: #26a269; }
.zone-path .git-modified     { color: #e9ad0c; }
.zone-path .git-deleted,
.zone-path .git-lines-del    { color: #c01c28; }
.zone-path .git-ahead        { color: #2a7bde; }
.zone-row { padding: 6px 8px; }
window.vmux-transparent { background-color: transparent; }
/* The flat headerbar relies on the window background removed above. Paint the
   replacement on the .top-bar revealer, NOT on the headerbar itself: GTK dims
   the windowhandle subtree to 50% on backdrop, which would fade an opaque
   headerbar background into the wallpaper. Scoped to the main chrome so the
   sidebar header keeps the sidebar background showing through. */
window.vmux-transparent toolbarview.vmux-chrome > .top-bar { background-color: var(--window-bg-color); }
/* Pane dividers also rely on the window background; render them like the
   sidebar's edge border (border color over sidebar bg). */
window.vmux-transparent paned > separator {
    background-color: var(--sidebar-bg-color);
    background-image: image(var(--sidebar-border-color));
    box-shadow: none;
}
/* Only the focused pane's selected tab carries the accent; selected tabs in
   unfocused panes fade to a faint gray so the eye finds focus immediately.
   Stock Adwaita hides the highlight entirely on single-tab panes, so these
   also deliberately out-rank tabbox.single-tab: a lone tab still shows
   focus. */
tabbar tab:selected { background-color: color-mix(in srgb, currentColor 5%, transparent); }
tabbar tab:selected:hover { background-color: color-mix(in srgb, currentColor 9%, transparent); }
tabbar tab:selected:active { background-color: color-mix(in srgb, currentColor 14%, transparent); }
.vmux-pane:focus-within tabbar tab:selected {
    background-color: color-mix(in srgb, var(--accent-bg-color) 25%, transparent);
    color: var(--accent-color);
}
.vmux-pane:focus-within tabbar tab:selected:hover { background-color: color-mix(in srgb, var(--accent-bg-color) 30%, transparent); }
.vmux-pane:focus-within tabbar tab:selected:active { background-color: color-mix(in srgb, var(--accent-bg-color) 38%, transparent); }
";

fn main() -> glib::ExitCode {
    // vte merges spawn envv OVER the parent environment, so anything not
    // scrubbed here leaks into every shell. Drop the tmux identity of
    // whatever launched vmux (single-threaded this early, so this is safe).
    unsafe {
        std::env::remove_var("TMUX");
        std::env::remove_var("TMUX_PANE");
    }
    // Force integer-scale rendering on fractional-scaled displays so diagonal
    // glyphs (powerline separators) don't stair-step: GTK rasterizes at 2× and
    // the compositor downscales, which supersamples the glyphs (matching what
    // foot does). Must run before GTK reads GDK_SCALE; an explicit GDK_SCALE in
    // the environment always wins. See Config::force_integer_scale.
    if state::load().config.force_integer_scale && std::env::var_os("GDK_SCALE").is_none() {
        unsafe {
            std::env::set_var("GDK_SCALE", "2");
        }
    }
    // Must precede the first vte::Terminal (vte rule for termprop installs).
    term::install_notify_termprop();
    term::install_fgproc_termprop();
    let application = adw::Application::builder().application_id(APP_ID).build();
    application.connect_activate(|gtk_app| {
        if let Some(existing) = gtk_app.windows().first() {
            existing.present();
            return;
        }
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        let provider = gtk::CssProvider::new();
        provider.load_from_data(CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        app::build(gtk_app);
    });
    application.run()
}
