//! The workspace switcher popup: a search entry over a filtered list of
//! zones. Enter (or a click) switches to the highlighted zone; Escape closes
//! and puts focus back on the terminal that had it.

use crate::app::App;
use gtk::gdk;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Candidate {
    /// Position in `App::zones` (and the sidebar) when the popup opened.
    zone_index: usize,
    name: String,
    /// Secondary line: the zone's git summary or directory basename.
    detail: String,
}

/// Case-insensitive substring filter over zone names, preserving sidebar
/// order. An empty query matches everything.
fn filter_indices(query: &str, names: &[String]) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    names
        .iter()
        .enumerate()
        .filter(|(_, name)| q.is_empty() || name.to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

pub fn present_zone_switcher(app: &Rc<App>) {
    let candidates: Rc<Vec<Candidate>> = Rc::new(
        app.zones
            .borrow()
            .iter()
            .enumerate()
            .map(|(i, z)| Candidate {
                zone_index: i,
                name: z.name.borrow().clone(),
                detail: z.git_text.borrow().clone(),
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
    window.set_default_size(420, 420);
    window.set_modal(true);
    window.set_transient_for(Some(&app.window));

    let entry = gtk::SearchEntry::new();
    entry.set_placeholder_text(Some("Workspace name…"));
    entry.set_hexpand(true);

    let list = gtk::ListBox::builder()
        .activate_on_single_click(true)
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("boxed-list");
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

    // Row index -> zone index for whatever the list currently shows.
    let visible: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
    rebuild_list(&list, &candidates, &visible, "");

    let choose = {
        let app = app.clone();
        let window = window.clone();
        let chosen = chosen.clone();
        Rc::new(move |zone_index: usize| {
            chosen.set(true);
            // Close first: the modal hands focus back to the parent window,
            // then select_zone's focus_later lands on the new zone's terminal.
            window.close();
            app.select_zone(zone_index);
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
            if let Some(zone_index) = usize::try_from(idx)
                .ok()
                .and_then(|i| visible.borrow().get(i).copied())
            {
                choose(zone_index);
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
            if let Some(zone_index) = usize::try_from(row.index())
                .ok()
                .and_then(|i| visible.borrow().get(i).copied())
            {
                choose(zone_index);
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
    visible: &Rc<RefCell<Vec<usize>>>,
    query: &str,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let names: Vec<String> = candidates.iter().map(|c| c.name.clone()).collect();
    let matches = filter_indices(query, &names);
    for &i in &matches {
        let c = &candidates[i];
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
        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(10)
            .margin_end(10)
            .build();
        row_box.append(&number);
        row_box.append(&name);
        row_box.append(&detail);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        list.append(&row);
    }
    *visible.borrow_mut() = matches;
    if let Some(row) = list.row_at_index(0) {
        list.select_row(Some(&row));
    }
}

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
    use super::filter_indices;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_query_matches_everything_in_order() {
        assert_eq!(filter_indices("", &names(&["alpha", "beta"])), vec![0, 1]);
        assert_eq!(filter_indices("   ", &names(&["alpha"])), vec![0]);
    }

    #[test]
    fn matches_are_case_insensitive_substrings() {
        let zones = names(&["Vmux", "willshell", "Music"]);
        assert_eq!(filter_indices("mu", &zones), vec![0, 2]);
        assert_eq!(filter_indices("SHELL", &zones), vec![1]);
        assert_eq!(filter_indices("zzz", &zones), Vec::<usize>::new());
    }
}
