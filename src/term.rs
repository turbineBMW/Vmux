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

/// Register the custom termprops vmux-relay uses to talk back to vmux (the
/// safe vte4 crate does not wrap the install call). vte requires these to run
/// before the first vte::Terminal exists.
pub fn install_notify_termprop() {
    // Ephemeral: the notification value is only readable inside its handler.
    install_termprop(vmux::osc_scan::TERMPROP_NAME, vte::ffi::VTE_PROPERTY_FLAG_EPHEMERAL);
}

/// Register the termprop carrying the foreground command for tab titles.
/// Non-ephemeral so refresh_title can read it on demand and vte de-dups
/// unchanged values; like the notify prop it must precede the first terminal.
pub fn install_fgproc_termprop() {
    install_termprop(vmux::osc_scan::FGPROC_TERMPROP_NAME, 0);
}

fn install_termprop(name: &str, flags: u32) {
    let cname = std::ffi::CString::new(name).unwrap();
    let id =
        unsafe { vte::ffi::vte_install_termprop(cname.as_ptr(), vte::ffi::VTE_PROPERTY_DATA, flags) };
    if id < 0 {
        eprintln!("vmux: failed to install termprop {name}");
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
        // Our own touch gesture (add_touch_scroll) drives finger scrolling;
        // the built-in kinetic-scroll gesture would otherwise claim the touch
        // sequence the instant it moves, starving ours of updates.
        .kinetic_scrolling(false)
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

    add_touch_scroll(&term);

    let argv = shell_argv(&app.config.borrow());
    spawn_argv(&term, &cwd, &argv);
    scrolled
}

/// Vertical distance a finger must travel before a touch drag is treated as a
/// scroll rather than a tap. Below this the touch is left to vte so taps still
/// reach the terminal.
const TOUCH_PAN_THRESHOLD: f64 = 8.0;

/// Drag-to-scroll with a finger, without aiming for the scrollbar.
///
/// vte binds touch drags to text selection and claims the touch sequence on
/// the first motion event, which denies any competing GtkGesture before it can
/// react — so a threshold-based GestureDrag never even receives an update.
/// Instead we watch the raw touch events in the capture phase (ahead of vte)
/// and steer them by hand: small movements and taps are passed through to vte
/// (`Proceed`), and once the finger clearly pans we consume the rest of the
/// sequence (`Stop`) and scroll it ourselves.
///
/// How we scroll depends on the foreground program. When vte's scrollback has
/// room to move (normal-screen shell output) we drag its adjustment directly —
/// smooth and pixel-accurate. When it doesn't (a full-screen/alt-screen app
/// such as an editor or pager) there is nothing in the scrollback to move, so
/// we synthesise mouse-wheel escapes and feed them to the child, which lets
/// mouse-aware TUIs (editors, Claude Code, …) scroll. vte4 exposes no way to
/// query the app's mouse mode, so programs that never enable mouse reporting
/// won't respond to this.
fn add_touch_scroll(term: &vte::Terminal) {
    use std::cell::Cell;
    // Y position (surface coords) where the current touch started.
    let start_y = Rc::new(Cell::new(0.0));
    // NaN until the gesture commits to a scroll; non-NaN also means "we own
    // this sequence now". In adjustment mode it holds the anchored adjustment
    // value; in wheel mode it holds the Y of the last emitted wheel step.
    let anchor = Rc::new(Cell::new(f64::NAN));
    // Once committed: true = feed wheel escapes to the child, false = drive the
    // scrollback adjustment.
    let wheel = Rc::new(Cell::new(false));

    let ctl = gtk::EventControllerLegacy::new();
    ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tw = term.downgrade();
    ctl.connect_event(move |_, ev| {
        use gtk::gdk::EventType;
        let Some(term) = tw.upgrade() else {
            return glib::Propagation::Proceed;
        };
        match ev.event_type() {
            EventType::TouchBegin => {
                start_y.set(ev.position().map_or(0.0, |(_, y)| y));
                anchor.set(f64::NAN);
                // Let vte see the press so a tap still positions/pastes.
                glib::Propagation::Proceed
            }
            EventType::TouchUpdate => {
                let Some((x, y)) = ev.position() else {
                    return glib::Propagation::Proceed;
                };
                let dy = y - start_y.get();
                if anchor.get().is_nan() {
                    // Still within the tap threshold: leave it to vte.
                    if dy.abs() < TOUCH_PAN_THRESHOLD {
                        return glib::Propagation::Proceed;
                    }
                    // Commit. Prefer the scrollback if it has anywhere to go;
                    // otherwise fall back to feeding wheel events to the app.
                    let has_scrollback = term
                        .vadjustment()
                        .is_some_and(|a| a.upper() - a.page_size() > 1.0);
                    wheel.set(!has_scrollback);
                    anchor.set(if has_scrollback {
                        term.vadjustment().map_or(0.0, |a| a.value()) + dy
                    } else {
                        y
                    });
                    // Drop any selection vte began during the pre-commit phase.
                    term.unselect_all();
                }
                if wheel.get() {
                    // One wheel notch per few rows of finger travel, so the
                    // on-screen motion roughly tracks the finger for a typical
                    // 3-line wheel step. Finger moving down (y grows) reveals
                    // earlier output, i.e. wheel up.
                    let step = 3.0 * term.char_height().max(1) as f64;
                    let mut last = anchor.get();
                    while y - last >= step {
                        feed_wheel(&term, x, y, true);
                        last += step;
                    }
                    while last - y >= step {
                        feed_wheel(&term, x, y, false);
                        last -= step;
                    }
                    anchor.set(last);
                } else if let Some(adj) = term.vadjustment() {
                    // Finger down-drag (positive dy) reveals earlier output,
                    // i.e. a smaller adjustment value.
                    adj.set_value(anchor.get() - dy);
                }
                glib::Propagation::Stop
            }
            EventType::TouchEnd | EventType::TouchCancel => {
                let owned = !anchor.get().is_nan();
                anchor.set(f64::NAN);
                // Only swallow the release if we actually took over; otherwise
                // let vte complete the tap.
                if owned {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
            _ => glib::Propagation::Proceed,
        }
    });
    term.add_controller(ctl);
}

/// Feed one SGR (1006) mouse-wheel event to the child at the cell under
/// (`x`, `y`), which are given in the terminal's surface coordinates.
fn feed_wheel(term: &vte::Terminal, x: f64, y: f64, up: bool) {
    // Translate surface coords to widget-local so the reported cell lands in
    // the right place (e.g. the correct split under the finger). Fall back to
    // the raw coords if the widget isn't in a surface yet.
    let (lx, ly) = term
        .root()
        .and_then(|root| term.compute_point(&root, &gtk::graphene::Point::new(0.0, 0.0)))
        .map_or((x, y), |o| (x - o.x() as f64, y - o.y() as f64));

    let col =
        ((lx / term.char_width().max(1) as f64) as i64 + 1).clamp(1, term.column_count().max(1));
    let row =
        ((ly / term.char_height().max(1) as f64) as i64 + 1).clamp(1, term.row_count().max(1));
    let btn = if up { 64 } else { 65 };
    term.feed_child(format!("\x1b[<{btn};{col};{row}M").as_bytes());
}

fn configure(term: &vte::Terminal, cfg: &Config, scale: f64) {
    term.set_mouse_autohide(true);
    // Scroll in pixels rather than whole rows, so a finger pan tracks
    // smoothly instead of snapping a line at a time.
    term.set_scroll_unit_is_pixels(true);
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
