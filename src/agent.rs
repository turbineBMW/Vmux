//! Coding-agent activity per zone, read from two signals vmux already has:
//! the foreground command vmux-relay reports for each terminal (which agent
//! is in front, if any) and the terminal's OSC window title (whether that
//! agent is busy or waiting on the user). No screen scraping: the agents
//! covered here all publish their state in the title.
//!
//! The per-zone result drives the sidebar chip — a badge while an agent is
//! open, pulsing while it works, and a ring once it stops or asks for input
//! while the zone isn't in view — and, optionally, moves the zone to the
//! top of the sidebar when its agent starts working.

use crate::app::App;
use crate::zone::Zone;
use crate::{pane, splits};
use gtk4 as gtk;
use gtk4::glib;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use vte4::prelude::*;

/// What a detected agent is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    /// Open at its prompt, nothing running.
    Idle,
    /// Stopped and waiting for the user (Codex's "Action Required").
    NeedsInput,
    /// Processing a turn.
    Working,
}

/// An agent in the foreground of one terminal, with its title-derived state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    /// Display name, e.g. "Claude" or "Codex".
    pub agent: &'static str,
    pub activity: Activity,
}

/// One title-rule table entry: how a given command name reports its state
/// through the terminal title.
struct Rule {
    command: &'static str,
    label: &'static str,
    working: fn(&str) -> bool,
    needs_input: fn(&str) -> bool,
}

const RULES: &[Rule] = &[
    Rule {
        command: "claude",
        label: "Claude",
        // Claude Code leads the title with a spinner glyph while busy:
        // braille up to 2.1.227, half-circles from 2.1.228.
        working: |title| {
            let mut chars = title.chars();
            matches!((chars.next(), chars.next()), (Some(c), Some(' ')) if is_spinner(c))
        },
        needs_input: |_| false,
    },
    Rule {
        command: "codex",
        label: "Codex",
        // Codex puts a lone braille spinner token somewhere in the title.
        working: |title| {
            title.split(' ').any(|tok| {
                let mut chars = tok.chars();
                matches!((chars.next(), chars.next()), (Some(c), None) if is_spinner(c))
            })
        },
        needs_input: |title| title.contains("Action Required"),
    },
];

fn is_spinner(c: char) -> bool {
    matches!(c, '\u{2800}'..='\u{28FF}' | '\u{25D0}'..='\u{25D3}')
}

/// Classify one terminal from its foreground command and window title.
/// `None` when no known agent is in front.
pub fn detect(command: &str, title: &str) -> Option<Detected> {
    let rule = RULES.iter().find(|r| r.command == command)?;
    let activity = if (rule.working)(title) {
        Activity::Working
    } else if (rule.needs_input)(title) {
        Activity::NeedsInput
    } else {
        Activity::Idle
    };
    Some(Detected {
        agent: rule.label,
        activity,
    })
}

/// The zone-level roll-up: the busiest terminal wins, so one working agent
/// makes the whole zone "working" even if another sits idle.
pub fn aggregate(items: impl IntoIterator<Item = Detected>) -> Option<Detected> {
    items.into_iter().max_by_key(|d| d.activity)
}

/// How long a zone must stay non-working before its agent counts as stopped.
/// Agents drop the spinner for an instant between tool calls; without this,
/// every such flicker would ring the chip.
const SETTLE_MS: u64 = 1200;

/// Per-zone tracking state; lives on [`Zone`].
#[derive(Default)]
pub struct ZoneAgent {
    /// The last committed roll-up (see `SETTLE_MS` for why "committed").
    pub current: Cell<Option<Detected>>,
    /// The agent stopped or asked for input while the zone wasn't in view;
    /// cleared when the user looks at the zone.
    pub unseen: Cell<bool>,
    settle: RefCell<Option<glib::SourceId>>,
}

/// What every terminal in `zone` currently reports, rolled up.
fn scan(zone: &Zone) -> Option<Detected> {
    let mut terms = Vec::new();
    splits::all_terminals_in(zone.page.upcast_ref(), &mut terms);
    aggregate(terms.iter().filter_map(|t| {
        let command = pane::fg_command(t)?;
        #[allow(deprecated)] // window_title: see pane::new_tab
        let title = t.window_title().map(|t| t.to_string()).unwrap_or_default();
        detect(&command, &title)
    }))
}

/// Re-read the zone's terminals and commit any change. Called from the
/// title-changed and fgproc signals and when a terminal goes away.
pub fn refresh(app: &Rc<App>, zone: &Rc<Zone>) {
    let now = scan(zone);
    let prev = zone.agent.current.get();
    let was_working = prev.map(|d| d.activity == Activity::Working).unwrap_or(false);
    let is_working = now.map(|d| d.activity == Activity::Working).unwrap_or(false);

    if is_working {
        if let Some(id) = zone.agent.settle.borrow_mut().take() {
            id.remove();
        }
        if now != prev {
            zone.agent.current.set(now);
            apply(zone);
            if !was_working {
                app.on_agent_started(zone);
            }
        }
        return;
    }

    if was_working {
        // Leaving "working" is debounced; the timer re-scans and commits.
        if zone.agent.settle.borrow().is_some() {
            return;
        }
        let app = app.clone();
        let zw = Rc::downgrade(zone);
        let id = glib::timeout_add_local_once(
            std::time::Duration::from_millis(SETTLE_MS),
            move || {
                let Some(zone) = zw.upgrade() else { return };
                *zone.agent.settle.borrow_mut() = None;
                let now = scan(&zone);
                if now.map(|d| d.activity) == Some(Activity::Working) {
                    return; // it was only a flicker
                }
                zone.agent.current.set(now);
                if !app.zone_in_view(&zone) {
                    zone.agent.unseen.set(true);
                }
                apply(&zone);
            },
        );
        *zone.agent.settle.borrow_mut() = Some(id);
        return;
    }

    if now != prev {
        let asked = now.map(|d| d.activity == Activity::NeedsInput).unwrap_or(false)
            && prev.map(|d| d.activity != Activity::NeedsInput).unwrap_or(true);
        zone.agent.current.set(now);
        if asked && !app.zone_in_view(zone) {
            zone.agent.unseen.set(true);
        }
        apply(zone);
    }
}

/// The user is looking at the zone: drop the "unseen" ring.
pub fn mark_seen(zone: &Zone) {
    if zone.agent.unseen.replace(false) {
        apply(zone);
    }
}

/// Push the tracked state onto the chip's badge and ring.
pub fn apply(zone: &Zone) {
    let d = zone.agent.current.get();
    style_badge(&zone.agent_badge, d);
    zone.agent_badge.set_visible(d.is_some());
    let ring = zone.agent.unseen.get()
        || d.map(|d| d.activity == Activity::NeedsInput).unwrap_or(false);
    set_class(&zone.agent_chip, "agent-attention", ring);
    zone.agent_chip.set_tooltip_text(tooltip(d, zone.agent.unseen.get()).as_deref());
}

/// Put the activity classes on a badge/dot widget.
pub fn style_badge(badge: &impl IsA<gtk::Widget>, d: Option<Detected>) {
    let working = d.map(|d| d.activity == Activity::Working).unwrap_or(false);
    let asking = d.map(|d| d.activity == Activity::NeedsInput).unwrap_or(false);
    set_class(badge, "working", working);
    set_class(badge, "attention", asking);
}

fn tooltip(d: Option<Detected>, unseen: bool) -> Option<String> {
    let d = d?;
    Some(match d.activity {
        Activity::Working => format!("{} is working", d.agent),
        Activity::NeedsInput => format!("{} needs your input", d.agent),
        Activity::Idle if unseen => format!("{} finished", d.agent),
        Activity::Idle => format!("{} is open", d.agent),
    })
}

fn set_class(w: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_commands_are_not_agents() {
        assert_eq!(detect("zsh", "⠋ anything"), None);
        assert_eq!(detect("", "⠋ anything"), None);
        assert_eq!(detect("vim", "Action Required"), None);
    }

    #[test]
    fn claude_spinner_leads_the_title() {
        let w = |t| detect("claude", t).map(|d| d.activity);
        assert_eq!(w("⠋ Fixing the tests"), Some(Activity::Working));
        assert_eq!(w("◐ Fixing the tests"), Some(Activity::Working));
        assert_eq!(w("✳ Claude Code"), Some(Activity::Idle));
        assert_eq!(w("Fixing ⠋ the tests"), Some(Activity::Idle));
        assert_eq!(w(""), Some(Activity::Idle));
        assert_eq!(detect("claude", "").unwrap().agent, "Claude");
    }

    #[test]
    fn codex_spinner_is_a_lone_token_anywhere() {
        let w = |t| detect("codex", t).map(|d| d.activity);
        assert_eq!(w("⠹ codex — Vmux"), Some(Activity::Working));
        assert_eq!(w("codex ⠹ Vmux"), Some(Activity::Working));
        assert_eq!(w("codex — Vmux"), Some(Activity::Idle));
        assert_eq!(w("⠹x codex"), Some(Activity::Idle));
        assert_eq!(w("Action Required: codex"), Some(Activity::NeedsInput));
    }

    #[test]
    fn working_outranks_the_rest_of_the_zone() {
        let d = |activity| Detected {
            agent: "x",
            activity,
        };
        assert_eq!(aggregate([]), None);
        assert_eq!(
            aggregate([d(Activity::Idle), d(Activity::NeedsInput)]).map(|d| d.activity),
            Some(Activity::NeedsInput)
        );
        assert_eq!(
            aggregate([d(Activity::NeedsInput), d(Activity::Working), d(Activity::Idle)])
                .map(|d| d.activity),
            Some(Activity::Working)
        );
    }
}
