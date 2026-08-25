use crate::app::App;
use crate::state::Config;
use gtk4 as gtk;
use gtk4::glib;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Action registry: (id, human title, default accelerator).
/// "" as default means unbound. Alt+1..9 zone switching is fixed.
pub const ACTIONS: &[(&str, &str, &str)] = &[
    ("new-tab", "New Tab", "<Control><Shift>t"),
    ("close-tab", "Close Tab", "<Control><Shift>w"),
    ("split-right", "Split Pane Right", "<Control><Shift>e"),
    ("split-down", "Split Pane Down", "<Control><Shift>o"),
    ("focus-next-pane", "Focus Next Pane", "<Control><Shift>a"),
    ("focus-pane-left", "Focus Pane Left", "<Control><Alt>Left"),
    ("focus-pane-right", "Focus Pane Right", "<Control><Alt>Right"),
    ("focus-pane-up", "Focus Pane Up", "<Control><Alt>Up"),
    ("focus-pane-down", "Focus Pane Down", "<Control><Alt>Down"),
    ("grow-pane", "Grow Pane", "<Control><Shift>equal"),
    ("shrink-pane", "Shrink Pane", "<Control><Shift>minus"),
    ("next-tab", "Next Tab", "<Control>Page_Down"),
    ("prev-tab", "Previous Tab", "<Control>Page_Up"),
    ("new-zone", "New Zone", "<Control><Shift>n"),
    ("close-zone", "Close Zone", "<Control><Alt>w"),
    ("prev-zone", "Previous Zone", "<Control><Alt>Page_Up"),
    ("next-zone", "Next Zone", "<Control><Alt>Page_Down"),
    ("move-zone-up", "Move Zone Up", "<Control><Shift>Page_Up"),
    ("move-zone-down", "Move Zone Down", "<Control><Shift>Page_Down"),
    ("copy", "Copy", "<Control><Shift>c"),
    ("paste", "Paste", "<Control><Shift>v"),
    ("font-inc", "Increase Font Size", "<Control>equal"),
    ("font-dec", "Decrease Font Size", "<Control>minus"),
    ("font-reset", "Reset Font Size", "<Control>0"),
    ("toggle-sidebar", "Toggle Sidebar", "F9"),
    ("preferences", "Preferences", "<Control>comma"),
];

pub fn accel_for(cfg: &Config, id: &str) -> String {
    if let Some(over) = cfg.keybindings.get(id) {
        return over.clone();
    }
    ACTIONS
        .iter()
        .find(|(aid, _, _)| *aid == id)
        .map(|(_, _, def)| def.to_string())
        .unwrap_or_default()
}

/// (id, title, effective accel) for every action.
pub fn merged(cfg: &Config) -> Vec<(String, String, String)> {
    ACTIONS
        .iter()
        .map(|(id, title, _)| (id.to_string(), title.to_string(), accel_for(cfg, id)))
        .collect()
}

pub fn pretty_accel(accel: &str) -> String {
    if accel.is_empty() {
        return "unbound".into();
    }
    match gtk::accelerator_parse(accel) {
        Some((key, mods)) => gtk::accelerator_get_label(key, mods).to_string(),
        None => accel.to_string(),
    }
}

pub fn add_sc(ctl: &gtk::ShortcutController, trig: &str, f: impl Fn() -> glib::Propagation + 'static) {
    let action = gtk::CallbackAction::new(move |_, _| f());
    if let Some(trigger) = gtk::ShortcutTrigger::parse_string(trig) {
        ctl.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
    } else {
        eprintln!("vmux: bad shortcut trigger: {trig}");
    }
}

/// The settings dialog: General behavior, the style.css entry point, and all
/// keybindings.
pub fn show_settings(app: &Rc<App>) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Preferences");

    // --- General page ---
    let general = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();
    let group = adw::PreferencesGroup::new();
    group.set_title("Terminal");

    let shell_row = adw::EntryRow::builder().title("Shell (empty = $SHELL)").build();
    shell_row.set_text(app.config.borrow().shell.as_deref().unwrap_or(""));
    shell_row.set_show_apply_button(true);
    {
        let app = app.clone();
        shell_row.connect_apply(move |row| {
            let text = row.text().trim().to_string();
            app.config.borrow_mut().shell = if text.is_empty() { None } else { Some(text) };
            app.save_now();
        });
    }
    group.add(&shell_row);

    let adj = gtk::Adjustment::new(
        app.config.borrow().scrollback_lines as f64,
        0.0,
        1_000_000.0,
        1000.0,
        10_000.0,
        0.0,
    );
    let scroll_row = adw::SpinRow::builder()
        .title("Scrollback lines")
        .adjustment(&adj)
        .digits(0)
        .build();
    {
        let app = app.clone();
        adj.connect_value_changed(move |a| {
            app.config.borrow_mut().scrollback_lines = a.value() as i64;
            app.apply_terminal_settings();
            app.schedule_save();
        });
    }
    group.add(&scroll_row);

    general.add(&group);

    let window_group = adw::PreferencesGroup::new();
    window_group.set_title("Window");
    let titlebar_row = adw::SwitchRow::builder()
        .title("Hide titlebar")
        .subtitle("Also hides the window controls — use the sidebar menu to quit")
        .active(app.config.borrow().hide_titlebar)
        .build();
    {
        let app = app.clone();
        titlebar_row.connect_active_notify(move |row| {
            app.config.borrow_mut().hide_titlebar = row.is_active();
            app.sync_titlebar();
            app.schedule_save();
        });
    }
    window_group.add(&titlebar_row);
    general.add(&window_group);

    let notif_group = adw::PreferencesGroup::new();
    notif_group.set_title("Notifications");
    let notif_row = adw::SwitchRow::builder()
        .title("Desktop notifications")
        .subtitle("Notify when a background terminal requests attention")
        .active(app.config.borrow().desktop_notifications)
        .build();
    {
        let app = app.clone();
        notif_row.connect_active_notify(move |row| {
            app.config.borrow_mut().desktop_notifications = row.is_active();
            app.schedule_save();
        });
    }
    notif_group.add(&notif_row);
    let bell_row = adw::SwitchRow::builder()
        .title("Notify on terminal bell")
        .subtitle("Also send a desktop notification when a background terminal rings the bell")
        .active(app.config.borrow().notify_on_bell)
        .build();
    {
        let app = app.clone();
        bell_row.connect_active_notify(move |row| {
            app.config.borrow_mut().notify_on_bell = row.is_active();
            app.schedule_save();
        });
    }
    notif_group.add(&bell_row);
    general.add(&notif_group);
    dialog.add(&general);

    dialog.add(&crate::appearance::page(app));

    // --- Keybindings page ---
    let keys_page = adw::PreferencesPage::builder()
        .title("Keybindings")
        .icon_name("input-keyboard-symbolic")
        .build();
    let keys_group = adw::PreferencesGroup::new();
    keys_group.set_title("Shortcuts");
    keys_group.set_description(Some(
        "Click a row, then press the new combination. Backspace unbinds, Esc cancels. Alt+1–9 switch zones (fixed).",
    ));

    let labels: Rc<RefCell<Vec<(String, gtk::ShortcutLabel)>>> = Rc::new(RefCell::new(Vec::new()));
    for (id, title, accel) in merged(&app.config.borrow()) {
        let row = adw::ActionRow::builder().title(&title).activatable(true).build();
        let label = gtk::ShortcutLabel::new(&accel);
        label.set_disabled_text("unbound");
        label.set_valign(gtk::Align::Center);
        row.add_suffix(&label);
        labels.borrow_mut().push((id.clone(), label));
        {
            let app = app.clone();
            let labels = labels.clone();
            let title = title.clone();
            row.connect_activated(move |_| {
                capture_binding(&app, &id, &title, &labels);
            });
        }
        keys_group.add(&row);
    }
    keys_page.add(&keys_group);
    dialog.add(&keys_page);

    dialog.present(Some(&app.window));
}

fn refresh_labels(app: &Rc<App>, labels: &Rc<RefCell<Vec<(String, gtk::ShortcutLabel)>>>) {
    let cfg = app.config.borrow();
    for (id, label) in labels.borrow().iter() {
        label.set_accelerator(&accel_for(&cfg, id));
    }
}

/// Modal "press a key" capture; applies the new binding, clearing it from any
/// other action that used it.
fn capture_binding(
    app: &Rc<App>,
    action_id: &str,
    title: &str,
    labels: &Rc<RefCell<Vec<(String, gtk::ShortcutLabel)>>>,
) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Shortcut for “{title}”")),
        Some("Press a key combination.\nBackspace unbinds · Esc cancels"),
    );
    dialog.add_responses(&[("cancel", "Cancel")]);
    dialog.set_close_response("cancel");

    let ctl = gtk::EventControllerKey::new();
    ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let app = app.clone();
        let labels = labels.clone();
        let action_id = action_id.to_string();
        let dialog = dialog.clone();
        ctl.connect_key_pressed(move |_, keyval, _code, state| {
            let mods = state & gtk::accelerator_get_default_mod_mask();
            if mods.is_empty() {
                if keyval == gtk::gdk::Key::Escape {
                    dialog.close();
                    return glib::Propagation::Stop;
                }
                if keyval == gtk::gdk::Key::BackSpace {
                    apply_binding(&app, &action_id, "", &labels);
                    dialog.close();
                    return glib::Propagation::Stop;
                }
            }
            let keyval = keyval.to_lower();
            if !gtk::accelerator_valid(keyval, mods) {
                return glib::Propagation::Stop; // modifier-only press: keep waiting
            }
            // Require a modifier (or an F-key) so plain typing keys can't be bound.
            let fkey = (gtk::gdk::Key::F1..=gtk::gdk::Key::F12).contains(&keyval);
            if mods.is_empty() && !fkey {
                return glib::Propagation::Stop;
            }
            let accel = gtk::accelerator_name(keyval, mods).to_string();
            apply_binding(&app, &action_id, &accel, &labels);
            dialog.close();
            glib::Propagation::Stop
        });
    }
    dialog.add_controller(ctl);
    dialog.present(Some(&app.window));
}

fn apply_binding(
    app: &Rc<App>,
    action_id: &str,
    accel: &str,
    labels: &Rc<RefCell<Vec<(String, gtk::ShortcutLabel)>>>,
) {
    {
        let mut cfg = app.config.borrow_mut();
        if !accel.is_empty() {
            // Steal the accel from any other action using it.
            let owners: Vec<String> = ACTIONS
                .iter()
                .map(|(id, _, _)| id.to_string())
                .filter(|id| id != action_id)
                .collect();
            for id in owners {
                if accel_for(&cfg, &id) == accel {
                    cfg.keybindings.insert(id, String::new());
                }
            }
        }
        cfg.keybindings.insert(action_id.to_string(), accel.to_string());
    }
    app.save_now();
    app.reinstall_shortcuts();
    refresh_labels(app, labels);
}
