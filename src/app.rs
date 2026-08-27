use crate::zone::Zone;
use crate::{git, keybinds, pane, splits, state, style, term, text_bindings, window};
use gtk4 as gtk;
use gtk4::{gio, glib};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use vte4 as vte;
use vte4::prelude::*;

pub struct App {
    pub window: adw::ApplicationWindow,
    pub split_view: adw::OverlaySplitView,
    pub stack: gtk::Stack,
    pub listbox: gtk::ListBox,
    pub titlebar: adw::HeaderBar,
    pub sidebar_hide_btn: gtk::Button,
    pub zones: RefCell<Vec<Rc<Zone>>>,
    pub config: RefCell<state::Config>,
    pub font_scale: Cell<f64>,
    /// Key -> bytes bindings from bindings.conf, shared by every terminal.
    pub text_bindings: RefCell<Vec<text_bindings::TextBinding>>,
    save_source: RefCell<Option<glib::SourceId>>,
    next_zone_id: Cell<u64>,
    shortcut_ctl: RefCell<Option<gtk::ShortcutController>>,
    bindings_monitor: RefCell<Option<gio::FileMonitor>>,
    /// User CSS from style.css, applied at the USER priority so it overrides
    /// both the Adwaita theme and vmux's built-in styles.
    style_provider: gtk::CssProvider,
    style_monitor: RefCell<Option<gio::FileMonitor>>,
    /// Panes whose last tab was torn off by a drag; resolved by
    /// [`Self::sweep_drag_emptied`] once the drag concludes.
    drag_emptied: RefCell<Vec<PendingCollapse>>,
}

/// A pane emptied by a tab drag tear-off. It must stay alive as a drop target
/// until the drag concludes (cancel and drop-back re-attach the page to it);
/// every conclusion re-attaches the page to some view, so a page-attached
/// anywhere triggers the sweep that resolves these.
struct PendingCollapse {
    pane: glib::WeakRef<gtk::Stack>,
    zone: Weak<Zone>,
    /// The torn-off tab, for reselecting/refocusing it after the drag.
    page: glib::WeakRef<adw::TabPage>,
}

pub fn build(gtk_app: &adw::Application) {
    let st = state::load();
    let chrome = window::build_chrome(gtk_app);
    let app = Rc::new(App {
        window: chrome.window.clone(),
        split_view: chrome.split_view.clone(),
        stack: chrome.stack.clone(),
        listbox: chrome.listbox.clone(),
        titlebar: chrome.titlebar.clone(),
        sidebar_hide_btn: chrome.sidebar_hide_btn.clone(),
        zones: RefCell::new(Vec::new()),
        config: RefCell::new(st.config.clone()),
        font_scale: Cell::new(1.0),
        text_bindings: RefCell::new(text_bindings::load()),
        save_source: RefCell::new(None),
        next_zone_id: Cell::new(1),
        shortcut_ctl: RefCell::new(None),
        bindings_monitor: RefCell::new(None),
        style_provider: gtk::CssProvider::new(),
        style_monitor: RefCell::new(None),
        drag_emptied: RefCell::new(Vec::new()),
    });
    window::wire_chrome(&app, &chrome);
    // Notification clicks land here: switch to the originating zone and
    // present the window (focus follows compositor policy via the
    // activation token, when the notification daemon provides one).
    {
        let app = app.clone();
        let act = gio::SimpleAction::new("focus-zone", Some(glib::VariantTy::UINT64));
        act.connect_activate(move |_, param| {
            let Some(id) = param.and_then(|v| v.get::<u64>()) else { return };
            let idx = app.zones.borrow().iter().position(|z| z.id == id);
            if let Some(idx) = idx {
                app.select_zone(idx);
            }
            app.window.present();
        });
        gtk_app.add_action(&act);
    }
    app.sync_titlebar();
    // Restore the saved sidebar visibility. Done after wire_chrome so the
    // titlebar toggle's bidirectional binding is already live and picks this
    // up, rather than clobbering it via sync_create().
    app.split_view.set_show_sidebar(st.config.show_sidebar);
    app.reinstall_shortcuts();
    app.install_user_css();
    app.sync_window_transparency();
    app.watch_text_bindings();
    app.watch_user_css();
    for zs in &st.zones {
        app.append_zone(zs);
    }
    app.select_zone(st.active_zone);
    app.start_git_polling();
    app.save_now();
    app.window.present();
    // Presenting resolves each terminal's CSS-derived Pango font against the
    // display; transfer that computed font and the palette into VTE now.
    app.apply_terminal_style();
}

impl App {
    // ----- actions -------------------------------------------------------

    pub fn run_action(self: &Rc<Self>, id: &str) -> glib::Propagation {
        match id {
            "new-tab" => self.new_tab_active(),
            "close-tab" => self.close_focused_tab(),
            "split-right" => self.split_focused(gtk::Orientation::Horizontal),
            "split-down" => self.split_focused(gtk::Orientation::Vertical),
            "focus-next-pane" => self.focus_next_pane(),
            "focus-pane-left" => self.focus_pane_directional(splits::Direction::Left),
            "focus-pane-right" => self.focus_pane_directional(splits::Direction::Right),
            "focus-pane-up" => self.focus_pane_directional(splits::Direction::Up),
            "focus-pane-down" => self.focus_pane_directional(splits::Direction::Down),
            "grow-pane" => self.resize_focused(true),
            "shrink-pane" => self.resize_focused(false),
            "next-tab" => self.cycle_tab(true),
            "prev-tab" => self.cycle_tab(false),
            "new-zone" => {
                window::new_zone_dialog(self);
                glib::Propagation::Stop
            }
            "close-zone" => {
                if let Some(zone) = self.active_zone() {
                    window::remove_zone_dialog(self, &zone);
                }
                glib::Propagation::Stop
            }
            "rename-zone" => {
                if let Some(zone) = self.active_zone() {
                    window::rename_zone_dialog(self, &zone);
                }
                glib::Propagation::Stop
            }
            "prev-zone" => self.cycle_zone(false),
            "next-zone" => self.cycle_zone(true),
            "move-zone-up" => self.move_zone(-1),
            "move-zone-down" => self.move_zone(1),
            "copy" => self.copy(),
            "paste" => self.paste(),
            "font-inc" => self.adjust_font_scale(Some(0.1)),
            "font-dec" => self.adjust_font_scale(Some(-0.1)),
            "font-reset" => self.adjust_font_scale(None),
            "toggle-sidebar" => {
                self.split_view.set_show_sidebar(!self.split_view.shows_sidebar());
                glib::Propagation::Stop
            }
            "preferences" => {
                keybinds::show_settings(self);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    /// (Re)build the capture-phase shortcut controller from current bindings.
    pub fn reinstall_shortcuts(self: &Rc<Self>) {
        if let Some(old) = self.shortcut_ctl.borrow_mut().take() {
            self.window.remove_controller(&old);
        }
        let ctl = gtk::ShortcutController::new();
        ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
        for (id, _title, accel) in keybinds::merged(&self.config.borrow()) {
            if accel.is_empty() {
                continue;
            }
            let app = self.clone();
            keybinds::add_sc(&ctl, &accel, move || app.run_action(&id));
        }
        for i in 1..=9usize {
            let app = self.clone();
            keybinds::add_sc(&ctl, &format!("<Alt>{i}"), move || {
                app.select_zone(i - 1);
                glib::Propagation::Stop
            });
        }
        self.window.add_controller(ctl.clone());
        *self.shortcut_ctl.borrow_mut() = Some(ctl);
    }

    /// Hot-reload bindings.conf whenever it changes on disk (Deleted included:
    /// load() then regenerates the template).
    fn watch_text_bindings(self: &Rc<Self>) {
        let file = gio::File::for_path(text_bindings::path());
        match file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
            Ok(monitor) => {
                let app = self.clone();
                monitor.connect_changed(move |_, _, _, event| {
                    use gio::FileMonitorEvent as E;
                    if matches!(
                        event,
                        E::ChangesDoneHint | E::Renamed | E::MovedIn | E::Created | E::Deleted
                    ) {
                        *app.text_bindings.borrow_mut() = text_bindings::load();
                    }
                });
                *self.bindings_monitor.borrow_mut() = Some(monitor);
            }
            Err(e) => {
                eprintln!("vmux: cannot watch {}: {e}", text_bindings::path().display());
            }
        }
    }

    /// Register the style.css provider on the display at the USER priority
    /// (above the THEME and APPLICATION providers, so user rules win), then
    /// load the file's current contents.
    fn install_user_css(self: &Rc<Self>) {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &self.style_provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        // Surface parse errors to the log rather than silently dropping the
        // offending rule (GTK keeps applying the valid remainder).
        self.style_provider.connect_parsing_error(|_, section, err| {
            eprintln!(
                "vmux: style.css line {}: {err}",
                section.start_location().lines() + 1
            );
        });
        self.reload_user_css();
    }

    /// Re-read style.css into the provider, restyling every widget live.
    pub(crate) fn reload_user_css(&self) {
        self.style_provider.load_from_data(&style::load());
        self.apply_terminal_style();
        self.sync_window_transparency();
    }

    /// Hot-reload style.css whenever it changes on disk (mirrors the
    /// bindings.conf watcher; Deleted re-seeds the template via load()).
    fn watch_user_css(self: &Rc<Self>) {
        let file = gio::File::for_path(style::path());
        match file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
            Ok(monitor) => {
                let app = self.clone();
                monitor.connect_changed(move |_, _, _, event| {
                    use gio::FileMonitorEvent as E;
                    if matches!(
                        event,
                        E::ChangesDoneHint | E::Renamed | E::MovedIn | E::Created | E::Deleted
                    ) {
                        app.reload_user_css();
                    }
                });
                *self.style_monitor.borrow_mut() = Some(monitor);
            }
            Err(e) => {
                eprintln!("vmux: cannot watch {}: {e}", style::path().display());
            }
        }
    }

    // ----- zones ---------------------------------------------------------

    fn append_zone(self: &Rc<Self>, zs: &state::ZoneState) -> Rc<Zone> {
        let id = self.next_zone_id.get();
        self.next_zone_id.set(id + 1);
        let zone = Zone::build(self, zs, id);
        self.stack.add_named(&zone.page, Some(&zone.stack_name()));
        self.listbox.append(&zone.row);
        self.zones.borrow_mut().push(zone.clone());
        self.update_sidebar_reveal();
        Self::refresh_zone_git(&zone);
        zone
    }

    pub fn add_zone(self: &Rc<Self>, name: &str, cwd: &str) {
        let zs = state::ZoneState {
            name: name.into(),
            cwd: cwd.into(),
            root: None,
            tabs: Vec::new(),
        };
        self.append_zone(&zs);
        let idx = self.zones.borrow().len() - 1;
        self.select_zone(idx);
        self.schedule_save();
    }

    pub fn remove_zone(self: &Rc<Self>, zone: &Rc<Zone>) {
        let idx_opt = self.zones.borrow().iter().position(|z| Rc::ptr_eq(z, zone));
        let Some(idx) = idx_opt else { return };
        self.withdraw_zone_notification(zone);
        self.zones.borrow_mut().remove(idx);
        self.listbox.remove(&zone.row);
        self.stack.remove(&zone.page);
        let len = self.zones.borrow().len();
        if len == 0 {
            self.add_zone("main", &state::home_dir());
        } else {
            self.select_zone(idx.min(len - 1));
        }
        self.schedule_save();
    }

    pub fn rename_zone(self: &Rc<Self>, zone: &Rc<Zone>, name: &str) {
        *zone.name.borrow_mut() = name.to_string();
        zone.name_label.set_label(name);
        self.schedule_save();
    }

    pub fn select_zone(self: &Rc<Self>, idx: usize) {
        if let Some(row) = self.listbox.row_at_index(idx as i32) {
            self.listbox.select_row(Some(&row));
        }
    }

    fn cycle_zone(self: &Rc<Self>, next: bool) -> glib::Propagation {
        let len = self.zones.borrow().len();
        if len == 0 {
            return glib::Propagation::Stop;
        }
        let cur = self
            .listbox
            .selected_row()
            .map(|r| r.index().max(0) as usize)
            .unwrap_or(0);
        let idx = if next { (cur + 1) % len } else { (cur + len - 1) % len };
        self.select_zone(idx);
        glib::Propagation::Stop
    }

    fn move_zone(self: &Rc<Self>, delta: i32) -> glib::Propagation {
        let Some(row) = self.listbox.selected_row() else {
            return glib::Propagation::Stop;
        };
        let cur = row.index();
        let new = cur + delta;
        if cur < 0 || new < 0 || new >= self.zones.borrow().len() as i32 {
            return glib::Propagation::Stop;
        }
        let (cur, new) = (cur as usize, new as usize);
        // Reorder by moving the NEIGHBOR row past the selected one, not the
        // selected row itself. Removing the selected row from the ListBox clears
        // the selection, and a follow-up select_row() on the re-inserted row does
        // not take — leaving nothing selected, which broke every subsequent
        // move/cycle (they read selected_row().index()). Moves are always between
        // adjacent slots, so swapping the untouched neighbor to the other side of
        // the selected row produces the same order while preserving the selection
        // and the selected row's (now-shifted) index.
        let neighbor = self.zones.borrow()[new].row.clone();
        self.zones.borrow_mut().swap(cur, new);
        self.listbox.remove(&neighbor);
        self.listbox.insert(&neighbor, cur as i32);
        self.schedule_save();
        glib::Propagation::Stop
    }

    pub fn on_zone_selected(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let idx = row.index();
        if idx < 0 {
            return;
        }
        let Some(zone) = self.zones.borrow().get(idx as usize).cloned() else {
            return;
        };
        // Read this before switching pages: GtkStack moves keyboard focus into
        // the new page's first focusable child, which fires the terminal's
        // focus-enter hook and clobbers `last_focused` with the top pane.
        let target = zone
            .last_focused
            .upgrade()
            .or_else(|| splits::first_terminal_in(zone.page.upcast_ref()));
        if std::env::var_os("VMUX_DEBUG_FOCUS").is_some() {
            eprintln!(
                "zone-selected: zone={} target={:?} window-focus={:?}",
                zone.name.borrow(),
                target.as_ref().map(|t| t.as_ptr()),
                GtkWindowExt::focus(&self.window).map(|w| w.type_().name().to_string())
            );
        }
        self.stack.set_visible_child_name(&zone.stack_name());
        zone.attention.set_visible(false);
        self.withdraw_zone_notification(&zone);
        if let Some(t) = target {
            term::focus_later(&t);
        }
        self.schedule_save();
    }

    pub fn active_zone(&self) -> Option<Rc<Zone>> {
        let idx = self.listbox.selected_row()?.index();
        if idx < 0 {
            return None;
        }
        self.zones.borrow().get(idx as usize).cloned()
    }

    // ----- focus helpers --------------------------------------------------

    pub fn focused_terminal(&self) -> Option<vte::Terminal> {
        if let Some(w) = GtkWindowExt::focus(&self.window)
            && let Ok(t) = w.downcast::<vte::Terminal>()
        {
            return Some(t);
        }
        self.active_zone().and_then(|z| z.last_focused.upgrade())
    }

    /// The pane that holds the focused terminal, else the zone's first pane.
    fn focused_pane(&self, zone: &Rc<Zone>) -> Option<gtk::Stack> {
        let by_focus = self
            .focused_terminal()
            .and_then(|t| splits::pane_of(t.upcast_ref()));
        let w = by_focus.or_else(|| {
            let mut panes = Vec::new();
            splits::all_panes_in(zone.page.upcast_ref(), &mut panes);
            panes.into_iter().next()
        })?;
        w.downcast::<gtk::Stack>().ok()
    }

    pub fn on_bell(self: &Rc<Self>, zone: &Rc<Zone>) {
        let visible = self
            .stack
            .visible_child_name()
            .map(|n| n == zone.stack_name())
            .unwrap_or(false);
        if !visible || !self.window.is_active() {
            zone.attention.set_visible(true);
            let notify = {
                let cfg = self.config.borrow();
                cfg.notify_on_bell && cfg.desktop_notifications
            };
            if notify {
                self.send_zone_notification(zone, &zone.name.borrow(), "Terminal bell");
            }
        }
    }

    /// A notification OSC (9/777/99) arrived from `terminal`, relayed by
    /// vmux-relay through the vte.ext.vmux.notify termprop.
    pub fn on_notify(
        self: &Rc<Self>,
        zone: &Rc<Zone>,
        terminal: Option<&vte::Terminal>,
        title: &str,
        body: &str,
    ) {
        let zone_visible = self
            .stack
            .visible_child_name()
            .map(|n| n == zone.stack_name())
            .unwrap_or(false);
        // A background tab's terminal is unmapped even when its zone is the
        // visible one; if the terminal is already gone, fall back to the
        // zone-level answer.
        let pane_visible = terminal.map(|t| t.is_mapped()).unwrap_or(zone_visible);
        if !zone_visible || !self.window.is_active() {
            zone.attention.set_visible(true);
        }
        if pane_visible && self.window.is_active() {
            return; // the user is looking at it — ghostty suppresses too
        }
        if !self.config.borrow().desktop_notifications {
            return;
        }
        // Name the zone in the message so a notification like "Claude is
        // waiting for your input" says which workspace it came from.
        let name = zone.name.borrow();
        let (title, body) = match (title.is_empty(), body.is_empty()) {
            (true, _) => (name.clone(), body.to_string()),
            (false, true) => (format!("{title} on {name}"), String::new()),
            (false, false) => (title.to_string(), format!("{body} on {name}")),
        };
        self.send_zone_notification(zone, &title, &body);
    }

    /// One notification slot per zone: a newer message replaces the stale
    /// one, and selecting the zone (or refocusing the window) withdraws it.
    /// Clicking switches to the zone via the app.focus-zone action.
    fn send_zone_notification(&self, zone: &Zone, title: &str, body: &str) {
        let Some(gtk_app) = self.window.application() else { return };
        let n = gio::Notification::new(title);
        if !body.is_empty() {
            n.set_body(Some(body));
        }
        n.set_default_action_and_target_value("app.focus-zone", Some(&zone.id.to_variant()));
        gtk_app.send_notification(Some(&zone.stack_name()), &n);
    }

    pub fn withdraw_zone_notification(&self, zone: &Zone) {
        if let Some(gtk_app) = self.window.application() {
            gtk_app.withdraw_notification(&zone.stack_name());
        }
    }

    pub fn on_term_exited(self: &Rc<Self>, zone: &Rc<Zone>, terminal: &vte::Terminal) {
        let _ = zone;
        if terminal.root().is_none() {
            return; // already torn down (tab closed, zone removed)
        }
        pane::close_tab_of(terminal);
        self.schedule_save();
    }

    // ----- tab drag-and-drop ------------------------------------------------

    /// Record a pane whose last tab was torn off by a drag (page-detached with
    /// zero pages left and no close in flight).
    pub fn pane_emptied_by_drag(&self, pane: &gtk::Stack, zone: &Weak<Zone>, page: &adw::TabPage) {
        self.drag_emptied.borrow_mut().push(PendingCollapse {
            pane: pane.downgrade(),
            zone: zone.clone(),
            page: page.downgrade(),
        });
    }

    /// Resolve drag-emptied panes. Called on every page-attached; deferred to
    /// an idle so the drop handler has fully unwound before any pane dies.
    pub fn schedule_drag_sweep(self: &Rc<Self>) {
        if self.drag_emptied.borrow().is_empty() {
            return;
        }
        let app = self.clone();
        glib::idle_add_local_once(move || app.sweep_drag_emptied());
    }

    fn sweep_drag_emptied(self: &Rc<Self>) {
        let entries = self.drag_emptied.take();
        for e in entries {
            let Some(pane) = e.pane.upgrade() else { continue };
            if pane.root().is_none() {
                continue; // torn down while the drag was in flight
            }
            let still_empty = pane::tab_view_of(pane.upcast_ref())
                .is_none_or(|v| v.n_pages() == 0);
            if still_empty {
                // Keep the pane alive past the drag's async dnd-finished:
                // adw's tab box only disconnects its GdkDrag handlers in
                // drag_end, which on Wayland can arrive after this idle.
                let keep = pane.clone();
                glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || {
                    drop(keep)
                });
                pane::collapse_empty_pane(self, &pane, &e.zone);
                self.schedule_save();
            }
            // Land selection and focus on the moved (or cancel-restored) tab;
            // adw's attach_page does not select the page it inserts.
            if let Some(page) = e.page.upgrade()
                && let Some(dest) = splits::pane_of(&page.child())
                && let Some(view) = pane::tab_view_of(&dest)
            {
                view.set_selected_page(&page);
                if let Some(t) = splits::first_terminal_in(&page.child()) {
                    term::focus_later(&t);
                }
            }
        }
    }

    // ----- tab / pane actions ---------------------------------------------

    fn new_tab_active(self: &Rc<Self>) -> glib::Propagation {
        let Some(zone) = self.active_zone() else {
            return glib::Propagation::Proceed;
        };
        let Some(p) = self.focused_pane(&zone) else {
            return glib::Propagation::Proceed;
        };
        let cwd = self.focused_terminal().and_then(|t| term::cwd_of(&t));
        pane::new_tab(self, &zone, &p, cwd);
        glib::Propagation::Stop
    }

    fn close_focused_tab(self: &Rc<Self>) -> glib::Propagation {
        let Some(terminal) = self.focused_terminal() else {
            return glib::Propagation::Proceed;
        };
        pane::close_tab_of(&terminal);
        self.schedule_save();
        glib::Propagation::Stop
    }

    fn split_focused(self: &Rc<Self>, orientation: gtk::Orientation) -> glib::Propagation {
        let Some(zone) = self.active_zone() else {
            return glib::Propagation::Proceed;
        };
        let Some(p) = self.focused_pane(&zone) else {
            return glib::Propagation::Proceed;
        };
        let cwd = self.focused_terminal().and_then(|t| term::cwd_of(&t));
        let new_pane = pane::build_pane(self, &Rc::downgrade(&zone));
        pane::new_tab(self, &zone, &new_pane, cwd);
        if let Some(paned) =
            splits::split_leaf(p.upcast_ref(), new_pane.upcast_ref(), orientation)
        {
            self.watch_paned(&paned);
        }
        if let Some(t) = splits::first_terminal_in(new_pane.upcast_ref()) {
            term::focus_later(&t);
        }
        self.schedule_save();
        glib::Propagation::Stop
    }

    fn focus_next_pane(self: &Rc<Self>) -> glib::Propagation {
        let Some(zone) = self.active_zone() else {
            return glib::Propagation::Proceed;
        };
        let mut panes = Vec::new();
        splits::all_panes_in(zone.page.upcast_ref(), &mut panes);
        if panes.is_empty() {
            return glib::Propagation::Stop;
        }
        let current = self
            .focused_terminal()
            .and_then(|t| splits::pane_of(t.upcast_ref()));
        let next_idx = current
            .and_then(|c| panes.iter().position(|p| *p == c))
            .map(|i| (i + 1) % panes.len())
            .unwrap_or(0);
        self.focus_pane(&panes[next_idx]);
        glib::Propagation::Stop
    }

    fn focus_pane_directional(self: &Rc<Self>, dir: splits::Direction) -> glib::Propagation {
        let Some(zone) = self.active_zone() else {
            return glib::Propagation::Proceed;
        };
        let Some(current) = self.focused_pane(&zone) else {
            return glib::Propagation::Stop;
        };
        if let Some(target) =
            splits::neighbor_pane(zone.page.upcast_ref(), current.upcast_ref(), dir)
        {
            self.focus_pane(&target);
        }
        glib::Propagation::Stop
    }

    /// Grow or shrink the focused pane within its immediate split by moving the
    /// enclosing divider one step. No-op when the focused pane isn't split (its
    /// parent is the zone's Bin slot, not a Paned). The move fires
    /// `connect_position_notify`, which refreshes the cached ratio and saves.
    fn resize_focused(self: &Rc<Self>, grow: bool) -> glib::Propagation {
        let Some(zone) = self.active_zone() else {
            return glib::Propagation::Proceed;
        };
        let Some(pane) = self.focused_pane(&zone) else {
            return glib::Propagation::Stop;
        };
        let pane_w: gtk::Widget = pane.upcast();
        let Some(paned) = pane_w.parent().and_downcast::<gtk::Paned>() else {
            return glib::Propagation::Stop; // only pane in the zone — nothing to resize
        };
        let size = splits::paned_size(&paned);
        if size <= 1 {
            return glib::Propagation::Stop;
        }
        let step = ((size as f64 * splits::RESIZE_STEP) as i32).max(1);
        // position is the start child's extent: growing the start child pushes
        // the divider toward the end, growing the end child pulls it back.
        let is_start = paned.start_child().as_ref() == Some(&pane_w);
        let delta = if grow == is_start { step } else { -step };
        paned.set_position((paned.position() + delta).clamp(0, size));
        glib::Propagation::Stop
    }

    /// Focus the selected tab's terminal in `pane`.
    fn focus_pane(&self, pane: &gtk::Widget) {
        let target = pane::tab_view_of(pane)
            .and_then(|view| view.selected_page())
            .and_then(|page| splits::first_terminal_in(&page.child()))
            .or_else(|| splits::first_terminal_in(pane));
        if let Some(t) = target {
            term::focus_later(&t);
        }
    }

    fn cycle_tab(self: &Rc<Self>, next: bool) -> glib::Propagation {
        let Some(zone) = self.active_zone() else {
            return glib::Propagation::Proceed;
        };
        let Some(p) = self.focused_pane(&zone) else {
            return glib::Propagation::Proceed;
        };
        let Some(view) = pane::tab_view_of(p.upcast_ref()) else {
            return glib::Propagation::Proceed;
        };
        if next {
            view.select_next_page();
        } else {
            view.select_previous_page();
        }
        if let Some(page) = view.selected_page()
            && let Some(t) = splits::first_terminal_in(&page.child())
        {
            term::focus_later(&t);
        }
        glib::Propagation::Stop
    }

    // ----- clipboard / fonts ----------------------------------------------

    fn copy(self: &Rc<Self>) -> glib::Propagation {
        match self.focused_terminal() {
            Some(t) => {
                t.copy_clipboard_format(vte::Format::Text);
                glib::Propagation::Stop
            }
            None => glib::Propagation::Proceed,
        }
    }

    fn paste(self: &Rc<Self>) -> glib::Propagation {
        match self.focused_terminal() {
            Some(t) => {
                t.paste_clipboard();
                glib::Propagation::Stop
            }
            None => glib::Propagation::Proceed,
        }
    }

    fn adjust_font_scale(self: &Rc<Self>, delta: Option<f64>) -> glib::Propagation {
        let scale = match delta {
            Some(d) => (self.font_scale.get() + d).clamp(0.5, 3.0),
            None => 1.0,
        };
        self.font_scale.set(scale);
        for t in self.all_terminals() {
            t.set_font_scale(scale);
        }
        glib::Propagation::Stop
    }

    fn all_terminals(&self) -> Vec<vte::Terminal> {
        let mut all = Vec::new();
        for zone in self.zones.borrow().iter() {
            splits::all_terminals_in(zone.page.upcast_ref(), &mut all);
        }
        all
    }

    /// Re-apply non-visual terminal behavior to every live terminal.
    pub fn apply_terminal_settings(self: &Rc<Self>) {
        let cfg = self.config.borrow().clone();
        for t in self.all_terminals() {
            term::apply_settings(&t, &cfg);
        }
    }

    /// Re-read the visual tokens and computed terminal font from style.css
    /// into VTE. GTK applies all other widget rules automatically.
    fn apply_terminal_style(&self) {
        for t in self.all_terminals() {
            term::apply_style(&t);
        }
    }

    /// The "vmux-transparent" class clears the window background so the
    /// terminal's alpha-blended background reaches the compositor.
    #[allow(deprecated)]
    pub fn sync_window_transparency(&self) {
        let transparent = self
            .window
            .style_context()
            .lookup_color(style::TERMINAL_BACKGROUND)
            .is_some_and(|color| color.alpha() < 1.0);
        if transparent {
            self.window.add_css_class("vmux-transparent");
        } else {
            self.window.remove_css_class("vmux-transparent");
        }
    }

    /// Apply the hide-titlebar setting and re-evaluate the sidebar-reveal
    /// button.
    pub fn sync_titlebar(self: &Rc<Self>) {
        let hide = self.config.borrow().hide_titlebar;
        self.titlebar.set_visible(!hide);
        // The sidebar header's own hide button stands in for the titlebar's
        // toggle while the titlebar is gone.
        self.sidebar_hide_btn.set_visible(hide);
        self.update_sidebar_reveal();
    }

    /// With the titlebar and sidebar both hidden there is no chrome left to
    /// bring the sidebar back, so the top-left pane's tab bar shows a
    /// "show sidebar" button.
    pub fn update_sidebar_reveal(self: &Rc<Self>) {
        let needed = self.config.borrow().hide_titlebar && !self.split_view.shows_sidebar();
        for zone in self.zones.borrow().iter() {
            let mut panes = Vec::new();
            splits::all_panes_in(zone.page.upcast_ref(), &mut panes);
            for (i, p) in panes.iter().enumerate() {
                pane::set_sidebar_reveal_visible(p, needed && i == 0);
            }
        }
    }

    /// Persist divider drags (and keep the cached ratio fresh).
    pub fn watch_paned(self: &Rc<Self>, paned: &gtk::Paned) {
        let app = self.clone();
        paned.connect_position_notify(move |p| {
            let size = splits::paned_size(p);
            if size > 1 {
                splits::set_cached_ratio(p, p.position() as f64 / size as f64);
                app.schedule_save();
            }
        });
    }

    // ----- git status -------------------------------------------------------

    /// Keep each zone's secondary line in sync with its directory's git status:
    /// refresh now, on a 3s timer while the window is focused, and whenever the
    /// window regains focus. Gating on focus avoids churn while in the
    /// background; the focus-in handler makes stats fresh the moment you return.
    pub fn start_git_polling(self: &Rc<Self>) {
        self.refresh_all_git();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_secs(3), move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if app.window.is_active() {
                app.refresh_all_git();
            }
            glib::ControlFlow::Continue
        });
        let weak = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |w| {
            if w.is_active()
                && let Some(app) = weak.upgrade()
            {
                app.refresh_all_git();
            }
        });
    }

    fn refresh_all_git(&self) {
        for zone in self.zones.borrow().iter() {
            Self::refresh_zone_git(zone);
        }
    }

    /// Recompute one zone's git summary off the main loop and update its
    /// secondary label, falling back to the directory basename outside a repo.
    fn refresh_zone_git(zone: &Rc<Zone>) {
        let cwd = zone.cwd.clone();
        let path_box = zone.path_box.clone();
        let cache = zone.git_text.clone();
        glib::spawn_future_local(async move {
            let summary = git::run_summary(&cwd).await;
            let dir = state::display_name(&cwd);
            let text = match &summary {
                Some(s) => git::format_summary(s),
                None => dir.clone(),
            };
            if *cache.borrow() == text {
                return;
            }
            cache.replace(text);
            crate::zone::populate_path_box(&path_box, summary.as_ref(), &dir);
        });
    }

    // ----- persistence ------------------------------------------------------

    fn snapshot(&self) -> state::AppState {
        let zones = self.zones.borrow().iter().map(|z| z.snapshot()).collect();
        let active = self
            .listbox
            .selected_row()
            .map(|r| r.index().max(0) as usize)
            .unwrap_or(0);
        state::AppState {
            config: self.config.borrow().clone(),
            zones,
            active_zone: active,
        }
    }

    pub fn save_now(&self) {
        if let Some(id) = self.save_source.borrow_mut().take() {
            id.remove();
        }
        state::save(&self.snapshot());
    }

    pub fn schedule_save(self: &Rc<Self>) {
        let app = self.clone();
        let id = glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
            *app.save_source.borrow_mut() = None;
            state::save(&app.snapshot());
        });
        if let Some(old) = self.save_source.borrow_mut().replace(id) {
            old.remove();
        }
    }
}
