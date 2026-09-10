use super::*;
use adw::prelude::BinExt;

// Run separately so GTK stays on one thread and never touches the real session:
// GTK_A11Y=none GDK_BACKEND=x11 xvfb-run -a cargo test --bin vmux \
//   workspace_switch_restores_input_focus -- --ignored --test-threads=1
#[test]
#[ignore = "requires a GTK display; run with xvfb-run"]
fn workspace_switch_restores_input_focus() {
    adw::init().unwrap();
    let gtk_app = adw::Application::builder()
        .application_id("dev.vmux.FocusTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    gtk_app.register(gio::Cancellable::NONE).unwrap();
    let chrome = window::build_chrome(&gtk_app);
    let app = Rc::new(App {
        window: chrome.window.clone(),
        split_view: chrome.split_view.clone(),
        stack: chrome.stack.clone(),
        listbox: chrome.listbox.clone(),
        titlebar: chrome.titlebar.clone(),
        sidebar_hide_btn: chrome.sidebar_hide_btn.clone(),
        zones: RefCell::new(Vec::new()),
        config: RefCell::new(state::Config::default()),
        notifier: None,
        font_scale: Cell::new(1.0),
        text_bindings: RefCell::new(Vec::new()),
        save_source: RefCell::new(None),
        next_zone_id: Cell::new(1),
        zone_history: Cell::new((None, None)),
        shortcut_ctl: RefCell::new(None),
        bindings_monitor: RefCell::new(None),
        style_provider: gtk::CssProvider::new(),
        style_monitor: RefCell::new(None),
        drag_emptied: RefCell::new(Vec::new()),
    });
    // Only wire selection: no settings, shell processes, or persistence hooks.
    let weak = Rc::downgrade(&app);
    app.listbox.connect_row_selected(move |_, row| {
        if let (Some(app), Some(row)) = (weak.upgrade(), row) {
            app.on_zone_selected(row);
        }
    });
    let zones: Vec<_> = (0..2)
        .map(|i| {
            let zone = Zone::build(
                &app,
                &state::ZoneState {
                    name: format!("test-{i}"),
                    root: Some(state::NodeState::Pane {
                        tabs: Vec::new(),
                        active_tab: 0,
                    }),
                    ..Default::default()
                },
                i,
            );
            app.stack.add_named(&zone.page, Some(&zone.stack_name()));
            app.listbox.append(&zone.row);
            app.zones.borrow_mut().push(zone.clone());
            zone
        })
        .collect();
    let (first_pane, first_view, first_tabs) = tabbed_pane();
    zones[0].page.set_child(Some(&first_pane));
    let (other_pane, _, other_tabs) = tabbed_pane();
    zones[1].page.set_child(Some(&other_pane));
    first_view.set_selected_page(&first_view.nth_page(1));
    app.window.present();

    // No focus history (e.g. restored state): use the selected second tab.
    app.select_zone(1);
    settle(&app);
    app.select_zone(0);
    settle(&app);
    assert_focus(&app, &first_tabs[1]);

    // A split's remembered pane must win over the first pane in the tree.
    let (second_pane, second_view, second_tabs) = tabbed_pane();
    zones[0].page.set_child(gtk::Widget::NONE);
    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&first_pane));
    split.set_end_child(Some(&second_pane));
    zones[0].page.set_child(Some(&split));
    zones[0].last_focused.set(Some(&second_tabs[0]));
    second_view.set_selected_page(&second_view.nth_page(1));
    app.select_zone(1);
    settle(&app);
    app.select_zone(0);
    settle(&app);
    assert_focus(&app, &second_tabs[1]);

    // Re-selecting the current workspace also restores input from the sidebar.
    zones[0].row.grab_focus();
    app.select_zone(0);
    settle(&app);
    assert_focus(&app, &second_tabs[1]);

    // Stale requests must not focus a hidden tab or a workspace left behind.
    app.select_zone(1);
    term::focus_later(&second_tabs[1]);
    term::focus_later(&other_tabs[1]);
    settle(&app);
    assert_focus(&app, &other_tabs[0]);

    // A tab dragged out of the zone is no longer a valid history target.
    zones[0].last_focused.set(Some(&other_tabs[0]));
    app.select_zone(0);
    settle(&app);
    assert_focus(&app, &first_tabs[1]);
    app.window.destroy();
}

fn tabbed_pane() -> (gtk::Stack, adw::TabView, [vte::Terminal; 2]) {
    let view = adw::TabView::new();
    let tabs = [vte::Terminal::new(), vte::Terminal::new()];
    for terminal in &tabs {
        view.append(&gtk::ScrolledWindow::builder().child(terminal).build());
    }
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&adw::TabBar::builder().view(&view).build());
    content.append(&view);
    let pane = gtk::Stack::new();
    pane.add_css_class(splits::PANE_CLASS);
    pane.add_named(&content, Some("tabs"));
    (pane, view, tabs)
}

fn settle(app: &App) {
    // Never let this GUI regression write the user's state file.
    if let Some(source) = app.save_source.borrow_mut().take() {
        source.remove();
    }
    let context = glib::MainContext::default();
    while context.pending() {
        context.iteration(false);
    }
}

fn assert_focus(app: &App, terminal: &vte::Terminal) {
    assert!(terminal.is_mapped());
    assert_eq!(
        GtkWindowExt::focus(&app.window).as_ref(),
        Some(terminal.upcast_ref())
    );
}
