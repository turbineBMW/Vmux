use crate::agent;
use crate::app::App;
use crate::zone::Zone;
use crate::{keybinds, splits, state, term};
use gtk4 as gtk;
use gtk4::{gdk, glib};
use libadwaita as adw;
use std::cell::Cell;
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
    setup_single_tab_dnd(&stack, &tab_bar, &tab_view);

    // Focus landing anywhere in this pane (tab strip, buttons) makes it the
    // zone's current pane, even though the terminal itself never got focus.
    // Terminals record themselves via their own controller; this covers the
    // rest so zone switching returns to the pane the user last used.
    {
        let zone = zone.clone();
        let tab_view = tab_view.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |c| {
            let Some(zone) = zone.upgrade() else { return };
            let focus_is_terminal = c
                .widget()
                .and_then(|w| w.root())
                .and_then(|r| r.focus())
                .is_some_and(|w| w.is::<vte::Terminal>());
            if focus_is_terminal {
                return;
            }
            if let Some(t) = tab_view
                .selected_page()
                .and_then(|page| splits::first_terminal_in(&page.child()))
            {
                if std::env::var_os("VMUX_DEBUG_FOCUS").is_some() {
                    eprintln!("pane-focus: last_focused <- {:?}", t.as_ptr());
                }
                zone.last_focused.set(Some(&t));
            }
            if let Some(w) = c.widget() {
                splits::remember_focused_pane(&w);
            }
        });
        stack.add_controller(focus);
    }

    {
        let app = app.clone();
        let zone = zone.clone();
        let stack = stack.clone();
        new_tab_btn.connect_clicked(move |_| {
            if let Some(z) = zone.upgrade() {
                let cwd = z
                    .last_focused
                    .upgrade()
                    .filter(|t| {
                        splits::pane_of(t.upcast_ref()).as_ref() == Some(stack.upcast_ref())
                    })
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
            // adw builds the tab widget in its own page-attached handler,
            // connected in set_view above and so run before this one — the
            // new tab is already there to be classed.
            refresh_tab_indicators(stack.upcast_ref());
            app.schedule_drag_sweep();
            app.schedule_save();
        });
    }
    // Whether the next page-detached is a tab close (as opposed to a drag
    // tear-off). The close-page default handler confirms synchronously, so
    // the pair never interleaves. (A close-confirmation flow, if ever added,
    // would break that and need a per-page marker instead.)
    let closing = Rc::new(Cell::new(false));
    {
        let closing = closing.clone();
        tab_view.connect_close_page(move |_, _| {
            closing.set(true);
            glib::Propagation::Proceed
        });
    }
    {
        let app = app.clone();
        let stack = stack.clone();
        let zone = zone.clone();
        tab_view.connect_page_detached(move |view, page, _| {
            if view.root().is_none() {
                return; // pane already being torn down
            }
            let was_close = closing.take(); // consume on every detach
            if view.n_pages() == 0 {
                if was_close {
                    collapse_empty_pane(&app, &stack, &zone);
                } else {
                    // Drag tear-off: fires as soon as the tab leaves the tab
                    // bar, mid-drag. The pane must survive as a drop target
                    // (cancel and drop-back re-attach the page here); the
                    // sweep collapses it once the drag concludes.
                    app.pane_emptied_by_drag(&stack, &zone, page);
                    return; // no save: the layout is transient mid-drag
                }
            } else if let Some(sel) = view.selected_page()
                && let Some(t) = splits::first_terminal_in(&sel.child())
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

/// Payload of a vmux-initiated last-tab drag (see [`setup_single_tab_dnd`]).
struct LastTabDrag {
    view: glib::WeakRef<adw::TabView>,
    page: glib::WeakRef<adw::TabPage>,
}

/// libadwaita refuses to start a tab drag while the view holds a single page
/// (adw-tab-box.c gates its tear-off on n_pages > 1), so a pane's last tab
/// could never be dragged out. Cover exactly that case with our own drag
/// source: a drop anywhere on a destination pane moves the page via
/// transfer_page, whose detach half lands in the deferred-collapse path just
/// like a native tear-off. Cancelled drags never detach anything, so they
/// also skip adw's create-window emission (a CRITICAL when unhandled).
fn setup_single_tab_dnd(stack: &gtk::Stack, tab_bar: &adw::TabBar, tab_view: &adw::TabView) {
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::MOVE);
    source.set_button(gdk::BUTTON_PRIMARY);
    // Capture phase: adw's internal reorder gesture on the tab would
    // otherwise claim the sequence first (pointlessly — with one tab there
    // is nothing to reorder).
    source.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let view = tab_view.clone();
        let bar = tab_bar.clone();
        source.connect_prepare(move |_, x, y| {
            if view.n_pages() != 1 {
                return None; // adw handles multi-tab drags natively
            }
            // Not from a button (tab close, new-tab, sidebar reveal).
            let mut w = bar.pick(x, y, gtk::PickFlags::DEFAULT)?;
            while w != *bar.upcast_ref::<gtk::Widget>() {
                if w.is::<gtk::Button>() {
                    return None;
                }
                w = w.parent()?;
            }
            let payload = LastTabDrag {
                view: view.downgrade(),
                page: view.nth_page(0).downgrade(),
            };
            Some(gdk::ContentProvider::for_value(
                &glib::BoxedAnyObject::new(payload).to_value(),
            ))
        });
    }
    {
        let view = tab_view.clone();
        source.connect_drag_begin(move |_, drag| {
            if view.n_pages() == 0 {
                return;
            }
            let label = gtk::Label::new(Some(&view.nth_page(0).title()));
            label.add_css_class("tab-drag-icon");
            gtk::DragIcon::for_drag(drag).set_child(Some(&label));
        });
    }
    tab_bar.add_controller(source);

    let target = gtk::DropTarget::new(glib::BoxedAnyObject::static_type(), gdk::DragAction::MOVE);
    {
        let stack = stack.clone();
        target.connect_drop(move |_, value, _, _| {
            let Ok(boxed) = value.get::<glib::BoxedAnyObject>() else {
                return false;
            };
            let Ok(payload) = boxed.try_borrow::<LastTabDrag>() else {
                return false;
            };
            let (Some(src), Some(page)) = (payload.view.upgrade(), payload.page.upgrade()) else {
                return false;
            };
            let Some(dest) = tab_view_of(stack.upcast_ref()) else {
                return false;
            };
            if src == dest {
                return true; // dropped back on its own pane: nothing to move
            }
            src.transfer_page(&page, &dest, dest.n_pages());
            true
        });
    }
    stack.add_controller(target);
}

/// Collapse a pane that has no tabs left: promote its split sibling, or drop
/// the zone when it was the zone's only pane, or fall back to the "empty"
/// status page. Runs immediately on a tab close; deferred to the drag sweep
/// on a tab drag tear-off.
pub(crate) fn collapse_empty_pane(app: &Rc<App>, stack: &gtk::Stack, zone: &Weak<Zone>) {
    let pane: gtk::Widget = stack.clone().upcast();
    let in_split = pane.parent().map(|p| p.is::<gtk::Paned>()).unwrap_or(false);
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
    } else {
        stack.set_visible_child_name("empty");
    }
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
            let app = app.clone();
            let zw = Rc::downgrade(zone);
            t.connect_window_title_changed(move |term| {
                if let Some(page) = pw.upgrade() {
                    refresh_title(term, &page);
                }
                if let Some(zone) = zw.upgrade() {
                    agent::refresh(&app, &zone);
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
            let app = app.clone();
            let zw = Rc::downgrade(zone);
            t.connect_termprop_changed(
                Some(vmux::osc_scan::FGPROC_TERMPROP_NAME),
                move |term, _name| {
                    if let Some(page) = pw.upgrade() {
                        refresh_title(term, &page);
                    }
                    // Resolve the pane at signal time — tabs migrate between
                    // panes via drag, so capturing it here would go stale.
                    if let Some(pane) = splits::pane_of(term.upcast_ref()) {
                        refresh_tab_indicators(&pane);
                    }
                    if let Some(zone) = zw.upgrade() {
                        agent::refresh(&app, &zone);
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
    let Some(leaf) = terminal.parent() else {
        return;
    };
    let Some(pane) = splits::pane_of(&leaf) else {
        return;
    };
    let Some(view) = tab_view_of(&pane) else {
        return;
    };
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
pub fn fg_command(terminal: &vte::Terminal) -> Option<String> {
    let (name, _) = fg_payload(terminal);
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The (command name, euid) pair vmux-relay last reported via the fgproc
/// termprop. Empty name/None uid when the relay never reported (no relay, or
/// the shell has been in front since startup with an unreadable uid).
fn fg_payload(terminal: &vte::Terminal) -> (String, Option<u32>) {
    let data = terminal.termprop_data(vmux::osc_scan::FGPROC_TERMPROP_NAME);
    vmux::osc_scan::parse_fgproc_payload(&data)
}

/// Foreground commands that hold a session on another machine (kgx's list).
const REMOTE_COMMANDS: &[&str] = &["ssh", "telnet", "mosh-client", "mosh", "et"];

/// Recolor every tab in a pane after *its own* foreground process:
/// `vmux-root` when that process runs as root (euid 0), `vmux-remote` when it
/// is a remote session (ssh and friends). Unselected tabs are marked too — a
/// root shell you can't see is exactly the one worth flagging — so the class
/// has to land per tab rather than on the pane.
///
/// Idempotent; call on any signal that adds a tab or changes what one runs.
/// Recomputing the whole pane (rather than the one tab that changed) keeps it
/// correct when the widgets are rebuilt under us, which adw does freely.
pub fn refresh_tab_indicators(pane: &gtk::Widget) {
    for tab in tab_widgets(pane) {
        let mut root = false;
        let mut remote = false;
        if let Some(page) = page_of_tab_widget(&tab)
            && let Some(t) = splits::first_terminal_in(&page.child())
        {
            let (name, uid) = fg_payload(&t);
            root = uid == Some(0);
            remote = REMOTE_COMMANDS.contains(&name.as_str());
        }
        for (class, on) in [("vmux-root", root), ("vmux-remote", remote)] {
            if on {
                tab.add_css_class(class);
            } else {
                tab.remove_css_class(class);
            }
        }
    }
}

/// The AdwTab widgets inside a pane's tab bar, one per page.
///
/// AdwTab is private, so match on the CSS node name (`tab`) instead of the
/// type; the node name is part of libadwaita's documented styling contract,
/// unlike the C type. Descends the whole tab bar because the widgets sit
/// several private containers deep (tabbox > tabboxchild > tab), and pinned
/// tabs live in a second tabbox.
fn tab_widgets(pane: &gtk::Widget) -> Vec<gtk::Widget> {
    fn walk(w: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
        if w.css_name() == "tab" {
            out.push(w.clone());
            return; // nothing nested inside a tab is another tab
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            walk(&c, out);
            child = c.next_sibling();
        }
    }
    let mut out = Vec::new();
    if let Some(bar) = tab_bar_of(pane) {
        walk(bar.upcast_ref(), &mut out);
    }
    out
}

/// The page an AdwTab widget stands for, read through its `page` property —
/// the only link back, as adw exposes no public tab-widget API at all. Typed
/// check first so a libadwaita that reworked the property drops the coloring
/// instead of panicking.
fn page_of_tab_widget(tab: &gtk::Widget) -> Option<adw::TabPage> {
    tab.has_property_with_type("page", adw::TabPage::static_type())
        .then(|| tab.property::<Option<adw::TabPage>>("page"))
        .flatten()
}
