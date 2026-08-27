# Vmux

A minimal, low-resource terminal workspace for Linux — a
[cmux](https://github.com/manaflow-ai/cmux)-style shell built on
**GTK4 + libadwaita + VTE** instead of libghostty. The whole UI idles at ~0% CPU.

Each **work zone** (left sidebar) is an independent workspace containing
**panes you split as needed**; every pane has **its own tab strip**:

```
┌──────────┬───────────────────────────┬───────────────────┐
│ zones    │ [tab][tab][tab]        +  │ [tab]          +  │
│          ├───────────────────────────┤                   │
│ ~ main   │                           │                   │
│ ● work   │  $ nvim .                 │  $ claude         │
│   blog   │                           │                   │
└──────────┴───────────────────────────┴───────────────────┘
            └── one pane ──────────────┘└── another pane ──┘
```

Nothing is created for you: a new zone starts as a single pane with one shell
tab. Split it (right/down) to add panes, open tabs per pane with the `+`
button or the New Tab shortcut, and run whatever you want in them (your editor
on the left, `claude` on the right, …). Closing a pane's last tab collapses
the split. Tabs can be dragged between panes.

## Features

- **Zones** — named workspaces in a sidebar, each remembering its own pane
  layout, tabs, working directories and focused pane. Reorder, rename and
  close them from the keyboard or the right-click menu.
- **Splits and tabs** — arbitrary right/down splits, a tab strip per pane,
  directional pane focus, resizable splits, drag-and-drop tabs between panes.
- **Desktop notifications** — programs that emit OSC 9 / OSC 777 / kitty
  OSC 99 (e.g. Claude Code, ninja, `notify-send`-style shell helpers) raise a
  real desktop notification naming the zone, even when it's hidden. Clicking
  the notification switches to that zone. A bell in a hidden zone shows an
  attention dot in the sidebar and can optionally notify too.
- **Git status** — each zone shows its branch and working-tree state under its name in the sidebar, in colours you can theme.
- **Live CSS themes** — every colour, font and chrome setting is a CSS token
  edited from Preferences or by hand; changes apply instantly and can be
  saved as named themes.
- **Rebindable keys** — every action is rebindable from Preferences.
- **Persistent state** — layout and settings survive restarts.

## Install

Dependencies (Arch): `pacman -S gtk4 libadwaita vte4` plus a Rust toolchain.
Other distros need the equivalent dev packages; **libvte ≥ 0.78** and
libadwaita ≥ 1.6 are required.

User-local install (binaries to `~/.local/bin`, plus the `.desktop` file and
icons so launchers and notifications show Vmux properly):

```sh
./install.sh
```

Or just build and run from the tree:

```sh
cargo build --release
./target/release/vmux
```

Two binaries are produced: `vmux` (the app) and `vmux-relay`, a small
`script(1)`-style PTY shim that vte spawns in front of your shell. libvte
offers no hook for unknown OSC sequences, so the relay watches the output
stream for notification OSCs and forwards them to Vmux as vte termprops,
passing everything else through untouched. Keep both on `PATH` (or use
`install.sh`, which does).

## Keybindings

All of these are **rebindable** in Preferences (gear icon, or `Ctrl+,`) —
click a row, press the new combination (Backspace unbinds, Esc cancels).
Assigning a combination that's in use steals it from the other action.
Defaults:

| Key | Action |
|---|---|
| `Ctrl+Shift+T` | New tab in the focused pane (inherits current directory) |
| `Ctrl+Shift+W` | Close focused tab (last tab closes the pane) |
| `Ctrl+PageUp` / `Ctrl+PageDown` | Previous / next tab |
| `Ctrl+Shift+E` | Split pane right |
| `Ctrl+Shift+O` | Split pane down |
| `Ctrl+Shift+A` | Focus next pane |
| `Ctrl+Alt+←/→/↑/↓` | Focus pane in that direction |
| `Ctrl+Shift+=` / `Ctrl+Shift+-` | Grow / shrink focused pane |
| `Ctrl+Shift+N` | New zone |
| `Ctrl+Alt+W` | Close zone |
| `Ctrl+Alt+R` | Rename zone |
| `Ctrl+Alt+PageUp` / `Ctrl+Alt+PageDown` | Previous / next zone |
| `Ctrl+Shift+PageUp` / `Ctrl+Shift+PageDown` | Move zone up / down in the sidebar |
| `Alt+1`…`Alt+9` | Switch to zone N (fixed) |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Font scale |
| `F9` | Toggle sidebar |
| `Ctrl+,` | Preferences |

Right-click a zone in the sidebar to rename or remove it.

## Preferences

The settings dialog has three pages:

- **General** — scrollback lines, shell override (new terminals), window
  behaviour, and notifications: enable/disable, notify on bell, and the
  notification sound (system default, a custom file, or none). Vmux never
  plays audio itself — the choice is passed as a hint to
  `org.freedesktop.Notifications`, so your desktop's do-not-disturb rules
  decide whether anything is heard. A "send a test notification" button lets
  you check the result.
- **Appearance** — quick-switch among saved themes, save the current
  stylesheet as a named theme, and labelled controls for the terminal font,
  every terminal and ANSI colour, window chrome, tab/session indicators, and
  Git status colours. Each control writes directly to the live-reloaded
  `style.css`; an Advanced row opens the file for hand editing.
- **Keybindings** — every action with its current shortcut; click to rebind.

### Files

| Path | Contents |
|---|---|
| `~/.config/vmux/state.json` | Zones, layout, tabs and behavioural settings (`config.*`, including `config.keybindings` overrides) |
| `~/.config/vmux/style.css` | Live stylesheet; every colour token is documented inline |
| `~/.config/vmux/themes/*.css` | Named theme snapshots |

Set terminal opacity by giving `vmux_terminal_background` an `rgba()` value
with an alpha below 1.0.

### Directory tracking (OSC 7)

"New tab in current directory" and cwd persistence rely on the shell emitting
OSC 7 (foot users usually have this already). For zsh, if you don't:

```zsh
function _vmux_osc7() { printf '\e]7;file://%s%s\e\\' "$HOST" "$PWD" }
add-zsh-hook -Uz chpwd _vmux_osc7 && _vmux_osc7
```

Without it, new tabs fall back to the zone's directory.

## Development

```sh
cargo test                 # unit tests (state, OSC scanner, splits, …)
cargo clippy --all-targets
cargo run                  # debug build
```

Source layout: `src/main.rs` is the app binary, `src/bin/vmux-relay.rs` the
PTY shim, and `src/lib.rs` exposes the OSC scanner they share. Everything else
in `src/` is one module per UI concern (`zone`, `pane`, `splits`, `term`,
`notify`, `appearance`, `keybinds`, …).

## Known limitations

- A tab's title/cwd tracking follows its first terminal.
- State is saved on changes (debounced) and on window close — not on
  SIGKILL/crash.
- Running programs are not restored across restarts (tabs reopen as shells in
  their saved directories).
- UI is forced dark.
- Linux only (relies on a Linux PTY and the freedesktop notification bus).

## License

[MIT](LICENSE)
