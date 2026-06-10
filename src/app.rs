use crate::zone::Zone;
use crate::{keybinds, pane, splits, state, term, window};
use gtk4 as gtk;
use gtk4::glib;
use gtk::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use vte4 as vte;
use vte4::prelude::*;

pub struct App {
    pub window: adw::ApplicationWindow,
    pub split_view: adw::OverlaySplitView,
    pub stack: gtk::Stack,
    pub listbox: gtk::ListBox,
    pub zones: RefCell<Vec<Rc<Zone>>>,
    pub config: RefCell<state::Config>,
    pub font_scale: Cell<f64>,
    save_source: RefCell<Option<glib::SourceId>>,
    next_zone_id: Cell<u64>,
    shortcut_ctl: RefCell<Option<gtk::ShortcutController>>,
}

pub fn build(gtk_app: &adw::Application) {
    let st = state::load();
    let chrome = window::build_chrome(gtk_app);
    let app = Rc::new(App {
        window: chrome.window.clone(),
        split_view: chrome.split_view.clone(),
        stack: chrome.stack.clone(),
        listbox: chrome.listbox.clone(),
        zones: RefCell::new(Vec::new()),
        config: RefCell::new(st.config.clone()),
        font_scale: Cell::new(1.0),
        save_source: RefCell::new(None),
        next_zone_id: Cell::new(1),
        shortcut_ctl: RefCell::new(None),
    });
    window::wire_chrome(&app, &chrome);
    app.reinstall_shortcuts();
    for zs in &st.zones {
        app.append_zone(zs);
    }
    app.select_zone(st.active_zone);
    app.save_now();
    app.window.present();
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
            "next-tab" => self.cycle_tab(true),
            "prev-tab" => self.cycle_tab(false),
            "new-zone" => {
                window::new_zone_dialog(self);
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

    // ----- zones ---------------------------------------------------------

    fn append_zone(self: &Rc<Self>, zs: &state::ZoneState) -> Rc<Zone> {
        let id = self.next_zone_id.get();
        self.next_zone_id.set(id + 1);
        let zone = Zone::build(self, zs, id);
        self.stack.add_named(&zone.page, Some(&zone.stack_name()));
        self.listbox.append(&zone.row);
        self.zones.borrow_mut().push(zone.clone());
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
        self.zones.borrow_mut().swap(cur as usize, new as usize);
        self.listbox.remove(&row);
        self.listbox.insert(&row, new);
        self.listbox.select_row(Some(&row));
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
        self.stack.set_visible_child_name(&zone.stack_name());
        zone.attention.set_visible(false);
        let target = zone
            .last_focused
            .upgrade()
            .or_else(|| splits::first_terminal_in(zone.page.upcast_ref()));
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

    /// Re-apply font/scrollback config to every live terminal.
    pub fn apply_terminal_config(self: &Rc<Self>) {
        let cfg = self.config.borrow().clone();
        for t in self.all_terminals() {
            term::apply_config(&t, &cfg);
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
