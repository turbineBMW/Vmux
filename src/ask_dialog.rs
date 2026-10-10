//! The Ask Agent popup: a request box whose contents go to the user's own
//! coding agent (see [`vmux::ask`]). In Command mode the agent is asked for
//! one shell command, which is pasted at the prompt of the terminal the user
//! was in, unrun; the popup waits with a spinner meanwhile. In Agent mode the
//! agent starts next to that terminal to work on the request. The agent
//! picker and the "include what's on screen" switch are the same settings
//! Preferences shows, so a change here sticks. Enter sends, Shift+Enter
//! starts a new line, Escape closes (stopping a pending answer) and puts
//! focus back.

use crate::app::App;
use gtk::gdk;
use gtk4 as gtk;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use vmux::ask::{self, AskAgent, AskMode};
use vte4 as vte;
use vte4::prelude::*;

/// The most directory entries the command prompt lists.
const MAX_FILES: usize = 60;

pub fn present(app: &Rc<App>, mode: AskMode) {
    let source = app.focused_terminal();
    let cwd = source
        .as_ref()
        .and_then(crate::term::cwd_of)
        .or_else(|| app.active_zone().map(|z| z.cwd.clone()));
    // Read now: the screen as it was when the user decided to ask.
    let screen = source.as_ref().and_then(recent_output);
    let sent = Rc::new(Cell::new(false));
    let mode = Rc::new(Cell::new(mode));
    // The process group of a pending command answer, to stop on close.
    let pending: Rc<Cell<Option<i32>>> = Rc::default();
    let closed = Rc::new(Cell::new(false));

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
    let command_btn = gtk::ToggleButton::with_label("Command");
    command_btn.set_tooltip_text(Some(
        "Get a shell command at your prompt, to review and run",
    ));
    let agent_btn = gtk::ToggleButton::with_label("Agent");
    agent_btn.set_tooltip_text(Some("Start the agent next to this terminal to work on it"));
    agent_btn.set_group(Some(&command_btn));
    let modes = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    modes.add_css_class("linked");
    modes.append(&command_btn);
    modes.append(&agent_btn);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    header.append(&modes);
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
    let placeholder = gtk::Label::new(None);
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
    let hint = gtk::Label::new(None);
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    hint.set_xalign(1.0);
    hint.set_hexpand(true);
    let spinner = adw::Spinner::new();
    spinner.set_visible(false);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    footer.append(&include);
    footer.append(&hint);
    footer.append(&spinner);
    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.set_selectable(true);
    error.set_visible(false);

    let sync_mode = {
        let placeholder = placeholder.clone();
        let hint = hint.clone();
        let error = error.clone();
        let mode = mode.clone();
        move || {
            let (text, keys) = match mode.get() {
                AskMode::Command => (
                    "What command do you need?",
                    "Enter gets the command · Shift+Enter for a new line",
                ),
                AskMode::Agent => (
                    "What are you trying to do?",
                    "Enter starts the agent · Shift+Enter for a new line",
                ),
            };
            placeholder.set_label(text);
            hint.set_label(keys);
            error.set_visible(false);
        }
    };
    sync_mode();
    match mode.get() {
        AskMode::Command => command_btn.set_active(true),
        AskMode::Agent => agent_btn.set_active(true),
    }
    for (btn, m) in [
        (&command_btn, AskMode::Command),
        (&agent_btn, AskMode::Agent),
    ] {
        let mode = mode.clone();
        let sync_mode = sync_mode.clone();
        let input = input.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                mode.set(m);
                sync_mode();
                input.grab_focus();
            }
        });
    }

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
    content.append(&error);
    window.set_content(Some(&content));

    // Waiting on a command answer: the request can't change meanwhile.
    let set_busy = {
        let input = input.clone();
        let header = header.clone();
        let include = include.clone();
        let hint = hint.clone();
        let spinner = spinner.clone();
        let sync_mode = sync_mode.clone();
        let has_screen = screen.is_some();
        move |busy: bool, agent: AskAgent| {
            input.set_sensitive(!busy);
            header.set_sensitive(!busy);
            include.set_sensitive(!busy && has_screen);
            spinner.set_visible(busy);
            if busy {
                hint.set_label(&format!("Asking {}…", agent.label()));
            } else {
                sync_mode();
                input.grab_focus();
            }
        }
    };

    let send = {
        let app = app.clone();
        let window = window.clone();
        let input = input.clone();
        let include = include.clone();
        let sent = sent.clone();
        let source = source.as_ref().map(|t| t.downgrade());
        let pending = pending.clone();
        let closed = closed.clone();
        let error = error.clone();
        let mode = mode.clone();
        move || {
            let buffer = input.buffer();
            let request = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            if request.trim().is_empty() || pending.get().is_some() {
                return;
            }
            let screen = screen.as_deref().filter(|_| include.is_active());
            let source: Option<vte::Terminal> = source.as_ref().and_then(|w| w.upgrade());
            if mode.get() == AskMode::Agent {
                sent.set(true);
                window.close();
                app.ask_agent(source.as_ref(), &request, screen);
                return;
            }
            let show_error = {
                let error = error.clone();
                move |text: &str| {
                    error.set_label(text);
                    error.set_visible(true);
                }
            };
            let Some(source) = source else {
                show_error("There's no terminal to put the command in.");
                return;
            };
            let (agent, shell) = {
                let cfg = app.config.borrow();
                (cfg.ask_agent, crate::state::default_shell(&cfg))
            };
            let shell = Path::new(&shell)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(shell);
            let os = ask::os_name();
            let files = cwd
                .as_deref()
                .map(|c| ask::list_dir(Path::new(c), MAX_FILES))
                .unwrap_or_default();
            let prompt = ask::command_prompt(
                &request,
                &ask::CommandContext {
                    shell: &shell,
                    os: os.as_deref(),
                    cwd: cwd.as_deref(),
                    files: &files,
                    screen,
                },
            );
            let (pgid, answer) = match spawn_answer(agent, prompt, cwd.as_deref()) {
                Ok(started) => started,
                Err(e) => {
                    show_error(&e);
                    return;
                }
            };
            pending.set(Some(pgid));
            error.set_visible(false);
            set_busy(true, agent);
            let window = window.clone();
            let sent = sent.clone();
            let pending = pending.clone();
            let closed = closed.clone();
            let set_busy = set_busy.clone();
            glib::spawn_future_local(async move {
                let answer = answer.await;
                pending.set(None);
                if closed.get() {
                    return;
                }
                set_busy(false, agent);
                match answer {
                    // The agent explaining why there's no command for it.
                    Ok(cmd) if cmd.starts_with('#') && !cmd.contains('\n') => {
                        show_error(cmd.trim_start_matches('#').trim());
                    }
                    Ok(cmd) => {
                        sent.set(true);
                        window.close();
                        source.paste_text(&cmd);
                        crate::term::focus_later(&source);
                    }
                    Err(e) => show_error(&e),
                }
            });
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
        // Dismissed without sending: stop a pending answer and put focus
        // back where it was.
        let source = source.as_ref().map(|t| t.downgrade());
        window.connect_close_request(move |_| {
            closed.set(true);
            if let Some(pgid) = pending.take() {
                // SAFETY: plain kill(2) on the answer's own process group.
                unsafe { libc::kill(-pgid, libc::SIGTERM) };
            }
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

/// Ask `agent` for a command, one-shot, in `cwd`. Returns the answer's
/// process group, to stop it early, and the cleaned-up command it answers
/// with, or why there's none.
///
/// Spawned with `std::process` and waited on a worker thread, as git.rs does,
/// to keep `fork()` and the wait off the UI thread.
fn spawn_answer(
    agent: AskAgent,
    prompt: String,
    cwd: Option<&str>,
) -> Result<(i32, impl Future<Output = Result<String, String>> + use<>), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    // Codex writes its answer here; a private file, swept like prompts.
    let answer_file = ask::write_prompt(&ask::prompt_dir(), "")
        .map_err(|e| format!("Couldn't make a file for the answer: {e}"))?;
    let argv = ask::command_argv(agent, &answer_file);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Not inside a vmux terminal: keeps vmux's own agent hook quiet.
        .env_remove("TERM_PROGRAM")
        .process_group(0);
    if let Some(cwd) = cwd.filter(|c| Path::new(c).is_dir()) {
        cmd.current_dir(cwd);
    }
    let mut child = cmd.spawn().map_err(|e| {
        let _ = std::fs::remove_file(&answer_file);
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("Couldn't find {} on your PATH.", argv[0])
        } else {
            format!("Couldn't start {}: {e}", argv[0])
        }
    })?;
    let pgid = child.id() as i32;
    let answer = async move {
        let out = gio::spawn_blocking(move || {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                // A failed write shows up as the agent's own error.
                let _ = stdin.write_all(prompt.as_bytes());
            }
            child.wait_with_output()
        })
        .await;
        let written = std::fs::read_to_string(&answer_file).unwrap_or_default();
        let _ = std::fs::remove_file(&answer_file);
        let out = match out {
            Ok(Ok(out)) => out,
            _ => return Err(format!("{} stopped unexpectedly.", agent.label())),
        };
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = stderr
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            let tail = tail[tail.len().saturating_sub(3)..].join("\n");
            return Err(if tail.is_empty() {
                format!("{} failed ({}).", agent.label(), out.status)
            } else {
                format!("{} failed: {tail}", agent.label())
            });
        }
        let reply = match agent {
            AskAgent::Claude => String::from_utf8_lossy(&out.stdout).into_owned(),
            AskAgent::Codex => written,
        };
        ask::clean_command(&reply).ok_or_else(|| format!("{} gave no command.", agent.label()))
    };
    Ok((pgid, answer))
}

/// The end of `term`'s output, up to and including the cursor's line.
fn recent_output(term: &vte::Terminal) -> Option<String> {
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
