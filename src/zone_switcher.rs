//! The workspace switcher popup: a search entry over a filtered list of
//! zones, each followed by a line per coding agent running in it. Enter (or
//! a click) switches to the highlighted zone, or straight to the highlighted
//! agent's tab; Escape closes and puts focus back on the terminal that had it.

use crate::agent::Status;
use crate::app::App;
use gtk::gdk;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use vte4 as vte;

struct Candidate {
    /// Position in `App::zones` (and the sidebar) when the popup opened —
    /// the number shown on the row.
    zone_index: usize,
    /// Resolved back to a position when chosen: an agent starting up can
    /// re-sort the sidebar while the popup is open.
    zone_id: u64,
    name: String,
    /// Secondary line: the zone's git summary or directory basename.
    detail: String,
    avatar: Option<std::path::PathBuf>,
    agents: Vec<AgentLine>,
}

/// A coding agent in a candidate zone, as it was when the popup opened.
struct AgentLine {
    term: glib::WeakRef<vte::Terminal>,
    agent: &'static str,
    status: Status,
    /// What it says it's working on; empty when it set no title.
    task: String,
}

/// A row of the list: a zone, or one of its agents (zone, agent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Zone(usize),
    Agent(usize, usize),
}

/// Case-insensitive substring filter over zone names and over each agent's
/// name and task, preserving sidebar order. A zone whose name matches shows
/// all its agents; one matched only through agents shows just those, under
/// it. An empty query matches everything. Also returns the row to highlight:
/// the first that matched in its own right, so a search for a task lands on
/// that agent.
fn filter_items(query: &str, zones: &[(String, Vec<String>)]) -> (Vec<Item>, usize) {
    let q = query.trim().to_lowercase();
    let hit = |text: &str| q.is_empty() || text.to_lowercase().contains(&q);
    let mut items = Vec::new();
    let mut first = None;
    for (z, (name, agents)) in zones.iter().enumerate() {
        if hit(name) {
            first.get_or_insert(items.len());
            items.push(Item::Zone(z));
            items.extend((0..agents.len()).map(|a| Item::Agent(z, a)));
            continue;
        }
        let matched: Vec<usize> = (0..agents.len()).filter(|&a| hit(&agents[a])).collect();
        if !matched.is_empty() {
            items.push(Item::Zone(z));
            first.get_or_insert(items.len());
            items.extend(matched.into_iter().map(|a| Item::Agent(z, a)));
        }
    }
    (items, first.unwrap_or(0))
}

pub fn present_zone_switcher(app: &Rc<App>) {
    let candidates: Rc<Vec<Candidate>> = Rc::new(
        app.zones
            .borrow()
            .iter()
            .enumerate()
            .map(|(i, z)| Candidate {
                zone_index: i,
                zone_id: z.id,
                name: z.name.borrow().clone(),
                detail: z.git_text.borrow().clone(),
                avatar: z.avatar_path.borrow().clone(),
                agents: crate::agent::entries_in(z)
                    .into_iter()
                    .map(|e| AgentLine {
                        task: crate::agent_panel::task_of(&e.term),
                        term: e.term.downgrade(),
                        agent: e.agent,
                        status: e.status,
                    })
                    .collect(),
            })
            .collect(),
    );
    // The terminal to give focus back to when the popup is dismissed.
    let prev_focus = app
        .focused_terminal()
        .map(|t| t.downgrade())
        .unwrap_or_default();
    let chosen = Rc::new(Cell::new(false));

    let window = adw::Window::new();
    window.set_title(Some("Switch Workspace"));
    window.set_default_size(480, 480);
    window.set_modal(true);
    window.set_transient_for(Some(&app.window));

    let entry = gtk::SearchEntry::new();
    entry.set_placeholder_text(Some("Workspace or agent…"));
    entry.set_hexpand(true);

    let list = gtk::ListBox::builder()
        .activate_on_single_click(true)
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("navigation-sidebar");
    list.add_css_class("zone-switcher");
    let scroll = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    content.append(&entry);
    content.append(&scroll);
    window.set_content(Some(&content));

    // Row index -> item for whatever the list currently shows.
    let visible: Rc<RefCell<Vec<Item>>> = Rc::new(RefCell::new(Vec::new()));
    rebuild_list(&list, &candidates, &visible, "");

    let choose = {
        let app = app.clone();
        let window = window.clone();
        let chosen = chosen.clone();
        let candidates = candidates.clone();
        Rc::new(move |item: Item| {
            chosen.set(true);
            let (z, term) = match item {
                Item::Zone(z) => (z, None),
                Item::Agent(z, a) => (z, candidates[z].agents[a].term.upgrade()),
            };
            // Close first: the modal hands focus back to the parent window,
            // then select_zone's focus_later lands on the new zone's terminal.
            window.close();
            // An agent's tab may have closed since the popup opened; its zone
            // is the next best thing.
            if let Some(term) = term {
                app.reveal_terminal(&term);
                return;
            }
            let zone_id = candidates[z].zone_id;
            let idx = app.zones.borrow().iter().position(|z| z.id == zone_id);
            if let Some(idx) = idx {
                app.select_zone(idx);
            }
        })
    };

    {
        let candidates = candidates.clone();
        let list = list.clone();
        let visible = visible.clone();
        entry.connect_search_changed(move |entry| {
            rebuild_list(&list, &candidates, &visible, entry.text().as_str());
        });
    }
    {
        let list = list.clone();
        let visible = visible.clone();
        let choose = choose.clone();
        entry.connect_activate(move |_| {
            let idx = list.selected_row().map(|r| r.index()).unwrap_or(-1);
            if let Some(item) = usize::try_from(idx)
                .ok()
                .and_then(|i| visible.borrow().get(i).copied())
            {
                choose(item);
            }
        });
    }
    {
        // Escape inside the entry.
        let window = window.clone();
        entry.connect_stop_search(move |_| window.close());
    }
    {
        // Arrow keys move the list highlight without leaving the entry.
        let list = list.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, modifier| {
            if modifier.intersects(
                gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::META_MASK
                    | gdk::ModifierType::SUPER_MASK,
            ) {
                return glib::Propagation::Proceed;
            }
            match keyval {
                gdk::Key::Up => {
                    move_selection(&list, -1);
                    glib::Propagation::Stop
                }
                gdk::Key::Down => {
                    move_selection(&list, 1);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        entry.add_controller(key);
    }
    {
        let visible = visible.clone();
        let choose = choose.clone();
        list.connect_row_activated(move |_, row| {
            if let Some(item) = usize::try_from(row.index())
                .ok()
                .and_then(|i| visible.borrow().get(i).copied())
            {
                choose(item);
            }
        });
    }
    {
        // Escape anywhere else in the popup.
        let window_for_key = window.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gdk::Key::Escape {
                window_for_key.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(key);
    }
    {
        // Dismissed without picking a zone (Escape, or a WM close): put focus
        // back where it was.
        let chosen = chosen.clone();
        window.connect_close_request(move |_| {
            if !chosen.get()
                && let Some(t) = prev_focus.upgrade()
            {
                crate::term::focus_later(&t);
            }
            glib::Propagation::Proceed
        });
    }

    window.present();
    entry.grab_focus();
}

fn rebuild_list(
    list: &gtk::ListBox,
    candidates: &[Candidate],
    visible: &Rc<RefCell<Vec<Item>>>,
    query: &str,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let haystack: Vec<(String, Vec<String>)> = candidates
        .iter()
        .map(|c| {
            let agents = c
                .agents
                .iter()
                .map(|a| format!("{} {}", a.agent, a.task))
                .collect();
            (c.name.clone(), agents)
        })
        .collect();
    let (items, highlight) = filter_items(query, &haystack);
    for &item in &items {
        let row = match item {
            Item::Zone(z) => zone_row(&candidates[z]),
            Item::Agent(z, a) => agent_row(&candidates[z].agents[a]),
        };
        list.append(&row);
    }
    *visible.borrow_mut() = items;
    if let Some(row) = list.row_at_index(highlight as i32) {
        list.select_row(Some(&row));
    }
}

fn zone_row(c: &Candidate) -> gtk::ListBoxRow {
    let avatar = adw::Avatar::new(AVATAR_SIZE, Some(&c.name), true);
    avatar.add_css_class("zone-avatar");
    avatar.set_valign(gtk::Align::Center);
    crate::zone::apply_avatar(&avatar, c.avatar.as_deref());
    let number = gtk::Label::new(Some(&(c.zone_index + 1).to_string()));
    number.add_css_class("zone-number");
    number.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(Some(&c.name));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let detail = gtk::Label::new(Some(&c.detail));
    detail.add_css_class("dim-label");
    detail.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, ROW_SPACING);
    row_box.append(&avatar);
    row_box.append(&number);
    row_box.append(&name);
    row_box.append(&detail);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row.add_css_class("zone-switcher-zone");
    row
}

fn agent_row(a: &AgentLine) -> gtk::ListBoxRow {
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("agent-dot");
    dot.set_valign(gtk::Align::Center);
    crate::agent::style_dot(&dot, a.status);
    let agent = gtk::Label::new(Some(a.agent));
    agent.add_css_class("agent-name");
    let task = gtk::Label::new(Some(&a.task));
    task.add_css_class("agent-task");
    task.set_xalign(0.0);
    task.set_hexpand(true);
    task.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    // Under the zone's name, past its picture.
    row_box.set_margin_start(AVATAR_SIZE + ROW_SPACING);
    row_box.append(&dot);
    row_box.append(&agent);
    row_box.append(&task);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row.add_css_class("zone-switcher-agent");
    row.set_tooltip_text(Some(&crate::agent::describe(a.agent, a.status)));
    row
}

const AVATAR_SIZE: i32 = 28;
const ROW_SPACING: i32 = 10;

/// Move the highlight up/down with wrap-around, keeping it in view.
fn move_selection(list: &gtk::ListBox, delta: i32) {
    let mut count = 0;
    while list.row_at_index(count).is_some() {
        count += 1;
    }
    if count == 0 {
        return;
    }
    let cur = list.selected_row().map(|r| r.index()).unwrap_or(-1);
    let next = if cur < 0 {
        if delta > 0 { 0 } else { count - 1 }
    } else {
        (cur + delta).rem_euclid(count)
    };
    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
        crate::window::scroll_row_into_view(list, &row);
    }
}

#[cfg(test)]
mod tests {
    use super::{Item, filter_items};

    fn zones(list: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        list.iter()
            .map(|(name, agents)| {
                (
                    name.to_string(),
                    agents.iter().map(|a| a.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn empty_query_matches_everything_in_order() {
        let z = zones(&[("alpha", &["Claude fix"]), ("beta", &[])]);
        assert_eq!(
            filter_items("", &z),
            (vec![Item::Zone(0), Item::Agent(0, 0), Item::Zone(1)], 0)
        );
        assert_eq!(filter_items("   ", &z).0.len(), 3);
    }

    #[test]
    fn zone_names_match_case_insensitive_substrings() {
        let z = zones(&[("Vmux", &[]), ("willshell", &[]), ("Music", &[])]);
        assert_eq!(
            filter_items("mu", &z),
            (vec![Item::Zone(0), Item::Zone(2)], 0)
        );
        assert_eq!(filter_items("SHELL", &z), (vec![Item::Zone(1)], 0));
        assert_eq!(filter_items("zzz", &z), (vec![], 0));
    }

    #[test]
    fn a_matching_zone_brings_all_its_agents() {
        let z = zones(&[("Bubo", &["Claude gif search", "Codex tests"])]);
        assert_eq!(
            filter_items("bubo", &z).0,
            vec![Item::Zone(0), Item::Agent(0, 0), Item::Agent(0, 1)]
        );
    }

    #[test]
    fn an_agent_match_shows_its_zone_and_highlights_the_agent() {
        let z = zones(&[
            ("Skali", &["Claude speech bubbles"]),
            ("Bubo", &["Codex tests", "Claude GIF search"]),
        ]);
        assert_eq!(
            filter_items("gif", &z),
            (vec![Item::Zone(1), Item::Agent(1, 1)], 1)
        );
    }
}
