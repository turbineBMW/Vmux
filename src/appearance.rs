use crate::app::App;
use crate::style::{self, Declaration};
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Copy)]
struct ColorField {
    declaration: Declaration,
    title: &'static str,
    description: &'static str,
    fallback: &'static str,
}

struct AppearanceUi {
    syncing: Cell<bool>,
    theme_row: gtk::glib::WeakRef<adw::ComboRow>,
    themes: RefCell<Vec<style::Theme>>,
    refreshers: RefCell<Vec<Box<dyn Fn()>>>,
}

impl Default for AppearanceUi {
    fn default() -> Self {
        Self {
            syncing: Cell::new(false),
            theme_row: gtk::glib::WeakRef::new(),
            themes: RefCell::new(Vec::new()),
            refreshers: RefCell::new(Vec::new()),
        }
    }
}

impl AppearanceUi {
    fn while_syncing(&self, update: impl FnOnce()) {
        let was_syncing = self.syncing.replace(true);
        update();
        self.syncing.set(was_syncing);
    }

    fn refresh_controls(&self) {
        self.while_syncing(|| {
            for refresh in self.refreshers.borrow().iter() {
                refresh();
            }
        });
    }

    fn refresh_themes(&self) -> std::io::Result<()> {
        let themes = style::themes()?;
        let current = style::current_theme(&themes);
        let Some(row) = self.theme_row.upgrade() else {
            return Ok(());
        };
        let model = gtk::StringList::new(&["Custom / unsaved"]);
        for theme in &themes {
            model.append(theme.name());
        }
        self.while_syncing(|| {
            row.set_model(Some(&model));
            row.set_selected(current.map_or(0, |index| index as u32 + 1));
        });
        *self.themes.borrow_mut() = themes;
        Ok(())
    }

    fn mark_custom(&self) {
        let Some(row) = self.theme_row.upgrade() else {
            return;
        };
        self.while_syncing(|| row.set_selected(0));
    }

    fn add_refresher(&self, refresh: impl Fn() + 'static) {
        self.refreshers.borrow_mut().push(Box::new(refresh));
    }
}

macro_rules! named {
    ($key:literal, $title:literal, $description:literal, $fallback:literal) => {
        ColorField {
            declaration: Declaration::NamedColor($key),
            title: $title,
            description: $description,
            fallback: $fallback,
        }
    };
}

macro_rules! custom {
    ($key:literal, $title:literal, $description:literal, $fallback:literal) => {
        ColorField {
            declaration: Declaration::CustomProperty($key),
            title: $title,
            description: $description,
            fallback: $fallback,
        }
    };
}

const TERMINAL_BASE: &[ColorField] = &[
    named!(
        "vmux_terminal_foreground",
        "Foreground",
        "Default text drawn inside every terminal.",
        "#d0cfcc"
    ),
    named!(
        "vmux_terminal_background",
        "Background",
        "Terminal canvas. Its alpha channel controls terminal transparency.",
        "#1d1d20"
    ),
    named!(
        "vmux_terminal_cursor",
        "Cursor",
        "Fill color of the terminal's block cursor.",
        "#d0cfcc"
    ),
];

const ANSI_COLORS: &[ColorField] = &[
    named!(
        "vmux_terminal_black",
        "Black (ANSI 0)",
        "Normal black requested by terminal applications.",
        "#171421"
    ),
    named!(
        "vmux_terminal_red",
        "Red (ANSI 1)",
        "Normal red used for errors and red terminal text.",
        "#c01c28"
    ),
    named!(
        "vmux_terminal_green",
        "Green (ANSI 2)",
        "Normal green used for success and green terminal text.",
        "#26a269"
    ),
    named!(
        "vmux_terminal_yellow",
        "Yellow (ANSI 3)",
        "Normal yellow used for warnings and yellow terminal text.",
        "#a2734c"
    ),
    named!(
        "vmux_terminal_blue",
        "Blue (ANSI 4)",
        "Normal blue requested by terminal applications.",
        "#12488b"
    ),
    named!(
        "vmux_terminal_magenta",
        "Magenta (ANSI 5)",
        "Normal magenta requested by terminal applications.",
        "#a347ba"
    ),
    named!(
        "vmux_terminal_cyan",
        "Cyan (ANSI 6)",
        "Normal cyan requested by terminal applications.",
        "#2aa1b3"
    ),
    named!(
        "vmux_terminal_white",
        "White (ANSI 7)",
        "Normal white/light gray requested by terminal applications.",
        "#d0cfcc"
    ),
    named!(
        "vmux_terminal_bright_black",
        "Bright black (ANSI 8)",
        "Bright black, commonly rendered as dark gray.",
        "#5e5c64"
    ),
    named!(
        "vmux_terminal_bright_red",
        "Bright red (ANSI 9)",
        "High-intensity red terminal text.",
        "#f66151"
    ),
    named!(
        "vmux_terminal_bright_green",
        "Bright green (ANSI 10)",
        "High-intensity green terminal text.",
        "#33d17a"
    ),
    named!(
        "vmux_terminal_bright_yellow",
        "Bright yellow (ANSI 11)",
        "High-intensity yellow terminal text.",
        "#e9ad0c"
    ),
    named!(
        "vmux_terminal_bright_blue",
        "Bright blue (ANSI 12)",
        "High-intensity blue terminal text.",
        "#2a7bde"
    ),
    named!(
        "vmux_terminal_bright_magenta",
        "Bright magenta (ANSI 13)",
        "High-intensity magenta terminal text.",
        "#c061cb"
    ),
    named!(
        "vmux_terminal_bright_cyan",
        "Bright cyan (ANSI 14)",
        "High-intensity cyan terminal text.",
        "#33c7de"
    ),
    named!(
        "vmux_terminal_bright_white",
        "Bright white (ANSI 15)",
        "Highest-intensity white terminal text.",
        "#ffffff"
    ),
];

const ACCENT_WINDOW: &[ColorField] = &[
    custom!(
        "--accent-bg-color",
        "Accent fill",
        "Background of emphasized buttons and selected controls.",
        "#3584e4"
    ),
    custom!(
        "--accent-fg-color",
        "Text on accent",
        "Text and icons drawn over the accent fill.",
        "#ffffff"
    ),
    custom!(
        "--accent-color",
        "Accent detail",
        "Links, focus indicators, and unfilled accent details.",
        "#62a0ea"
    ),
    custom!(
        "--window-bg-color",
        "Window background",
        "Base behind the main window and tab strips.",
        "#1d1d20"
    ),
    custom!(
        "--window-fg-color",
        "Window foreground",
        "Default text and icons on the window background.",
        "#ffffff"
    ),
    custom!(
        "--view-bg-color",
        "Content background",
        "Background of content views and lists.",
        "#1d1d20"
    ),
    custom!(
        "--view-fg-color",
        "Content foreground",
        "Text and icons inside content views and lists.",
        "#ffffff"
    ),
];

const HEADER_SIDEBARS: &[ColorField] = &[
    custom!(
        "--headerbar-bg-color",
        "Header background",
        "Background of the top header bar.",
        "#303030"
    ),
    custom!(
        "--headerbar-fg-color",
        "Header foreground",
        "Text and window controls in the header bar.",
        "#ffffff"
    ),
    custom!(
        "--headerbar-backdrop-color",
        "Inactive header",
        "Header background while the vmux window is unfocused.",
        "#242424"
    ),
    custom!(
        "--headerbar-border-color",
        "Header border",
        "Divider along the edge of the header bar.",
        "#3d3d3d"
    ),
    custom!(
        "--sidebar-bg-color",
        "Sidebar background",
        "Background of the zone sidebar.",
        "#242424"
    ),
    custom!(
        "--sidebar-fg-color",
        "Sidebar foreground",
        "Zone names and icons in the sidebar.",
        "#ffffff"
    ),
    custom!(
        "--sidebar-backdrop-color",
        "Inactive sidebar",
        "Zone sidebar background while the window is unfocused.",
        "#202020"
    ),
    custom!(
        "--sidebar-border-color",
        "Sidebar border",
        "Divider between the zone sidebar and terminal panes.",
        "#3d3d3d"
    ),
    custom!(
        "--secondary-sidebar-bg-color",
        "Secondary sidebar background",
        "Background used by secondary libadwaita sidebars.",
        "#242424"
    ),
    custom!(
        "--secondary-sidebar-fg-color",
        "Secondary sidebar foreground",
        "Text and icons in secondary sidebars.",
        "#ffffff"
    ),
    custom!(
        "--secondary-sidebar-backdrop-color",
        "Inactive secondary sidebar",
        "Secondary sidebar background in an unfocused window.",
        "#202020"
    ),
    custom!(
        "--secondary-sidebar-border-color",
        "Secondary sidebar border",
        "Edge divider used by secondary sidebars.",
        "#3d3d3d"
    ),
];

const SURFACES: &[ColorField] = &[
    custom!(
        "--card-bg-color",
        "Card background",
        "Raised preference cards and boxed content.",
        "#303030"
    ),
    custom!(
        "--card-fg-color",
        "Card foreground",
        "Text and icons shown on cards.",
        "#ffffff"
    ),
    custom!(
        "--popover-bg-color",
        "Popover background",
        "Menus, popovers, and transient floating surfaces.",
        "#303030"
    ),
    custom!(
        "--popover-fg-color",
        "Popover foreground",
        "Text and icons inside menus and popovers.",
        "#ffffff"
    ),
    custom!(
        "--dialog-bg-color",
        "Dialog background",
        "Background of dialogs and preference windows.",
        "#242424"
    ),
    custom!(
        "--dialog-fg-color",
        "Dialog foreground",
        "Text and icons inside dialogs.",
        "#ffffff"
    ),
];

const SEMANTIC: &[ColorField] = &[
    custom!(
        "--destructive-bg-color",
        "Destructive fill",
        "Background of destructive action buttons.",
        "#c01c28"
    ),
    custom!(
        "--destructive-fg-color",
        "Text on destructive fill",
        "Text and icons on destructive action buttons.",
        "#ffffff"
    ),
    custom!(
        "--destructive-color",
        "Destructive detail",
        "Destructive text, icons, and outlines.",
        "#f66151"
    ),
    custom!(
        "--success-bg-color",
        "Success fill",
        "Background of success-status elements.",
        "#26a269"
    ),
    custom!(
        "--success-fg-color",
        "Text on success fill",
        "Text and icons on success backgrounds.",
        "#ffffff"
    ),
    custom!(
        "--success-color",
        "Success detail",
        "Success text, icons, and outlines.",
        "#33d17a"
    ),
    custom!(
        "--warning-bg-color",
        "Warning fill",
        "Background of warning-status elements.",
        "#cd9309"
    ),
    custom!(
        "--warning-fg-color",
        "Text on warning fill",
        "Text and icons on warning backgrounds.",
        "#000000"
    ),
    custom!(
        "--warning-color",
        "Warning detail",
        "Warning text, icons, and outlines.",
        "#e9ad0c"
    ),
    custom!(
        "--error-bg-color",
        "Error fill",
        "Background of error-status elements.",
        "#c01c28"
    ),
    custom!(
        "--error-fg-color",
        "Text on error fill",
        "Text and icons on error backgrounds.",
        "#ffffff"
    ),
    custom!(
        "--error-color",
        "Error detail",
        "Error text, icons, and outlines.",
        "#f66151"
    ),
];

const TABS_SESSIONS: &[ColorField] = &[
    custom!(
        "--vmux-root-color",
        "Root session",
        "Security indicator for a tab running a root shell or command.",
        "#c01c28"
    ),
    custom!(
        "--vmux-remote-color",
        "Remote session",
        "Indicator for a tab running SSH, mosh, telnet, or Eternal Terminal.",
        "#9141ac"
    ),
    named!(
        "vmux_focused_tab_background",
        "Focused tab background",
        "Selected tab in the pane that currently owns keyboard focus.",
        "#243247"
    ),
    named!(
        "vmux_focused_tab_foreground",
        "Focused tab foreground",
        "Label and icon on the focused pane's selected tab.",
        "#62a0ea"
    ),
    named!(
        "vmux_focused_tab_hover",
        "Focused tab hover",
        "Focused selected tab while the pointer is over it.",
        "#293952"
    ),
    named!(
        "vmux_focused_tab_pressed",
        "Focused tab pressed",
        "Focused selected tab while it is being pressed.",
        "#304563"
    ),
];

const GIT_COLORS: &[ColorField] = &[
    named!(
        "vmux_git_added",
        "Files added",
        "Added-file count in each zone's Git summary.",
        "#26a269"
    ),
    named!(
        "vmux_git_modified",
        "Files modified",
        "Modified-file count in each zone's Git summary.",
        "#e9ad0c"
    ),
    named!(
        "vmux_git_deleted",
        "Files deleted",
        "Deleted-file count in each zone's Git summary.",
        "#c01c28"
    ),
    named!(
        "vmux_git_lines_added",
        "Lines inserted",
        "Inserted-line count in each zone's Git summary.",
        "#26a269"
    ),
    named!(
        "vmux_git_lines_deleted",
        "Lines deleted",
        "Deleted-line count in each zone's Git summary.",
        "#c01c28"
    ),
    named!(
        "vmux_git_ahead",
        "Unpushed commits",
        "Ahead-of-upstream commit count in each zone's Git summary.",
        "#2a7bde"
    ),
];

/// Appearance has one source of truth: every control reads and writes the
/// live-reloaded style.css file.
pub fn page(app: &Rc<App>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Appearance")
        .icon_name("applications-graphics-symbolic")
        .build();
    let ui = Rc::new(AppearanceUi::default());

    add_theme_group(&page, app, &ui);
    add_font_group(&page, app, &ui);
    add_color_group(
        &page,
        app,
        &ui,
        "Terminal",
        "Default terminal canvas and text colors.",
        TERMINAL_BASE,
    );
    add_color_group(
        &page,
        app,
        &ui,
        "ANSI Palette",
        "The 16 colors requested by command-line applications.",
        ANSI_COLORS,
    );
    add_color_group(
        &page,
        app,
        &ui,
        "Accent & Window",
        "Core libadwaita accent, window, and content colors.",
        ACCENT_WINDOW,
    );
    add_color_group(
        &page,
        app,
        &ui,
        "Headers & Sidebars",
        "Window chrome, zone sidebar, and pane-divider colors.",
        HEADER_SIDEBARS,
    );
    add_color_group(
        &page,
        app,
        &ui,
        "Raised Surfaces",
        "Cards, menus, popovers, and dialogs.",
        SURFACES,
    );
    add_color_group(
        &page,
        app,
        &ui,
        "Status Colors",
        "Semantic action and feedback colors used by libadwaita.",
        SEMANTIC,
    );
    add_color_group(
        &page,
        app,
        &ui,
        "Tabs & Sessions",
        "Focused-tab states and root/remote security indicators.",
        TABS_SESSIONS,
    );
    add_color_group(
        &page,
        app,
        &ui,
        "Git Status",
        "Colors in the secondary line of each zone.",
        GIT_COLORS,
    );
    add_opacity_group(&page, app, &ui);
    add_stylesheet_group(&page);
    ui.refresh_controls();
    if let Err(error) = ui.refresh_themes() {
        eprintln!("vmux: cannot list saved themes: {error}");
    }
    page
}

fn add_theme_group(page: &adw::PreferencesPage, app: &Rc<App>, ui: &Rc<AppearanceUi>) {
    let group = adw::PreferencesGroup::new();
    group.set_title("Themes");
    group.set_description(Some(
        "Saved themes are complete style.css snapshots. Switching replaces the active stylesheet immediately.",
    ));

    let model = gtk::StringList::new(&["Custom / unsaved"]);
    let theme_row = adw::ComboRow::builder()
        .title("Theme")
        .subtitle("Quick-switch between CSS themes saved for vmux.")
        .model(&model)
        .expression(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            gtk::Expression::NONE,
            "string",
        ))
        .build();
    ui.theme_row.set(Some(&theme_row));
    {
        let app = app.clone();
        let ui = ui.clone();
        theme_row.connect_selected_notify(move |row| {
            if ui.syncing.get() || row.selected() == 0 {
                return;
            }
            let theme = ui.themes.borrow().get(row.selected() as usize - 1).cloned();
            let Some(theme) = theme else {
                return;
            };
            match style::apply_theme(&theme) {
                Ok(()) => {
                    app.reload_user_css();
                    ui.refresh_controls();
                }
                Err(error) => {
                    show_theme_error(&app, "Could Not Switch Theme", &error);
                    if let Err(refresh_error) = ui.refresh_themes() {
                        eprintln!("vmux: cannot refresh saved themes: {refresh_error}");
                    }
                }
            }
        });
    }
    group.add(&theme_row);

    let path = style::themes_path().display().to_string();
    let save_row = adw::ActionRow::builder()
        .title("Save as Theme…")
        .subtitle(&path)
        .activatable(true)
        .build();
    let save = gtk::Button::with_label("Save");
    save.set_valign(gtk::Align::Center);
    save.add_css_class("suggested-action");
    save_row.add_suffix(&save);
    save_row.set_activatable_widget(Some(&save));
    {
        let app = app.clone();
        let ui = ui.clone();
        save.connect_clicked(move |_| save_theme_dialog(&app, &ui));
    }
    group.add(&save_row);
    page.add(&group);
}

fn add_font_group(page: &adw::PreferencesPage, app: &Rc<App>, ui: &Rc<AppearanceUi>) {
    let group = adw::PreferencesGroup::new();
    group.set_title("Terminal Font");
    group.set_description(Some("The typeface and base size used by every terminal."));

    let raw_family = style::value(Declaration::TerminalProperty("font-family"))
        .unwrap_or_else(|| "monospace".into());
    let configured_family = unquote(&raw_family);
    let cur_family = (configured_family != "monospace").then_some(configured_family);
    let cur_size = style::value(Declaration::TerminalProperty("font-size"))
        .and_then(|value| value.trim_end_matches("pt").trim().parse::<f64>().ok())
        .unwrap_or(11.0);

    let mut families: Vec<String> = app
        .window
        .pango_context()
        .list_families()
        .iter()
        .filter(|family| family.is_monospace())
        .map(|family| family.name().to_string())
        .collect();
    families.sort_unstable_by_key(|family| family.to_lowercase());
    if let Some(family) = &cur_family
        && !families.contains(family)
    {
        families.insert(0, family.clone());
    }

    let model = gtk::StringList::new(&[]);
    model.append("System monospace");
    for family in &families {
        model.append(family);
    }
    let font_row = adw::ComboRow::builder()
        .title("Font family")
        .subtitle("Monospace typeface used to render terminal cells.")
        .model(&model)
        .enable_search(true)
        .expression(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            gtk::Expression::NONE,
            "string",
        ))
        .build();
    let selected = cur_family
        .as_ref()
        .and_then(|family| families.iter().position(|candidate| candidate == family))
        .map_or(0, |index| (index + 1) as u32);
    font_row.set_selected(selected);

    let size_adjustment = gtk::Adjustment::new(cur_size, 6.0, 72.0, 1.0, 2.0, 0.0);
    let size_row = adw::SpinRow::builder()
        .title("Font size")
        .subtitle("Base terminal text size in points; zoom shortcuts remain temporary.")
        .adjustment(&size_adjustment)
        .digits(0)
        .build();

    let families = Rc::new(RefCell::new(families));
    {
        let font_row = font_row.downgrade();
        let model = model.downgrade();
        let size_adjustment = size_adjustment.downgrade();
        let families = families.clone();
        ui.add_refresher(move || {
            let (Some(font_row), Some(model), Some(size_adjustment)) = (
                font_row.upgrade(),
                model.upgrade(),
                size_adjustment.upgrade(),
            ) else {
                return;
            };
            let raw_family = style::value(Declaration::TerminalProperty("font-family"))
                .unwrap_or_else(|| "monospace".into());
            let configured_family = unquote(&raw_family);
            let selected = if configured_family == "monospace" {
                0
            } else if let Some(index) = families
                .borrow()
                .iter()
                .position(|family| family == &configured_family)
            {
                index as u32 + 1
            } else {
                model.append(&configured_family);
                let mut families = families.borrow_mut();
                families.push(configured_family);
                families.len() as u32
            };
            font_row.set_selected(selected);
            let size = style::value(Declaration::TerminalProperty("font-size"))
                .and_then(|value| value.trim_end_matches("pt").trim().parse::<f64>().ok())
                .unwrap_or(11.0);
            size_adjustment.set_value(size);
        });
    }

    {
        let app = app.clone();
        let ui = ui.clone();
        font_row.connect_selected_notify(move |row| {
            let family = if row.selected() == 0 {
                "monospace".to_string()
            } else {
                row.selected_item()
                    .and_downcast::<gtk::StringObject>()
                    .map(|item| format!("\"{}\"", item.string().replace('"', "\\\"")))
                    .unwrap_or_else(|| "monospace".into())
            };
            write(
                &app,
                &ui,
                Declaration::TerminalProperty("font-family"),
                &family,
            );
        });
    }
    {
        let app = app.clone();
        let ui = ui.clone();
        size_adjustment.connect_value_changed(move |adjustment| {
            write(
                &app,
                &ui,
                Declaration::TerminalProperty("font-size"),
                &format!("{}pt", adjustment.value() as i32),
            );
        });
    }

    group.add(&font_row);
    group.add(&size_row);
    page.add(&group);
}

fn add_color_group(
    page: &adw::PreferencesPage,
    app: &Rc<App>,
    ui: &Rc<AppearanceUi>,
    title: &str,
    description: &str,
    fields: &'static [ColorField],
) {
    let group = adw::PreferencesGroup::new();
    group.set_title(title);
    group.set_description(Some(description));
    for field in fields {
        let row = adw::ActionRow::builder()
            .title(field.title)
            .subtitle(field.description)
            .build();
        let dialog = gtk::ColorDialog::builder().with_alpha(true).build();
        let button = gtk::ColorDialogButton::new(Some(dialog));
        button.set_valign(gtk::Align::Center);
        button.set_tooltip_text(Some(declaration_name(field.declaration)));
        let value = style::value(field.declaration).unwrap_or_else(|| field.fallback.into());
        let color = gtk::gdk::RGBA::parse(&value)
            .or_else(|_| gtk::gdk::RGBA::parse(field.fallback))
            .expect("appearance fallback colors are valid");
        button.set_rgba(&color);
        {
            let button = button.downgrade();
            let field = *field;
            ui.add_refresher(move || {
                let Some(button) = button.upgrade() else {
                    return;
                };
                let value =
                    style::value(field.declaration).unwrap_or_else(|| field.fallback.to_string());
                let color = gtk::gdk::RGBA::parse(&value)
                    .or_else(|_| gtk::gdk::RGBA::parse(field.fallback))
                    .expect("appearance fallback colors are valid");
                button.set_rgba(&color);
            });
        }
        let declaration = field.declaration;
        let app = app.clone();
        let ui = ui.clone();
        button.connect_rgba_notify(move |button| {
            write(&app, &ui, declaration, &css_color(&button.rgba()))
        });
        row.add_suffix(&button);
        group.add(&row);
    }
    page.add(&group);
}

fn add_opacity_group(page: &adw::PreferencesPage, app: &Rc<App>, ui: &Rc<AppearanceUi>) {
    let group = adw::PreferencesGroup::new();
    group.set_title("Secondary Text");
    group.set_description(Some("Opacity of subdued zone metadata."));
    for (key, title, description) in [
        (
            "--vmux-git-clean-opacity",
            "Clean repository opacity",
            "Visibility of the check mark shown for a clean Git repository.",
        ),
        (
            "--vmux-zone-path-opacity",
            "Directory opacity",
            "Visibility of the directory name shown outside a Git repository.",
        ),
    ] {
        let current = style::value(Declaration::CustomProperty(key))
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.55);
        let adjustment = gtk::Adjustment::new(current * 100.0, 0.0, 100.0, 5.0, 10.0, 0.0);
        let row = adw::SpinRow::builder()
            .title(title)
            .subtitle(description)
            .adjustment(&adjustment)
            .digits(0)
            .build();
        {
            let adjustment = adjustment.downgrade();
            ui.add_refresher(move || {
                let Some(adjustment) = adjustment.upgrade() else {
                    return;
                };
                let current = style::value(Declaration::CustomProperty(key))
                    .and_then(|value| value.parse::<f64>().ok())
                    .unwrap_or(0.55);
                adjustment.set_value(current * 100.0);
            });
        }
        let app = app.clone();
        let ui = ui.clone();
        adjustment.connect_value_changed(move |adjustment| {
            write(
                &app,
                &ui,
                Declaration::CustomProperty(key),
                &format!("{:.2}", adjustment.value() / 100.0),
            );
        });
        group.add(&row);
    }
    page.add(&group);
}

fn add_stylesheet_group(page: &adw::PreferencesPage) {
    let group = adw::PreferencesGroup::new();
    group.set_title("Advanced");
    group.set_description(Some(
        "Open the source stylesheet for selectors and effects beyond the mapped settings above.",
    ));
    let css_path = style::path().display().to_string();
    let row = adw::ActionRow::builder()
        .title("style.css")
        .subtitle(&css_path)
        .activatable(true)
        .build();
    let open = gtk::Button::with_label("Open");
    open.set_valign(gtk::Align::Center);
    open.add_css_class("flat");
    let launch = move |widget: &gtk::Widget| {
        let _ = style::load();
        let file = gtk::gio::File::for_path(style::path());
        let launcher = gtk::FileLauncher::new(Some(&file));
        let parent = widget.root().and_downcast::<gtk::Window>();
        launcher.launch(parent.as_ref(), gtk::gio::Cancellable::NONE, |result| {
            if let Err(error) = result {
                eprintln!("vmux: cannot open style.css: {error}");
            }
        });
    };
    open.connect_clicked(move |button| launch(button.upcast_ref()));
    row.connect_activated(move |row| launch(row.upcast_ref()));
    row.add_suffix(&open);
    group.add(&row);
    page.add(&group);
}

fn save_theme_dialog(app: &Rc<App>, ui: &Rc<AppearanceUi>) {
    let name = gtk::Entry::builder()
        .placeholder_text("Theme name")
        .activates_default(true)
        .max_length(80)
        .build();
    let dialog = adw::AlertDialog::new(
        Some("Save as Theme"),
        Some(
            "Name this snapshot of the complete active stylesheet. Saving an existing name replaces that theme.",
        ),
    );
    dialog.set_extra_child(Some(&name));
    dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save Theme")]);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    {
        let app = app.clone();
        let ui = ui.clone();
        dialog.connect_response(Some("save"), move |_, _| {
            match style::save_theme(name.text().as_str()) {
                Ok(_) => {
                    if let Err(error) = ui.refresh_themes() {
                        show_theme_error(&app, "Theme Saved, but List Could Not Refresh", &error);
                    }
                }
                Err(error) => show_theme_error(&app, "Could Not Save Theme", &error),
            }
        });
    }
    dialog.present(Some(&app.window));
}

fn show_theme_error(app: &App, title: &str, error: &dyn std::fmt::Display) {
    let dialog = adw::AlertDialog::new(Some(title), Some(&error.to_string()));
    dialog.add_responses(&[("close", "Close")]);
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present(Some(&app.window));
}

fn write(app: &App, ui: &AppearanceUi, declaration: Declaration, value: &str) {
    if ui.syncing.get() {
        return;
    }
    if let Err(error) = style::set_value(declaration, value) {
        eprintln!(
            "vmux: cannot update {} in style.css: {error}",
            declaration_name(declaration)
        );
    } else {
        app.reload_user_css();
        ui.mark_custom();
    }
}

fn declaration_name(declaration: Declaration) -> &'static str {
    match declaration {
        Declaration::NamedColor(name)
        | Declaration::CustomProperty(name)
        | Declaration::TerminalProperty(name) => name,
    }
}

fn css_color(color: &gtk::gdk::RGBA) -> String {
    let channel = |value: f32| (value * 255.0).round() as u8;
    if color.alpha() >= 0.999 {
        format!(
            "#{:02x}{:02x}{:02x}",
            channel(color.red()),
            channel(color.green()),
            channel(color.blue())
        )
    } else {
        let alpha = format!("{:.3}", color.alpha());
        format!(
            "rgba({}, {}, {}, {})",
            channel(color.red()),
            channel(color.green()),
            channel(color.blue()),
            alpha.trim_end_matches('0').trim_end_matches('.')
        )
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_color_control_is_unique_and_backed_by_the_template() {
        let groups = [
            TERMINAL_BASE,
            ANSI_COLORS,
            ACCENT_WINDOW,
            HEADER_SIDEBARS,
            SURFACES,
            SEMANTIC,
            TABS_SESSIONS,
            GIT_COLORS,
        ];
        let mut names = HashSet::new();
        for field in groups.into_iter().flatten() {
            let name = declaration_name(field.declaration);
            assert!(names.insert(name), "duplicate appearance field: {name}");
            assert!(style::TEMPLATE.contains(name), "template is missing {name}");
        }
        assert_eq!(names.len(), 68);
        for name in [
            "font-family",
            "font-size",
            "--vmux-git-clean-opacity",
            "--vmux-zone-path-opacity",
        ] {
            assert!(style::TEMPLATE.contains(name));
        }
    }
}
