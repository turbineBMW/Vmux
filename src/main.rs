mod app;
mod appearance;
mod keybinds;
mod open_path_dialog;
mod pane;
mod splits;
mod state;
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
.zone-path { font-size: 0.85em; opacity: 0.55; }
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
";

fn main() -> glib::ExitCode {
    // vte merges spawn envv OVER the parent environment, so anything not
    // scrubbed here leaks into every shell. Drop the tmux identity of
    // whatever launched vmux (single-threaded this early, so this is safe).
    unsafe {
        std::env::remove_var("TMUX");
        std::env::remove_var("TMUX_PANE");
    }
    // Must precede the first vte::Terminal (vte rule for termprop installs).
    term::install_notify_termprop();
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
