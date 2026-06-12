use crate::app::App;
use crate::state;
use crate::zone::Zone;
use gtk4 as gtk;
use gtk4::{gio, glib};
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

pub struct Chrome {
    pub window: adw::ApplicationWindow,
    pub split_view: adw::OverlaySplitView,
    pub stack: gtk::Stack,
    pub listbox: gtk::ListBox,
    pub titlebar: adw::HeaderBar,
    pub sidebar_hide_btn: gtk::Button,
    pub new_zone_btn: gtk::Button,
    pub settings_btn: gtk::Button,
}

pub fn build_chrome(gtk_app: &adw::Application) -> Chrome {
    let window = adw::ApplicationWindow::new(gtk_app);
    window.set_title(Some("Vmux"));
    window.set_default_size(1280, 820);

    let listbox = gtk::ListBox::new();
    listbox.set_selection_mode(gtk::SelectionMode::Single);
    listbox.add_css_class("navigation-sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .child(&listbox)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    // Sidebar header: app title plus the overflow menu, so the window can
    // still be driven when the titlebar is hidden.
    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.set_show_start_title_buttons(false);
    sidebar_header.set_show_end_title_buttons(false);
    sidebar_header.set_title_widget(Some(&adw::WindowTitle::new("Vmux", "")));
    // Only shown while the titlebar (and its sidebar toggle) is hidden;
    // visibility is managed by App::sync_titlebar.
    let sidebar_hide_btn = gtk::Button::from_icon_name("sidebar-show-symbolic");
    sidebar_hide_btn.set_tooltip_text(Some("Hide sidebar"));
    sidebar_hide_btn.set_visible(false);
    sidebar_header.pack_start(&sidebar_hide_btn);
    let menu = gio::Menu::new();
    menu.append(Some("Hide Sidebar"), Some("win.hide-sidebar"));
    menu.append(Some("Add Workspace"), Some("win.add-workspace"));
    menu.append(Some("Close Workspace"), Some("win.close-workspace"));
    menu.append(Some("Settings"), Some("win.preferences"));
    menu.append(Some("Quit"), Some("win.quit"));
    let menu_btn = gtk::MenuButton::new();
    menu_btn.set_icon_name("view-more-symbolic");
    menu_btn.set_tooltip_text(Some("Menu"));
    menu_btn.set_menu_model(Some(&menu));
    sidebar_header.pack_end(&menu_btn);

    let sidebar = adw::ToolbarView::new();
    sidebar.add_top_bar(&sidebar_header);
    sidebar.set_content(Some(&scroll));

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let split_view = adw::OverlaySplitView::new();
    split_view.set_min_sidebar_width(170.0);
    split_view.set_max_sidebar_width(280.0);
    split_view.set_sidebar(Some(&sidebar));
    split_view.set_content(Some(&stack));
    split_view.set_show_sidebar(true);
    {
        let split_view = split_view.clone();
        sidebar_hide_btn.connect_clicked(move |_| split_view.set_show_sidebar(false));
    }

    let header = adw::HeaderBar::new();
    let toggle = gtk::ToggleButton::new();
    toggle.set_icon_name("sidebar-show-symbolic");
    toggle.set_tooltip_text(Some("Toggle sidebar"));
    toggle.set_active(true);
    toggle
        .bind_property("active", &split_view, "show-sidebar")
        .bidirectional()
        .sync_create()
        .build();
    header.pack_start(&toggle);
    let new_zone_btn = gtk::Button::from_icon_name("list-add-symbolic");
    new_zone_btn.set_tooltip_text(Some("New zone"));
    header.pack_start(&new_zone_btn);
    let settings_btn = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings_btn.set_tooltip_text(Some("Preferences"));
    header.pack_end(&settings_btn);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_css_class("vmux-chrome");
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&split_view));
    window.set_content(Some(&toolbar_view));

    Chrome {
        window,
        split_view,
        stack,
        listbox,
        titlebar: header,
        sidebar_hide_btn,
        new_zone_btn,
        settings_btn,
    }
}

pub fn wire_chrome(app: &Rc<App>, chrome: &Chrome) {
    {
        let app = app.clone();
        chrome.listbox.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                app.on_zone_selected(row);
            }
        });
    }
    {
        let app = app.clone();
        chrome
            .new_zone_btn
            .connect_clicked(move |_| new_zone_dialog(&app));
    }
    {
        let app = app.clone();
        chrome
            .settings_btn
            .connect_clicked(move |_| crate::keybinds::show_settings(&app));
    }
    {
        let app = app.clone();
        chrome.window.connect_close_request(move |_| {
            app.save_now();
            glib::Propagation::Proceed
        });
    }
    // Sidebar overflow-menu actions ("win." scope).
    {
        let split_view = chrome.split_view.clone();
        let act = gio::SimpleAction::new("hide-sidebar", None);
        act.connect_activate(move |_, _| split_view.set_show_sidebar(false));
        chrome.window.add_action(&act);
    }
    {
        let app = app.clone();
        let act = gio::SimpleAction::new("add-workspace", None);
        act.connect_activate(move |_, _| new_zone_dialog(&app));
        chrome.window.add_action(&act);
    }
    {
        let app = app.clone();
        let act = gio::SimpleAction::new("close-workspace", None);
        act.connect_activate(move |_, _| {
            let _ = app.run_action("close-zone");
        });
        chrome.window.add_action(&act);
    }
    {
        let app = app.clone();
        let act = gio::SimpleAction::new("preferences", None);
        act.connect_activate(move |_, _| crate::keybinds::show_settings(&app));
        chrome.window.add_action(&act);
    }
    {
        let window = chrome.window.downgrade();
        let act = gio::SimpleAction::new("quit", None);
        act.connect_activate(move |_, _| {
            if let Some(w) = window.upgrade() {
                w.close();
            }
        });
        chrome.window.add_action(&act);
    }
    {
        let app = app.clone();
        chrome
            .split_view
            .connect_show_sidebar_notify(move |_| app.update_sidebar_reveal());
    }
    {
        let app = app.clone();
        chrome.window.connect_is_active_notify(move |w| {
            if w.is_active()
                && let Some(zone) = app.active_zone()
            {
                zone.attention.set_visible(false);
                app.withdraw_zone_notification(&zone);
            }
        });
    }
}

pub fn new_zone_dialog(app: &Rc<App>) {
    let on_open = {
        let app = app.clone();
        Rc::new(move |path: std::path::PathBuf| {
            let cwd = path.to_string_lossy().into_owned();
            app.add_zone(&state::display_name(&cwd), &cwd);
        })
    };
    crate::open_path_dialog::present_open_path_dialog(crate::open_path_dialog::OpenPathDialogInput {
        parent: app.window.clone().upcast(),
        initial_directory: std::path::PathBuf::from(state::home_dir()),
        on_open,
    });
}

pub fn rename_zone_dialog(app: &Rc<App>, zone: &Rc<Zone>) {
    let name = gtk::Entry::builder()
        .text(zone.name.borrow().as_str())
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::new(Some("Rename Zone"), None);
    dialog.set_extra_child(Some(&name));
    dialog.add_responses(&[("cancel", "Cancel"), ("rename", "Rename")]);
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("rename"));
    dialog.set_close_response("cancel");
    {
        let app = app.clone();
        let zone = zone.clone();
        dialog.connect_response(Some("rename"), move |_, _| {
            let n = name.text().trim().to_string();
            if !n.is_empty() {
                app.rename_zone(&zone, &n);
            }
        });
    }
    dialog.present(Some(&app.window));
}

pub fn remove_zone_dialog(app: &Rc<App>, zone: &Rc<Zone>) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Remove “{}”?", zone.name.borrow())),
        Some("Running terminals in this zone will be terminated."),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("remove", "Remove")]);
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    {
        let app = app.clone();
        let zone = zone.clone();
        dialog.connect_response(Some("remove"), move |_, _| {
            app.remove_zone(&zone);
        });
    }
    dialog.present(Some(&app.window));
}

pub fn show_zone_menu(app: &Rc<App>, zone: &Rc<Zone>, x: f64, y: f64) {
    let rename = gtk::Button::with_label("Rename…");
    rename.add_css_class("flat");
    let remove = gtk::Button::with_label("Remove…");
    remove.add_css_class("flat");
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bx.append(&rename);
    bx.append(&remove);

    let pop = gtk::Popover::new();
    pop.set_child(Some(&bx));
    pop.set_parent(&zone.row);
    pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    pop.set_has_arrow(false);
    {
        let app = app.clone();
        let zone = zone.clone();
        let pop = pop.clone();
        rename.connect_clicked(move |_| {
            pop.popdown();
            rename_zone_dialog(&app, &zone);
        });
    }
    {
        let app = app.clone();
        let zone = zone.clone();
        let pop = pop.clone();
        remove.connect_clicked(move |_| {
            pop.popdown();
            remove_zone_dialog(&app, &zone);
        });
    }
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}
