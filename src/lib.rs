//! Library half of the vmux package: the core shared by the `vmux` desktop
//! app, the `vmux-relay` PTY shim and other front-ends (the omarchy-mobile
//! phone app): zones and tabs state, building terminals, the OSC relay
//! protocol, remote-host labels, desktop notifications, git status and the
//! Omarchy theme. Nothing here depends on how a front-end lays out its window.

pub mod agents;
pub mod git;
pub mod notify;
pub mod omarchy;
pub mod osc_scan;
pub mod relay;
pub mod remote;
pub mod remote_tab;
pub mod state;
pub mod style;
pub mod term;
pub mod text_bindings;
