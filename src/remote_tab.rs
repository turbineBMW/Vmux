//! The machine a tab's remote session (ssh, mosh, et, telnet) is on, for the
//! tab title and tooltip.
//!
//! vmux-relay reports the foreground remote command's argv; the destination
//! parsed from it is the host as typed (often an ssh_config alias), and
//! `ssh -G` resolves what that really connects to. When the remote shell
//! emits OSC 7 with a hostname of its own, that wins as the label: it names
//! the innermost machine after further hops (`ssh bastion` → `ssh db`).

use gtk4::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use vmux::remote::{self, Destination, Resolved};
use vte4 as vte;
use vte4::prelude::*;

/// What a tab shows about its remote session.
pub struct RemoteHost {
    /// Short host label for the tab title.
    pub label: String,
    /// Pango markup for the tab tooltip.
    pub tooltip: String,
}

/// Object-data key for the window title a terminal had when its remote
/// session started (see `mark_session_start`).
const TITLE_AT_START_KEY: &str = "vmux-title-at-remote-start";

/// `ssh -G` outcomes keyed by the argv they resolve; None while in flight or
/// after a failure, so each distinct command line spawns ssh at most once.
type Cache = HashMap<Vec<String>, Option<Resolved>>;

thread_local! {
    static RESOLVED: RefCell<Cache> = RefCell::new(HashMap::new());
}

/// Bound on the cache; clearing it just means re-running `ssh -G` later.
const CACHE_CAP: usize = 128;
const SSH_G_TIMEOUT: Duration = Duration::from_secs(2);

/// The foreground remote command's argv, empty when there is none.
fn argv_of(term: &vte::Terminal) -> Vec<String> {
    remote::split_argv(&term.termprop_data(remote::REMOTE_TERMPROP_NAME))
}

/// Call when the remote termprop changes. Remembers the window title the
/// session started under: the local shell's title lingers until the remote
/// side sets one, and prefixing the host to a stale local title would claim
/// the remote machine is in a local directory.
#[allow(deprecated)] // window_title: see the connect_* note in pane::new_tab
pub fn mark_session_start(term: &vte::Terminal) {
    let title = argv_of(term).first().map(|_| {
        term.window_title()
            .map(|t| t.to_string())
            .unwrap_or_default()
    });
    // SAFETY: this key is only ever read back as Option<String>, below.
    unsafe { term.set_data(TITLE_AT_START_KEY, title) };
}

/// The window title set since the remote session started, or None while the
/// title is still the one inherited from before it.
#[allow(deprecated)]
pub fn fresh_title(term: &vte::Terminal) -> Option<String> {
    let title = term.window_title().filter(|t| !t.is_empty())?.to_string();
    // SAFETY: TITLE_AT_START_KEY only ever holds Option<String>.
    let stale = unsafe { term.data::<Option<String>>(TITLE_AT_START_KEY) }
        .and_then(|p| unsafe { p.as_ref() }.clone());
    (stale.as_deref() != Some(title.as_str())).then_some(title)
}

/// The remote host of `term`'s foreground session, or None when it isn't
/// running one. A first sighting of a command line starts its `ssh -G`
/// resolution in the background and calls `on_resolved` once it lands, so
/// the caller can refresh the tooltip.
pub fn host_of(term: &vte::Terminal, on_resolved: impl FnOnce() + 'static) -> Option<RemoteHost> {
    let argv = argv_of(term);
    let dest = remote::destination(&argv)?;
    let resolved = lookup(&argv, &dest, on_resolved);
    // The remote shell's OSC 7 naming the machine ssh was pointed at (by
    // alias or by resolved name) adds nothing; keep the name as typed.
    let nested = osc7_host(term).filter(|h| {
        !remote::is_local_host(h, &dest.host)
            && !resolved
                .as_ref()
                .is_some_and(|r| remote::is_local_host(h, &r.hostname))
    });

    let command = remote::command_name(&argv[0]);
    let mut lines = vec![format!("<b>{command}</b> {}", esc(&dest.display()))];
    if let Some(r) = &resolved {
        let shown = resolved_display(r);
        if shown != dest.display() {
            lines.push(format!("→ {}", esc(&shown)));
        }
    }
    if let Some(h) = &nested {
        lines.push(format!("now on <b>{}</b>", esc(h)));
    }
    Some(RemoteHost {
        label: nested.unwrap_or(dest.host),
        tooltip: lines.join("\n"),
    })
}

/// The hostname in the terminal's OSC 7 cwd URI when it names another
/// machine. vte keeps the last OSC 7 it saw, so this is only asked while a
/// remote command is in front — after it exits, a remote host lingering
/// there until the local shell's next OSC 7 must not be read as remote.
#[allow(deprecated)] // current_directory_uri: see term::cwd_of
fn osc7_host(term: &vte::Terminal) -> Option<String> {
    let uri = term.current_directory_uri()?;
    let (_, host) = glib::filename_from_uri(&uri).ok()?;
    let host = host?.to_string();
    (!remote::is_local_host(&host, &glib::host_name())).then_some(host)
}

fn lookup(
    argv: &[String],
    dest: &Destination,
    on_resolved: impl FnOnce() + 'static,
) -> Option<Resolved> {
    if let Some(hit) = RESOLVED.with_borrow(|c| c.get(argv).cloned()) {
        return hit;
    }
    let args = remote::ssh_g_args(argv, dest)?;
    RESOLVED.with_borrow_mut(|c| {
        if c.len() >= CACHE_CAP {
            c.clear();
        }
        c.insert(argv.to_vec(), None);
    });
    let key = argv.to_vec();
    glib::spawn_future_local(async move {
        let r = gio::spawn_blocking(move || ssh_g(&args))
            .await
            .ok()
            .flatten();
        if r.is_some() {
            RESOLVED.with_borrow_mut(|c| c.insert(key, r));
            on_resolved();
        }
    });
    None
}

/// Run `ssh -G <args>` (prints the effective config, never connects).
/// Killed after a timeout: a `Match exec` in ssh_config can run anything.
/// posix_spawn on a worker thread, for the reasons in git::git_output.
fn ssh_g(args: &[String]) -> Option<Resolved> {
    let mut child = std::process::Command::new("ssh")
        .arg("-G")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < SSH_G_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    remote::parse_ssh_g(&String::from_utf8_lossy(&out.stdout))
}

/// `user@hostname`, plus `:port` when it isn't ssh's default.
fn resolved_display(r: &Resolved) -> String {
    let mut s = format!("{}@{}", r.user, r.hostname);
    if r.port != "22" {
        s.push(':');
        s.push_str(&r.port);
    }
    s
}

fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}
