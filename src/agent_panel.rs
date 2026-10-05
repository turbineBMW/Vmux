//! The sidebar's Agents section: every terminal with a coding agent in
//! front, from every zone, one row each. A row shows the agent's state dot,
//! its name and zone, and what it is working on (its window title);
//! activating it switches to that zone, selects that tab and focuses the
//! terminal.
//!
//! The section sits under the zone list at the height of its rows (up to
//! `MAX_LIST_HEIGHT`, then it scrolls), hides itself while no agent is
//! running, and folds down to its header on a click.
//! Rows follow [`crate::agent`]'s commits through [`AgentPanel::schedule_refresh`],
//! coalesced to one rebuild per main-loop pass and done in place when the
//! set and order of terminals is unchanged — there's no polling.

use crate::agent::{self, Entry};
use crate::app::App;
use crate::state::AgentOrder;
use crate::zone::Zone;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use vte4 as vte;
use vte4::prelude::*;

/// The tallest the list grows before it scrolls, in pixels: about six rows.
const MAX_LIST_HEIGHT: i32 = 320;

pub struct AgentPanel {
    /// The whole section: header plus list. Hidden while there are no agents.
    pub root: gtk::Box,
    header: gtk::Button,
    arrow: gtk::Image,
    count: gtk::Label,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    rows: RefCell<Vec<Row>>,
    pending: Cell<bool>,
}

struct Row {
    term: glib::WeakRef<vte::Terminal>,
    dot: gtk::Box,
    agent: gtk::Label,
    zone: gtk::Label,
    task: gtk::Label,
    row: gtk::ListBoxRow,
}

impl AgentPanel {
    pub fn new() -> Rc<Self> {
        let arrow = gtk::Image::from_icon_name("pan-down-symbolic");
        let title = gtk::Label::new(Some("Agents"));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        let count = gtk::Label::new(None);
        count.add_css_class("agent-count");
        let header_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header_box.append(&arrow);
        header_box.append(&title);
        header_box.append(&count);
        let header = gtk::Button::new();
        header.set_child(Some(&header_box));
        header.add_css_class("flat");
        header.add_css_class("agent-panel-header");

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("navigation-sidebar");
        list.add_css_class("agent-list");
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(MAX_LIST_HEIGHT)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("agent-panel");
        root.append(&header);
        root.append(&scroll);
        root.set_visible(false);

        Rc::new(Self {
            root,
            header,
            arrow,
            count,
            list,
            scroll,
            rows: RefCell::new(Vec::new()),
            pending: Cell::new(false),
        })
    }

    /// Connect the header and rows to the app. Called once, after `App` exists.
    pub fn wire(self: &Rc<Self>, app: &Rc<App>) {
        {
            let app = app.clone();
            self.header.connect_clicked(move |_| {
                let collapsed = !app.config.borrow().agent_panel_collapsed;
                app.config.borrow_mut().agent_panel_collapsed = collapsed;
                app.schedule_save();
                app.agent_panel.refresh(&app);
            });
        }
        {
            let app = app.clone();
            let me = Rc::downgrade(self);
            self.list.connect_row_activated(move |_, row| {
                let Some(me) = me.upgrade() else { return };
                let term = me
                    .rows
                    .borrow()
                    .get(row.index().max(0) as usize)
                    .and_then(|r| r.term.upgrade());
                if let Some(term) = term {
                    app.reveal_terminal(&term);
                }
            });
        }
    }

    /// Refresh once the current main-loop pass is done, however many agents
    /// changed in it.
    pub fn schedule_refresh(self: &Rc<Self>, app: &Rc<App>) {
        if self.pending.replace(true) {
            return;
        }
        let app = Rc::downgrade(app);
        glib::idle_add_local_once(move || {
            if let Some(app) = app.upgrade() {
                app.agent_panel.pending.set(false);
                app.agent_panel.refresh(&app);
            }
        });
    }

    pub fn refresh(&self, app: &Rc<App>) {
        let (show, order, collapsed) = {
            let cfg = app.config.borrow();
            (
                cfg.agent_panel,
                cfg.agent_panel_order,
                cfg.agent_panel_collapsed,
            )
        };
        let mut items = entries(app);
        if order == AgentOrder::Attention {
            agent::sort_by_attention(&mut items, |(_, e)| (e.status, e.changed));
        }

        self.root.set_visible(show && !items.is_empty());
        self.count.set_label(&items.len().to_string());
        self.scroll.set_visible(!collapsed);
        self.arrow.set_icon_name(Some(if collapsed {
            "pan-end-symbolic"
        } else {
            "pan-down-symbolic"
        }));
        self.header.set_tooltip_text(Some(if collapsed {
            "Show agents"
        } else {
            "Hide agents"
        }));

        let same = {
            let rows = self.rows.borrow();
            rows.len() == items.len()
                && rows
                    .iter()
                    .zip(&items)
                    .all(|(r, (_, e))| r.term.upgrade().as_ref() == Some(&e.term))
        };
        if !same {
            self.rebuild(&items);
        }
        for (row, (zone, e)) in self.rows.borrow().iter().zip(&items) {
            agent::style_dot(&row.dot, e.status);
            row.agent.set_label(e.agent);
            row.zone.set_label(&zone.name.borrow());
            row.task.set_label(&task_of(&e.term));
            row.row
                .set_tooltip_text(Some(&agent::describe(e.agent, e.status)));
        }
    }

    fn rebuild(&self, items: &[(Rc<Zone>, Entry)]) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut rows = self.rows.borrow_mut();
        rows.clear();
        for (_, e) in items {
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("agent-dot");
            dot.set_valign(gtk::Align::Center);
            let agent = gtk::Label::new(None);
            agent.add_css_class("agent-name");
            let zone = gtk::Label::new(None);
            zone.add_css_class("agent-zone");
            zone.set_xalign(0.0);
            zone.set_hexpand(true);
            zone.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            top.append(&agent);
            top.append(&zone);
            let task = gtk::Label::new(None);
            task.add_css_class("agent-task");
            task.set_xalign(0.0);
            task.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_hexpand(true);
            text.append(&top);
            text.append(&task);
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row_box.append(&dot);
            row_box.append(&text);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&row_box));
            row.add_css_class("agent-row");
            self.list.append(&row);
            rows.push(Row {
                term: e.term.downgrade(),
                dot,
                agent,
                zone,
                task,
                row,
            });
        }
    }

    /// Unfold the section and put keyboard focus on its first row.
    pub fn focus(&self, app: &Rc<App>) {
        if app.config.borrow().agent_panel_collapsed {
            app.config.borrow_mut().agent_panel_collapsed = false;
            app.schedule_save();
            self.refresh(app);
        }
        if let Some(row) = self.list.row_at_index(0) {
            row.grab_focus();
        }
    }
}

/// Every agent in every zone, with its zone, in sidebar order.
pub fn entries(app: &App) -> Vec<(Rc<Zone>, Entry)> {
    app.zones
        .borrow()
        .iter()
        .flat_map(|z| {
            agent::entries_in(z)
                .into_iter()
                .map(move |e| (z.clone(), e))
        })
        .collect()
}

/// The row's second line: what the agent says it's working on.
fn task_of(term: &vte::Terminal) -> String {
    #[allow(deprecated)] // window_title: see pane::new_tab
    let title = term
        .window_title()
        .map(|t| t.to_string())
        .unwrap_or_default();
    vmux::agents::task_title(&title).to_string()
}
