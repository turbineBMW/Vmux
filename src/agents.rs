//! Which coding agent is in the foreground of a terminal, and what it is
//! doing. Three signals: the foreground command vmux-relay reports (which
//! agent, if any), the terminal's OSC window title (busy, or waiting on the
//! user) and the bottom of the live screen, for what an agent's title
//! doesn't say (an approval or question dialog, or for agents that leave
//! the title alone, being busy at all).

/// What a detected agent is doing right now, least urgent first: the derived
/// order is what [`aggregate`] ranks by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    /// Open at its prompt, nothing running.
    Idle,
    /// Processing a turn.
    Working,
    /// Stopped and waiting for the user: an approval prompt, a question, or
    /// Codex's "Action Required".
    NeedsInput,
}

/// An agent in the foreground of one terminal, with its detected state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    /// Display name, e.g. "Claude" or "Codex".
    pub agent: &'static str,
    pub activity: Activity,
}

/// One rule-table entry: how an agent reports its state.
struct Rule {
    /// Foreground command names it runs as (see `relay::command_name` for
    /// how scripts under node and friends are named).
    commands: &'static [&'static str],
    label: &'static str,
    working: fn(&str) -> bool,
    needs_input: fn(&str) -> bool,
    /// Reads the live screen for a dialog waiting on the user. `None` when
    /// the title says everything.
    screen_needs_input: Option<fn(&str) -> bool>,
    /// Reads the live screen for a turn in progress, for agents whose title
    /// doesn't show it.
    screen_working: Option<fn(&str) -> bool>,
}

fn never(_: &str) -> bool {
    false
}

/// Screen patterns for the agents below Claude and Codex follow herdr's
/// manifests (gemini.toml, opencode.toml, github-copilot.toml).
const RULES: &[Rule] = &[
    Rule {
        commands: &["claude"],
        label: "Claude",
        // Claude Code leads the title with a spinner glyph while busy:
        // braille up to 2.1.227, half-circles from 2.1.228.
        working: |title| {
            let mut chars = title.chars();
            matches!((chars.next(), chars.next()), (Some(c), Some(' ')) if is_spinner(c))
        },
        // The title only says busy or not; approvals show on screen.
        needs_input: never,
        screen_needs_input: Some(claude_screen_needs_input),
        screen_working: None,
    },
    Rule {
        commands: &["codex"],
        label: "Codex",
        // Codex puts a lone braille spinner token somewhere in the title.
        working: |title| {
            title.split(' ').any(|tok| {
                let mut chars = tok.chars();
                matches!((chars.next(), chars.next()), (Some(c), None) if is_spinner(c))
            })
        },
        needs_input: |title| title.contains("Action Required"),
        screen_needs_input: Some(codex_screen_needs_input),
        screen_working: None,
    },
    Rule {
        commands: &["gemini"],
        label: "Gemini",
        working: never,
        needs_input: never,
        screen_needs_input: Some(gemini_screen_needs_input),
        // "(esc to cancel, 12s)" beside the spinner.
        screen_working: Some(|screen| screen.to_lowercase().contains("esc to cancel")),
    },
    Rule {
        commands: &["opencode"],
        label: "OpenCode",
        working: never,
        needs_input: never,
        screen_needs_input: Some(opencode_screen_needs_input),
        screen_working: Some(opencode_screen_working),
    },
    Rule {
        commands: &["copilot"],
        label: "Copilot",
        working: never,
        needs_input: never,
        screen_needs_input: Some(copilot_screen_needs_input),
        screen_working: Some(copilot_screen_working),
    },
];

fn is_spinner(c: char) -> bool {
    matches!(c, '\u{2800}'..='\u{28FF}' | '\u{25D0}'..='\u{25D3}')
}

fn rule_for(command: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.commands.contains(&command))
}

/// Classify one terminal from its foreground command and window title.
/// `None` when no known agent is in front.
pub fn detect(command: &str, title: &str) -> Option<Detected> {
    detect_with_screen(command, title, String::new)
}

/// [`detect`], also reading the live screen where the agent's rules need it.
/// `screen` is called at most once, and only when its answer could change
/// the result, so a front-end can pass a lazy read of the terminal.
pub fn detect_with_screen(
    command: &str,
    title: &str,
    screen: impl FnOnce() -> String,
) -> Option<Detected> {
    let rule = rule_for(command)?;
    // A busy title wins: the agents that report it there stop their spinner
    // while a dialog waits on the user, so a spinning title means none is up.
    let activity = if (rule.working)(title) {
        Activity::Working
    } else if (rule.needs_input)(title) {
        Activity::NeedsInput
    } else if rule.screen_needs_input.is_none() && rule.screen_working.is_none() {
        Activity::Idle
    } else {
        let screen = bottom_lines(&screen(), SCREEN_LINES);
        // A dialog outranks a working hint: some agents keep "esc to cancel"
        // on screen while asking.
        if rule.screen_needs_input.is_some_and(|f| f(&screen)) {
            Activity::NeedsInput
        } else if rule.screen_working.is_some_and(|f| f(&screen)) {
            Activity::Working
        } else {
            Activity::Idle
        }
    };
    Some(Detected {
        agent: rule.label,
        activity,
    })
}

/// When a front-end should re-read an agent's screen as its output changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenUse {
    /// Title and command say everything (or no agent).
    Never,
    /// Only while it isn't working: a working turn is followed through the
    /// title, which changes whenever the state does.
    WhileIdle,
    /// Always: the screen is the only place it shows it is working.
    Always,
}

pub fn screen_use(command: &str) -> ScreenUse {
    match rule_for(command) {
        Some(r) if r.screen_working.is_some() => ScreenUse::Always,
        Some(r) if r.screen_needs_input.is_some() => ScreenUse::WhileIdle,
        _ => ScreenUse::Never,
    }
}

/// How many non-empty lines from the bottom of the screen the screen rules
/// look at: enough for the tallest dialog, few enough that a stale dialog
/// scrolled up into the transcript is out of reach.
const SCREEN_LINES: usize = 30;

/// The last `n` non-empty lines of `screen`, trailing blanks trimmed.
fn bottom_lines(screen: &str, n: usize) -> String {
    let lines: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Claude Code's dialogs that wait on the user, from the bottom of its
/// screen. Patterns follow herdr's claude.toml manifest:
/// - a selection form (questions, plan approval): an "esc to cancel" footer
///   with an "enter to confirm" or "enter to select … navigate" hint;
/// - a permission prompt ("Do you want to proceed?", "… make this edit …")
///   with numbered Yes/No options — unless the input box is showing, which
///   means the question is old transcript, not a live dialog.
fn claude_screen_needs_input(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    let form = lower.contains("esc to cancel")
        && (lower.contains("enter to confirm")
            || (lower.contains("enter to select") && lower.contains("navigate")));
    if form {
        return true;
    }
    let asks = lower.contains("do you want to") || lower.contains("would you like to");
    asks && screen.lines().any(is_yes_no_option) && !claude_input_box_open(screen)
}

/// A numbered Yes/No dialog option, selected (`❯ 1. Yes`) or not
/// (`  3. No, and tell Claude what to do differently`).
fn is_yes_no_option(line: &str) -> bool {
    let Some(rest) = numbered_option(line) else {
        return false;
    };
    let rest = rest.to_lowercase();
    rest.starts_with("yes") || rest.starts_with("no")
}

/// The text after a dialog option's `N. ` number, skipping a box border and
/// the cursor mark.
fn numbered_option(line: &str) -> Option<&str> {
    let t = line.trim_start_matches(|c: char| c.is_whitespace() || c == '│' || c == '❯');
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    t[digits..].strip_prefix(". ")
}

/// Claude's input box: a `❯` line between the last two horizontal rules
/// that isn't a numbered dialog option.
fn claude_input_box_open(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let rules: Vec<usize> = (0..lines.len())
        .filter(|&i| is_horizontal_rule(lines[i]))
        .collect();
    let [.., top, bottom] = rules[..] else {
        return false;
    };
    lines[top + 1..bottom]
        .iter()
        .any(|l| l.trim_start().starts_with('❯') && numbered_option(l).is_none())
}

fn is_horizontal_rule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 8 && t.chars().all(|c| matches!(c, '─' | '━' | '═'))
}

/// Codex's approval and question dialogs, for when its title has not (yet)
/// switched to "Action Required". Footers from herdr's codex.toml.
fn codex_screen_needs_input(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    [
        "press enter to confirm or esc to cancel",
        "enter to submit answer",
        "enter to submit all",
        "allow command?",
    ]
    .iter()
    .any(|s| lower.contains(s))
}

/// Gemini CLI's confirmation dialogs: apply a change, allow a command, or a
/// "do you want to proceed" with a Yes option.
fn gemini_screen_needs_input(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    lower.contains("│ apply this change")
        || lower.contains("│ allow execution")
        || (lower.contains("yes")
            && (lower.contains("waiting for user confirmation")
                || lower.contains("do you want to proceed")))
        || lower.lines().any(|l| {
            l.trim_start()
                .strip_prefix('❯')
                .is_some_and(|rest| rest.contains("yes") || rest.contains("allow"))
        })
}

/// OpenCode's permission prompt, or one of its dialogs: a dismiss footer
/// with a confirm/submit/toggle hint and a select or tab hint.
fn opencode_screen_needs_input(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    lower.contains("△ permission required")
        || (lower.contains("esc dismiss")
            && ["enter confirm", "enter submit", "enter toggle"]
                .iter()
                .any(|s| lower.contains(s))
            && (lower.contains("↑↓ select") || lower.contains("⇆ tab")))
}

/// OpenCode at work: an interrupt hint, or its block progress bar.
fn opencode_screen_working(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    ["esc to interrupt", "ctrl+c to interrupt"]
        .iter()
        .any(|s| lower.contains(s))
        || lower.lines().any(|l| {
            l.contains("opencode")
                && (l.contains("esc interrupt") || l.contains("esc again to interrupt"))
        })
        || has_run_of(screen, |c| c == '■' || c == '⬝', 4)
}

/// Whether `screen` has `n` consecutive characters matching `f`.
fn has_run_of(screen: &str, f: impl Fn(char) -> bool, n: usize) -> bool {
    let mut run = 0;
    for c in screen.chars() {
        run = if f(c) { run + 1 } else { 0 };
        if run >= n {
            return true;
        }
    }
    false
}

/// Copilot CLI's selection dialogs: a cancel footer with an enter hint.
fn copilot_screen_needs_input(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    (lower.contains("esc to cancel") || lower.contains("esc cancel"))
        && [
            "enter to select",
            "enter to confirm",
            "enter to submit",
            "enter accept",
        ]
        .iter()
        .any(|s| lower.contains(s))
}

/// Copilot CLI at work: its cancel or interrupt hint, or waiting on its
/// background agents.
fn copilot_screen_working(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    ["esc to cancel", "esc cancel", "esc interrupt"]
        .iter()
        .any(|s| lower.contains(s))
        || lower.lines().any(|l| {
            l.trim_start()
                .starts_with("◎ waiting for background agents")
        })
}

/// The roll-up of several terminals: the most urgent one wins, so an agent
/// waiting on the user is never hidden behind another that is working.
pub fn aggregate(items: impl IntoIterator<Item = Detected>) -> Option<Detected> {
    items.into_iter().max_by_key(|d| d.activity)
}

/// A tracked agent's state as the user should see it: [`Activity`] plus
/// whether its last stop has been looked at. Least urgent first, so the
/// derived order ranks attention: an agent waiting on the user, then one
/// that finished unseen, then one at work, then one idle and seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Idle,
    Working,
    /// Idle, but it stopped while its terminal was out of view.
    Done,
    NeedsInput,
}

impl Status {
    pub fn of(activity: Activity, seen: bool) -> Status {
        match activity {
            Activity::Idle if seen => Status::Idle,
            Activity::Idle => Status::Done,
            Activity::Working => Status::Working,
            Activity::NeedsInput => Status::NeedsInput,
        }
    }

    /// Whether the user should go and look: it asked, or it finished unseen.
    pub fn wants_attention(self) -> bool {
        matches!(self, Status::NeedsInput | Status::Done)
    }
}

/// A group's (a zone's) agent summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rollup {
    /// The most urgent agent in the group, and its status.
    pub top: Option<(&'static str, Status)>,
    /// Some agent in the group is working, whatever `top` is.
    pub working: bool,
}

pub fn rollup(items: impl IntoIterator<Item = (&'static str, Status)>) -> Rollup {
    items
        .into_iter()
        .fold(Rollup::default(), |acc, (agent, status)| Rollup {
            top: match acc.top {
                Some((_, s)) if s >= status => acc.top,
                _ => Some((agent, status)),
            },
            working: acc.working || status == Status::Working,
        })
}

/// Sort for an agent overview, most urgent first: by [`Status`], then by
/// the most recent state change (`changed`, larger is newer). The sort is
/// stable, so equal entries keep their incoming (zone) order.
pub fn sort_by_attention<T>(items: &mut [T], key: impl Fn(&T) -> (Status, u64)) {
    items.sort_by_key(|t| std::cmp::Reverse(key(t)));
}

/// What an agent's window title says it is working on, without the state
/// glyph some agents lead it with (Claude's spinner or idle `✳`).
pub fn task_title(title: &str) -> &str {
    let mut chars = title.chars();
    match (chars.next(), chars.next()) {
        (Some(c), Some(' ')) if is_spinner(c) || c == '✳' => chars.as_str().trim_start(),
        _ => title,
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
        assert_eq!(screen_use("zsh"), ScreenUse::Never);
        assert_eq!(screen_use("claude"), ScreenUse::WhileIdle);
        assert_eq!(screen_use("gemini"), ScreenUse::Always);
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

    const RULE: &str = "────────────────────────────────────────";

    fn claude_on(screen: &str) -> Option<Activity> {
        detect_with_screen("claude", "✳ Claude Code", || screen.to_string()).map(|d| d.activity)
    }

    #[test]
    fn claude_permission_prompt_needs_input() {
        let bash = "\
⏺ Bash(cargo test)
╭──────────────────────────────────────╮
│ Bash command                         │
│   cargo test                         │
│ Do you want to proceed?              │
│ ❯ 1. Yes                             │
│   2. Yes, and don't ask again for: cargo test │
│   3. No, and tell Claude what to do differently (esc) │
╰──────────────────────────────────────╯";
        assert_eq!(claude_on(bash), Some(Activity::NeedsInput));

        let edit = "\
Edit file
 src/agents.rs
Do you want to make this edit to agents.rs?
❯ 1. Yes
  2. Yes, allow all edits during this session (shift+tab)
  3. No, and tell Claude what to do differently (esc)";
        assert_eq!(claude_on(edit), Some(Activity::NeedsInput));
    }

    #[test]
    fn claude_selection_form_needs_input() {
        let question = format!(
            "\
Which approach should I take?
❯ 1. Rewrite the parser
  2. Patch the tokenizer
{RULE}
Enter to select · ↑/↓ to navigate · Esc to cancel"
        );
        assert_eq!(claude_on(&question), Some(Activity::NeedsInput));
    }

    #[test]
    fn claude_old_question_above_the_input_box_is_not_live() {
        let screen = format!(
            "\
Do you want to proceed?
❯ 1. Yes
  2. No
⏺ Done — the tests pass.
{RULE}
❯
{RULE}
  ? for shortcuts"
        );
        assert_eq!(claude_on(&screen), Some(Activity::Idle));
        // Text typed into the box doesn't change that.
        let typed = screen.replace("❯ \n", "❯ now run the linter\n");
        assert_eq!(claude_on(&typed), Some(Activity::Idle));
    }

    #[test]
    fn claude_busy_title_skips_the_screen() {
        let read = std::cell::Cell::new(false);
        let d = detect_with_screen("claude", "⠋ Working", || {
            read.set(true);
            "Do you want to proceed?\n❯ 1. Yes".into()
        });
        assert_eq!(d.map(|d| d.activity), Some(Activity::Working));
        assert!(!read.get());
    }

    #[test]
    fn claude_plain_transcript_is_idle() {
        assert_eq!(
            claude_on("Do you want to see the diff?\nSure."),
            Some(Activity::Idle)
        );
        assert_eq!(claude_on(""), Some(Activity::Idle));
    }

    #[test]
    fn codex_dialog_footer_needs_input() {
        let d = detect_with_screen("codex", "codex — Vmux", || {
            "Allow command?\n  cargo build\nPress enter to confirm or esc to cancel".into()
        });
        assert_eq!(d.map(|d| d.activity), Some(Activity::NeedsInput));
    }

    #[test]
    fn only_the_bottom_of_the_screen_counts() {
        let mut screen = String::from("Do you want to proceed?\n❯ 1. Yes\n");
        for i in 0..SCREEN_LINES {
            screen.push_str(&format!("line {i}\n\n"));
        }
        assert_eq!(claude_on(&screen), Some(Activity::Idle));
    }

    fn on(command: &str, screen: &str) -> Option<Activity> {
        detect_with_screen(command, "", || screen.to_string()).map(|d| d.activity)
    }

    #[test]
    fn gemini_reads_everything_from_the_screen() {
        assert_eq!(detect("gemini", "").map(|d| d.agent), Some("Gemini"));
        assert_eq!(on("gemini", "> Type your message"), Some(Activity::Idle));
        assert_eq!(
            on("gemini", "⠼ Reading files (esc to cancel, 4s)"),
            Some(Activity::Working)
        );
        let confirm = "\
╭──────────────────────────╮
│ Shell cargo test         │
│ Allow execution?         │
│ ● 1. Yes, allow once     │
╰──────────────────────────╯
⠼ Waiting for user confirmation... (esc to cancel, 9s)";
        assert_eq!(on("gemini", confirm), Some(Activity::NeedsInput));
    }

    #[test]
    fn opencode_permission_and_progress() {
        assert_eq!(
            on("opencode", "△ Permission required\nbash: ls"),
            Some(Activity::NeedsInput)
        );
        assert_eq!(
            on("opencode", "⬝⬝⬝■■■■⬝  esc interrupt"),
            Some(Activity::Working)
        );
        assert_eq!(
            on("opencode", "Build  claude-opus\nctrl+p commands"),
            Some(Activity::Idle)
        );
        // Three blocks are not a progress bar.
        assert_eq!(on("opencode", "■■■ done"), Some(Activity::Idle));
    }

    #[test]
    fn copilot_dialog_outranks_its_cancel_hint() {
        assert_eq!(
            on(
                "copilot",
                "Run this command?\n❯ 1. Yes\nEnter to select · Esc to cancel"
            ),
            Some(Activity::NeedsInput)
        );
        assert_eq!(
            on("copilot", "∙ Thinking (Esc to cancel)"),
            Some(Activity::Working)
        );
        assert_eq!(
            on("copilot", "◎ Waiting for background agents · 2 running"),
            Some(Activity::Working)
        );
        assert_eq!(on("copilot", "> Ask Copilot"), Some(Activity::Idle));
    }

    #[test]
    fn needing_input_outranks_the_rest_of_the_zone() {
        let d = |activity| Detected {
            agent: "x",
            activity,
        };
        assert_eq!(aggregate([]), None);
        assert_eq!(
            aggregate([d(Activity::Idle), d(Activity::Working)]).map(|d| d.activity),
            Some(Activity::Working)
        );
        assert_eq!(
            aggregate([
                d(Activity::NeedsInput),
                d(Activity::Working),
                d(Activity::Idle)
            ])
            .map(|d| d.activity),
            Some(Activity::NeedsInput)
        );
    }

    #[test]
    fn status_folds_in_whether_the_stop_was_seen() {
        assert_eq!(Status::of(Activity::Idle, true), Status::Idle);
        assert_eq!(Status::of(Activity::Idle, false), Status::Done);
        assert_eq!(Status::of(Activity::Working, false), Status::Working);
        assert_eq!(Status::of(Activity::NeedsInput, true), Status::NeedsInput);
        assert!(Status::Done.wants_attention());
        assert!(!Status::Working.wants_attention());
    }

    #[test]
    fn rollup_keeps_working_alongside_the_most_urgent() {
        assert_eq!(rollup([]), Rollup::default());
        let r = rollup([
            ("Claude", Status::Working),
            ("Codex", Status::Done),
            ("Claude", Status::Idle),
        ]);
        assert_eq!(r.top, Some(("Codex", Status::Done)));
        assert!(r.working);
        let r = rollup([("Claude", Status::Idle), ("Codex", Status::Idle)]);
        assert_eq!(r.top, Some(("Claude", Status::Idle)));
        assert!(!r.working);
    }

    #[test]
    fn attention_sort_ranks_status_then_recency() {
        let mut items = [
            ("a", Status::Idle, 9),
            ("b", Status::Working, 1),
            ("c", Status::NeedsInput, 2),
            ("d", Status::Working, 5),
            ("e", Status::Done, 3),
        ];
        sort_by_attention(&mut items, |&(_, s, n)| (s, n));
        let order: Vec<_> = items.iter().map(|i| i.0).collect();
        assert_eq!(order, ["c", "e", "d", "b", "a"]);
    }

    #[test]
    fn task_title_drops_the_state_glyph() {
        assert_eq!(task_title("⠋ Fixing the tests"), "Fixing the tests");
        assert_eq!(task_title("✳ Claude Code"), "Claude Code");
        assert_eq!(task_title("codex — Vmux"), "codex — Vmux");
        assert_eq!(task_title(""), "");
    }
}
