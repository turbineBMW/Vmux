//! The Ask Agent popup: a request box whose contents go to the user's own
//! coding agent (see [`vmux::ask`]), started next to the terminal they were
//! in. The agent picker and the "include what's on screen" switch are the
//! same settings Preferences shows, so a change here sticks. Enter sends,
//! Shift+Enter starts a new line, Escape closes and puts focus back.

use crate::app::App;
use gtk::gdk;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use vmux::ask::{self, AskAgent};
use vte4 as vte;

pub fn present(app: &Rc<App>) {
    let source = app.focused_terminal();
    let cwd = source
        .as_ref()
        .and_then(crate::term::cwd_of)
        .or_else(|| app.active_zone().map(|z| z.cwd.clone()));
    // Read now: the screen as it was when the user decided to ask.
    let screen = source.as_ref().and_then(recent_output);
    let sent = Rc::new(Cell::new(false));

    let window = adw::Window::new();
    window.set_title(Some("Ask Agent"));
    window.set_default_size(560, -1);
    window.set_modal(true);
    window.set_transient_for(Some(&app.window));

    let labels: Vec<&str> = AskAgent::ALL.iter().map(|a| a.label()).collect();
    let picker = gtk::DropDown::from_strings(&labels);
    let current = app.config.borrow().ask_agent;
    picker.set_selected(
        AskAgent::ALL
            .iter()
            .position(|a| *a == current)
            .unwrap_or(0) as u32,
    );
    picker.set_tooltip_text(Some("The agent to start"));
    {
        let app = app.clone();
        picker.connect_selected_notify(move |p| {
            if let Some(&agent) = AskAgent::ALL.get(p.selected() as usize) {
                app.config.borrow_mut().ask_agent = agent;
                app.schedule_save();
            }
        });
    }
    let place = gtk::Label::new(
        cwd.as_deref()
            .map(|c| format!("in {}", tilde(c)))
            .as_deref(),
    );
    place.add_css_class("dim-label");
    place.set_xalign(0.0);
    place.set_hexpand(true);
    place.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    header.append(&picker);
    header.append(&place);

    let input = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(10)
        .right_margin(10)
        .build();
    input.add_css_class("ask-agent-input");
    let placeholder = gtk::Label::new(Some("What are you trying to do?"));
    placeholder.add_css_class("dim-label");
    placeholder.set_halign(gtk::Align::Start);
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_margin_top(8);
    placeholder.set_margin_start(10);
    placeholder.set_can_target(false);
    {
        let placeholder = placeholder.clone();
        input
            .buffer()
            .connect_changed(move |b| placeholder.set_visible(b.char_count() == 0));
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(96)
        .max_content_height(280)
        .propagate_natural_height(true)
        .child(&input)
        .build();
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&scroll));
    overlay.add_overlay(&placeholder);
    let frame = gtk::Frame::new(None);
    frame.set_child(Some(&overlay));

    let include = gtk::CheckButton::with_label("Include what's on screen");
    include.set_active(app.config.borrow().ask_agent_screen);
    include.set_sensitive(screen.is_some());
    include.set_tooltip_text(Some(
        "Send the terminal's recent output along with the request",
    ));
    {
        let app = app.clone();
        include.connect_toggled(move |c| {
            app.config.borrow_mut().ask_agent_screen = c.is_active();
            app.schedule_save();
        });
    }
    let hint = gtk::Label::new(Some("Enter sends · Shift+Enter for a new line"));
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    hint.set_xalign(1.0);
    hint.set_hexpand(true);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    footer.append(&include);
    footer.append(&hint);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    content.append(&header);
    content.append(&frame);
    content.append(&footer);
    window.set_content(Some(&content));

    let send = {
        let app = app.clone();
        let window = window.clone();
        let input = input.clone();
        let include = include.clone();
        let sent = sent.clone();
        let source = source.as_ref().map(|t| t.downgrade());
        move || {
            let buffer = input.buffer();
            let request = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            if request.trim().is_empty() {
                return;
            }
            sent.set(true);
            window.close();
            let screen = screen.as_deref().filter(|_| include.is_active());
            let source: Option<vte::Terminal> = source.as_ref().and_then(|w| w.upgrade());
            app.ask_agent(source.as_ref(), &request, screen);
        }
    };
    {
        // Capture, so Enter sends before the text view inserts a newline.
        let win = window.clone();
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, keyval, _, modifier| match keyval {
            gdk::Key::Escape => {
                win.close();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter
                if !modifier.contains(gdk::ModifierType::SHIFT_MASK) =>
            {
                send();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        window.add_controller(key);
    }
    {
        // Dismissed without sending: put focus back where it was.
        let source = source.as_ref().map(|t| t.downgrade());
        window.connect_close_request(move |_| {
            if !sent.get()
                && let Some(t) = source.as_ref().and_then(|w| w.upgrade())
            {
                crate::term::focus_later(&t);
            }
            glib::Propagation::Proceed
        });
    }

    window.present();
    input.grab_focus();
}

/// The end of `term`'s output, up to and including the cursor's line.
fn recent_output(term: &vte::Terminal) -> Option<String> {
    use vte4::prelude::*;
    let (_, cursor) = term.cursor_position();
    let text = term
        .text_range_format(
            vte::Format::Text,
            (cursor - ask::SCREEN_MAX_LINES as i64).max(0),
            0,
            cursor,
            term.column_count(),
        )
        .0?;
    ask::screen_tail(&text, ask::SCREEN_MAX_LINES, ask::SCREEN_MAX_CHARS)
}

/// `path` with the home directory shown as `~`.
fn tilde(path: &str) -> String {
    let home = crate::state::home_dir();
    match path.strip_prefix(&home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}
