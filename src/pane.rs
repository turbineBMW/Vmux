use crate::app::App;
use crate::zone::Zone;
use crate::{keybinds, splits, state, term};
use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use std::rc::{Rc, Weak};
use vte4 as vte;
use vte4::prelude::*;

/// A pane is the unit splits operate on: a tab bar plus its own tabs.
/// Widget shape:  Stack[.vmux-pane] { "tabs": Box[TabBar, TabView], "empty": StatusPage }
pub fn build_pane(app: &Rc<App>, zone: &Weak<Zone>) -> gtk::Stack {
    let tab_view = adw::TabView::new();
    tab_view.set_hexpand(true);
    tab_view.set_vexpand(true);
    let tab_bar = adw::TabBar::new();
    tab_bar.set_view(Some(&tab_view));
    tab_bar.set_autohide(false);

    let new_tab_btn = gtk::Button::from_icon_name("tab-new-symbolic");
    new_tab_btn.add_css_class("flat");
    new_tab_btn.set_tooltip_text(Some("New tab in this pane"));
    tab_bar.set_end_action_widget(Some(&new_tab_btn));

    // Escape hatch shown by App::update_sidebar_reveal when the titlebar and
    // sidebar are both hidden (only on the top-left pane).
    let reveal_btn = gtk::Button::from_icon_name("sidebar-show-symbolic");
    reveal_btn.add_css_class("flat");
    reveal_btn.set_tooltip_text(Some("Show sidebar"));
    reveal_btn.set_visible(false);
    tab_bar.set_start_action_widget(Some(&reveal_btn));
    {
        let app = app.clone();
        reveal_btn.connect_clicked(move |_| app.split_view.set_show_sidebar(true));
    }

    let tabs_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tabs_box.append(&tab_bar);
    tabs_box.append(&tab_view);

    let empty = adw::StatusPage::builder()
        .title("No tabs")
        .description(format!(
            "Press {} to open a terminal",
            keybinds::pretty_accel(&keybinds::accel_for(&app.config.borrow(), "new-tab"))
        ))
        .icon_name("utilities-terminal-symbolic")
        .build();

    let stack = gtk::Stack::new();
    stack.add_named(&tabs_box, Some("tabs"));
    stack.add_named(&empty, Some("empty"));
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_css_class(splits::PANE_CLASS);

    {
        let app = app.clone();
        let zone = zone.clone();
        let stack = stack.clone();
        new_tab_btn.connect_clicked(move |_| {
            if let Some(z) = zone.upgrade() {
                let cwd = z
                    .last_focused
                    .upgrade()
                    .filter(|t| splits::pane_of(t.upcast_ref()).as_ref() == Some(stack.upcast_ref()))
                    .and_then(|t| term::cwd_of(&t));
                new_tab(&app, &z, &stack, cwd);
            }
        });
    }
    {
        let app = app.clone();
        let stack = stack.clone();
        tab_view.connect_page_attached(move |_, _, _| {
            stack.set_visible_child_name("tabs");
            app.schedule_save();
        });
    }
    {
        let app = app.clone();
        let stack = stack.clone();
        let zone = zone.clone();
        tab_view.connect_page_detached(move |view, _, _| {
            if view.root().is_none() {
                return; // pane already being torn down
            }
            if view.n_pages() == 0 {
                let pane: gtk::Widget = stack.clone().upcast();
                let in_split = pane
                    .parent()
                    .map(|p| p.is::<gtk::Paned>())
                    .unwrap_or(false);
                if in_split {
                    if let Some(promoted) = splits::collapse_leaf(&pane)
                        && let Some(t) = splits::first_terminal_in(&promoted)
                    {
                        term::focus_later(&t);
                    }
                    // The collapse may have promoted a new top-left pane.
                    app.update_sidebar_reveal();
                } else if let Some(z) = zone.upgrade() {
                    // Last tab of the zone's only pane: drop the zone itself
                    // (remove_zone focuses the next one, or recreates "main").
                    app.remove_zone(&z);
                    return;
                } else {
                    stack.set_visible_child_name("empty");
                }
            } else if let Some(page) = view.selected_page()
                && let Some(t) = splits::first_terminal_in(&page.child())
            {
                term::focus_later(&t);
            }
            app.schedule_save();
        });
    }
    {
        let app = app.clone();
        tab_view.connect_page_reordered(move |_, _, _| app.schedule_save());
    }

    stack
}

pub fn tab_view_of(pane: &gtk::Widget) -> Option<adw::TabView> {
    let stack = pane.downcast_ref::<gtk::Stack>()?;
    let tabs_box = stack.child_by_name("tabs")?;
    let mut child = tabs_box.first_child();
    while let Some(c) = child {
        if let Ok(view) = c.clone().downcast::<adw::TabView>() {
            return Some(view);
        }
        child = c.next_sibling();
    }
    None
}

fn tab_bar_of(pane: &gtk::Widget) -> Option<adw::TabBar> {
    let stack = pane.downcast_ref::<gtk::Stack>()?;
    let tabs_box = stack.child_by_name("tabs")?;
    let mut child = tabs_box.first_child();
    while let Some(c) = child {
        if let Ok(bar) = c.clone().downcast::<adw::TabBar>() {
            return Some(bar);
        }
        child = c.next_sibling();
    }
    None
}

/// Show/hide this pane's "show sidebar" button (the tab bar's start action
/// widget).
pub fn set_sidebar_reveal_visible(pane: &gtk::Widget, visible: bool) {
    if let Some(bar) = tab_bar_of(pane)
        && let Some(btn) = bar.start_action_widget()
    {
        btn.set_visible(visible);
    }
}

pub fn new_tab(app: &Rc<App>, zone: &Rc<Zone>, pane: &gtk::Stack, cwd: Option<String>) {
    let Some(view) = tab_view_of(pane.upcast_ref()) else {
        return;
    };
    let cwd = cwd
        .filter(|c| std::path::Path::new(c).is_dir())
        .unwrap_or_else(|| zone.cwd.clone());
    let leaf = term::build_leaf(app, &Rc::downgrade(zone), cwd.clone());
    let page = view.append(&leaf);
    page.set_keyword(&cwd);
    page.set_title(&state::display_name(&cwd));

    if let Some(t) = splits::first_terminal_in(leaf.upcast_ref()) {
        // vte 0.78 deprecates the title/cwd accessors in favor of termprops;
        // the old signals still work, so migrating them is its own change.
        #[allow(deprecated)]
        {
            let pw = page.downgrade();
            t.connect_window_title_changed(move |term| {
                if let Some(page) = pw.upgrade() {
                    refresh_title(term, &page);
                }
            });
        }
        #[allow(deprecated)]
        {
            let pw = page.downgrade();
            let app = app.clone();
            t.connect_current_directory_uri_changed(move |term| {
                if let Some(page) = pw.upgrade() {
                    if let Some(cwd) = term::cwd_of(term) {
                        page.set_keyword(&cwd);
                    }
                    refresh_title(term, &page);
                }
                app.schedule_save();
            });
        }
        {
            let pw = page.downgrade();
            t.connect_termprop_changed(
                Some(vmux::osc_scan::FGPROC_TERMPROP_NAME),
                move |term, _name| {
                    if let Some(page) = pw.upgrade() {
                        refresh_title(term, &page);
                    }
                },
            );
        }
        term::focus_later(&t);
    }
    view.set_selected_page(&page);
    app.schedule_save();
}

/// Close the tab that contains `terminal` (the pane collapses automatically
/// when its last tab goes, via page-detached).
pub fn close_tab_of(terminal: &vte::Terminal) {
    let Some(leaf) = terminal.parent() else { return };
    let Some(pane) = splits::pane_of(&leaf) else { return };
    let Some(view) = tab_view_of(&pane) else { return };
    for i in 0..view.n_pages() {
        let page = view.nth_page(i);
        if page.child() == leaf {
            view.close_page(&page);
            return;
        }
    }
}

#[allow(deprecated)] // window_title: see the connect_* note in new_tab
fn refresh_title(terminal: &vte::Terminal, page: &adw::TabPage) {
    let title = terminal
        .window_title()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .or_else(|| fg_command(terminal))
        .or_else(|| term::cwd_of(terminal).map(|c| state::display_name(&c)))
        .or_else(|| {
            // Shells without OSC 7 (e.g. bash) report no live cwd, so once a
            // finished command's name clears, fall back to the cwd vmux cached
            // on the page — otherwise the title would stay stuck on the
            // command instead of reverting to the directory.
            page.keyword()
                .filter(|k| !k.is_empty())
                .map(|k| state::display_name(&k))
        })
        .unwrap_or_else(|| page.title().to_string());
    page.set_title(&title);
}

/// The foreground command vmux-relay last reported via the fgproc termprop,
/// or None when unset/empty (the shell itself is in front).
fn fg_command(terminal: &vte::Terminal) -> Option<String> {
    let data = terminal.termprop_data(vmux::osc_scan::FGPROC_TERMPROP_NAME);
    let name = String::from_utf8_lossy(&data);
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}
