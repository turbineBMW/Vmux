use crate::app::App;
use crate::state::{self, Theme};
use crate::term;
use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

const PALETTE_NAMES: [&str; 8] = [
    "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
];

type SetColor = Box<dyn Fn(&mut Theme, String)>;

fn hex(c: &gtk::gdk::RGBA) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        (c.red() * 255.0).round() as u8,
        (c.green() * 255.0).round() as u8,
        (c.blue() * 255.0).round() as u8
    )
}

/// A color picker button that writes its color into the theme via `set`,
/// re-applies the terminal config, and schedules a save.
fn color_button(
    app: &Rc<App>,
    initial: &str,
    fallback: &str,
    set: impl Fn(&mut Theme, String) + 'static,
) -> gtk::ColorDialogButton {
    let dialog = gtk::ColorDialog::builder().with_alpha(false).build();
    let btn = gtk::ColorDialogButton::new(Some(dialog));
    btn.set_valign(gtk::Align::Center);
    btn.set_rgba(&term::parse_color(initial, fallback));
    let app = app.clone();
    btn.connect_rgba_notify(move |b| {
        set(&mut app.config.borrow_mut().theme, hex(&b.rgba()));
        app.apply_terminal_config();
        app.schedule_save();
    });
    btn
}

/// The Appearance settings page: foreground/background/cursor colors,
/// background opacity, and the 16-color ANSI palette.
pub fn page(app: &Rc<App>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Appearance")
        .icon_name("applications-graphics-symbolic")
        .build();
    let theme = app.config.borrow().theme.clone();
    // (button, default color) pairs so Reset can restore everything.
    let mut buttons: Vec<(gtk::ColorDialogButton, gtk::gdk::RGBA)> = Vec::new();

    let colors = adw::PreferencesGroup::new();
    colors.set_title("Terminal Colors");
    let reset = gtk::Button::with_label("Reset");
    reset.set_tooltip_text(Some("Reset all colors to defaults"));
    reset.set_valign(gtk::Align::Center);
    reset.add_css_class("flat");
    colors.set_header_suffix(Some(&reset));

    let mut add_color_row = |group: &adw::PreferencesGroup,
                             title: &str,
                             initial: &str,
                             fallback: &str,
                             set: SetColor| {
        let row = adw::ActionRow::builder().title(title).build();
        let btn = color_button(app, initial, fallback, set);
        buttons.push((btn.clone(), term::parse_color(fallback, fallback)));
        row.add_suffix(&btn);
        group.add(&row);
    };

    add_color_row(
        &colors,
        "Foreground",
        &theme.foreground,
        state::DEFAULT_FOREGROUND,
        Box::new(|t, c| t.foreground = c),
    );
    add_color_row(
        &colors,
        "Background",
        &theme.background,
        state::DEFAULT_BACKGROUND,
        Box::new(|t, c| t.background = c),
    );
    add_color_row(
        &colors,
        "Cursor",
        &theme.cursor,
        state::DEFAULT_FOREGROUND,
        Box::new(|t, c| t.cursor = c),
    );

    let opacity_adj = gtk::Adjustment::new(
        (app.config.borrow().background_opacity * 100.0).clamp(20.0, 100.0),
        20.0,
        100.0,
        5.0,
        10.0,
        0.0,
    );
    let opacity_row = adw::SpinRow::builder()
        .title("Background opacity (%)")
        .adjustment(&opacity_adj)
        .digits(0)
        .build();
    {
        let app = app.clone();
        opacity_adj.connect_value_changed(move |a| {
            app.config.borrow_mut().background_opacity = a.value() / 100.0;
            app.apply_terminal_config();
            app.schedule_save();
        });
    }
    colors.add(&opacity_row);

    let palette = adw::PreferencesGroup::new();
    palette.set_title("Palette");
    palette.set_description(Some("ANSI colors — normal and bright variants."));
    for (i, name) in PALETTE_NAMES.iter().enumerate() {
        let row = adw::ActionRow::builder().title(*name).build();
        for (slot, label) in [(i, name.to_string()), (i + 8, format!("Bright {name}"))] {
            let btn = color_button(
                app,
                theme.palette.get(slot).map_or("", String::as_str),
                state::DEFAULT_PALETTE[slot],
                move |t, c| t.palette[slot] = c,
            );
            btn.set_tooltip_text(Some(&label));
            buttons.push((
                btn.clone(),
                term::parse_color(state::DEFAULT_PALETTE[slot], state::DEFAULT_PALETTE[slot]),
            ));
            row.add_suffix(&btn);
        }
        palette.add(&row);
    }

    // Reset pushes defaults back through each button; the rgba-notify
    // handlers then update the config and re-apply.
    reset.connect_clicked(move |_| {
        for (btn, def) in &buttons {
            btn.set_rgba(def);
        }
    });

    page.add(&colors);
    page.add(&palette);
    page
}
