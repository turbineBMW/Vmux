use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use libadwaita::prelude::*;
use vte4 as vte;

/// CSS class marking a pane root widget (the tabbed container that splits
/// operate on). Used to find panes while walking the widget tree.
pub const PANE_CLASS: &str = "vmux-pane";

/// Typed descent — never rely on first_child() order for known containers.
pub fn first_terminal_in(w: &gtk::Widget) -> Option<vte::Terminal> {
    if let Some(t) = w.downcast_ref::<vte::Terminal>() {
        return Some(t.clone());
    }
    if let Some(b) = w.downcast_ref::<adw::Bin>() {
        return b.child().and_then(|c| first_terminal_in(&c));
    }
    if let Some(s) = w.downcast_ref::<gtk::ScrolledWindow>() {
        return s.child().and_then(|c| first_terminal_in(&c));
    }
    if let Some(p) = w.downcast_ref::<gtk::Paned>() {
        return p
            .start_child()
            .and_then(|c| first_terminal_in(&c))
            .or_else(|| p.end_child().and_then(|c| first_terminal_in(&c)));
    }
    if let Some(st) = w.downcast_ref::<gtk::Stack>() {
        return st.visible_child().and_then(|c| first_terminal_in(&c));
    }
    if let Some(view) = w.downcast_ref::<adw::TabView>() {
        return view
            .selected_page()
            .and_then(|page| first_terminal_in(&page.child()));
    }
    // Generic containers (gtk::Box, …)
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(t) = first_terminal_in(&c) {
            return Some(t);
        }
        child = c.next_sibling();
    }
    None
}

pub fn all_terminals_in(w: &gtk::Widget, out: &mut Vec<vte::Terminal>) {
    if let Some(t) = w.downcast_ref::<vte::Terminal>() {
        out.push(t.clone());
        return;
    }
    if let Some(b) = w.downcast_ref::<adw::Bin>() {
        if let Some(c) = b.child() {
            all_terminals_in(&c, out);
        }
        return;
    }
    if let Some(s) = w.downcast_ref::<gtk::ScrolledWindow>() {
        if let Some(c) = s.child() {
            all_terminals_in(&c, out);
        }
        return;
    }
    if let Some(p) = w.downcast_ref::<gtk::Paned>() {
        if let Some(c) = p.start_child() {
            all_terminals_in(&c, out);
        }
        if let Some(c) = p.end_child() {
            all_terminals_in(&c, out);
        }
        return;
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        all_terminals_in(&c, out);
        child = c.next_sibling();
    }
}

/// All pane roots under `w`, in visual (depth-first) order.
pub fn all_panes_in(w: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
    if w.has_css_class(PANE_CLASS) {
        out.push(w.clone());
        return; // panes do not nest
    }
    if let Some(b) = w.downcast_ref::<adw::Bin>() {
        if let Some(c) = b.child() {
            all_panes_in(&c, out);
        }
        return;
    }
    if let Some(p) = w.downcast_ref::<gtk::Paned>() {
        if let Some(c) = p.start_child() {
            all_panes_in(&c, out);
        }
        if let Some(c) = p.end_child() {
            all_panes_in(&c, out);
        }
        return;
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        all_panes_in(&c, out);
        child = c.next_sibling();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Directional neighbor: the pane whose center is nearest to `current`'s
/// center in `dir`, weighing perpendicular drift double so a straight-across
/// pane beats a closer diagonal one — then, when that pane sits in a sibling
/// split, the pane last focused within that split.
pub fn neighbor_pane(
    root: &gtk::Widget,
    current: &gtk::Widget,
    dir: Direction,
) -> Option<gtk::Widget> {
    let mut panes = Vec::new();
    all_panes_in(root, &mut panes);
    let cur = current.compute_bounds(root)?;
    let (cx, cy) = (cur.x() + cur.width() / 2.0, cur.y() + cur.height() / 2.0);
    let mut best: Option<(f32, gtk::Widget)> = None;
    for pane in panes {
        if pane == *current {
            continue;
        }
        let Some(b) = pane.compute_bounds(root) else {
            continue;
        };
        let (px, py) = (b.x() + b.width() / 2.0, b.y() + b.height() / 2.0);
        let (forward, drift) = match dir {
            Direction::Left => (cx - px, (py - cy).abs()),
            Direction::Right => (px - cx, (py - cy).abs()),
            Direction::Up => (cy - py, (px - cx).abs()),
            Direction::Down => (py - cy, (px - cx).abs()),
        };
        if forward <= 1.0 {
            continue;
        }
        let score = forward + drift * 2.0;
        if best.as_ref().is_none_or(|(s, _)| score < *s) {
            best = Some((score, pane));
        }
    }
    best.map(|(_, pane)| remembered_in_subtree(current, &pane))
}

/// Per-split memory of the pane last focused beneath it, so that moving back
/// into a subtree returns to where the user left off (i3/tmux behaviour)
/// instead of whichever pane is geometrically nearest.
const LAST_PANE_KEY: &str = "vmux-last-pane";

/// Record `pane` as the most recently focused leaf of every split above it.
pub fn remember_focused_pane(pane: &gtk::Widget) {
    let mut cur = pane.parent();
    while let Some(w) = cur {
        if let Some(paned) = w.downcast_ref::<gtk::Paned>() {
            let weak: glib::WeakRef<gtk::Widget> = pane.downgrade();
            // SAFETY: the key is private to this module and always holds a
            // WeakRef<gtk::Widget>; it is freed with the paned.
            unsafe { paned.set_data(LAST_PANE_KEY, weak) };
        }
        cur = w.parent();
    }
}

fn remembered_pane(paned: &gtk::Paned) -> Option<gtk::Widget> {
    // SAFETY: see remember_focused_pane — same key, same type.
    let weak = unsafe { paned.data::<glib::WeakRef<gtk::Widget>>(LAST_PANE_KEY)? };
    let pane = unsafe { weak.as_ref() }.upgrade()?;
    // Panes can be dragged elsewhere; only trust memory still under this split.
    pane.is_ancestor(paned).then_some(pane)
}

/// `candidate` is the geometric neighbor of `current`. If it lives in a
/// sibling subtree (a split that does not contain `current`), prefer the
/// pane last focused in that whole subtree.
fn remembered_in_subtree(current: &gtk::Widget, candidate: &gtk::Widget) -> gtk::Widget {
    let mut sub = candidate.clone();
    while let Some(parent) = sub.parent() {
        if parent == *current || current.is_ancestor(&parent) {
            break;
        }
        sub = parent;
    }
    sub.downcast_ref::<gtk::Paned>()
        .and_then(remembered_pane)
        .unwrap_or_else(|| candidate.clone())
}

/// Nearest enclosing pane root of `w` (or `w` itself).
pub fn pane_of(w: &gtk::Widget) -> Option<gtk::Widget> {
    let mut cur = Some(w.clone());
    while let Some(c) = cur {
        if c.has_css_class(PANE_CLASS) {
            return Some(c);
        }
        cur = c.parent();
    }
    None
}

/// Replace `old` with `new` in whatever slot holds `old` (adw::Bin or
/// gtk::Paned). `new` must already be unparented; setters auto-unparent `old`.
fn replace_in_slot(slot: &gtk::Widget, old: &gtk::Widget, new: &gtk::Widget) {
    if let Some(bin) = slot.downcast_ref::<adw::Bin>() {
        bin.set_child(Some(new));
    } else if let Some(paned) = slot.downcast_ref::<gtk::Paned>() {
        if paned.start_child().as_ref() == Some(old) {
            paned.set_start_child(Some(new));
        } else {
            paned.set_end_child(Some(new));
        }
    } else {
        eprintln!(
            "vmux: replace_in_slot: unexpected slot type {}",
            slot.type_()
        );
    }
}

/// Fraction of a split's extent that one grow/shrink keypress shifts the divider.
pub const RESIZE_STEP: f64 = 0.05;

/// The divider ratio is cached on the paned itself (widget name slot) so a
/// snapshot never depends on the paned being allocated (hidden zones).
pub fn set_cached_ratio(paned: &gtk::Paned, ratio: f64) {
    paned.set_widget_name(&format!("vmux-ratio={ratio:.4}"));
}

pub fn cached_ratio(paned: &gtk::Paned) -> f64 {
    paned
        .widget_name()
        .strip_prefix("vmux-ratio=")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.5)
}

pub fn paned_size(paned: &gtk::Paned) -> i32 {
    if paned.orientation() == gtk::Orientation::Horizontal {
        paned.width()
    } else {
        paned.height()
    }
}

/// GtkPaned ignores set_position before allocation; apply once it has a size.
pub fn position_when_allocated(paned: &gtk::Paned, ratio: f64) {
    paned.add_tick_callback(move |p, _clock| {
        let size = paned_size(p);
        if size > 1 {
            p.set_position((size as f64 * ratio) as i32);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

pub fn new_split_paned(orientation: gtk::Orientation, ratio: f64) -> gtk::Paned {
    let paned = gtk::Paned::new(orientation);
    paned.set_hexpand(true);
    paned.set_vexpand(true);
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(true);
    set_cached_ratio(&paned, ratio);
    position_when_allocated(&paned, ratio);
    paned
}

/// Split `leaf` in two: its slot gets a new Paned holding {leaf, new_leaf}.
/// Returns the new paned so the caller can wire save-on-resize.
pub fn split_leaf(
    leaf: &gtk::Widget,
    new_leaf: &gtk::Widget,
    orientation: gtk::Orientation,
) -> Option<gtk::Paned> {
    let slot = leaf.parent()?;
    let paned = new_split_paned(orientation, 0.5);
    // Putting the paned into the slot unparents the leaf; then re-home both.
    replace_in_slot(&slot, leaf, paned.upcast_ref());
    paned.set_start_child(Some(leaf));
    paned.set_end_child(Some(new_leaf));
    Some(paned)
}

/// Remove `leaf`; promote its sibling into the parent slot. Returns the
/// promoted widget, or None if `leaf` sits directly in a Bin slot (it is the
/// zone's only pane) — the caller decides what that means.
pub fn collapse_leaf(leaf: &gtk::Widget) -> Option<gtk::Widget> {
    let parent = leaf.parent()?;
    let paned = parent.downcast_ref::<gtk::Paned>()?.clone();
    let sibling = if paned.start_child().as_ref() == Some(leaf) {
        paned.end_child()
    } else {
        paned.start_child()
    }?;
    let grand = paned.parent()?;
    // Detach the sibling (keeping a strong ref) before re-homing it.
    if paned.start_child().as_ref() == Some(&sibling) {
        paned.set_start_child(None::<&gtk::Widget>);
    } else {
        paned.set_end_child(None::<&gtk::Widget>);
    }
    replace_in_slot(&grand, paned.upcast_ref(), &sibling);
    Some(sibling)
}
