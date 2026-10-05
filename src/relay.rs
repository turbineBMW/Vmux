//! vmux-relay: a script(1)-style PTY shim between vte and the user's shell.
//! The `vmux-relay` binary calls [`main`].
//!
//! vte spawns `vmux-relay <command...>` with the outer pty slave on fds
//! 0/1/2. The relay runs the command on an inner pty of its own and pumps
//! bytes both ways unmodified, watching the output stream for the
//! desktop-notification OSCs (9 / 777 / kitty 99); each one found is
//! re-emitted upstream as a vte termprop OSC (666) that vmux receives via
//! the termprop-changed signal. libvte offers no hook for unknown OSC
//! sequences, hence this shim. It also reports the inner pty's foreground
//! command (tab titles, root/remote tab colors) and, for ssh and friends,
//! that command's argv (the remote host shown on the tab).
//!
//! Teardown relies on the kernel: when vte closes the outer master the
//! relay (foreground process group of the outer pty) gets a default-action
//! SIGHUP and dies, which closes the inner master and HUPs the shell.

use crate::osc_scan::{Scanner, encode_fgproc_termprop, encode_termprop};
use crate::remote::{encode_remote_termprop, is_remote_command};
use std::ffi::CString;
use std::os::unix::ffi::OsStringExt;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// Self-pipe write end for the SIGWINCH/SIGCHLD handler.
static SELF_PIPE_W: AtomicI32 = AtomicI32::new(-1);

extern "C" fn wake(_sig: libc::c_int) {
    let fd = SELF_PIPE_W.load(Ordering::Relaxed);
    if fd >= 0 {
        // EAGAIN means a wakeup is already pending — fine either way.
        unsafe { libc::write(fd, b"w".as_ptr().cast(), 1) };
    }
}

/// The relay's entry point: runs `<command> [args...]` from the process's
/// own arguments and exits with its status. The `vmux-relay` binary is only
/// this call, so every front-end can ship the same relay.
pub fn main() -> ! {
    let args: Vec<CString> = std::env::args_os()
        .skip(1)
        .map(|a| CString::new(a.into_vec()).unwrap_or_default())
        .collect();
    if args.is_empty() || args.iter().any(|a| a.is_empty()) {
        eprintln!("usage: vmux-relay <command> [args...]");
        std::process::exit(127);
    }
    std::process::exit(run(&args));
}

fn run(args: &[CString]) -> i32 {
    let orig = tcgetattr(0);
    let ws = winsize_of(0).unwrap_or(libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    });

    // Self-pipe and handlers go in before fork/openpty so neither an early
    // child exit nor an early resize can be missed.
    let mut pipe_fds = [0i32; 2];
    if unsafe { libc::pipe2(pipe_fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
        eprintln!("vmux-relay: pipe2 failed");
        return 126;
    }
    SELF_PIPE_W.store(pipe_fds[1], Ordering::Relaxed);
    install_wake_handler(libc::SIGWINCH);
    install_wake_handler(libc::SIGCHLD);

    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let termp = orig
        .as_ref()
        .map_or(std::ptr::null(), |t| t as *const libc::termios);
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            termp.cast_mut(),
            &ws as *const libc::winsize as *mut libc::winsize,
        )
    } < 0
    {
        eprintln!("vmux-relay: openpty failed");
        return 126;
    }

    let child = unsafe { libc::fork() };
    if child < 0 {
        eprintln!("vmux-relay: fork failed");
        return 126;
    }
    if child == 0 {
        // Child: become the session leader on the inner pty and exec the
        // command. Handler dispositions reset on exec; the pipe is CLOEXEC.
        unsafe {
            libc::setsid();
            libc::ioctl(slave, libc::TIOCSCTTY, 0);
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            if slave > 2 {
                libc::close(slave);
            }
            libc::close(master);
            // Ignored dispositions survive exec (GTK ignores SIGPIPE
            // process-wide, launchers vary); give the command a clean
            // slate like vte's own spawn does.
            for sig in 1..32 {
                libc::signal(sig, libc::SIG_DFL);
            }
            let argv: Vec<*const libc::c_char> = args
                .iter()
                .map(|a| a.as_ptr())
                .chain(std::iter::once(std::ptr::null()))
                .collect();
            libc::execvp(args[0].as_ptr(), argv.as_ptr());
            let msg = format!("vmux-relay: exec {:?} failed\r\n", args[0]);
            libc::write(2, msg.as_ptr().cast(), msg.len());
            libc::_exit(127);
        }
    }
    unsafe {
        libc::close(slave);
        // Ptys report EIO rather than SIGPIPE, but manual pipe-driven runs
        // (vmux-relay ... | head) should get EPIPE errors, not death.
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    // Raw mode on the outer pty is mandatory: its line discipline would
    // otherwise echo every keystroke back to vte (the inner shell already
    // echoes), turn ^C into SIGINT for the relay instead of a byte for the
    // inner shell's job control, and re-cook \n into \r\n on output.
    if let Some(t) = &orig {
        let mut raw = *t;
        unsafe {
            libc::cfmakeraw(&mut raw);
            libc::tcsetattr(0, libc::TCSANOW, &raw);
        }
    }
    set_nonblock(0);
    set_nonblock(1);
    set_nonblock(master);

    let code = pump(master, pipe_fds[0], child);

    if let Some(t) = &orig {
        unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, t) };
    }
    code
}

enum IoRes {
    Done(usize),
    Again,
    /// EOF, EIO (pty side fully closed) or EPIPE.
    Closed,
}

fn read_fd(fd: i32, buf: &mut [u8]) -> IoRes {
    match unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) } {
        0 => IoRes::Closed,
        n if n > 0 => IoRes::Done(n as usize),
        _ => match errno() {
            libc::EAGAIN | libc::EINTR => IoRes::Again,
            _ => IoRes::Closed,
        },
    }
}

fn write_fd(fd: i32, buf: &[u8]) -> IoRes {
    match unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) } {
        n if n >= 0 => IoRes::Done(n as usize),
        _ => match errno() {
            libc::EAGAIN | libc::EINTR => IoRes::Again,
            _ => IoRes::Closed,
        },
    }
}

/// The foreground process group's command name on the inner pty, for use
/// as a tab title and for agent detection (see [`command_name`]), plus its
/// raw /proc cmdline when it is a remote session (empty otherwise). Prefers
/// /proc/<pgid>/cmdline (untruncated argv) and falls back to
/// /proc/<pgid>/comm; None on any read failure (caller retries).
fn fg_name(pgid: libc::pid_t) -> Option<(String, Vec<u8>)> {
    if let Ok(cmdline) = std::fs::read(format!("/proc/{pgid}/cmdline")) {
        let argv: Vec<String> = cmdline
            .split(|&b| b == 0)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        let name = command_name(&argv);
        if !name.is_empty() {
            let remote = if is_remote_command(&argv[0]) {
                cmdline.clone()
            } else {
                Vec::new()
            };
            return Some((name, remote));
        }
    }
    let comm = std::fs::read_to_string(format!("/proc/{pgid}/comm")).ok()?;
    let comm = comm.trim_end_matches('\n');
    (!comm.is_empty()).then(|| (comm.to_string(), Vec::new()))
}

/// The name a command goes by: argv[0]'s basename, except that a script run
/// by an interpreter (node, bun, deno, python) is named after the script —
/// `node /usr/bin/gemini` is "gemini", not "node" — and an npm package's
/// generic entry point (`…/@google/gemini-cli/dist/index.js`) after the
/// command that package installs. Empty when argv[0] is.
pub fn command_name(argv: &[String]) -> String {
    let Some(arg0) = argv.first() else {
        return String::new();
    };
    let base = basename(arg0);
    if !is_interpreter(base) {
        return base.to_string();
    }
    // The script: the first argument that isn't an option (or deno's `run`).
    let script = argv[1..]
        .iter()
        .find(|a| !a.starts_with('-') && !(base == "deno" && a.as_str() == "run"));
    let Some(script) = script else {
        return base.to_string();
    };
    if let Some(command) = command_of_package(script) {
        return command.to_string();
    }
    let name = basename(script);
    let stem = [".js", ".mjs", ".cjs", ".ts", ".py"]
        .iter()
        .find_map(|ext| name.strip_suffix(ext))
        .unwrap_or(name);
    if stem.is_empty() {
        base.to_string()
    } else {
        stem.to_string()
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_interpreter(name: &str) -> bool {
    matches!(name, "node" | "nodejs" | "bun" | "deno")
        || name
            .strip_prefix("python")
            .is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// Coding agents published as npm packages, by package path, and the command
/// each installs: run straight from the package, their entry point is named
/// something generic like `index.js`.
fn command_of_package(script: &str) -> Option<&'static str> {
    const PACKAGES: &[(&str, &str)] = &[
        ("/node_modules/@anthropic-ai/claude-code/", "claude"),
        ("/node_modules/@openai/codex/", "codex"),
        ("/node_modules/@google/gemini-cli/", "gemini"),
        ("/node_modules/opencode-ai/", "opencode"),
        ("/node_modules/@github/copilot/", "copilot"),
    ];
    PACKAGES
        .iter()
        .find(|(path, _)| script.contains(path))
        .map(|(_, command)| *command)
}

/// Effective uid of the foreground process group leader, from the Uid: line
/// of /proc/<pgid>/status (fields: real, effective, saved, fs). The euid is
/// what matters for setuid binaries like sudo. None on any read/parse
/// failure — the payload then goes out name-only.
fn fg_uid(pgid: libc::pid_t) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pgid}/status")).ok()?;
    let uids = status.lines().find_map(|l| l.strip_prefix("Uid:"))?;
    uids.split_whitespace().nth(1)?.parse().ok()
}

/// The relay core: a poll loop over stdin (vte→shell), the inner pty master
/// (both directions) and stdout (shell→vte), plus the self-pipe.
///
/// Invariant: a side is only read while its direction's pending buffer is
/// empty — natural backpressure with bounded memory and no blocking writes.
fn pump(master: i32, pipe_r: i32, child: libc::pid_t) -> i32 {
    let mut scanner = Scanner::new();
    let mut seq: u64 = 0;

    let mut in_buf = [0u8; 4096]; // vte → shell
    let mut in_start = 0usize;
    let mut in_len = 0usize;
    let mut chunk = [0u8; 8192]; // shell → vte, pre-injection
    let mut out: Vec<u8> = Vec::new();
    let mut out_start = 0usize;

    let mut stdin_open = true;
    let mut status: Option<libc::c_int> = None;
    let mut drain_ticks = 0u32;
    // Foreground-command tracking on the inner pty (for tab titles and the
    // root/remote tab indicator): (command name, euid). last_sent starts as
    // ("", None), so the first check always emits one payload carrying the
    // shell's uid — required to color the tabs of a root-run vmux. The
    // remote-session argv travels separately and only when it changes.
    let mut last_pgid: libc::pid_t = -1;
    let mut last_sent: Option<(String, Option<u32>)> = Some((String::new(), None));
    let mut pending_fg: Option<(String, Option<u32>)> = None;
    let mut last_remote: Vec<u8> = Vec::new();
    let mut pending_remote: Option<Vec<u8>> = None;
    // A pgid change is not the only way the foreground command changes: a
    // wrapper script that ends in `exec` becomes a different program in the
    // same process group (~/.local/bin/claude is `#!/bin/bash` ... `exec mise
    // x claude`, so the pgid gate alone reports "bash" for the whole
    // session). Re-read on a throttle as well, but only while the loop is
    // awake for other reasons, so an idle terminal still costs nothing.
    const FG_RECHECK: Duration = Duration::from_millis(500);
    let mut last_fg_read = Instant::now();
    // Short poll ticks scheduled after user input, to catch a silent command
    // (e.g. `sleep`) taking the foreground without producing output. Lapses
    // back to a blocking wait when idle, so there are no wakeups at rest.
    let mut poll_ticks: u32 = 0;

    'outer: loop {
        let mut fds: Vec<libc::pollfd> = Vec::with_capacity(4);
        let mut push = |fd: i32, events: libc::c_short| {
            fds.push(libc::pollfd {
                fd,
                events,
                revents: 0,
            });
            fds.len() - 1
        };
        let pipe_idx = push(pipe_r, libc::POLLIN);
        let mut master_ev: libc::c_short = 0;
        if out_start >= out.len() {
            master_ev |= libc::POLLIN;
        }
        if in_len > 0 {
            master_ev |= libc::POLLOUT;
        }
        let master_idx = (master_ev != 0).then(|| push(master, master_ev));
        let stdin_idx = (stdin_open && in_len == 0).then(|| push(0, libc::POLLIN));
        let stdout_idx = (out_start < out.len()).then(|| push(1, libc::POLLOUT));

        // Once the shell is reaped, only drain what the pty still holds. While
        // running, block unless a recent keystroke armed the foreground poll
        // (then tick every 120ms until it lapses).
        let timeout: libc::c_int = if status.is_some() {
            50
        } else if poll_ticks > 0 || pending_fg.is_some() || pending_remote.is_some() {
            120
        } else {
            -1
        };
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if r < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            break;
        }
        if r == 0 {
            if status.is_some() {
                drain_ticks += 1;
                if out_start >= out.len() || drain_ticks > 10 {
                    break;
                }
                continue;
            }
            // Running: a foreground-poll tick. Fall through to the fg check
            // below; no fds are ready, so the revents handlers are no-ops.
            poll_ticks = poll_ticks.saturating_sub(1);
        }

        if fds[pipe_idx].revents != 0 {
            let mut sink = [0u8; 64];
            while let IoRes::Done(_) = read_fd(pipe_r, &mut sink) {}
            // SIGWINCH: mirror the outer pty's size onto the inner one.
            if let Some(ws) = winsize_of(0)
                && ws.ws_col != 0
                && ws.ws_row != 0
            {
                unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &ws) };
            }
            // SIGCHLD: reap, then keep draining the pty buffer.
            if status.is_none() {
                let mut st: libc::c_int = 0;
                if unsafe { libc::waitpid(child, &mut st, libc::WNOHANG) } == child {
                    status = Some(st);
                }
            }
        }

        if let Some(i) = master_idx {
            let re = fds[i].revents;
            if re & libc::POLLOUT != 0 && in_len > 0 {
                match write_fd(master, &in_buf[in_start..in_start + in_len]) {
                    IoRes::Done(n) => {
                        in_start += n;
                        in_len -= n;
                        if in_len == 0 {
                            in_start = 0;
                        }
                    }
                    IoRes::Again => {}
                    IoRes::Closed => break,
                }
            }
            if re & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 && out_start >= out.len() {
                match read_fd(master, &mut chunk) {
                    IoRes::Done(n) => {
                        out.clear();
                        out_start = 0;
                        // Splice termprop injections in at the exact offsets
                        // the scanner reports (just past each terminator) —
                        // chunk boundaries are not sequence boundaries.
                        let mut injections: Vec<(usize, Vec<u8>)> = Vec::new();
                        scanner.scan(&chunk[..n], &mut |off, notif| {
                            if injections.len() < 16 {
                                seq += 1;
                                injections.push((off, encode_termprop(&notif, seq)));
                            }
                        });
                        let mut prev = 0;
                        for (off, inj) in injections {
                            out.extend_from_slice(&chunk[prev..off]);
                            out.extend_from_slice(&inj);
                            prev = off;
                        }
                        out.extend_from_slice(&chunk[prev..n]);
                    }
                    IoRes::Closed => {
                        // Inner side fully gone: best-effort flush, then out.
                        flush_blocking(1, &out[out_start.min(out.len())..]);
                        break;
                    }
                    IoRes::Again => {
                        if status.is_some() && out_start >= out.len() {
                            break; // drained everything the shell left behind
                        }
                    }
                }
            }
        }

        if let Some(i) = stdin_idx
            && fds[i].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
        {
            match read_fd(0, &mut in_buf) {
                IoRes::Done(n) => {
                    in_start = 0;
                    in_len = n;
                    // A command may be about to take the foreground silently;
                    // poll briefly so its name reaches the tab title.
                    poll_ticks = 8;
                }
                // vte teardown arrives as SIGHUP (default death) before this
                // is ever seen; EOF here serves pipe-driven manual runs.
                IoRes::Closed => stdin_open = false,
                IoRes::Again => {}
            }
        }

        if let Some(i) = stdout_idx
            && fds[i].revents != 0
            && out_start < out.len()
        {
            match write_fd(1, &out[out_start..]) {
                IoRes::Done(n) => out_start += n,
                IoRes::Again => {}
                IoRes::Closed => break 'outer,
            }
        }

        // Tab-title / root-remote hint: report the inner pty's foreground
        // command and euid (empty name = the shell itself is in front). The
        // /proc reads happen only when the pgid changes; the termprop is
        // queued and spliced into the output at the next sequence boundary,
        // so it can never split another sequence.
        if status.is_none() {
            let pg = unsafe { libc::tcgetpgrp(master) };
            // The shell in front cannot exec into something else, so it needs
            // no recheck; anything else may still be settling.
            let recheck = pg > 0 && pg != child && last_fg_read.elapsed() >= FG_RECHECK;
            if pg != last_pgid || recheck {
                let name = if pg == child {
                    Some((String::new(), Vec::new()))
                } else {
                    fg_name(pg)
                };
                if let Some((name, remote)) = name {
                    last_pgid = pg;
                    last_fg_read = Instant::now();
                    // Uid read failure doesn't retry like a name failure: a
                    // name-only payload still titles the tab correctly.
                    let entry = (name, fg_uid(pg));
                    if last_sent.as_ref() != Some(&entry) {
                        pending_fg = Some(entry);
                    }
                    pending_remote = (remote != last_remote).then_some(remote);
                }
            }
            // Flush at any sequence boundary. If the output buffer is already
            // drained, reset it; otherwise append after the queued bytes —
            // still a boundary, since at_ground reflects the end of what is
            // queued, so the OSC can't split another sequence.
            if scanner.at_ground() && (pending_fg.is_some() || pending_remote.is_some()) {
                if out_start >= out.len() {
                    out.clear();
                    out_start = 0;
                }
                // Remote argv first, so it is already in place when vmux
                // reacts to the command name changing.
                if let Some(remote) = pending_remote.take() {
                    out.extend_from_slice(&encode_remote_termprop(&remote));
                    last_remote = remote;
                }
                if let Some((name, uid)) = pending_fg.take() {
                    out.extend_from_slice(&encode_fgproc_termprop(&name, uid));
                    last_sent = Some((name, uid));
                }
            }
        }
    }

    let st = status.unwrap_or_else(|| {
        // Exiting with the shell still alive means a relay stream closed
        // under us (pipe-driven runs, vte gone without the usual SIGHUP):
        // hang up the shell like a vanishing terminal would, then reap.
        unsafe { libc::kill(child, libc::SIGHUP) };
        let mut st: libc::c_int = 0;
        if unsafe { libc::waitpid(child, &mut st, 0) } == child {
            st
        } else {
            0
        }
    });
    if libc::WIFEXITED(st) {
        libc::WEXITSTATUS(st)
    } else if libc::WIFSIGNALED(st) {
        128 + libc::WTERMSIG(st)
    } else {
        0
    }
}

/// Final flush toward vte after the inner pty closed; bounded best effort.
fn flush_blocking(fd: i32, mut buf: &[u8]) {
    for _ in 0..100 {
        if buf.is_empty() {
            return;
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        unsafe { libc::poll(&mut pfd, 1, 20) };
        match write_fd(fd, buf) {
            IoRes::Done(n) => buf = &buf[n..],
            IoRes::Again => {}
            IoRes::Closed => return,
        }
    }
}

fn errno() -> libc::c_int {
    unsafe { *libc::__errno_location() }
}

fn install_wake_handler(sig: libc::c_int) {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        let handler: extern "C" fn(libc::c_int) = wake;
        sa.sa_sigaction = handler as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(sig, &sa, std::ptr::null_mut());
    }
}

fn tcgetattr(fd: i32) -> Option<libc::termios> {
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    (unsafe { libc::tcgetattr(fd, &mut t) } == 0).then_some(t)
}

fn winsize_of(fd: i32) -> Option<libc::winsize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    (unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0).then_some(ws)
}

fn set_nonblock(fd: i32) {
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        if fl >= 0 {
            libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::command_name;

    fn name(argv: &[&str]) -> String {
        command_name(&argv.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn plain_commands_are_their_basename() {
        assert_eq!(name(&["/usr/bin/nvim", "."]), "nvim");
        assert_eq!(name(&["claude"]), "claude");
        assert_eq!(name(&[""]), "");
        assert_eq!(name(&[]), "");
    }

    #[test]
    fn interpreted_scripts_are_named_after_the_script() {
        assert_eq!(name(&["node", "/usr/bin/gemini"]), "gemini");
        assert_eq!(
            name(&["node", "--no-warnings", "/usr/bin/copilot", "-p"]),
            "copilot"
        );
        assert_eq!(
            name(&["/usr/bin/python3.12", "manage.py", "runserver"]),
            "manage"
        );
        assert_eq!(name(&["deno", "run", "-A", "tool.ts"]), "tool");
        // A bare interpreter (a REPL) stays as it is.
        assert_eq!(name(&["node"]), "node");
        assert_eq!(name(&["python3", "-i"]), "python3");
        // Not an interpreter, despite the prefix.
        assert_eq!(name(&["pythonic", "x.py"]), "pythonic");
    }

    #[test]
    fn package_entry_points_are_named_after_their_command() {
        assert_eq!(
            name(&[
                "node",
                "/usr/lib/node_modules/@google/gemini-cli/dist/index.js"
            ]),
            "gemini"
        );
        assert_eq!(
            name(&[
                "bun",
                "/home/u/.bun/install/global/node_modules/opencode-ai/bin/opencode"
            ]),
            "opencode"
        );
    }
}
