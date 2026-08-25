# Vmux

A minimal, low-resource terminal workspace manager for Linux — a [cmux](https://github.com/manaflow-ai/cmux)-style shell built on **GTK4 + libadwaita + VTE** instead of libghostty. The whole UI idles at ~0% CPU.

Each **work zone** (left sidebar) is an independent workspace containing **panes you split as needed**; every pane has **its own tab strip**:

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

Nothing is created for you: a new zone starts as a single pane with one shell tab. Split it (right/down) to add panes, open tabs per pane with the `+` button or the New Tab shortcut, and run whatever you want in them (your editor on the left, `claude` on the right, …). Closing a pane's last tab collapses the split. Tabs can be dragged between panes. A bell in a hidden zone shows an attention dot in the sidebar.

The layout and behavioral settings persist across restarts in
`~/.config/vmux/state.json`. Appearance is kept separately in
`~/.config/vmux/style.css`, with reusable CSS snapshots in
`~/.config/vmux/themes/`.

## Build & run

Dependencies (Arch): `pacman -S gtk4 libadwaita vte4` plus a Rust toolchain.

```sh
cargo build --release
./target/release/vmux
```

## Keybindings

All of these are **rebindable** in Preferences (gear icon, or `Ctrl+,`) — click a row, press the new combination (Backspace unbinds, Esc cancels). Defaults:

| Key | Action |
|---|---|
| `Ctrl+Shift+T` | New tab in the focused pane (inherits current directory) |
| `Ctrl+Shift+W` | Close focused tab (last tab closes the pane) |
| `Ctrl+Shift+E` | Split pane right |
| `Ctrl+Shift+O` | Split pane down |
| `Ctrl+Shift+A` | Focus next pane |
| `Ctrl+PageUp` / `Ctrl+PageDown` | Previous / next tab in the focused pane |
| `Alt+1`…`Alt+9` | Switch zone (fixed) |
| `Ctrl+Shift+N` | New zone |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Font scale |
| `F9` | Toggle sidebar |
| `Ctrl+,` | Preferences |

Right-click a zone in the sidebar to rename or remove it.

## Preferences

The settings dialog has three pages:

- **General** — scrollback lines, shell override (new terminals), window, and notification behavior.
- **Appearance** — quick-switch among saved themes, save the current stylesheet as a named theme, and use labeled controls for the terminal font, every terminal and ANSI color, window chrome, tab/session indicators, and Git status colors. Each control writes directly to the live-reloaded `style.css`; an Advanced row still opens the file for hand editing. Themes are complete `.css` snapshots stored in `~/.config/vmux/themes/`.
- **Keybindings** — every action with its current shortcut; click to rebind. Assigning a combination that's in use steals it from the other action. Overrides are stored in `config.keybindings` in `state.json`.

The generated stylesheet documents every terminal color token. Set terminal
opacity by giving `vmux_terminal_background` an `rgba()` value with an alpha
below 1.0.

### Directory tracking (OSC 7)

"New tab in current directory" and cwd persistence rely on the shell emitting OSC 7 (foot users usually have this already). For zsh, if you don't:

```zsh
function _vmux_osc7() { printf '\e]7;file://%s%s\e\\' "$HOST" "$PWD" }
add-zsh-hook -Uz chpwd _vmux_osc7 && _vmux_osc7
```

Without it, new tabs fall back to the zone's directory.

## Known limitations (MVP)

- A tab's title/cwd tracking follows its first terminal.
- State is saved on changes (debounced) and on window close — not on SIGKILL/crash.
- Running programs are not restored across restarts (tabs reopen as shells in their saved directories).
- UI is forced dark.
