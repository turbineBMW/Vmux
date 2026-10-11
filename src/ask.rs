//! Asking a coding agent from anywhere: a request typed into a popup goes to
//! the user's own agent CLI (Claude Code or Codex). By default the agent is
//! asked, one-shot, for a single shell command, which lands at the prompt of
//! the terminal they were in, unrun, for them to review ([`AskMode::Command`]).
//! Or the agent starts in a terminal next to theirs, in its directory, to work
//! on the request ([`AskMode::Agent`]). Optionally the request carries that
//! terminal's recent output, so "why did this fail?" has something to go on.
//!
//! For a command, the agent runs without tools, hooks or a saved session, on
//! a quick model: it only has to answer.
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

/// What the popup does with a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AskMode {
    /// Ask for one shell command and put it at the prompt, unrun.
    #[default]
    Command,
    /// Start the agent next to the asking terminal to work on the request.
    Agent,
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
    let fence = fence_for(screen);
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

/// Where the command will run, told to the agent along with the request.
#[derive(Default)]
pub struct CommandContext<'a> {
    /// The shell's name, e.g. `bash`.
    pub shell: &'a str,
    pub os: Option<&'a str>,
    pub cwd: Option<&'a str>,
    /// Some of the entries in `cwd`, directories marked with a trailing `/`.
    pub files: &'a [String],
    /// The asking terminal's recent output.
    pub screen: Option<&'a str>,
}

/// The prompt that asks for a command: the rules for the reply, what's known
/// about where it will run, then the request.
pub fn command_prompt(request: &str, cx: &CommandContext) -> String {
    let mut p = String::from(
        "Turn the request at the end into a single shell command. It will be put at my \
         shell prompt for me to review and run myself.\n\n\
         Reply with the command and nothing else: no explanation, no Markdown, no code \
         fence. Keep it to one line, chaining steps with && or pipes. If a shell \
         command can't do it, reply with one line starting with \"# \" that says why.\n\n",
    );
    p.push_str(&format!("Shell: {}\n", cx.shell));
    if let Some(os) = cx.os {
        p.push_str(&format!("Operating system: {os}\n"));
    }
    if let Some(cwd) = cx.cwd {
        p.push_str(&format!("Working directory: {cwd}\n"));
    }
    if !cx.files.is_empty() {
        p.push_str(&format!("Files there: {}\n", cx.files.join(", ")));
    }
    if let Some(screen) = cx.screen.filter(|s| !s.trim().is_empty()) {
        let fence = fence_for(screen);
        p.push_str(&format!(
            "\nMy terminal's recent output:\n\n{fence}\n{screen}\n{fence}\n"
        ));
    }
    p.push_str(&format!("\nRequest: {}", request.trim()));
    p
}

/// The argv that asks `agent` one question, the prompt on stdin. Claude
/// answers on stdout; Codex writes its answer to `answer_file`.
pub fn command_argv(agent: AskAgent, answer_file: &Path) -> Vec<String> {
    let args: &[&str] = match agent {
        AskAgent::Claude => &[
            "claude",
            "-p",
            "--model",
            "haiku",
            "--tools",
            "",
            "--no-session-persistence",
            "--settings",
            r#"{"disableAllHooks":true}"#,
        ],
        AskAgent::Codex => &[
            "codex",
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "-c",
            r#"model_reasoning_effort="low""#,
            "--output-last-message",
        ],
    };
    let mut argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    if agent == AskAgent::Codex {
        argv.push(answer_file.to_string_lossy().into_owned());
        // The prompt comes on stdin.
        argv.push("-".into());
    }
    argv
}

/// The command in an agent's reply: control characters dropped, a code fence
/// or wrapping backticks and a `$ ` prompt taken off. `None` when nothing is
/// left.
pub fn clean_command(reply: &str) -> Option<String> {
    let reply: String = reply
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect();
    let mut lines: Vec<&str> = reply.trim().lines().collect();
    if lines
        .first()
        .is_some_and(|l| l.trim_start().starts_with("```"))
    {
        lines.remove(0);
        if let Some(end) = lines.iter().position(|l| l.trim_start().starts_with("```")) {
            lines.truncate(end);
        }
    }
    let body = lines.iter().filter(|l| !l.trim().is_empty());
    if body.clone().count() > 0 && body.clone().all(|l| l.starts_with("$ ")) {
        for l in &mut lines {
            *l = l.strip_prefix("$ ").unwrap_or(l);
        }
    }
    let mut cmd = lines.join("\n").trim().to_string();
    if !cmd.contains('\n')
        && cmd.len() > 2
        && cmd.starts_with('`')
        && cmd.ends_with('`')
        && !cmd[1..cmd.len() - 1].contains('`')
    {
        cmd = cmd[1..cmd.len() - 1].trim().to_string();
    }
    (!cmd.is_empty()).then_some(cmd)
}

/// The distribution's name from os-release text (`PRETTY_NAME`, else `NAME`).
pub fn os_release_name(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    };
    field("PRETTY_NAME").or_else(|| field("NAME"))
}

/// This machine's OS, for the command prompt.
pub fn os_name() -> Option<String> {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| os_release_name(&t))
        .or_else(|| Some(std::env::consts::OS.to_string()))
}

/// Up to `max` entries of `dir`, sorted, directories marked with `/`.
pub fn list_dir(dir: &Path, max: usize) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    names.truncate(max);
    names
}

/// A backtick fence longer than any backtick run in `text`.
fn fence_for(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
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
    fn command_prompt_carries_the_context_and_the_request() {
        let files = vec!["Cargo.toml".to_string(), "src/".to_string()];
        let p = command_prompt(
            " run the tests \n",
            &CommandContext {
                shell: "bash",
                os: Some("Arch Linux"),
                cwd: Some("/home/me/vmux"),
                files: &files,
                screen: Some("$ cargo test\nerror"),
            },
        );
        assert!(p.contains("\nShell: bash\n"));
        assert!(p.contains("\nOperating system: Arch Linux\n"));
        assert!(p.contains("\nWorking directory: /home/me/vmux\n"));
        assert!(p.contains("\nFiles there: Cargo.toml, src/\n"));
        assert!(p.contains("\n```\n$ cargo test\nerror\n```\n"));
        assert!(p.ends_with("\nRequest: run the tests"));

        let bare = command_prompt(
            "x",
            &CommandContext {
                shell: "fish",
                ..Default::default()
            },
        );
        assert!(!bare.contains("Working directory"));
        assert!(!bare.contains("recent output"));
    }

    #[test]
    fn command_argv_asks_without_tools_or_a_session() {
        let file = Path::new("/run/user/1000/vmux/ask-answer.txt");
        let claude = command_argv(AskAgent::Claude, file);
        assert_eq!(claude[..2], ["claude", "-p"]);
        assert!(claude.windows(2).any(|w| w == ["--tools", ""]));
        assert!(!claude.iter().any(|a| a.contains("ask-answer")));
        let codex = command_argv(AskAgent::Codex, file);
        assert_eq!(codex[..2], ["codex", "exec"]);
        assert!(codex.windows(2).any(|w| w == ["--sandbox", "read-only"]));
        assert_eq!(
            codex[codex.len() - 3..],
            [
                "--output-last-message",
                "/run/user/1000/vmux/ask-answer.txt",
                "-"
            ]
        );
    }

    #[test]
    fn clean_command_unwraps_what_models_add() {
        assert_eq!(
            clean_command("find . -size +100M\n").as_deref(),
            Some("find . -size +100M")
        );
        assert_eq!(
            clean_command("```bash\nls -la | head\n```\n").as_deref(),
            Some("ls -la | head")
        );
        assert_eq!(clean_command("`du -sh *`").as_deref(), Some("du -sh *"));
        assert_eq!(clean_command("$ git status").as_deref(), Some("git status"));
        assert_eq!(clean_command("echo `date`").as_deref(), Some("echo `date`"));
        assert_eq!(clean_command("ls\x1b[0m\x07").as_deref(), Some("ls[0m"));
        assert_eq!(clean_command("  \n```\n```"), None);
    }

    #[test]
    fn os_release_name_prefers_the_pretty_name() {
        assert_eq!(
            os_release_name("NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n")
                .as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(os_release_name("NAME=Fedora\n").as_deref(), Some("Fedora"));
        assert_eq!(os_release_name("ID=x\n"), None);
    }

    #[test]
    fn list_dir_marks_directories_and_caps_the_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        assert_eq!(
            list_dir(dir.path(), 10),
            vec!["Cargo.toml", "README.md", "src/"]
        );
        assert_eq!(list_dir(dir.path(), 1), vec!["Cargo.toml"]);
        assert!(list_dir(&dir.path().join("missing"), 10).is_empty());
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
