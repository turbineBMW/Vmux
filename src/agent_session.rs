//! Reopening coding-agent sessions after a restart.
//!
//! A Claude Code `SessionStart` hook (`vmux-relay --agent-hook claude`)
//! reports the session id of the agent that just started. Claude runs its
//! hooks without a controlling terminal, so the hook finds the terminal
//! through its parent — Claude itself, whose stdout is the terminal — and
//! writes a vte termprop OSC there carrying the id. The relay passes it up
//! untouched, so it lands on exactly the terminal the agent runs in. vmux
//! keeps it with the tab, saves it, and on restore types the agent's resume
//! command into the tab's new shell.
//!
//! Anything printed to a terminal can carry that OSC, so a report is only
//! taken while the agent it names is in the foreground, and ids are
//! restricted to a shape that's safe on a command line (and quoted anyway).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// The termprop session reports travel over.
pub const SESSION_TERMPROP_NAME: &str = "vte.ext.vmux.session";

/// The flag that makes `vmux-relay` act as the hook instead of a relay.
pub const HOOK_FLAG: &str = "--agent-hook";

/// Agents vmux can resume: (name in reports, foreground command it runs as,
/// resume flag).
const RESUMABLE: &[(&str, &str, &str)] = &[("claude", "claude", "--resume")];

/// One agent session, as saved with its tab.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    pub agent: String,
    pub id: String,
}

impl AgentSession {
    /// A session vmux knows how to resume, with a well-formed id; `None`
    /// otherwise.
    pub fn new(agent: &str, id: &str) -> Option<Self> {
        let known = RESUMABLE.iter().any(|(a, _, _)| *a == agent);
        let id_ok = (1..=64).contains(&id.len())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        (known && id_ok).then(|| Self {
            agent: agent.to_string(),
            id: id.to_string(),
        })
    }

    fn entry(&self) -> Option<&'static (&'static str, &'static str, &'static str)> {
        RESUMABLE.iter().find(|(a, _, _)| *a == self.agent)
    }

    /// The foreground command the agent runs as: a report only counts while
    /// it is in front.
    pub fn command(&self) -> Option<&'static str> {
        self.entry().map(|(_, command, _)| *command)
    }

    /// The shell command that reopens this session. `None` for a session
    /// that doesn't validate (e.g. hand-edited state).
    pub fn resume_command(&self) -> Option<String> {
        let valid = Self::new(&self.agent, &self.id)?;
        let (_, command, flag) = valid.entry()?;
        Some(format!("{command} {flag} {}", shell_quote(&valid.id)))
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// What the termprop carries. `seq` only makes repeated reports of the same
/// session distinct values: vte ignores an assignment that changes nothing.
#[derive(Serialize, Deserialize)]
struct Payload {
    agent: String,
    id: String,
    #[serde(default)]
    seq: u64,
}

/// The termprop OSC reporting `session`.
pub fn encode_termprop(session: &AgentSession, seq: u64) -> Vec<u8> {
    let json = serde_json::to_vec(&Payload {
        agent: session.agent.clone(),
        id: session.id.clone(),
        seq,
    })
    .unwrap_or_default();
    crate::osc_scan::osc666_termprop(SESSION_TERMPROP_NAME, &json)
}

/// A report read back from the termprop, validated.
pub fn parse_payload(data: &[u8]) -> Option<AgentSession> {
    let p: Payload = serde_json::from_slice(data).ok()?;
    AgentSession::new(&p.agent, &p.id)
}

// ----- the hook -------------------------------------------------------------

/// `vmux-relay --agent-hook <agent>`: read the hook's JSON on stdin and
/// report its session to the terminal. Always exits 0 and quietly — a hook
/// must never get in the agent's way — and does nothing outside vmux.
pub fn hook_main(agent: &str) -> i32 {
    if std::env::var("TERM_PROGRAM").as_deref() != Ok("vmux") {
        return 0;
    }
    let mut input = String::new();
    use std::io::Read;
    if std::io::stdin()
        .take(1 << 16)
        .read_to_string(&mut input)
        .is_err()
    {
        return 0;
    }
    let Some(session) = serde_json::from_str::<Value>(&input)
        .ok()
        .and_then(|v| v.get("session_id")?.as_str().map(str::to_string))
        .and_then(|id| AgentSession::new(agent, &id))
    else {
        return 0;
    };
    let Some(tty) = agent_terminal() else {
        return 0;
    };
    let seq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or_default();
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(tty) {
        use std::io::Write;
        let _ = f.write_all(&encode_termprop(&session, seq));
    }
    0
}

/// The terminal the agent runs in: the stdout of the nearest ancestor whose
/// stdout is a pty. Claude starts hooks in a session of their own, without a
/// controlling terminal, so /dev/tty isn't an option.
fn agent_terminal() -> Option<PathBuf> {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..8 {
        if pid <= 1 {
            return None;
        }
        if let Ok(out) = std::fs::read_link(format!("/proc/{pid}/fd/1"))
            && out.starts_with("/dev/pts/")
        {
            return Some(out);
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The parent pid is the second field after the parenthesized name,
        // which may itself contain spaces or parentheses.
        pid = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
    }
    None
}

// ----- installing the hook ------------------------------------------------------

/// Claude Code's user settings file.
pub fn claude_settings_path() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(crate::state::home_dir()).join(".claude"))
        .join("settings.json")
}

/// The hook command for the relay at `relay`.
pub fn hook_command(relay: &str) -> String {
    format!("{} {HOOK_FLAG} claude", shell_quote(relay))
}

/// Whether a hook entry is ours, from any vmux install.
fn is_our_hook(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains("vmux-relay") && c.contains(HOOK_FLAG))
}

/// `settings` with exactly one vmux SessionStart hook, running `command`.
/// Everything else is left as it was, in its order.
pub fn with_hook(settings: Value, command: &str) -> Value {
    let mut settings = without_hook(settings);
    if !settings.is_object() {
        settings = json!({});
    }
    let hooks = settings
        .as_object_mut()
        .expect("object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let start = hooks
        .as_object_mut()
        .expect("object")
        .entry("SessionStart")
        .or_insert_with(|| json!([]));
    if !start.is_array() {
        *start = json!([]);
    }
    start.as_array_mut().expect("array").push(json!({
        "matcher": "",
        "hooks": [{ "type": "command", "command": command, "timeout": 10 }],
    }));
    settings
}

/// `settings` without any vmux hook, dropping a group, the SessionStart list
/// or the hooks table only when removing ours emptied it.
pub fn without_hook(mut settings: Value) -> Value {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return settings;
    };
    let Some(start) = hooks.get_mut("SessionStart").and_then(Value::as_array_mut) else {
        return settings;
    };
    let before = start.len();
    let mut touched = false;
    for group in start.iter_mut() {
        if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            let n = list.len();
            list.retain(|h| !is_our_hook(h));
            touched |= list.len() != n;
        }
    }
    if !touched {
        return settings;
    }
    start.retain(|g| {
        g.get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|l| !l.is_empty())
    });
    if start.is_empty() && before > 0 {
        hooks.remove("SessionStart");
        if hooks.is_empty() {
            settings.as_object_mut().expect("object").remove("hooks");
        }
    }
    settings
}

/// Add (`Some(relay)`) or remove (`None`) the hook in Claude's settings.
/// The file is only written when it changes, atomically, and never when it
/// isn't valid JSON — that is reported instead.
pub fn set_claude_hook(relay: Option<&str>) -> Result<(), String> {
    let path = claude_settings_path();
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("{} isn't valid JSON ({e}); not changing it", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if relay.is_none() {
                return Ok(());
            }
            json!({})
        }
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let updated = match relay {
        Some(relay) => with_hook(current.clone(), &hook_command(relay)),
        None => without_hook(current.clone()),
    };
    if updated == current {
        return Ok(());
    }
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.vmux-tmp");
        let mut text = serde_json::to_string_pretty(&updated).unwrap_or_default();
        text.push('\n');
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    };
    write().map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "4defbfd4-3aef-4516-aa58-fd5f3eb4fb6e";

    #[test]
    fn only_known_agents_and_plain_ids_validate() {
        assert!(AgentSession::new("claude", ID).is_some());
        assert!(AgentSession::new("codex", ID).is_none());
        assert!(AgentSession::new("claude", "").is_none());
        assert!(AgentSession::new("claude", "x; rm -rf ~").is_none());
        assert!(AgentSession::new("claude", "$(id)").is_none());
        assert!(AgentSession::new("claude", &"a".repeat(65)).is_none());
    }

    #[test]
    fn resume_command_quotes_and_revalidates() {
        let s = AgentSession::new("claude", ID).unwrap();
        assert_eq!(s.command(), Some("claude"));
        assert_eq!(
            s.resume_command().unwrap(),
            format!("claude --resume '{ID}'")
        );
        // Saved state is hand-editable: re-check before building a command.
        let forged = AgentSession {
            agent: "claude".into(),
            id: "'; reboot; '".into(),
        };
        assert_eq!(forged.resume_command(), None);
    }

    #[test]
    fn termprop_payload_round_trips() {
        let s = AgentSession::new("claude", ID).unwrap();
        let osc = encode_termprop(&s, 7);
        assert!(osc.starts_with(b"\x1b]666;vte.ext.vmux.session="));
        let json = serde_json::to_vec(&Payload {
            agent: "claude".into(),
            id: ID.into(),
            seq: 7,
        })
        .unwrap();
        assert_eq!(parse_payload(&json), Some(s));
        assert_eq!(parse_payload(br#"{"agent":"claude","id":"$(id)"}"#), None);
        assert_eq!(parse_payload(b"not json"), None);
    }

    #[test]
    fn installing_keeps_everything_else_in_order() {
        let settings: Value = serde_json::from_str(
            r#"{
                "theme": "dark",
                "hooks": {
                    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "audit"}]}],
                    "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "hello"}]}]
                },
                "model": "opus"
            }"#,
        )
        .unwrap();
        let cmd = hook_command("/home/u/.local/bin/vmux-relay");
        let installed = with_hook(settings.clone(), &cmd);
        let keys: Vec<_> = installed.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["theme", "hooks", "model"]);
        let start = installed["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2);
        assert_eq!(start[0]["hooks"][0]["command"], "hello");
        assert_eq!(start[1]["hooks"][0]["command"], cmd.as_str());
        // Installing again (e.g. from a moved binary) replaces, not adds.
        let again = with_hook(installed.clone(), &hook_command("/opt/vmux/vmux-relay"));
        assert_eq!(again["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        // Removing restores the original exactly.
        assert_eq!(without_hook(installed), settings);
    }

    #[test]
    fn removing_drops_only_what_it_emptied() {
        let cmd = hook_command("/usr/bin/vmux-relay");
        let installed = with_hook(json!({}), &cmd);
        assert_eq!(installed["hooks"]["SessionStart"][0]["matcher"], "");
        assert_eq!(without_hook(installed), json!({}));
        // Nothing of ours: untouched, empty lists included.
        let theirs = json!({"hooks": {"SessionStart": []}});
        assert_eq!(without_hook(theirs.clone()), theirs);
        assert_eq!(without_hook(json!(["odd"])), json!(["odd"]));
    }

    #[test]
    fn hook_command_quotes_the_path() {
        assert_eq!(
            hook_command("/home/a b/vmux-relay"),
            "'/home/a b/vmux-relay' --agent-hook claude"
        );
    }
}
