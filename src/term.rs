use crate::app::App;
use crate::state::Config;
use crate::zone::Zone;
use gtk4 as gtk;
use gtk4::{gio, glib};
use gtk::prelude::*;
use std::rc::{Rc, Weak};
use vte4 as vte;
use vte4::prelude::*;

// GNOME dark palette
const PALETTE: [&str; 16] = [
    "#171421", "#C01C28", "#26A269", "#A2734C", "#12488B", "#A347BA", "#2AA1B3", "#D0CFCC",
    "#5E5C64", "#F66151", "#33D17A", "#E9AD0C", "#2A7BDE", "#C061CB", "#33C7DE", "#FFFFFF",
];
const FOREGROUND: &str = "#D0CFCC";
const BACKGROUND: &str = "#1D1D20";

/// Build a terminal leaf (ScrolledWindow wrapping a vte::Terminal) and spawn
/// the configured shell in it. Handlers capture Weak<Zone> only — a removed
/// zone must be able to finalize even if a terminal is still being torn down.
pub fn build_leaf(app: &Rc<App>, zone: &Weak<Zone>, cwd: String) -> gtk::ScrolledWindow {
    let term = vte::Terminal::new();
    configure(&term, &app.config.borrow(), app.font_scale.get());

    let scrolled = gtk::ScrolledWindow::builder()
        .child(&term)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .build();

    let focus = gtk::EventControllerFocus::new();
    {
        let zone = zone.clone();
        let tw = term.downgrade();
        focus.connect_enter(move |_| {
            if let (Some(zone), Some(term)) = (zone.upgrade(), tw.upgrade()) {
                zone.last_focused.set(Some(&term));
            }
        });
    }
    term.add_controller(focus);

    {
        let app = app.clone();
        let zone = zone.clone();
        term.connect_bell(move |_| {
            if let Some(zone) = zone.upgrade() {
                app.on_bell(&zone);
            }
        });
    }
    {
        let app = app.clone();
        let zone = zone.clone();
        term.connect_child_exited(move |term, _status| {
            if let Some(zone) = zone.upgrade() {
                app.on_term_exited(&zone, term);
            }
        });
    }

    let argv = shell_argv(&app.config.borrow());
    spawn_argv(&term, &cwd, &argv);
    scrolled
}

fn configure(term: &vte::Terminal, cfg: &Config, scale: f64) {
    let fg = gtk::gdk::RGBA::parse(FOREGROUND).unwrap();
    let bg = gtk::gdk::RGBA::parse(BACKGROUND).unwrap();
    let palette: Vec<gtk::gdk::RGBA> = PALETTE
        .iter()
        .map(|c| gtk::gdk::RGBA::parse(*c).unwrap())
        .collect();
    let refs: Vec<&gtk::gdk::RGBA> = palette.iter().collect();
    term.set_colors(Some(&fg), Some(&bg), &refs);
    term.set_scrollback_lines(cfg.scrollback_lines);
    term.set_mouse_autohide(true);
    term.set_font_scale(scale);
    if let Some(font) = &cfg.font {
        term.set_font(Some(&gtk::pango::FontDescription::from_string(font)));
    }
    term.set_hexpand(true);
    term.set_vexpand(true);
}

pub fn apply_config(term: &vte::Terminal, cfg: &Config) {
    term.set_scrollback_lines(cfg.scrollback_lines);
    match &cfg.font {
        Some(font) => term.set_font(Some(&gtk::pango::FontDescription::from_string(font))),
        None => term.set_font(None),
    }
}

/// vte4 cannot pass NULL envv ("inherit"); an empty slice means an EMPTY
/// environment, so always pass the full current environment explicitly.
pub fn spawn_argv(term: &vte::Terminal, cwd: &str, argv: &[String]) {
    let argv_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let env: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    let env_refs: Vec<&str> = env.iter().map(|s| s.as_str()).collect();
    let tw = term.downgrade();
    term.spawn_async(
        vte::PtyFlags::DEFAULT,
        Some(cwd),
        &argv_refs,
        &env_refs,
        glib::SpawnFlags::SEARCH_PATH,
        || {},
        -1,
        None::<&gio::Cancellable>,
        move |res| {
            if let Err(e) = res
                && let Some(term) = tw.upgrade()
            {
                feed_line(&term, &format!("[vmux] spawn failed: {e}"));
            }
        },
    );
}

pub fn feed_line(term: &vte::Terminal, msg: &str) {
    term.feed(format!("\r\n\x1b[2m{msg}\x1b[0m\r\n").as_bytes());
}

pub fn cwd_of(term: &vte::Terminal) -> Option<String> {
    let uri = term.current_directory_uri()?;
    let (path, _) = glib::filename_from_uri(&uri).ok()?;
    Some(path.to_string_lossy().into_owned())
}

pub fn focus_later(term: &vte::Terminal) {
    let tw = term.downgrade();
    glib::idle_add_local_once(move || {
        if let Some(t) = tw.upgrade() {
            t.grab_focus();
        }
    });
}

pub fn shell_argv(cfg: &Config) -> Vec<String> {
    vec![crate::state::default_shell(cfg)]
}
