//! Asking a coding agent from anywhere: a request typed into a popup is
//! handed to the user's own agent CLI (Claude Code or Codex), started in a
//! terminal next to the one they were in, in its directory. Optionally the
//! request carries that terminal's recent output, so "why did this fail?"
//! has something to go on.
//!
//! The agent runs as a job of an ordinary shell, so it is tracked like any
//! agent the user starts by hand and the shell stays behind when it exits.
//! That means typing a command line into the shell, and a request is free
//! text, possibly many lines of it: it travels in a private file instead,
//! read back by a command substitution that bash, zsh and fish (3.4+) all
//! understand.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which agent the popup starts.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AskAgent {
    #[default]
    Claude,
    Codex,
}

impl AskAgent {
    pub const ALL: [AskAgent; 2] = [AskAgent::Claude, AskAgent::Codex];

    pub fn label(self) -> &'static str {
        match self {
            AskAgent::Claude => "Claude Code",
            AskAgent::Codex => "Codex",
        }
    }

    /// The command it runs as. Both take an initial prompt as their one
    /// argument and stay open for the rest of the conversation.
    pub fn command(self) -> &'static str {
        match self {
            AskAgent::Claude => "claude",
            AskAgent::Codex => "codex",
        }
    }
}

/// Where the agent's terminal opens, relative to the asking terminal.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AskPlacement {
    #[default]
    SplitRight,
    SplitDown,
    NewTab,
}

impl AskPlacement {
    pub const ALL: [AskPlacement; 3] = [
        AskPlacement::SplitRight,
        AskPlacement::SplitDown,
        AskPlacement::NewTab,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AskPlacement::SplitRight => "Split right",
            AskPlacement::SplitDown => "Split down",
            AskPlacement::NewTab => "New tab",
        }
    }
}

/// The most terminal output a request carries.
pub const SCREEN_MAX_LINES: usize = 80;
pub const SCREEN_MAX_CHARS: usize = 8000;

/// The end of a terminal's text worth sending: trailing whitespace and blank
/// lines dropped, then the last `max_lines` lines that fit in `max_chars`.
/// `None` when nothing is left.
pub fn screen_tail(text: &str, max_lines: usize, max_chars: usize) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let end = lines.iter().rposition(|l| !l.is_empty())? + 1;
    let mut kept = Vec::new();
    let mut size = 0;
    for line in lines[..end].iter().rev().take(max_lines) {
        size += line.chars().count() + 1;
        if size > max_chars && !kept.is_empty() {
            break;
        }
        kept.push(*line);
    }
    // Don't open on blank lines.
    while kept.last().is_some_and(|l| l.is_empty()) {
        kept.pop();
    }
    kept.reverse();
    Some(kept.join("\n"))
}

/// The prompt the agent gets: the request, then the terminal's output (if
/// any) in a fence longer than any backtick run inside it.
pub fn compose_prompt(request: &str, screen: Option<&str>) -> String {
    let request = request.trim();
    let Some(screen) = screen.filter(|s| !s.trim().is_empty()) else {
        return request.to_string();
    };
    let longest = screen.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!(
        "{request}\n\nThis is what my terminal was showing when I asked:\n\n{fence}\n{screen}\n{fence}"
    )
}

/// The line typed into the new terminal's shell: the agent, given the
/// prompt file's contents as its one argument. `--` keeps a request that
/// starts with a dash from reading as an option.
pub fn command_line(agent: AskAgent, prompt_file: &Path) -> String {
    format!(
        "{} -- \"$(cat {})\"",
        agent.command(),
        crate::agent_session::shell_quote(&prompt_file.to_string_lossy())
    )
}

/// Prompt files older than this are swept when a new one is written. They
/// outlive the ask so the shell's history entry still works for a while.
const PROMPT_FILE_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Write `prompt` to a new file only the user can read, under the runtime
/// directory (a tmpfs on systemd systems, cleared at logout).
pub fn write_prompt(dir: &Path, prompt: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    sweep(dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let path = dir.join(format!("ask-{}-{stamp}.txt", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(prompt.as_bytes())?;
    Ok(path)
}

fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let stale = e.file_name().to_string_lossy().starts_with("ask-")
            && e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > PROMPT_FILE_TTL);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Where prompt files go.
pub fn prompt_dir() -> PathBuf {
    gtk4::glib::user_runtime_dir().join("vmux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_tail_drops_trailing_blanks_and_keeps_the_end() {
        let text = "one\ntwo  \nthree\n\n   \n\n";
        assert_eq!(
            screen_tail(text, 80, 8000).as_deref(),
            Some("one\ntwo\nthree")
        );
        assert_eq!(screen_tail(text, 2, 8000).as_deref(), Some("two\nthree"));
        assert_eq!(screen_tail("\n  \n", 80, 8000), None);
        assert_eq!(screen_tail("", 80, 8000), None);
    }

    #[test]
    fn screen_tail_fits_the_char_budget_but_keeps_one_line() {
        assert_eq!(
            screen_tail("aaaa\nbbbb\ncccc", 80, 10).as_deref(),
            Some("bbbb\ncccc")
        );
        assert_eq!(
            screen_tail("a very long line", 80, 4).as_deref(),
            Some("a very long line")
        );
        // A cut landing on a blank line doesn't open the excerpt with it.
        assert_eq!(screen_tail("x\n\nlast", 2, 8000).as_deref(), Some("last"));
    }

    #[test]
    fn prompt_is_the_request_alone_without_output() {
        assert_eq!(compose_prompt("  fix the build \n", None), "fix the build");
        assert_eq!(compose_prompt("fix it", Some("  \n")), "fix it");
    }

    #[test]
    fn prompt_fences_output_past_its_own_backticks() {
        let p = compose_prompt("why?", Some("$ make\nerror"));
        assert_eq!(
            p,
            "why?\n\nThis is what my terminal was showing when I asked:\n\n```\n$ make\nerror\n```"
        );
        let p = compose_prompt("why?", Some("see ````x````"));
        assert!(p.ends_with("\n`````\nsee ````x````\n`````"));
    }

    #[test]
    fn command_line_reads_the_quoted_file() {
        assert_eq!(
            command_line(AskAgent::Claude, Path::new("/run/user/1000/vmux/ask-1.txt")),
            "claude -- \"$(cat '/run/user/1000/vmux/ask-1.txt')\""
        );
        assert_eq!(
            command_line(AskAgent::Codex, Path::new("/tmp/it's here")),
            "codex -- \"$(cat '/tmp/it'\\''s here')\""
        );
    }

    #[test]
    fn prompt_files_are_private_and_old_ones_swept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("vmux");
        let path = write_prompt(&sub, "hello\nworld").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\nworld");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&sub), 0o700);

        let old = sub.join("ask-old.txt");
        std::fs::write(&old, "x").unwrap();
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(std::time::SystemTime::now() - PROMPT_FILE_TTL * 2)
            .unwrap();
        let other = sub.join("keep-me");
        std::fs::write(&other, "x").unwrap();
        write_prompt(&sub, "again").unwrap();
        assert!(!old.exists());
        assert!(other.exists());
        assert!(path.exists());
    }
}
