use crate::app::App;
use crate::state::Config;
use crate::zone::Zone;
use gtk4 as gtk;
use gtk4::{gio, glib};
use gtk::prelude::*;
use std::rc::{Rc, Weak};
use vte4 as vte;
use vte4::prelude::*;

/// Parse a config color, falling back to the built-in default if the string
/// is invalid (e.g. hand-edited config).
pub fn parse_color(s: &str, fallback: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(s).unwrap_or_else(|_| gtk::gdk::RGBA::parse(fallback).unwrap())
}

/// Register the custom termprop vmux-relay uses to deliver desktop
/// notifications (the safe vte4 crate does not wrap the install call).
/// vte requires this to run before the first vte::Terminal exists.
pub fn install_notify_termprop() {
    let name = std::ffi::CString::new(vmux::osc_scan::TERMPROP_NAME).unwrap();
    let id = unsafe {
        vte::ffi::vte_install_termprop(
            name.as_ptr(),
            vte::ffi::VTE_PROPERTY_DATA,
            vte::ffi::VTE_PROPERTY_FLAG_EPHEMERAL,
        )
    };
    if id < 0 {
        eprintln!("vmux: failed to install the notification termprop");
    }
}

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

    // Text bindings (bindings.conf): capture on the parent so they win over
    // vte's own key handling, but still lose to the window-level shortcuts.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let app = app.clone();
        let tw = term.downgrade();
        keys.connect_key_pressed(move |_, keyval, _code, state| {
            let Some(term) = tw.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let mods = state & gtk::accelerator_get_default_mod_mask();
            let bindings = app.text_bindings.borrow();
            let Some(b) = bindings.iter().find(|b| b.matches(keyval, mods)) else {
                return glib::Propagation::Proceed;
            };
            term.feed_child(&b.bytes);
            glib::Propagation::Stop
        });
    }
    scrolled.add_controller(keys);

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
        term.connect_termprop_changed(Some(vmux::osc_scan::TERMPROP_NAME), move |term, _name| {
            // Ephemeral termprop: the value is only readable during this
            // emission, and vte allows nothing but get_termprop_* calls in
            // the handler — copy the data out and defer the real work.
            let data = term.termprop_data(vmux::osc_scan::TERMPROP_NAME);
            let Ok(msg) = serde_json::from_slice::<vmux::osc_scan::NotifyPayload>(&data) else {
                return;
            };
            let app = app.clone();
            let zone = zone.clone();
            let tw = term.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(zone) = zone.upgrade() {
                    app.on_notify(&zone, tw.upgrade().as_ref(), &msg.title, &msg.body);
                }
            });
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
    term.set_mouse_autohide(true);
    term.set_font_scale(scale);
    term.set_hexpand(true);
    term.set_vexpand(true);
    apply_config(term, cfg);
}

pub fn apply_config(term: &vte::Terminal, cfg: &Config) {
    let theme = &cfg.theme;
    let fg = parse_color(&theme.foreground, crate::state::DEFAULT_FOREGROUND);
    let mut bg = parse_color(&theme.background, crate::state::DEFAULT_BACKGROUND);
    bg.set_alpha(cfg.background_opacity.clamp(0.0, 1.0) as f32);
    let palette: Vec<gtk::gdk::RGBA> = crate::state::DEFAULT_PALETTE
        .iter()
        .enumerate()
        .map(|(i, def)| parse_color(theme.palette.get(i).map_or(*def, String::as_str), def))
        .collect();
    let refs: Vec<&gtk::gdk::RGBA> = palette.iter().collect();
    term.set_colors(Some(&fg), Some(&bg), &refs);
    term.set_color_cursor(Some(&parse_color(&theme.cursor, crate::state::DEFAULT_FOREGROUND)));
    term.set_scrollback_lines(cfg.scrollback_lines);
    match &cfg.font {
        Some(font) => term.set_font(Some(&gtk::pango::FontDescription::from_string(font))),
        None => term.set_font(None),
    }
}

/// vte merges the supplied envv OVER the parent environment (adding
/// COLORTERM/VTE_VERSION, and TERM only if missing), so the parent env is
/// passed explicitly anyway to not depend on that merge. TERM is forced to
/// describe vte itself, not whatever terminal vmux was launched from (same
/// for the TERM_PROGRAM identity); TMUX vars are scrubbed from the process
/// env in main() since envv filtering cannot unset what the merge re-adds.
///
/// The command is wrapped in vmux-relay, the PTY shim that watches for
/// notification OSCs; without it the shell still works, just without
/// desktop notifications.
pub fn spawn_argv(term: &vte::Terminal, cwd: &str, argv: &[String]) {
    const SCRUB: &[&str] = &["TERM", "TMUX", "TMUX_PANE", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"];
    let mut env: Vec<String> = std::env::vars()
        .filter(|(k, _)| !SCRUB.contains(&k.as_str()))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    env.push("TERM=xterm-256color".into());
    env.push("TERM_PROGRAM=vmux".into());
    env.push(concat!("TERM_PROGRAM_VERSION=", env!("CARGO_PKG_VERSION")).to_string());
    match relay_path() {
        Some(relay) => {
            let mut wrapped = Vec::with_capacity(argv.len() + 1);
            wrapped.push(relay);
            wrapped.extend(argv.iter().cloned());
            spawn_with_env(term, cwd, wrapped, env, Some(argv.to_vec()));
        }
        None => {
            feed_line(term, "[vmux] vmux-relay not found; desktop notifications disabled");
            spawn_with_env(term, cwd, argv.to_vec(), env, None);
        }
    }
}

/// Spawn `argv`; if that fails and `fallback` is set (the bare command, for
/// when the relay itself is broken), retry without the relay.
fn spawn_with_env(
    term: &vte::Terminal,
    cwd: &str,
    argv: Vec<String>,
    env: Vec<String>,
    fallback: Option<Vec<String>>,
) {
    let argv_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let env_refs: Vec<&str> = env.iter().map(|s| s.as_str()).collect();
    let tw = term.downgrade();
    let cwd_owned = cwd.to_string();
    let env_again = env.clone();
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
            let Err(e) = res else { return };
            let Some(term) = tw.upgrade() else { return };
            match fallback {
                Some(bare) => {
                    feed_line(
                        &term,
                        &format!("[vmux] relay spawn failed ({e}); desktop notifications disabled"),
                    );
                    spawn_with_env(&term, &cwd_owned, bare, env_again, None);
                }
                None => feed_line(&term, &format!("[vmux] spawn failed: {e}")),
            }
        },
    );
}

/// The vmux-relay binary: next to the vmux executable (cargo target dir or
/// install prefix), else on PATH.
fn relay_path() -> Option<String> {
    let sibling = std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name("vmux-relay"));
    let path_hits = std::env::var_os("PATH").map(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("vmux-relay"))
            .collect::<Vec<_>>()
    });
    sibling
        .into_iter()
        .chain(path_hits.into_iter().flatten())
        .find(|p| is_executable(p))
        .map(|p| p.to_string_lossy().into_owned())
}

fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn feed_line(term: &vte::Terminal, msg: &str) {
    term.feed(format!("\r\n\x1b[2m{msg}\x1b[0m\r\n").as_bytes());
}

// vte 0.78 deprecates this accessor in favor of termprops; it still works.
#[allow(deprecated)]
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
