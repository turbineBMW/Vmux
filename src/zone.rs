use crate::app::App;
use crate::{git, pane, splits, state};
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use vte4 as vte;

pub struct Zone {
    pub id: u64,
    pub name: RefCell<String>,
    pub cwd: String,
    /// Zone root slot; its child is the pane/split tree.
    pub page: adw::Bin,
    pub last_focused: glib::WeakRef<vte::Terminal>,
    pub row: gtk::ListBoxRow,
    pub name_label: gtk::Label,
    /// Secondary line: a directory basename label, or one colored label per
    /// git-status token once refreshed. See [`populate_path_box`].
    pub path_box: gtk::Box,
    /// Plain-text form of what `path_box` currently shows, so a refresh that
    /// computes the same string can skip rebuilding the labels.
    pub git_text: Rc<RefCell<String>>,
    pub attention: gtk::Image,
}

impl Zone {
    pub fn stack_name(&self) -> String {
        format!("zone-{}", self.id)
    }

    pub fn build(app: &Rc<App>, zs: &state::ZoneState, id: u64) -> Rc<Zone> {
        let page = adw::Bin::new();
        page.set_hexpand(true);
        page.set_vexpand(true);

        // Sidebar row.
        let name_label = gtk::Label::new(Some(&zs.name));
        name_label.set_xalign(0.0);
        let dir = state::display_name(&zs.cwd);
        // Token separators are baked into each label's text, so spacing is 0.
        let path_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        path_box.add_css_class("zone-path");
        populate_path_box(&path_box, None, &dir);
        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text_box.set_hexpand(true);
        text_box.append(&name_label);
        text_box.append(&path_box);
        let attention = gtk::Image::from_icon_name("media-record-symbolic");
        attention.add_css_class("attention-dot");
        attention.set_visible(false);
        let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row_box.append(&text_box);
        row_box.append(&attention);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        row.add_css_class("zone-row");

        let zone = Rc::new(Zone {
            id,
            name: RefCell::new(zs.name.clone()),
            cwd: zs.cwd.clone(),
            page: page.clone(),
            last_focused: glib::WeakRef::new(),
            row,
            name_label,
            path_box,
            git_text: Rc::new(RefCell::new(dir)),
            attention,
        });

        {
            let app = app.clone();
            let zw = Rc::downgrade(&zone);
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3);
            gesture.connect_pressed(move |_, _, x, y| {
                if let Some(z) = zw.upgrade() {
                    crate::window::show_zone_menu(&app, &z, x, y);
                }
            });
            zone.row.add_controller(gesture);
        }

        let root = restore_node(app, &zone, &zs.effective_root());
        page.set_child(Some(&root));
        zone
    }

    pub fn snapshot(&self) -> state::ZoneState {
        let root = self
            .page
            .child()
            .map(|c| snapshot_node(&c, &self.cwd))
            .unwrap_or(state::NodeState::Pane {
                tabs: Vec::new(),
                active_tab: 0,
            });
        state::ZoneState {
            name: self.name.borrow().clone(),
            cwd: self.cwd.clone(),
            root: Some(root),
            tabs: Vec::new(),
        }
    }
}

/// Rebuild the secondary line's contents, replacing any existing children.
/// Inside a repo (`Some`), one label per [`git::Segment`] carries that token's
/// color class; outside one (`None`), a single dimmed label shows `dir`.
pub fn populate_path_box(path_box: &gtk::Box, summary: Option<&git::GitSummary>, dir: &str) {
    while let Some(child) = path_box.first_child() {
        path_box.remove(&child);
    }
    match summary {
        Some(summary) => {
            for seg in git::summary_segments(summary) {
                let label = gtk::Label::new(Some(&seg.text));
                label.set_xalign(0.0);
                label.add_css_class(seg.class);
                path_box.append(&label);
            }
        }
        None => {
            let label = gtk::Label::new(Some(dir));
            label.set_xalign(0.0);
            label.add_css_class("zone-path-dir");
            path_box.append(&label);
        }
    }
}

fn restore_node(app: &Rc<App>, zone: &Rc<Zone>, node: &state::NodeState) -> gtk::Widget {
    match node {
        state::NodeState::Pane { tabs, active_tab } => {
            let p = pane::build_pane(app, &Rc::downgrade(zone));
            for tab in tabs {
                pane::new_tab(app, zone, &p, Some(tab.cwd.clone()));
            }
            if tabs.is_empty() {
                p.set_visible_child_name("empty");
            } else if let Some(view) = pane::tab_view_of(p.upcast_ref()) {
                let active = (*active_tab).min(tabs.len().saturating_sub(1)) as i32;
                if active < view.n_pages() {
                    view.set_selected_page(&view.nth_page(active));
                }
            }
            p.upcast()
        }
        state::NodeState::Split {
            orientation,
            ratio,
            first,
            second,
        } => {
            let orient = if orientation == "vertical" {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            };
            let paned = splits::new_split_paned(orient, ratio.clamp(0.05, 0.95));
            let a = restore_node(app, zone, first);
            let b = restore_node(app, zone, second);
            paned.set_start_child(Some(&a));
            paned.set_end_child(Some(&b));
            app.watch_paned(&paned);
            paned.upcast()
        }
    }
}

fn snapshot_node(w: &gtk::Widget, fallback_cwd: &str) -> state::NodeState {
    if let Some(paned) = w.downcast_ref::<gtk::Paned>() {
        let first = paned.start_child().map(|c| snapshot_node(&c, fallback_cwd));
        let second = paned.end_child().map(|c| snapshot_node(&c, fallback_cwd));
        return match (first, second) {
            (Some(first), Some(second)) => state::NodeState::Split {
                orientation: if paned.orientation() == gtk::Orientation::Vertical {
                    "vertical".into()
                } else {
                    "horizontal".into()
                },
                ratio: splits::cached_ratio(paned),
                first: Box::new(first),
                second: Box::new(second),
            },
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => state::NodeState::Pane {
                tabs: Vec::new(),
                active_tab: 0,
            },
        };
    }
    if w.has_css_class(splits::PANE_CLASS)
        && let Some(view) = pane::tab_view_of(w)
    {
        let mut tabs = Vec::new();
        for i in 0..view.n_pages() {
            let page = view.nth_page(i);
            let cwd = splits::first_terminal_in(&page.child())
                .and_then(|t| crate::term::cwd_of(&t))
                .or_else(|| {
                    page.keyword()
                        .filter(|k| !k.is_empty())
                        .map(|k| k.to_string())
                })
                .unwrap_or_else(|| fallback_cwd.to_string());
            tabs.push(state::TabState { cwd });
        }
        let active_tab = view
            .selected_page()
            .map(|p| view.page_position(&p).max(0) as usize)
            .unwrap_or(0);
        return state::NodeState::Pane { tabs, active_tab };
    }
    state::NodeState::Pane {
        tabs: Vec::new(),
        active_tab: 0,
    }
}
