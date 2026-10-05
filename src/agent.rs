//! Coding-agent activity per terminal, read from the signals vmux already
//! has: the foreground command vmux-relay reports for each terminal (which
//! agent is in front, if any), the terminal's OSC window title (busy, or
//! waiting on the user) and, for agents that need it, the bottom of the live
//! screen (an approval dialog). See [`vmux::agents`] for the rules.
//!
//! Each terminal keeps its own state — what its agent is doing, and whether
//! the user has seen it stop — which drives three views: the tab (a spinner
//! while working, a mark when it asks or finishes unseen), the zone chip (a
//! badge while an agent is open, pulsing while one works, and a ring while
//! one waits on the user or finished out of view) and the sidebar's Agents
//! section. A zone whose agent starts working can optionally move to the top
//! of the sidebar.

use crate::app::App;
use crate::zone::Zone;
use crate::{pane, splits};
use gtk4 as gtk;
use gtk4::{gio, glib};
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use vte4 as vte;
use vte4::prelude::*;

pub use vmux::agents::{Activity, Detected, Rollup, Status, rollup, sort_by_attention};

/// How long a terminal must stay non-working before its agent counts as
/// stopped. Agents drop the spinner for an instant between tool calls;
/// without this, every such flicker would ring the chip.
const SETTLE_MS: u64 = 1200;

/// The most often an idle agent's screen is re-read as its output changes.
const SCREEN_MS: u64 = 300;

/// Object-data key for [`TermAgent`].
const TRACK_KEY: &str = "vmux-agent";

/// Per-terminal tracking state. It lives on the terminal itself, so it
/// follows a tab dragged to another pane and goes away with the terminal.
struct TermAgent {
    /// The last committed state (see `SETTLE_MS` for why "committed").
    current: Cell<Option<Detected>>,
    /// False once the agent stopped or asked while out of view; set again
    /// when the user looks at the terminal.
    seen: Cell<bool>,
    /// [`next_seq`] at the last committed change, for recency ordering.
    changed: Cell<u64>,
    /// The agent in front reads its state from the screen too.
    reads_screen: Cell<bool>,
    settle: RefCell<Option<glib::SourceId>>,
    /// A throttled screen re-read is scheduled.
    screen_pending: Cell<bool>,
}

fn tracked(term: &vte::Terminal) -> &TermAgent {
    // SAFETY: the data is only ever set here, once, with this type, and is
    // never replaced or stolen, so the pointer stays valid for as long as
    // the terminal (and so the borrow) lives.
    unsafe {
        if term.data::<TermAgent>(TRACK_KEY).is_none() {
            term.set_data(
                TRACK_KEY,
                TermAgent {
                    current: Cell::new(None),
                    seen: Cell::new(true),
                    changed: Cell::new(0),
                    reads_screen: Cell::new(false),
                    settle: RefCell::new(None),
                    screen_pending: Cell::new(false),
                },
            );
        }
        term.data::<TermAgent>(TRACK_KEY)
            .expect("just set")
            .as_ref()
    }
}

/// Tracking state for `term` if any was ever recorded, without creating it.
fn tracked_if_any(term: &vte::Terminal) -> Option<&TermAgent> {
    // SAFETY: as in `tracked`.
    unsafe { term.data::<TermAgent>(TRACK_KEY).map(|p| p.as_ref()) }
}

fn next_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Per-zone summary; lives on [`Zone`].
#[derive(Default)]
pub struct ZoneAgent {
    pub rollup: Cell<Rollup>,
}

/// One terminal with an agent in front.
#[derive(Clone)]
pub struct Entry {
    pub term: vte::Terminal,
    pub agent: &'static str,
    pub status: Status,
    /// Sequence number of its last state change; larger is newer.
    pub changed: u64,
}

pub fn entry(term: &vte::Terminal) -> Option<Entry> {
    let st = tracked_if_any(term)?;
    let d = st.current.get()?;
    Some(Entry {
        term: term.clone(),
        agent: d.agent,
        status: Status::of(d.activity, st.seen.get()),
        changed: st.changed.get(),
    })
}

/// Every agent in `zone`, in layout order.
pub fn entries_in(zone: &Zone) -> Vec<Entry> {
    let mut terms = Vec::new();
    splits::all_terminals_in(zone.page.upcast_ref(), &mut terms);
    terms.iter().filter_map(entry).collect()
}

/// What the agent in `term` is doing, from its command, title and screen.
fn detect(term: &vte::Terminal) -> Option<Detected> {
    let command = pane::fg_command(term)?;
    #[allow(deprecated)] // window_title: see pane::new_tab
    let title = term
        .window_title()
        .map(|t| t.to_string())
        .unwrap_or_default();
    tracked(term)
        .reads_screen
        .set(vmux::agents::reads_screen(&command));
    let d = vmux::agents::detect_with_screen(&command, &title, || {
        let screen = live_screen(term);
        if std::env::var_os("VMUX_DEBUG_AGENTS").is_some() {
            eprintln!("agent-screen: {command} {title:?}\n{screen}\n-- end screen");
        }
        screen
    });
    if std::env::var_os("VMUX_DEBUG_AGENTS").is_some() {
        eprintln!("agent-detect: {command} {title:?} -> {d:?}");
    }
    d
}

/// The live screen's text, wherever the user has scrolled the view to.
/// vte has no accessor for where the live screen starts in the buffer, but
/// the cursor is always on it and its row is absolute, so a screen's height
/// either side of the cursor covers the whole screen (rows past the end of
/// the buffer read as nothing).
fn live_screen(term: &vte::Terminal) -> String {
    let rows = term.row_count();
    let (_, cursor) = term.cursor_position();
    term.text_range_format(
        vte::Format::Text,
        (cursor - rows).max(0),
        0,
        cursor + rows,
        term.column_count(),
    )
    .0
    .map(|s| s.to_string())
    .unwrap_or_default()
}

/// Whether the user can be assumed to see `term`: its tab is showing in the
/// visible zone, and the window is active.
fn in_view(app: &App, term: &vte::Terminal) -> bool {
    term.is_mapped() && app.window.is_active()
}

fn is_working(d: Option<Detected>) -> bool {
    d.is_some_and(|d| d.activity == Activity::Working)
}

/// Re-read one terminal and commit any change. Called from its title-changed
/// and fgproc signals, and (throttled) as its screen changes.
pub fn refresh_terminal(app: &Rc<App>, zone: &Rc<Zone>, term: &vte::Terminal) {
    let now = detect(term);
    let st = tracked(term);
    let prev = st.current.get();

    if is_working(now) {
        if let Some(id) = st.settle.borrow_mut().take() {
            id.remove();
        }
        if now != prev {
            commit(app, zone, term, now);
        }
    } else if is_working(prev) {
        // Leaving "working" is debounced; the timer re-reads and commits.
        if st.settle.borrow().is_none() {
            let app = app.clone();
            let zw = Rc::downgrade(zone);
            let tw = term.downgrade();
            let id = glib::timeout_add_local_once(
                std::time::Duration::from_millis(SETTLE_MS),
                move || {
                    let Some(term) = tw.upgrade() else { return };
                    *tracked(&term).settle.borrow_mut() = None;
                    let Some(zone) = zw.upgrade() else { return };
                    let now = detect(&term);
                    if !is_working(now) {
                        commit(&app, &zone, &term, now);
                    }
                },
            );
            *st.settle.borrow_mut() = Some(id);
        }
    } else if now != prev {
        commit(app, zone, term, now);
    }

    if now.is_some() {
        // The task title shown in the Agents section may have changed.
        app.agent_panel.schedule_refresh(app);
    }
}

fn commit(app: &Rc<App>, zone: &Rc<Zone>, term: &vte::Terminal, now: Option<Detected>) {
    let st = tracked(term);
    let prev = st.current.replace(now).map(|d| d.activity);
    st.changed.set(next_seq());
    match now.map(|d| d.activity) {
        None | Some(Activity::Working) => st.seen.set(true),
        // It stopped, or started asking: unseen unless the user is looking.
        Some(a)
            if prev == Some(Activity::Working)
                || (a == Activity::NeedsInput && prev != Some(a)) =>
        {
            st.seen.set(in_view(app, term));
        }
        Some(_) => {}
    }
    apply_tab(term);
    zone_changed(app, zone);
}

/// Re-roll the zone's summary from its terminals and push it everywhere.
fn zone_changed(app: &Rc<App>, zone: &Rc<Zone>) {
    let r = rollup(entries_in(zone).iter().map(|e| (e.agent, e.status)));
    let was_working = zone.agent.rollup.replace(r).working;
    apply(zone);
    if r.working && !was_working {
        app.on_agent_started(zone);
    }
    app.agent_panel.schedule_refresh(app);
}

/// Recompute `zone`'s summary, e.g. after one of its terminals went away.
pub fn refresh(app: &Rc<App>, zone: &Rc<Zone>) {
    zone_changed(app, zone);
}

/// Output changed in `term`: if an idle agent there reads its state from the
/// screen, re-read it soon. Cheap when there's nothing to do — this runs on
/// every screen update of every terminal.
pub fn contents_changed(app: &Rc<App>, zone: &std::rc::Weak<Zone>, term: &vte::Terminal) {
    let Some(st) = tracked_if_any(term) else {
        return;
    };
    // A working agent is followed through its title; no agent, nothing to read.
    if !st.reads_screen.get() || st.current.get().is_none() || is_working(st.current.get()) {
        return;
    }
    if st.screen_pending.replace(true) {
        return;
    }
    let app = app.clone();
    let zone = zone.clone();
    let tw = term.downgrade();
    glib::timeout_add_local_once(std::time::Duration::from_millis(SCREEN_MS), move || {
        let Some(term) = tw.upgrade() else { return };
        tracked(&term).screen_pending.set(false);
        if let Some(zone) = zone.upgrade()
            && term.root().is_some()
        {
            refresh_terminal(&app, &zone, &term);
        }
    });
}

/// The user may now be looking at `term` (it was mapped, or the window was
/// activated): if so, its agent's last stop counts as seen.
pub fn mark_seen(app: &Rc<App>, zone: &Rc<Zone>, term: &vte::Terminal) {
    let Some(st) = tracked_if_any(term) else {
        return;
    };
    if st.seen.get() || !in_view(app, term) {
        return;
    }
    st.seen.set(true);
    apply_tab(term);
    zone_changed(app, zone);
}

/// [`mark_seen`] for every terminal of `zone` that is showing.
pub fn mark_visible_seen(app: &Rc<App>, zone: &Rc<Zone>) {
    let mut terms = Vec::new();
    splits::all_terminals_in(zone.page.upcast_ref(), &mut terms);
    for t in &terms {
        mark_seen(app, zone, t);
    }
}

/// Show the terminal's agent state on its tab: adw's loading spinner while
/// working; a question mark (needs input) or a check (finished unseen) as
/// the indicator, with the tab flagged for attention so a tab scrolled out
/// of the strip lights the strip's edge.
fn apply_tab(term: &vte::Terminal) {
    let Some(page) = pane::page_of(term) else {
        return;
    };
    let e = entry(term);
    let status = e.as_ref().map(|e| e.status);
    page.set_loading(status == Some(Status::Working));
    page.set_needs_attention(status.is_some_and(Status::wants_attention));
    let icon = match status {
        Some(Status::NeedsInput) => Some("dialog-question-symbolic"),
        Some(Status::Done) => Some("object-select-symbolic"),
        _ => None,
    };
    page.set_indicator_icon(icon.map(gio::ThemedIcon::new).as_ref());
    page.set_indicator_tooltip(
        &e.filter(|_| icon.is_some())
            .map(|e| describe(e.agent, e.status))
            .unwrap_or_default(),
    );
}

/// Push the zone's summary onto the chip's badge and ring.
pub fn apply(zone: &Zone) {
    let r = zone.agent.rollup.get();
    style_badge(&zone.agent_badge, r);
    zone.agent_badge.set_visible(r.top.is_some());
    set_class(&zone.agent_chip, "agent-attention", wants_attention(r));
    zone.agent_chip
        .set_tooltip_text(r.top.map(|(a, s)| describe(a, s)).as_deref());
}

/// Whether a summary should ring: some agent asked or finished unseen.
pub fn wants_attention(r: Rollup) -> bool {
    r.top.is_some_and(|(_, s)| s.wants_attention())
}

/// Put a zone summary's classes on a badge/dot widget: pulsing while any
/// agent works, red while one waits on the user.
pub fn style_badge(badge: &impl IsA<gtk::Widget>, r: Rollup) {
    set_class(badge, "working", r.working);
    set_class(
        badge,
        "attention",
        r.top.is_some_and(|(_, s)| s == Status::NeedsInput),
    );
}

/// Put one agent's status classes on a dot widget.
pub fn style_dot(dot: &impl IsA<gtk::Widget>, status: Status) {
    set_class(dot, "working", status == Status::Working);
    set_class(dot, "attention", status == Status::NeedsInput);
    set_class(dot, "done", status == Status::Done);
}

pub fn describe(agent: &str, status: Status) -> String {
    match status {
        Status::Working => format!("{agent} is working"),
        Status::NeedsInput => format!("{agent} needs your input"),
        Status::Done => format!("{agent} finished"),
        Status::Idle => format!("{agent} is open"),
    }
}

fn set_class(w: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}
