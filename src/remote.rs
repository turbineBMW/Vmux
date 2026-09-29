//! Which machine a remote session (ssh and friends) is connected to.
//!
//! vmux-relay reports the foreground command's full argv over the
//! `vte.ext.vmux.remote` termprop whenever that command is a remote session;
//! vmux parses the destination back out of it to label the tab. Parsing is
//! pure (no config lookups) — `ssh -G` resolution of aliases happens on the
//! GUI side, off the UI thread.

use crate::osc_scan::osc666_termprop;

/// The termprop carrying the foreground remote command's argv, NUL-separated
/// (empty when the foreground command is not a remote session).
pub const REMOTE_TERMPROP_NAME: &str = "vte.ext.vmux.remote";

/// Foreground commands that hold a session on another machine (kgx's list).
pub const REMOTE_COMMANDS: &[&str] = &["ssh", "telnet", "mosh-client", "mosh", "et"];

/// argv beyond this is truncated before it goes out (the destination always
/// sits near the front; the tail is a remote command line nobody needs).
/// vte rejects — and so resets — data termprops over 2048 decoded bytes.
const ARGV_CAP: usize = 2048;

/// The command a raw argv[0] names, for matching against REMOTE_COMMANDS.
/// mosh execs its client with the whole display string in argv[0]
/// (`mosh-client -# user@host | 10.0.0.1 60001` style), so only the first
/// word of the basename counts for it.
pub fn command_name(arg0: &str) -> &str {
    let base = arg0.rsplit('/').next().unwrap_or("");
    match base.split_once(' ') {
        Some((first, _)) if first == "mosh-client" => first,
        _ => base,
    }
}

pub fn is_remote_command(arg0: &str) -> bool {
    REMOTE_COMMANDS.contains(&command_name(arg0))
}

/// The termprop OSC carrying a raw /proc cmdline (NUL-separated argv).
pub fn encode_remote_termprop(cmdline: &[u8]) -> Vec<u8> {
    let mut end = cmdline.len().min(ARGV_CAP);
    // Keep whole args only; a half-cut one could read as a different host.
    if end < cmdline.len() {
        end = cmdline[..end].iter().rposition(|&b| b == 0).unwrap_or(0);
    }
    osc666_termprop(REMOTE_TERMPROP_NAME, &cmdline[..end])
}

/// Split a NUL-separated cmdline back into args (the trailing NUL /proc
/// leaves is not an empty final arg).
pub fn split_argv(data: &[u8]) -> Vec<String> {
    let data = data.strip_suffix(&[0]).unwrap_or(data);
    if data.is_empty() {
        return Vec::new();
    }
    data.split(|&b| b == 0)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

/// Where a remote session points, as written on its command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// The host as typed — an ssh_config alias, a hostname or an address.
    pub host: String,
    pub user: Option<String>,
    pub port: Option<String>,
}

impl Destination {
    /// `user@host:port`, leaving out the parts that weren't given.
    pub fn display(&self) -> String {
        let mut s = String::new();
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        s.push_str(&self.host);
        if let Some(p) = &self.port {
            s.push(':');
            s.push_str(p);
        }
        s
    }
}

/// Parse the destination out of a remote command's argv. None when argv
/// isn't a recognized remote command or names no host (e.g. `ssh -V`).
pub fn destination(argv: &[String]) -> Option<Destination> {
    let (arg0, args) = argv.split_first()?;
    match command_name(arg0) {
        "ssh" => ssh_destination(args),
        "mosh" => mosh_destination(args),
        "mosh-client" => mosh_client_destination(arg0, args),
        "telnet" => telnet_destination(args),
        "et" => et_destination(args),
        _ => None,
    }
}

/// Walk getopt-style args: short flags cluster (`-4v`), and a flag in
/// `takes_value` consumes the rest of its cluster or the next arg. Long
/// options (`--name[=value]`) consume the next arg when listed in
/// `long_with_value` and given without `=`. `on_opt` sees each option that
/// carried a value; the first operand ends the walk and is returned along
/// with the operands after it.
fn getopt<'a>(
    args: &'a [String],
    takes_value: &str,
    long_with_value: &[&str],
    on_opt: &mut dyn FnMut(&str, &str),
) -> &'a [String] {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            return &args[i + 1..];
        }
        if let Some(long) = a.strip_prefix("--") {
            match long.split_once('=') {
                Some((name, value)) => on_opt(name, value),
                None if long_with_value.contains(&long) => {
                    i += 1;
                    if let Some(v) = args.get(i) {
                        on_opt(long, v);
                    }
                }
                None => {}
            }
        } else if let Some(cluster) = a.strip_prefix('-').filter(|c| !c.is_empty()) {
            for (at, c) in cluster.char_indices() {
                if takes_value.contains(c) {
                    let rest = &cluster[at + c.len_utf8()..];
                    let value = if rest.is_empty() {
                        i += 1;
                        args.get(i).map_or("", String::as_str)
                    } else {
                        rest
                    };
                    on_opt(&c.to_string(), value);
                    break;
                }
            }
        } else {
            return &args[i..];
        }
        i += 1;
    }
    &[]
}

/// Split `[user@]host`; the last `@` wins, as in ssh.
fn split_user(s: &str) -> (Option<String>, String) {
    match s.rsplit_once('@') {
        Some((u, h)) if !u.is_empty() => (Some(u.to_string()), h.to_string()),
        Some((_, h)) => (None, h.to_string()),
        None => (None, s.to_string()),
    }
}

/// Split `host[:port]`, leaving bare IPv6 addresses (several colons) alone
/// and unwrapping `[v6]:port`.
fn split_port(s: &str) -> (String, Option<String>) {
    if let Some(inner) = s.strip_prefix('[')
        && let Some((host, rest)) = inner.split_once(']')
    {
        let port = rest.strip_prefix(':').filter(|p| !p.is_empty());
        return (host.to_string(), port.map(str::to_string));
    }
    match s.split_once(':') {
        Some((h, p)) if !p.contains(':') && !p.is_empty() => (h.to_string(), Some(p.to_string())),
        _ => (s.to_string(), None),
    }
}

fn nonempty(d: Destination) -> Option<Destination> {
    (!d.host.is_empty()).then_some(d)
}

fn ssh_destination(args: &[String]) -> Option<Destination> {
    let mut user = None;
    let mut port = None;
    // `-l` / `-p` from `man ssh`'s synopsis, plus the `-o Key=value` and
    // `-o "Key value"` spellings of the same two settings.
    let rest = getopt(
        args,
        "BbcDEeFIiJLlmOoPpQRSWw",
        &[],
        &mut |opt, v| match opt {
            "l" => user = Some(v.to_string()),
            "p" => port = Some(v.to_string()),
            "o" => {
                let (k, val) = v
                    .split_once(['=', ' '])
                    .map_or((v, ""), |(k, val)| (k, val.trim_start_matches(['=', ' '])));
                if k.eq_ignore_ascii_case("user") {
                    user = Some(val.to_string());
                } else if k.eq_ignore_ascii_case("port") {
                    port = Some(val.to_string());
                }
            }
            _ => {}
        },
    );
    let dest = rest.first()?;
    let (dest_user, host, dest_port) = match dest.strip_prefix("ssh://") {
        Some(uri) => {
            let (u, hp) = split_user(uri.trim_end_matches('/'));
            let (h, p) = split_port(&hp);
            (u, h, p)
        }
        None => {
            let (u, h) = split_user(dest);
            (u, h, None)
        }
    };
    // ssh gives -l precedence over user@ in the destination; a port in the
    // URI form overrides -p.
    nonempty(Destination {
        host,
        user: user.or(dest_user),
        port: dest_port.or(port),
    })
}

fn mosh_destination(args: &[String]) -> Option<Destination> {
    // mosh's -p/--port is its UDP port range, not an ssh port, so it is
    // skipped like any other option value rather than kept.
    let rest = getopt(
        args,
        "p",
        &[
            "client",
            "server",
            "ssh",
            "port",
            "predict",
            "family",
            "bind-server",
            "experimental-remote-ip",
        ],
        &mut |_, _| {},
    );
    let (user, host) = split_user(rest.first()?);
    nonempty(Destination {
        host,
        user,
        port: None,
    })
}

/// mosh-client carries the user's original `mosh` arguments in argv[0]
/// after `-# ` (up to ` |`), and the resolved IP as its first operand.
fn mosh_client_destination(arg0: &str, args: &[String]) -> Option<Destination> {
    if let Some((_, shown)) = arg0.split_once(" -# ") {
        let shown = shown.rsplit_once(" |").map_or(shown, |(s, _)| s);
        let words: Vec<String> = shown.split_whitespace().map(str::to_string).collect();
        if let Some(d) = mosh_destination(&words) {
            return Some(d);
        }
    }
    let rest = getopt(args, "", &[], &mut |_, _| {});
    nonempty(Destination {
        host: rest.first()?.clone(),
        user: None,
        port: None,
    })
}

fn telnet_destination(args: &[String]) -> Option<Destination> {
    let mut user = None;
    let rest = getopt(args, "belnSXk", &[], &mut |opt, v| {
        if opt == "l" {
            user = Some(v.to_string());
        }
    });
    let (dest_user, host) = split_user(rest.first()?);
    nonempty(Destination {
        host,
        user: user.or(dest_user),
        port: rest.get(1).cloned(),
    })
}

fn et_destination(args: &[String]) -> Option<Destination> {
    let mut port = None;
    let rest = getopt(
        args,
        "ctrplux",
        &[
            "command",
            "tunnel",
            "reversetunnel",
            "port",
            "logdir",
            "username",
            "jumphost",
            "jport",
            "serverfifo",
            "ssh-socket",
            "terminal-path",
            "macserver",
            "prefix",
        ],
        &mut |opt, v| {
            if opt == "p" || opt == "port" {
                port = Some(v.to_string());
            }
        },
    );
    let (user, hp) = split_user(rest.first()?);
    let (host, dest_port) = split_port(&hp);
    nonempty(Destination {
        host,
        user,
        port: dest_port.or(port),
    })
}

/// The `hostname` / `user` / `port` lines of `ssh -G` output: what ssh
/// actually connects to once ssh_config has been applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    pub hostname: String,
    pub user: String,
    pub port: String,
}

pub fn parse_ssh_g(out: &str) -> Option<Resolved> {
    let mut r = Resolved::default();
    for line in out.lines() {
        let Some((k, v)) = line.split_once(' ') else {
            continue;
        };
        match k {
            "hostname" => r.hostname = v.to_string(),
            "user" => r.user = v.to_string(),
            "port" => r.port = v.to_string(),
            _ => {}
        }
    }
    (!r.hostname.is_empty()).then_some(r)
}

/// The argv to hand `ssh -G` (after `ssh`) to resolve `argv`'s destination:
/// ssh's own options verbatim so `-F`, `-J`, `-o` and friends apply; for the
/// ssh-based wrappers just user and host (their ports are not ssh ports).
/// None for telnet, which never consults ssh_config.
pub fn ssh_g_args(argv: &[String], dest: &Destination) -> Option<Vec<String>> {
    let (arg0, args) = argv.split_first()?;
    match command_name(arg0) {
        "ssh" => Some(args.to_vec()),
        "mosh" | "mosh-client" | "et" => {
            let mut v = Vec::new();
            if let Some(u) = &dest.user {
                v.extend(["-l".to_string(), u.clone()]);
            }
            v.push(dest.host.clone());
            Some(v)
        }
        _ => None,
    }
}

/// Whether hostname `host` names the machine called `local`. An empty host
/// (`file:///`) and `localhost` always do; otherwise compare the first DNS
/// label, case-insensitively, so `box` and `box.lan` match.
pub fn is_local_host(host: &str, local: &str) -> bool {
    let label = |s: &str| s.split('.').next().unwrap_or("").to_ascii_lowercase();
    host.is_empty() || host.eq_ignore_ascii_case("localhost") || label(host) == label(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn dest(host: &str, user: Option<&str>, port: Option<&str>) -> Option<Destination> {
        Some(Destination {
            host: host.into(),
            user: user.map(Into::into),
            port: port.map(Into::into),
        })
    }

    #[test]
    fn ssh_plain_alias_and_user_at_host() {
        assert_eq!(destination(&argv("ssh prod")), dest("prod", None, None));
        assert_eq!(
            destination(&argv("/usr/bin/ssh deploy@prod uptime")),
            dest("prod", Some("deploy"), None)
        );
    }

    #[test]
    fn ssh_skips_option_values() {
        assert_eq!(
            destination(&argv("ssh -i key -p 2222 -J bastion -v box")),
            dest("box", None, Some("2222"))
        );
        // Clustered flags, value attached to the flag, and `--`.
        assert_eq!(
            destination(&argv("ssh -tAp2222 -lroot -- box")),
            dest("box", Some("root"), Some("2222"))
        );
        assert_eq!(
            destination(&argv("ssh -o User=me -o Port=10 box")),
            dest("box", Some("me"), Some("10"))
        );
    }

    #[test]
    fn ssh_l_beats_destination_user() {
        assert_eq!(
            destination(&argv("ssh -l a b@box")),
            dest("box", Some("a"), None)
        );
    }

    #[test]
    fn ssh_uri_form() {
        assert_eq!(
            destination(&argv("ssh ssh://me@box.lan:2200")),
            dest("box.lan", Some("me"), Some("2200"))
        );
        assert_eq!(
            destination(&argv("ssh ssh://[::1]:22")),
            dest("::1", None, Some("22"))
        );
    }

    #[test]
    fn no_destination_is_none() {
        assert_eq!(destination(&argv("ssh -V")), None);
        assert_eq!(destination(&argv("ssh")), None);
        assert_eq!(destination(&argv("vim file")), None);
    }

    #[test]
    fn mosh_and_its_client() {
        assert_eq!(
            destination(&argv("mosh --ssh=ssh\\ -p2 -p 60001 me@box")),
            dest("box", Some("me"), None)
        );
        let client = vec![
            "mosh-client -# me@box | 10.0.0.9 60001".to_string(),
            "10.0.0.9".to_string(),
            "60001".to_string(),
        ];
        assert_eq!(destination(&client), dest("box", Some("me"), None));
        // No display string: fall back to the address operand.
        assert_eq!(
            destination(&argv("mosh-client 10.0.0.9 60001")),
            dest("10.0.0.9", None, None)
        );
    }

    #[test]
    fn telnet_and_et() {
        assert_eq!(
            destination(&argv("telnet -l me towel.blinkenlights.nl 23")),
            dest("towel.blinkenlights.nl", Some("me"), Some("23"))
        );
        assert_eq!(
            destination(&argv("et -c htop me@box:8080")),
            dest("box", Some("me"), Some("8080"))
        );
    }

    #[test]
    fn command_name_handles_mosh_argv0() {
        assert!(is_remote_command("/usr/bin/ssh"));
        assert!(is_remote_command("mosh-client -# box | 1.2.3.4 60001"));
        assert!(!is_remote_command("sshd"));
        assert!(!is_remote_command("vim"));
    }

    #[test]
    fn argv_roundtrip_and_cap() {
        assert_eq!(split_argv(b"ssh\0box\0"), argv("ssh box"));
        assert!(split_argv(b"").is_empty());
        let long: Vec<u8> = [&b"ssh\0box\0"[..], &vec![b'x'; 5000]].concat();
        let osc = String::from_utf8(encode_remote_termprop(&long)).unwrap();
        let b64 = &osc["\x1b]666;vte.ext.vmux.remote=".len()..osc.len() - 2];
        let back = crate::osc_scan::base64_decode(b64.as_bytes()).unwrap();
        assert_eq!(split_argv(&back), argv("ssh box"));
    }

    #[test]
    fn ssh_g_output() {
        let out = "user me\nhostname 10.0.3.4\nport 2222\nforwardagent no\n";
        assert_eq!(
            parse_ssh_g(out),
            Some(Resolved {
                hostname: "10.0.3.4".into(),
                user: "me".into(),
                port: "2222".into()
            })
        );
        assert_eq!(parse_ssh_g(""), None);
    }

    #[test]
    fn ssh_g_args_per_command() {
        let a = argv("ssh -F cfg prod");
        let d = destination(&a).unwrap();
        assert_eq!(ssh_g_args(&a, &d), Some(argv("-F cfg prod")));
        let a = argv("et me@box:8080");
        let d = destination(&a).unwrap();
        assert_eq!(ssh_g_args(&a, &d), Some(argv("-l me box")));
        let a = argv("telnet box");
        let d = destination(&a).unwrap();
        assert_eq!(ssh_g_args(&a, &d), None);
    }

    #[test]
    fn local_host_matching() {
        assert!(is_local_host("", "box"));
        assert!(is_local_host("localhost", "box"));
        assert!(is_local_host("BOX.lan", "box"));
        assert!(!is_local_host("prod", "box"));
    }
}
