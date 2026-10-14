//! Play notification sounds locally when the notification server does not.
//! Omarchy Shell displays freedesktop notifications but ignores sound hints.

use crate::state::Sound;
use gtk4::{gio, glib, prelude::*};
use std::cell::RefCell;
use std::io::Read;
use std::path::{Path, PathBuf};

const EVENT: &str = "message-new-instant";
pub const SUPPORTED_MIME_TYPES: &[&str] = &[
    "audio/ogg",
    "audio/x-vorbis+ogg",
    "audio/x-opus+ogg",
    "audio/opus",
    "audio/flac",
    "audio/x-flac",
    "audio/x-wav",
    "audio/wav",
    "audio/vnd.wave",
    "audio/mp4",
    "audio/aac",
];

thread_local! {
    // Dropping a GtkMediaFile while GStreamer starts it can crash the app.
    static PLAYING: RefCell<Vec<gtk4::MediaFile>> = const { RefCell::new(Vec::new()) };
}

pub fn play_notification(sound: &Sound) {
    if do_not_disturb() {
        return;
    }
    let path = match sound {
        Sound::SystemDefault => system_sound_path(),
        Sound::File(path) => Some(path.clone()),
        Sound::None => None,
    };
    if let Some(path) = path {
        play_file(&path);
    }
}

fn do_not_disturb() -> bool {
    // Omarchy Shell persists its DND switch here. A missing file means its
    // default, off. Restrict this check to Omarchy so stale state from a
    // different desktop cannot silence notifications there.
    if std::env::var_os("OMARCHY_PATH").is_some() {
        // The shell currently uses this exact path, regardless of
        // XDG_STATE_HOME.
        let settings = glib::home_dir().join(".local/state/omarchy/notifications.json");
        return std::fs::read_to_string(settings)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| value.get("dnd").and_then(|dnd| dnd.as_bool()))
            .unwrap_or(false);
    }

    // GNOME implements DND by disabling notification banners.
    const SCHEMA: &str = "org.gnome.desktop.notifications";
    gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(SCHEMA, true))
        .is_some_and(|_| !gio::Settings::new(SCHEMA).boolean("show-banners"))
}

fn system_sound_path() -> Option<PathBuf> {
    const SCHEMA: &str = "org.gnome.desktop.sound";
    let settings = gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(SCHEMA, true))
        .map(|_| gio::Settings::new(SCHEMA));
    if settings
        .as_ref()
        .is_some_and(|settings| !settings.boolean("event-sounds"))
    {
        return None;
    }
    let theme = settings
        .map(|settings| settings.string("theme-name").to_string())
        .filter(|theme| !theme.is_empty())
        .unwrap_or_else(|| "freedesktop".into());
    theme_sound_path(EVENT, &theme)
}

fn theme_sound_path(event: &str, theme: &str) -> Option<PathBuf> {
    let mut dirs = vec![
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| glib::home_dir().join(".local/share"))
            .join("sounds"),
    ];
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| Path::new(dir).join("sounds")),
    );
    let mut visited = Vec::new();
    let mut queue = vec![theme.to_string()];
    while let Some(theme) = queue.pop() {
        if visited.contains(&theme) {
            continue;
        }
        visited.push(theme.clone());
        for dir in &dirs {
            let theme_dir = dir.join(&theme);
            if !theme_dir.is_dir() {
                continue;
            }
            let index = std::fs::read_to_string(theme_dir.join("index.theme")).unwrap_or_default();
            let (subdirs, parents) = parse_theme_index(&index);
            let mut name = event;
            loop {
                for subdir in subdirs.iter().map(String::as_str).chain(["stereo"]) {
                    let stem = theme_dir.join(subdir).join(name);
                    if stem.with_extension("disabled").is_file() {
                        return None;
                    }
                    for extension in ["oga", "ogg", "wav"] {
                        let path = stem.with_extension(extension);
                        if path.is_file() {
                            return Some(path);
                        }
                    }
                }
                match name.rsplit_once('-') {
                    Some((shorter, _)) => name = shorter,
                    None => break,
                }
            }
            queue.extend(parents.into_iter().rev());
        }
        if theme != "freedesktop" && !queue.iter().any(|name| name == "freedesktop") {
            queue.insert(0, "freedesktop".into());
        }
    }
    None
}

fn parse_theme_index(index: &str) -> (Vec<String>, Vec<String>) {
    let mut dirs = Vec::new();
    let mut parents = Vec::new();
    let mut in_theme = false;
    for line in index.lines().map(str::trim) {
        if line.starts_with('[') {
            in_theme = line == "[Sound Theme]";
        } else if in_theme {
            if let Some((key, value)) = line.split_once('=') {
                let values = value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(String::from);
                match key.trim() {
                    "Directories" => dirs.extend(values),
                    "Inherits" => parents.extend(values),
                    _ => {}
                }
            }
        }
    }
    (dirs, parents)
}

fn is_playable(path: &Path) -> bool {
    let mut header = [0u8; 512];
    let read = std::fs::File::open(path)
        .and_then(|mut file| file.read(&mut header))
        .unwrap_or(0);
    let header = &header[..read];
    // GTK's GStreamer backend can abort on MP3 in decodebin3; catch an MP3
    // even if it was renamed to a supported extension.
    if header.starts_with(b"ID3")
        || (header.len() >= 2 && header[0] == 0xff && header[1] & 0xe6 == 0xe2)
    {
        return false;
    }
    let name = path.file_name().map(|name| name.to_string_lossy());
    let (guess, _) = gio::content_type_guess(name.as_deref(), header);
    let mime = gio::content_type_get_mime_type(&guess).unwrap_or_default();
    SUPPORTED_MIME_TYPES.contains(&mime.as_str())
}

fn play_file(path: &Path) {
    if !is_playable(path) {
        eprintln!(
            "vmux: refusing unsupported notification sound: {}",
            path.display()
        );
        return;
    }
    let already = PLAYING.with(|playing| {
        playing.borrow().iter().any(|media| {
            media.file().and_then(|file| file.path()).as_deref() == Some(path) && !media.is_ended()
        })
    });
    if already {
        return;
    }
    let media = gtk4::MediaFile::for_filename(path);
    let shown = path.display().to_string();
    media.connect_error_notify(move |media| {
        if let Some(error) = media.error() {
            eprintln!("vmux: could not play notification sound {shown}: {error}");
        }
        forget(media);
    });
    media.connect_ended_notify(forget);
    media.play();
    PLAYING.with(|playing| playing.borrow_mut().push(media));
}

fn forget(media: &gtk4::MediaFile) {
    PLAYING.with(|playing| playing.borrow_mut().retain(|kept| kept != media));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_disguised_mp3_and_accepts_ogg() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = dir.path().join("fake.ogg");
        std::fs::write(&mp3, b"ID3\x04\0\0\0\0\0\0\xff\xfb").unwrap();
        assert!(!is_playable(&mp3));
        let ogg = dir.path().join("sound.ogg");
        std::fs::write(&ogg, b"OggS\0\x02\0\0\0\0\0\0\0\0").unwrap();
        assert!(is_playable(&ogg));
    }
}
