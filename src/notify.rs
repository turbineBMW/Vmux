//! Desktop notifications over org.freedesktop.Notifications directly
//! (rather than `gio::Notification`), so we control the hints and actions:
//! the sound request, the desktop-entry/image-path for the app icon, and
//! the xdg-activation token the daemon hands us when the user clicks — the
//! only portable way to take focus on Wayland.
//!
//! Vmux never plays audio itself: the sound choice travels as a hint on the
//! notification, so the daemon (and the shell's do-not-disturb rules) stays
//! in charge of whether anything is actually heard.
use crate::state::Sound;
use gtk4::gio;
use gtk4::glib::{self, prelude::*};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const BUS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const APP_NAME: &str = "Vmux";
const DESKTOP_ID: &str = "dev.vmux.Vmux";

type OnOpen = Box<dyn Fn(u64, Option<String>)>;

pub struct Notifier {
    conn: gio::DBusConnection,
    /// Live notification id per zone, so a newer message replaces the
    /// previous bubble and selecting the zone can withdraw it.
    by_zone: RefCell<HashMap<u64, u32>>,
    /// Reverse map, for the daemon's id-keyed signals.
    zone_of: RefCell<HashMap<u32, u64>>,
    /// Activation token the daemon sent just before an ActionInvoked.
    tokens: RefCell<HashMap<u32, String>>,
    on_open: RefCell<Option<OnOpen>>,
}

impl Notifier {
    pub fn new() -> Option<Rc<Self>> {
        let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .map_err(|e| eprintln!("vmux: session bus unavailable, notifications off: {e}"))
            .ok()?;
        let me = Rc::new(Self {
            conn,
            by_zone: RefCell::default(),
            zone_of: RefCell::default(),
            tokens: RefCell::default(),
            on_open: RefCell::default(),
        });
        for sig in ["ActivationToken", "ActionInvoked", "NotificationClosed"] {
            let weak = Rc::downgrade(&me);
            #[allow(deprecated)] // subscribe_to_signal needs gio v2_74 feature
            me.conn.signal_subscribe(
                Some(BUS),
                Some(BUS),
                Some(sig),
                Some(PATH),
                None,
                gio::DBusSignalFlags::NONE,
                move |_, _, _, _, name, params| {
                    if let Some(me) = weak.upgrade() {
                        me.on_signal(name, params);
                    }
                },
            );
        }
        Some(me)
    }

    /// Called with (zone id, activation token) when a notification is clicked.
    pub fn set_on_open(&self, f: impl Fn(u64, Option<String>) + 'static) {
        *self.on_open.borrow_mut() = Some(Box::new(f));
    }

    fn on_signal(&self, name: &str, params: &glib::Variant) {
        let Some(id) = params.child_value(0).get::<u32>() else { return };
        match name {
            "ActivationToken" => {
                if let Some(t) = params.child_value(1).get::<String>() {
                    self.tokens.borrow_mut().insert(id, t);
                }
            }
            "ActionInvoked" => {
                let token = self.tokens.borrow_mut().remove(&id);
                let zone = self.zone_of.borrow().get(&id).copied();
                if let (Some(zone), Some(f)) = (zone, self.on_open.borrow().as_ref()) {
                    f(zone, token);
                }
            }
            "NotificationClosed" => self.forget(id),
            _ => {}
        }
    }

    fn forget(&self, id: u32) {
        if let Some(zone) = self.zone_of.borrow_mut().remove(&id) {
            let mut by_zone = self.by_zone.borrow_mut();
            if by_zone.get(&zone) == Some(&id) {
                by_zone.remove(&zone);
            }
        }
        self.tokens.borrow_mut().remove(&id);
    }

    pub fn send(self: &Rc<Self>, zone: u64, title: &str, body: &str, sound: &Sound) {
        let replaces = self.by_zone.borrow().get(&zone).copied().unwrap_or(0);
        let actions = vec!["default".to_string(), "Open".to_string()];
        let icon = app_icon();
        let mut hints: HashMap<String, glib::Variant> = HashMap::new();
        hints.insert("desktop-entry".into(), DESKTOP_ID.to_variant());
        if icon.starts_with('/') {
            hints.insert("image-path".into(), icon.to_variant());
        }
        hints.insert("category".into(), "im".to_variant());
        hints.insert("urgency".into(), 1u8.to_variant());
        match sound {
            Sound::SystemDefault => {
                hints.insert("sound-name".into(), "message-new-instant".to_variant());
            }
            Sound::File(p) => {
                hints.insert("sound-file".into(), p.to_string_lossy().as_ref().to_variant());
            }
            Sound::None => {
                hints.insert("suppress-sound".into(), true.to_variant());
            }
        }
        let args = (APP_NAME, replaces, icon.as_str(), title, body, actions, hints, -1i32).to_variant();
        let me = self.clone();
        glib::spawn_future_local(async move {
            match me
                .conn
                .call_future(Some(BUS), PATH, BUS, "Notify", Some(&args), None, gio::DBusCallFlags::NONE, 5000)
                .await
            {
                Ok(r) => {
                    let Some(id) = r.child_value(0).get::<u32>() else { return };
                    if replaces != 0 && replaces != id {
                        me.zone_of.borrow_mut().remove(&replaces);
                    }
                    me.by_zone.borrow_mut().insert(zone, id);
                    me.zone_of.borrow_mut().insert(id, zone);
                }
                Err(e) => eprintln!("vmux: Notify failed: {e}"),
            }
        });
    }

    /// Withdraw the zone's notification, if one is showing.
    pub fn close(&self, zone: u64) {
        let Some(id) = self.by_zone.borrow_mut().remove(&zone) else { return };
        self.zone_of.borrow_mut().remove(&id);
        self.tokens.borrow_mut().remove(&id);
        self.conn.call(
            Some(BUS),
            PATH,
            BUS,
            "CloseNotification",
            Some(&(id,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
            |_| {},
        );
    }
}

/// The app icon as an absolute path when it's installed (any daemon can show
/// that), else the theme name for the daemon to resolve itself.
fn app_icon() -> String {
    let user = glib::user_data_dir().join(format!("icons/hicolor/128x128/apps/{DESKTOP_ID}.png"));
    let system = std::path::PathBuf::from(format!("/usr/share/icons/hicolor/128x128/apps/{DESKTOP_ID}.png"));
    [user, system]
        .into_iter()
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| DESKTOP_ID.into())
}
