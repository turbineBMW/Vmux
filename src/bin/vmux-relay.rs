//! vmux-relay: the PTY shim vte spawns in front of the user's shell. The
//! code lives in the library ([`vmux::relay`]) so every front-end ships the
//! same relay.

fn main() {
    vmux::relay::main()
}
