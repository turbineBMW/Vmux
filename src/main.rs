mod app;
mod keybinds;
mod open_path_dialog;
mod pane;
mod splits;
mod state;
mod term;
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
";

fn main() -> glib::ExitCode {
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
