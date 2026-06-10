use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::state;

pub struct OpenPathDialogInput {
    pub parent: gtk::Window,
    pub initial_directory: PathBuf,
    pub on_open: Rc<dyn Fn(PathBuf)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PathInputState {
    resolved_path: PathBuf,
    valid_directory: bool,
    helper_text: String,
    error_text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SuggestionSession {
    input_prefix: String,
    suggestions: Vec<String>,
    selected_index: Option<usize>,
}

#[derive(Default)]
struct OpenPathDialogState {
    suggestion_session: SuggestionSession,
    suppress_entry_change: bool,
    suggestions_dismissed: bool,
}

pub fn present_open_path_dialog(input: OpenPathDialogInput) {
    let window = adw::Window::new();
    window.set_title(Some("New Zone"));
    window.set_default_size(720, 320);
    window.set_modal(true);
    window.set_transient_for(Some(&input.parent));
    if let Some(app) = input.parent.application() {
        window.set_application(Some(&app));
    }

    let entry = gtk::Entry::builder().hexpand(true).build();
    entry.set_text(&display_path_input(&input.initial_directory));

    let suggestions_list = gtk::ListBox::builder()
        .activate_on_single_click(true)
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    suggestions_list.add_css_class("boxed-list");

    let suggestions_scroll = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(180)
        .child(&suggestions_list)
        .build();

    let helper_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .xalign(0.0)
        .build();
    helper_label.add_css_class("dim-label");

    let error_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .xalign(0.0)
        .visible(false)
        .build();
    error_label.add_css_class("error");

    let cancel_button = gtk::Button::with_label("Cancel");
    let open_button = gtk::Button::with_label("Create");
    open_button.add_css_class("suggested-action");

    let button_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::End)
        .build();
    button_box.append(&cancel_button);
    button_box.append(&open_button);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    content.append(
        &gtk::Label::builder()
            .label("Type a directory path. Arrow keys or click select a directory, / descends, Enter accepts a selection, then Enter creates the zone.")
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .build(),
    );
    content.append(&entry);
    content.append(&suggestions_scroll);
    content.append(&helper_label);
    content.append(&error_label);
    content.append(&button_box);
    window.set_content(Some(&content));

    let dialog_state = Rc::new(RefCell::new(OpenPathDialogState::default()));
    rebuild_session_from_entry(
        &entry,
        &suggestions_list,
        &suggestions_scroll,
        &helper_label,
        &error_label,
        &open_button,
        &dialog_state,
        &input.initial_directory,
    );

    {
        let suggestions_list = suggestions_list.clone();
        let suggestions_scroll = suggestions_scroll.clone();
        let helper_label = helper_label.clone();
        let error_label = error_label.clone();
        let open_button = open_button.clone();
        let dialog_state = dialog_state.clone();
        let initial_directory = input.initial_directory.clone();
        entry.connect_changed(move |entry| {
            if dialog_state.borrow().suppress_entry_change {
                return;
            }

            rebuild_session_from_entry(
                entry,
                &suggestions_list,
                &suggestions_scroll,
                &helper_label,
                &error_label,
                &open_button,
                &dialog_state,
                &initial_directory,
            );
        });
    }

    {
        let entry = entry.clone();
        let suggestions_list = suggestions_list.clone();
        let suggestions_scroll = suggestions_scroll.clone();
        let helper_label = helper_label.clone();
        let error_label = error_label.clone();
        let open_button = open_button.clone();
        let dialog_state = dialog_state.clone();
        let initial_directory = input.initial_directory.clone();
        let key = gtk::EventControllerKey::new();
        let entry_for_key = entry.clone();
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
                    move_suggestion_selection(
                        &entry_for_key,
                        &suggestions_list,
                        &suggestions_scroll,
                        &helper_label,
                        &error_label,
                        &open_button,
                        &dialog_state,
                        &initial_directory,
                        -1,
                    );
                    glib::Propagation::Stop
                }
                gdk::Key::Down => {
                    move_suggestion_selection(
                        &entry_for_key,
                        &suggestions_list,
                        &suggestions_scroll,
                        &helper_label,
                        &error_label,
                        &open_button,
                        &dialog_state,
                        &initial_directory,
                        1,
                    );
                    glib::Propagation::Stop
                }
                gdk::Key::slash => {
                    if append_separator_for_selection(
                        &entry_for_key,
                        &suggestions_list,
                        &suggestions_scroll,
                        &helper_label,
                        &error_label,
                        &open_button,
                        &dialog_state,
                        &initial_directory,
                    ) {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    if dismiss_suggestions_for_enter(
                        &entry_for_key,
                        &suggestions_list,
                        &suggestions_scroll,
                        &helper_label,
                        &error_label,
                        &open_button,
                        &dialog_state,
                        &initial_directory,
                    ) {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                _ => glib::Propagation::Proceed,
            }
        });
        entry.add_controller(key);
    }

    {
        let window_for_key = window.clone();
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
                gdk::Key::Escape => {
                    window_for_key.close();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        window.add_controller(key);
    }

    {
        let entry = entry.clone();
        let suggestions_list = suggestions_list.clone();
        let suggestions_list_for_signal = suggestions_list.clone();
        let suggestions_scroll = suggestions_scroll.clone();
        let helper_label = helper_label.clone();
        let error_label = error_label.clone();
        let open_button = open_button.clone();
        let dialog_state = dialog_state.clone();
        let initial_directory = input.initial_directory.clone();
        suggestions_list_for_signal.connect_row_activated(move |_, row| {
            let Ok(index) = usize::try_from(row.index()) else {
                return;
            };
            commit_suggestion_selection(
                &entry,
                &suggestions_list,
                &suggestions_scroll,
                &helper_label,
                &error_label,
                &open_button,
                &dialog_state,
                &initial_directory,
                index,
            );
        });
    }

    {
        let window = window.clone();
        cancel_button.connect_clicked(move |_| {
            window.close();
        });
    }

    {
        let window = window.clone();
        let entry = entry.clone();
        let initial_directory = input.initial_directory.clone();
        let on_open = input.on_open.clone();
        open_button.connect_clicked(move |_| {
            if let Some(path) = open_directory_target(entry.text().as_str(), &initial_directory) {
                on_open(path);
                window.close();
            }
        });
    }

    {
        let window = window.clone();
        let entry = entry.clone();
        let initial_directory = input.initial_directory.clone();
        let on_open = input.on_open.clone();
        entry.connect_activate(move |entry| {
            if let Some(path) = open_directory_target(entry.text().as_str(), &initial_directory) {
                on_open(path);
                window.close();
            }
        });
    }

    window.present();
    entry.grab_focus();
    schedule_cursor_to_end(&entry);
}

#[allow(clippy::too_many_arguments)]
fn rebuild_session_from_entry(
    entry: &gtk::Entry,
    suggestions_list: &gtk::ListBox,
    suggestions_scroll: &gtk::ScrolledWindow,
    helper_label: &gtk::Label,
    error_label: &gtk::Label,
    open_button: &gtk::Button,
    dialog_state: &Rc<RefCell<OpenPathDialogState>>,
    initial_directory: &Path,
) {
    let mut state = dialog_state.borrow_mut();
    state.suggestion_session = build_suggestion_session(entry.text().as_str(), initial_directory);
    state.suggestions_dismissed = false;
    drop(state);
    sync_path_input_ui(
        entry,
        suggestions_list,
        suggestions_scroll,
        helper_label,
        error_label,
        open_button,
        dialog_state,
        initial_directory,
    );
}

#[allow(clippy::too_many_arguments)]
fn sync_path_input_ui(
    entry: &gtk::Entry,
    suggestions_list: &gtk::ListBox,
    suggestions_scroll: &gtk::ScrolledWindow,
    helper_label: &gtk::Label,
    error_label: &gtk::Label,
    open_button: &gtk::Button,
    dialog_state: &Rc<RefCell<OpenPathDialogState>>,
    initial_directory: &Path,
) {
    let (session, suggestions_visible) = {
        let state = dialog_state.borrow();
        let session = state.suggestion_session.clone();
        let visible = !state.suggestions_dismissed && !session.suggestions.is_empty();
        (session, visible)
    };
    let state = analyze_path_input(
        entry.text().as_str(),
        initial_directory,
        suggestions_visible,
        session.selected_index.is_some(),
    );
    sync_suggestions_list(
        suggestions_list,
        suggestions_scroll,
        if suggestions_visible {
            Some(&session)
        } else {
            None
        },
    );
    helper_label.set_text(&state.helper_text);
    open_button.set_sensitive(state.valid_directory);
    if let Some(error_text) = state.error_text {
        error_label.set_text(&error_text);
        error_label.set_visible(true);
    } else {
        error_label.set_text("");
        error_label.set_visible(false);
    }
}

#[allow(clippy::too_many_arguments)]
fn move_suggestion_selection(
    entry: &gtk::Entry,
    suggestions_list: &gtk::ListBox,
    suggestions_scroll: &gtk::ScrolledWindow,
    helper_label: &gtk::Label,
    error_label: &gtk::Label,
    open_button: &gtk::Button,
    dialog_state: &Rc<RefCell<OpenPathDialogState>>,
    initial_directory: &Path,
    delta: isize,
) {
    if dialog_state.borrow().suggestions_dismissed {
        return;
    }

    let next_index = {
        let state = dialog_state.borrow();
        let suggestions = &state.suggestion_session.suggestions;
        if suggestions.is_empty() {
            return;
        }

        let len = suggestions.len();
        match (state.suggestion_session.selected_index, delta.is_negative()) {
            (Some(index), true) if index > 0 => index - 1,
            (Some(0), true) => len - 1,
            (Some(index), false) if index + 1 < len => index + 1,
            (Some(_), false) => 0,
            (None, true) => len - 1,
            (None, false) => 0,
            _ => 0,
        }
    };

    commit_suggestion_selection(
        entry,
        suggestions_list,
        suggestions_scroll,
        helper_label,
        error_label,
        open_button,
        dialog_state,
        initial_directory,
        next_index,
    );
}

#[allow(clippy::too_many_arguments)]
fn dismiss_suggestions_for_enter(
    entry: &gtk::Entry,
    suggestions_list: &gtk::ListBox,
    suggestions_scroll: &gtk::ScrolledWindow,
    helper_label: &gtk::Label,
    error_label: &gtk::Label,
    open_button: &gtk::Button,
    dialog_state: &Rc<RefCell<OpenPathDialogState>>,
    initial_directory: &Path,
) -> bool {
    let should_dismiss = {
        let state = dialog_state.borrow();
        !state.suggestions_dismissed && state.suggestion_session.selected_index.is_some()
    };
    if !should_dismiss {
        return false;
    }

    dialog_state.borrow_mut().suggestions_dismissed = true;
    sync_path_input_ui(
        entry,
        suggestions_list,
        suggestions_scroll,
        helper_label,
        error_label,
        open_button,
        dialog_state,
        initial_directory,
    );
    true
}

#[allow(clippy::too_many_arguments)]
fn commit_suggestion_selection(
    entry: &gtk::Entry,
    suggestions_list: &gtk::ListBox,
    suggestions_scroll: &gtk::ScrolledWindow,
    helper_label: &gtk::Label,
    error_label: &gtk::Label,
    open_button: &gtk::Button,
    dialog_state: &Rc<RefCell<OpenPathDialogState>>,
    initial_directory: &Path,
    index: usize,
) {
    let Some(committed) = selected_suggestion_text(dialog_state, index) else {
        return;
    };

    dialog_state.borrow_mut().suggestion_session.selected_index = Some(index);
    set_entry_text(entry, dialog_state, &committed);
    entry.grab_focus();
    schedule_cursor_to_end(entry);
    sync_path_input_ui(
        entry,
        suggestions_list,
        suggestions_scroll,
        helper_label,
        error_label,
        open_button,
        dialog_state,
        initial_directory,
    );
}

#[allow(clippy::too_many_arguments)]
fn append_separator_for_selection(
    entry: &gtk::Entry,
    suggestions_list: &gtk::ListBox,
    suggestions_scroll: &gtk::ScrolledWindow,
    helper_label: &gtk::Label,
    error_label: &gtk::Label,
    open_button: &gtk::Button,
    dialog_state: &Rc<RefCell<OpenPathDialogState>>,
    initial_directory: &Path,
) -> bool {
    let selected_index = dialog_state.borrow().suggestion_session.selected_index;
    let Some(selected_index) = selected_index else {
        return false;
    };
    let Some(committed) = selected_suggestion_text(dialog_state, selected_index) else {
        return false;
    };

    let next_text = if committed.ends_with('/') {
        committed
    } else {
        format!("{committed}/")
    };
    set_entry_text(entry, dialog_state, &next_text);
    entry.grab_focus();
    schedule_cursor_to_end(entry);
    rebuild_session_from_entry(
        entry,
        suggestions_list,
        suggestions_scroll,
        helper_label,
        error_label,
        open_button,
        dialog_state,
        initial_directory,
    );
    true
}

fn sync_suggestions_list(
    suggestions_list: &gtk::ListBox,
    suggestions_scroll: &gtk::ScrolledWindow,
    session: Option<&SuggestionSession>,
) {
    while let Some(child) = suggestions_list.first_child() {
        suggestions_list.remove(&child);
    }

    if let Some(session) = session {
        for suggestion in &session.suggestions {
            let label = gtk::Label::builder()
                .label(suggestion)
                .xalign(0.0)
                .margin_top(8)
                .margin_bottom(8)
                .margin_start(10)
                .margin_end(10)
                .build();
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&label));
            suggestions_list.append(&row);
        }

        suggestions_scroll.set_visible(!session.suggestions.is_empty());
        if let Some(index) = session.selected_index {
            if let Ok(index) = i32::try_from(index) {
                if let Some(row) = suggestions_list.row_at_index(index) {
                    suggestions_list.select_row(Some(&row));
                } else {
                    suggestions_list.unselect_all();
                }
            } else {
                suggestions_list.unselect_all();
            }
        } else {
            suggestions_list.unselect_all();
        }
    } else {
        suggestions_scroll.set_visible(false);
        suggestions_list.unselect_all();
    }
}

fn set_entry_text(entry: &gtk::Entry, dialog_state: &Rc<RefCell<OpenPathDialogState>>, text: &str) {
    dialog_state.borrow_mut().suppress_entry_change = true;
    entry.set_text(text);
    dialog_state.borrow_mut().suppress_entry_change = false;
    schedule_cursor_to_end(entry);
}

fn schedule_cursor_to_end(entry: &gtk::Entry) {
    let entry = entry.clone();
    glib::idle_add_local_once(move || {
        entry.set_position(entry.text_length() as i32);
    });
}

fn open_directory_target(input: &str, initial_directory: &Path) -> Option<PathBuf> {
    let state = analyze_path_input(input, initial_directory, false, false);
    state.valid_directory.then_some(state.resolved_path)
}

fn analyze_path_input(
    input: &str,
    initial_directory: &Path,
    suggestions_visible: bool,
    selection_active: bool,
) -> PathInputState {
    let resolved_path = resolve_input_path(input, initial_directory);
    let valid_directory = resolved_path.is_dir();
    let helper_text = if suggestions_visible && selection_active {
        "Press Enter to accept the selected directory. Press Enter again to create the zone."
            .to_string()
    } else if valid_directory && !input.trim().ends_with('/') {
        format!(
            "Press Enter to create a zone in {}. Type / to browse child directories.",
            resolved_path.display()
        )
    } else if suggestions_visible {
        "Matching directories shown below. Arrow keys or click fills the current segment."
            .to_string()
    } else if valid_directory {
        format!("Create a zone in {}", resolved_path.display())
    } else {
        "Type a directory path".to_string()
    };

    let error_text = if valid_directory {
        None
    } else if resolved_path.exists() {
        Some("Path is not a directory".to_string())
    } else {
        Some("Directory does not exist".to_string())
    };

    PathInputState {
        resolved_path,
        valid_directory,
        helper_text,
        error_text,
    }
}

fn display_path_input(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if raw == "/" {
        raw.to_string()
    } else {
        format!("{raw}/")
    }
}

fn resolve_input_path(input: &str, initial_directory: &Path) -> PathBuf {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return initial_directory.to_path_buf();
    }
    let expanded = PathBuf::from(state::expand_tilde(trimmed));
    if expanded.is_absolute() {
        expanded
    } else {
        initial_directory.join(expanded)
    }
}

fn build_suggestion_session(input: &str, initial_directory: &Path) -> SuggestionSession {
    let Some((browse_dir, input_prefix, segment_prefix)) =
        suggestion_parent_and_prefix(input, initial_directory)
    else {
        return SuggestionSession::default();
    };

    SuggestionSession {
        input_prefix,
        suggestions: read_matching_directories(&browse_dir, &segment_prefix),
        selected_index: None,
    }
}

fn selected_suggestion_text(
    dialog_state: &Rc<RefCell<OpenPathDialogState>>,
    index: usize,
) -> Option<String> {
    let state = dialog_state.borrow();
    let suggestion = state.suggestion_session.suggestions.get(index)?;
    Some(format!(
        "{}{}",
        state.suggestion_session.input_prefix, suggestion
    ))
}

fn suggestion_parent_and_prefix(
    input: &str,
    initial_directory: &Path,
) -> Option<(PathBuf, String, String)> {
    let trimmed = input.trim();
    if trimmed == "~" {
        return None;
    }

    if trimmed.ends_with('/') {
        return Some((
            resolve_input_path(trimmed, initial_directory),
            trimmed.to_string(),
            String::new(),
        ));
    }

    let separator = trimmed.rfind('/').map_or(0, |index| index + 1);
    let input_prefix = trimmed[..separator].to_string();
    let segment_prefix = trimmed[separator..].to_string();
    let parent = if input_prefix.is_empty() {
        initial_directory.to_path_buf()
    } else {
        resolve_input_path(&input_prefix, initial_directory)
    };

    Some((parent, input_prefix, segment_prefix))
}

fn read_matching_directories(parent: &Path, prefix: &str) -> Vec<String> {
    let show_hidden = prefix.starts_with('.');
    let mut matches = fs::read_dir(parent)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            name.starts_with(prefix).then_some(name)
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn resolve_input_path_expands_home_and_relative_paths() {
        let base = Path::new("/tmp/work");
        let home = PathBuf::from(state::home_dir());

        assert_eq!(
            resolve_input_path("project", base),
            Path::new("/tmp/work/project")
        );
        assert_eq!(resolve_input_path("~/Code", base), home.join("Code"));
    }

    #[test]
    fn build_suggestion_session_filters_current_segment() {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir_all(dir.path().join("FieldHook-a")).unwrap();
        fs::create_dir_all(dir.path().join("FieldHook-b")).unwrap();

        let session = build_suggestion_session(&format!("{}/F", dir.path().display()), dir.path());

        assert_eq!(session.input_prefix, format!("{}/", dir.path().display()));
        assert_eq!(
            session.suggestions,
            vec!["FieldHook-a".to_string(), "FieldHook-b".to_string()]
        );
    }

    #[test]
    fn build_suggestion_session_lists_children_for_trailing_separator() {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir_all(dir.path().join("Code")).unwrap();
        fs::create_dir_all(dir.path().join("Docs")).unwrap();

        let session = build_suggestion_session(&format!("{}/", dir.path().display()), dir.path());

        assert_eq!(session.input_prefix, format!("{}/", dir.path().display()));
        assert_eq!(
            session.suggestions,
            vec!["Code".to_string(), "Docs".to_string()]
        );
    }

    #[test]
    fn read_matching_directories_hides_dot_directories_without_dot_prefix() {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir_all(dir.path().join(".cache")).unwrap();
        fs::create_dir_all(dir.path().join("Code")).unwrap();

        assert_eq!(
            read_matching_directories(dir.path(), "C"),
            vec!["Code".to_string()]
        );
        assert_eq!(
            read_matching_directories(dir.path(), ".c"),
            vec![".cache".to_string()]
        );
    }

    #[test]
    fn analyze_path_input_marks_existing_directories_as_openable() {
        let dir = TempDir::new().expect("temp dir");
        let state = analyze_path_input(
            &format!("{}/", dir.path().display()),
            dir.path(),
            false,
            false,
        );

        assert!(state.valid_directory);
        assert!(state.error_text.is_none());
    }

    #[test]
    fn analyze_path_input_prompts_to_accept_selected_suggestion_before_opening() {
        let dir = TempDir::new().expect("temp dir");
        let state = analyze_path_input(
            dir.path().to_string_lossy().as_ref(),
            dir.path(),
            true,
            true,
        );

        assert_eq!(
            state.helper_text,
            "Press Enter to accept the selected directory. Press Enter again to create the zone."
        );
    }

    #[test]
    fn selected_suggestion_text_replaces_only_current_segment() {
        let dialog_state = Rc::new(RefCell::new(OpenPathDialogState {
            suggestion_session: SuggestionSession {
                input_prefix: "projects/".to_string(),
                suggestions: vec!["vmux".to_string(), "other".to_string()],
                selected_index: None,
            },
            suppress_entry_change: false,
            suggestions_dismissed: false,
        }));

        assert_eq!(
            selected_suggestion_text(&dialog_state, 0).as_deref(),
            Some("projects/vmux")
        );
    }

    #[test]
    fn suggestion_parent_and_prefix_supports_root_and_relative_inputs() {
        let initial_directory = Path::new("/tmp/work");

        assert_eq!(
            suggestion_parent_and_prefix("/usr/lo", initial_directory),
            Some((PathBuf::from("/usr"), "/usr/".to_string(), "lo".to_string()))
        );
        assert_eq!(
            suggestion_parent_and_prefix("projects/li", initial_directory),
            Some((
                PathBuf::from("/tmp/work/projects"),
                "projects/".to_string(),
                "li".to_string()
            ))
        );
    }
}
