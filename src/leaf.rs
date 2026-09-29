//! The desktop's side of a terminal: [`vmux::term::build`] with the desktop
//! app and one of its zones as the terminal's host.

use crate::app::App;
use crate::zone::Zone;
use gtk4 as gtk;
use std::rc::{Rc, Weak};
use vmux::state::Config;
use vmux::term::TerminalHost;
use vte4 as vte;
use vte4::prelude::*;

/// A terminal in one of the desktop's zones. Holds the zone weakly: a removed
/// zone must be able to finalize even if a terminal is still being torn down.
struct ZoneTerminal {
    app: Rc<App>,
    zone: Weak<Zone>,
}

impl TerminalHost for ZoneTerminal {
    fn config(&self) -> Config {
        self.app.config.borrow().clone()
    }

    fn font_scale(&self) -> f64 {
        self.app.font_scale.get()
    }

    fn text_binding(&self, keyval: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> Option<Vec<u8>> {
        let bindings = self.app.text_bindings.borrow();
        bindings
            .iter()
            .find(|b| b.matches(keyval, mods))
            .map(|b| b.bytes.clone())
    }

    fn focused(&self, term: &vte::Terminal) {
        let Some(zone) = self.zone.upgrade() else {
            return;
        };
        if std::env::var_os("VMUX_DEBUG_FOCUS").is_some() {
            let y = term
                .root()
                .and_then(|r| term.compute_bounds(&r))
                .map(|b| b.y());
            eprintln!(
                "focus-enter: zone={} term={:?} y={:?}",
                zone.name.borrow(),
                term.as_ptr(),
                y
            );
        }
        zone.last_focused.set(Some(term));
        if let Some(pane) = crate::splits::pane_of(term.upcast_ref()) {
            crate::splits::remember_focused_pane(&pane);
        }
    }

    fn bell(&self, _term: &vte::Terminal) {
        if let Some(zone) = self.zone.upgrade() {
            self.app.on_bell(&zone);
        }
    }

    fn notified(&self, term: Option<&vte::Terminal>, title: &str, body: &str) {
        if let Some(zone) = self.zone.upgrade() {
            self.app.on_notify(&zone, term, title, body);
        }
    }

    fn exited(&self, term: &vte::Terminal) {
        if let Some(zone) = self.zone.upgrade() {
            self.app.on_term_exited(&zone, term);
        }
    }
}

/// Build a terminal leaf for `zone`, starting in `cwd`.
pub fn build_leaf(app: &Rc<App>, zone: &Weak<Zone>, cwd: String) -> gtk::ScrolledWindow {
    vmux::term::build(
        Rc::new(ZoneTerminal {
            app: app.clone(),
            zone: zone.clone(),
        }),
        cwd,
    )
}
