//! Which coding agent is in the foreground of a terminal, and what it is
//! doing, from two signals vmux already has: the foreground command
//! vmux-relay reports (which agent, if any) and the terminal's OSC window
//! title (busy, or waiting on the user). No screen scraping: the agents
//! covered here all publish their state in the title.

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
            aggregate([
                d(Activity::NeedsInput),
                d(Activity::Working),
                d(Activity::Idle)
            ])
            .map(|d| d.activity),
            Some(Activity::Working)
        );
    }
}
