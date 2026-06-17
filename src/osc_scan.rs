//! Streaming scanner for desktop-notification OSC sequences.
//!
//! libvte parses escape sequences internally and silently drops the
//! notification OSCs (9 / 777 / kitty 99), so vmux-relay watches the raw
//! child→terminal byte stream with this scanner — without modifying it —
//! and re-emits each notification as a vte termprop OSC (666) that vmux
//! receives through the termprop-changed signal.

use serde::{Deserialize, Serialize};

/// The custom vte termprop notifications travel over. vte requires the
/// "vte.ext." prefix and at least four dot-separated components.
pub const TERMPROP_NAME: &str = "vte.ext.vmux.notify";

/// The termprop carrying the inner pty's current foreground command name
/// (empty when the shell itself is in the foreground). vmux falls back to it
/// for the tab title when no program set an explicit OSC window title.
pub const FGPROC_TERMPROP_NAME: &str = "vte.ext.vmux.fgproc";

/// OSC payloads beyond this are dropped (passthrough is unaffected).
const PAYLOAD_CAP: usize = 8192;
const TITLE_MAX_CHARS: usize = 128;
const BODY_MAX_CHARS: usize = 1024;
/// Kitty multi-part accumulation: ids tracked at once / bytes per field.
const KITTY_MAX_IDS: usize = 4;
const KITTY_FIELD_CAP: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

/// JSON carried inside the termprop value. `seq` only exists to make
/// consecutive identical notifications distinct values: vte ignores a
/// termprop assignment whose value is unchanged.
#[derive(Serialize)]
struct PayloadOut<'a> {
    v: u32,
    seq: u64,
    title: &'a str,
    body: &'a str,
}

/// The vmux-side view of that JSON (`v`/`seq` are ignored on read).
#[derive(Deserialize)]
pub struct NotifyPayload {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
}

/// The vte termprop OSC carrying one notification:
/// `OSC 666 ; vte.ext.vmux.notify=<base64(json)> ST`.
/// Always ST-terminated — vte rejects a BEL-terminated termprop OSC.
pub fn encode_termprop(n: &Notification, seq: u64) -> Vec<u8> {
    let json = serde_json::to_string(&PayloadOut {
        v: 1,
        seq,
        title: &n.title,
        body: &n.body,
    })
    .unwrap_or_default();
    osc666_termprop(TERMPROP_NAME, json.as_bytes())
}

/// The vte termprop OSC carrying the current foreground command (empty value
/// clears it): `OSC 666 ; vte.ext.vmux.fgproc=<base64(cmd)> ST`.
pub fn encode_fgproc_termprop(cmd: &str) -> Vec<u8> {
    osc666_termprop(FGPROC_TERMPROP_NAME, cmd.as_bytes())
}

/// Frame one termprop OSC: `OSC 666 ; name=<base64(value)> ST`. Always
/// ST-terminated — vte rejects a BEL-terminated termprop OSC.
fn osc666_termprop(name: &str, value: &[u8]) -> Vec<u8> {
    let b64 = base64_encode(value);
    let mut out = Vec::with_capacity(b64.len() + name.len() + 16);
    out.extend_from_slice(b"\x1b]666;");
    out.extend_from_slice(name.as_bytes());
    out.push(b'=');
    out.extend_from_slice(b64.as_bytes());
    out.extend_from_slice(b"\x1b\\");
    out
}

enum State {
    Ground,
    /// Saw ESC. The canonical VT parser's "anywhere: ESC → escape" rule makes
    /// the contents of DCS/SOS/PM/APC strings scan identically to Ground, so
    /// no separate string-tracking states are needed; C1 intros (0x9D) are
    /// deliberately not recognized either — 0x9D is a valid UTF-8
    /// continuation byte, and real emitters use 7-bit `ESC ]`.
    Esc,
    /// Inside `ESC ]`, collecting the numeric OSC code.
    OscNum { num: u32, digits: u8 },
    /// Collecting the payload of an OSC 9/777/99 (code in `Scanner::code`).
    OscPayload,
    OscPayloadEsc,
    /// Inside an OSC we don't care about; waiting for its terminator.
    OscSkip,
}

#[derive(Default)]
struct KittyAccum {
    title: Vec<u8>,
    body: Vec<u8>,
}

pub struct Scanner {
    state: State,
    code: u32,
    payload: Vec<u8>,
    overflow: bool,
    /// Kitty notification parts by id, insertion-ordered (oldest evicted).
    kitty: Vec<(Vec<u8>, KittyAccum)>,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            code: 0,
            payload: Vec::new(),
            overflow: false,
            kitty: Vec::new(),
        }
    }

    /// Scan one chunk of the stream. For every notification that completes
    /// inside it, `emit` is called with the offset just past the sequence's
    /// terminator (where an injection belongs) and the parsed notification.
    /// Offsets are strictly increasing. The chunk itself is never modified.
    pub fn scan(&mut self, chunk: &[u8], emit: &mut dyn FnMut(usize, Notification)) {
        use State::*;
        for (i, &b) in chunk.iter().enumerate() {
            self.state = match std::mem::replace(&mut self.state, Ground) {
                Ground => match b {
                    0x1b => Esc,
                    _ => Ground,
                },
                Esc => match b {
                    b']' => OscNum { num: 0, digits: 0 },
                    0x1b => Esc,
                    _ => Ground,
                },
                OscNum { num, digits } => match b {
                    b'0'..=b'9' if digits < 6 => OscNum {
                        num: num * 10 + u32::from(b - b'0'),
                        digits: digits + 1,
                    },
                    b';' if matches!(num, 9 | 99 | 777) => {
                        self.code = num;
                        self.payload.clear();
                        self.overflow = false;
                        OscPayload
                    }
                    b';' => OscSkip,
                    0x07 | 0x18 | 0x1a => Ground,
                    0x1b => Esc,
                    _ => OscSkip,
                },
                OscPayload => match b {
                    0x07 => {
                        self.finish(i + 1, emit);
                        Ground
                    }
                    0x1b => OscPayloadEsc,
                    // CAN/SUB abort the sequence (canonical VT parser).
                    0x18 | 0x1a => Ground,
                    _ => {
                        if self.payload.len() < PAYLOAD_CAP {
                            self.payload.push(b);
                        } else {
                            self.overflow = true;
                        }
                        OscPayload
                    }
                },
                OscPayloadEsc => match b {
                    b'\\' => {
                        self.finish(i + 1, emit);
                        Ground
                    }
                    // ESC-other aborts the OSC; an immediate `]` opens a new one.
                    b']' => OscNum { num: 0, digits: 0 },
                    0x1b => Esc,
                    _ => Ground,
                },
                OscSkip => match b {
                    0x07 | 0x18 | 0x1a => Ground,
                    // ESC \ terminates via Esc; ESC ] starts a new OSC there too.
                    0x1b => Esc,
                    _ => OscSkip,
                },
            };
        }
    }

    /// True when the scanner sits between sequences (canonical Ground state) —
    /// the only point at which splicing an out-of-band OSC into the forwarded
    /// stream cannot land in the middle of another escape sequence.
    pub fn at_ground(&self) -> bool {
        matches!(self.state, State::Ground)
    }

    fn finish(&mut self, end: usize, emit: &mut dyn FnMut(usize, Notification)) {
        let payload = std::mem::take(&mut self.payload);
        if std::mem::take(&mut self.overflow) {
            return;
        }
        let n = match self.code {
            9 => parse_osc9(&payload),
            777 => parse_osc777(&payload),
            99 => self.parse_osc99(&payload),
            _ => None,
        };
        if let Some(n) = n
            && !(n.title.is_empty() && n.body.is_empty())
        {
            emit(end, n);
        }
    }

    /// Minimal subset of kitty's desktop-notification protocol
    /// (<https://sw.kovidgoyal.net/kitty/desktop-notifications/>): the
    /// payload is `metadata ; content` with `:`-separated `k=v` metadata.
    /// Only i/d/p/e are honored; unknown keys are ignored per the spec.
    /// Parts accumulate per notification id until `d=1`. Anything that is
    /// not plain title/body content (queries, close requests, icons,
    /// buttons) drops the sequence — vmux never writes protocol replies.
    fn parse_osc99(&mut self, payload: &[u8]) -> Option<Notification> {
        let (meta, content) = match payload.iter().position(|&b| b == b';') {
            Some(p) => (&payload[..p], &payload[p + 1..]),
            None => (payload, b"".as_slice()),
        };
        let mut id: &[u8] = b"0";
        let mut done = true;
        let mut part: &[u8] = b"title";
        let mut b64 = false;
        for kv in meta.split(|&b| b == b':') {
            if kv.is_empty() {
                continue;
            }
            let eq = kv.iter().position(|&b| b == b'=').unwrap_or(kv.len());
            let (k, v) = (&kv[..eq], kv.get(eq + 1..).unwrap_or(b""));
            match k {
                b"i" => id = v,
                b"d" => done = v != b"0",
                b"p" => part = v,
                b"e" => b64 = v == b"1",
                _ => {}
            }
        }
        let field_is_body = match part {
            b"title" => false,
            b"body" => true,
            _ => {
                // Close request, query, icon, … — forget the id entirely.
                self.kitty.retain(|(k, _)| k != id);
                return None;
            }
        };
        let content = if b64 {
            base64_decode(content)?
        } else {
            content.to_vec()
        };
        let idx = match self.kitty.iter().position(|(k, _)| k == id) {
            Some(idx) => idx,
            None => {
                if self.kitty.len() >= KITTY_MAX_IDS {
                    self.kitty.remove(0);
                }
                self.kitty.push((id.to_vec(), KittyAccum::default()));
                self.kitty.len() - 1
            }
        };
        let acc = &mut self.kitty[idx].1;
        let field = if field_is_body { &mut acc.body } else { &mut acc.title };
        let room = KITTY_FIELD_CAP.saturating_sub(field.len());
        field.extend_from_slice(&content[..content.len().min(room)]);
        if !done {
            return None;
        }
        let (_, acc) = self.kitty.remove(idx);
        Some(Notification {
            title: clean(&acc.title, TITLE_MAX_CHARS, false),
            body: clean(&acc.body, BODY_MAX_CHARS, true),
        })
    }
}

/// `OSC 9 ; message` — iTerm2/Ghostty notification. A message that itself
/// starts with `digits;` is a ConEmu subcommand (9;4 progress and friends),
/// not a notification; vte consumes those from the passthrough already.
fn parse_osc9(payload: &[u8]) -> Option<Notification> {
    let digits = payload.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 && payload.get(digits) == Some(&b';') {
        return None;
    }
    Some(Notification {
        title: String::new(),
        body: clean(payload, BODY_MAX_CHARS, true),
    })
}

/// `OSC 777 ; notify ; title ; body` — urxvt/Ghostty notification. The body
/// keeps any further semicolons.
fn parse_osc777(payload: &[u8]) -> Option<Notification> {
    let mut parts = payload.splitn(3, |&b| b == b';');
    if parts.next() != Some(b"notify".as_slice()) {
        return None;
    }
    let title = parts.next()?;
    let body = parts.next().unwrap_or(b"");
    Some(Notification {
        title: clean(title, TITLE_MAX_CHARS, false),
        body: clean(body, BODY_MAX_CHARS, true),
    })
}

/// UTF-8 (lossy) decode, drop control characters (bodies keep newlines),
/// truncate to a character budget.
fn clean(bytes: &[u8], max_chars: usize, keep_newlines: bool) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|&c| !c.is_control() || (keep_newlines && c == '\n'))
        .take(max_chars)
        .collect()
}

// Hand-rolled base64 (standard alphabet, padded) to keep the relay free of
// extra dependencies. The alphabet contains no `;` and the `=` padding sits
// after the first `=` of the termprop assignment, so encoded values can
// never break the OSC 666 `name=value` grammar.

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

fn base64_decode(data: &[u8]) -> Option<Vec<u8>> {
    let data: Vec<u8> = data.iter().copied().filter(|&b| b != b'\n' && b != b'\r').collect();
    let data = match data.iter().position(|&b| b == b'=') {
        Some(p) if p + (data.len() - p) <= data.len() && data[p..].iter().all(|&b| b == b'=') => {
            &data[..p]
        }
        Some(_) => return None,
        None => &data[..],
    };
    if data.len() % 4 == 1 {
        return None;
    }
    let val = |b: u8| -> Option<u32> { B64.iter().position(|&c| c == b).map(|p| p as u32) };
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    for chunk in data.chunks(4) {
        let mut n = 0u32;
        for &b in chunk {
            n = (n << 6) | val(b)?;
        }
        n <<= 6 * (4 - chunk.len());
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notif(title: &str, body: &str) -> Notification {
        Notification { title: title.into(), body: body.into() }
    }

    /// Feed the bytes in one piece and return (offset, notification) pairs.
    fn collect(bytes: &[u8]) -> Vec<(usize, Notification)> {
        let mut s = Scanner::new();
        let mut got = Vec::new();
        s.scan(bytes, &mut |off, n| got.push((off, n)));
        got
    }

    /// The notifications must be identical no matter where the stream is
    /// split — every byte boundary is tried.
    fn assert_split_stable(bytes: &[u8]) -> Vec<Notification> {
        let whole: Vec<Notification> = collect(bytes).into_iter().map(|(_, n)| n).collect();
        for cut in 1..bytes.len() {
            let mut s = Scanner::new();
            let mut got = Vec::new();
            s.scan(&bytes[..cut], &mut |_, n| got.push(n.clone()));
            s.scan(&bytes[cut..], &mut |_, n| got.push(n.clone()));
            assert_eq!(got, whole, "split at {cut} diverged");
        }
        whole
    }

    #[test]
    fn osc9_bel_and_st() {
        assert_eq!(assert_split_stable(b"ab\x1b]9;hello\x07cd"), vec![notif("", "hello")]);
        assert_eq!(assert_split_stable(b"ab\x1b]9;hello\x1b\\cd"), vec![notif("", "hello")]);
    }

    #[test]
    fn osc9_emit_offset_is_after_terminator() {
        let got = collect(b"xx\x1b]9;hi\x07yy");
        assert_eq!(got, vec![(9, notif("", "hi"))]); // "xx" + 5-byte intro + "hi" + BEL
        let got = collect(b"\x1b]9;hi\x1b\\tail");
        assert_eq!(got, vec![(8, notif("", "hi"))]);
    }

    #[test]
    fn osc9_conemu_guard() {
        assert!(assert_split_stable(b"\x1b]9;4;1;50\x07").is_empty());
        assert!(collect(b"\x1b]9;12;anything\x07").is_empty());
        // All-digit message has no `digits;` prefix — it is a notification.
        assert_eq!(collect(b"\x1b]9;42\x07"), vec![(7, notif("", "42"))]);
    }

    #[test]
    fn osc9_empty_message_is_dropped() {
        assert!(collect(b"\x1b]9;\x07").is_empty());
    }

    #[test]
    fn osc777_notify() {
        assert_eq!(
            assert_split_stable(b"\x1b]777;notify;Title;Body;with;semis\x07"),
            vec![notif("Title", "Body;with;semis")]
        );
        assert!(collect(b"\x1b]777;other;Title;Body\x07").is_empty());
        assert_eq!(collect(b"\x1b]777;notify;OnlyTitle\x07").len(), 1);
    }

    #[test]
    fn osc99_defaults_title() {
        assert_eq!(assert_split_stable(b"\x1b]99;;hello\x1b\\"), vec![notif("hello", "")]);
    }

    #[test]
    fn osc99_multi_part() {
        let bytes = b"\x1b]99;i=1:d=0;The Title\x1b\\mid\x1b]99;i=1:d=1:p=body;The Body\x1b\\";
        assert_eq!(assert_split_stable(bytes), vec![notif("The Title", "The Body")]);
    }

    #[test]
    fn osc99_ids_are_independent() {
        let bytes = b"\x1b]99;i=a:d=0;A\x1b\\\x1b]99;i=b:d=1;B\x1b\\\x1b]99;i=a:d=1:p=body;abody\x1b\\";
        assert_eq!(
            assert_split_stable(bytes),
            vec![notif("B", ""), notif("A", "abody")]
        );
    }

    #[test]
    fn osc99_base64_payload() {
        // "hi there" -> aGkgdGhlcmU=
        assert_eq!(
            assert_split_stable(b"\x1b]99;e=1;aGkgdGhlcmU=\x1b\\"),
            vec![notif("hi there", "")]
        );
        // Broken base64 drops the part.
        assert!(collect(b"\x1b]99;e=1;!!!\x1b\\").is_empty());
    }

    #[test]
    fn osc99_non_content_payloads_dropped() {
        assert!(collect(b"\x1b]99;i=1:p=?;\x1b\\").is_empty());
        assert!(collect(b"\x1b]99;i=1:p=close;\x1b\\").is_empty());
        // A close even forgets accumulated parts for that id.
        let bytes = b"\x1b]99;i=1:d=0;part\x1b\\\x1b]99;i=1:p=close;\x1b\\\x1b]99;i=1:d=1:p=body;b\x1b\\";
        assert_eq!(collect(bytes).len(), 1);
        assert_eq!(collect(bytes)[0].1, notif("", "b"));
    }

    #[test]
    fn osc99_unknown_keys_ignored() {
        assert_eq!(
            collect(b"\x1b]99;i=1:o=unfocused:u=2:w=5000:d=1;msg\x1b\\").len(),
            1
        );
    }

    #[test]
    fn osc99_accumulator_eviction() {
        let mut bytes = Vec::new();
        for id in ["a", "b", "c", "d", "e"] {
            bytes.extend_from_slice(format!("\x1b]99;i={id}:d=0;t{id}\x1b\\").as_bytes());
        }
        // "a" was evicted by "e": finishing it now starts a fresh accum.
        bytes.extend_from_slice(b"\x1b]99;i=a:d=1:p=body;only-body\x1b\\");
        assert_eq!(collect(&bytes), vec![(bytes.len(), notif("", "only-body"))]);
    }

    #[test]
    fn esc_abort_then_new_osc() {
        // First OSC aborted by ESC] — its payload must not leak.
        assert_eq!(
            assert_split_stable(b"\x1b]9;lost\x1b]9;kept\x07"),
            vec![notif("", "kept")]
        );
        // ESC-other aborts entirely.
        assert!(collect(b"\x1b]9;lost\x1b[1mplain").is_empty());
    }

    #[test]
    fn uninteresting_oscs_skipped() {
        assert!(collect(b"\x1b]0;title\x07\x1b]8;;http://x\x1b\\\x1b]52;c;Zm9v\x07").is_empty());
        // ...but a notification right after them is found.
        assert_eq!(
            assert_split_stable(b"\x1b]0;title\x07\x1b]9;yes\x07"),
            vec![notif("", "yes")]
        );
    }

    #[test]
    fn can_sub_abort() {
        assert!(collect(b"\x1b]9;lost\x18plain").is_empty());
        assert!(collect(b"\x1b]9;lost\x1aplain").is_empty());
    }

    #[test]
    fn oversized_payload_dropped() {
        let mut bytes = b"\x1b]9;".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', PAYLOAD_CAP + 1));
        bytes.push(0x07);
        assert!(collect(&bytes).is_empty());
        // The scanner recovered: a following notification still works.
        bytes.extend_from_slice(b"\x1b]9;after\x07");
        assert_eq!(collect(&bytes), vec![(bytes.len(), notif("", "after"))]);
    }

    #[test]
    fn control_chars_stripped_and_truncated() {
        assert_eq!(
            collect(b"\x1b]777;notify;a\tb;line1\nline2\x07")[0].1,
            notif("ab", "line1\nline2")
        );
        let mut bytes = b"\x1b]9;".to_vec();
        bytes.extend(std::iter::repeat_n(b'y', 5000));
        bytes.push(0x07);
        assert_eq!(collect(&bytes)[0].1.body.chars().count(), BODY_MAX_CHARS);
    }

    #[test]
    fn invalid_utf8_is_lossy() {
        assert_eq!(collect(b"\x1b]9;a\xffb\x07")[0].1, notif("", "a\u{fffd}b"));
    }

    #[test]
    fn base64_roundtrip() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            assert_eq!(
                base64_decode(base64_encode(data).as_bytes()).as_deref(),
                Some(data)
            );
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_decode(b"Zm9vYg==").as_deref(), Some(&b"foob"[..]));
        assert!(base64_decode(b"a").is_none());
        assert!(base64_decode(b"ab=c").is_none());
    }

    #[test]
    fn encode_termprop_shape() {
        let bytes = encode_termprop(&notif("t", "b"), 7);
        let s = String::from_utf8(bytes.clone()).unwrap();
        assert!(s.starts_with("\x1b]666;vte.ext.vmux.notify="));
        assert!(s.ends_with("\x1b\\"));
        let b64 = &s["\x1b]666;vte.ext.vmux.notify=".len()..s.len() - 2];
        let json = base64_decode(b64.as_bytes()).unwrap();
        let p: NotifyPayload = serde_json::from_slice(&json).unwrap();
        assert_eq!((p.title.as_str(), p.body.as_str()), ("t", "b"));
        // Different seq values must yield different termprop values.
        assert_ne!(bytes, encode_termprop(&notif("t", "b"), 8));
    }
}
