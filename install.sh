#!/usr/bin/env bash
# User-local install: binaries, .desktop, icons. No root needed.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release
install -Dm755 target/release/vmux ~/.local/bin/vmux
install -Dm755 target/release/vmux-relay ~/.local/bin/vmux-relay
# Launchers often lack ~/.local/bin on PATH, and .desktop files don't expand ~,
# so bake the absolute binary path into the installed copy.
mkdir -p ~/.local/share/applications
sed "s#^Exec=.*#Exec=$HOME/.local/bin/vmux#" data/dev.vmux.Vmux.desktop \
  > ~/.local/share/applications/dev.vmux.Vmux.desktop
chmod 644 ~/.local/share/applications/dev.vmux.Vmux.desktop
for n in 16 32 48 64 128 256 512; do
  install -Dm644 data/icons/vmux-$n.png ~/.local/share/icons/hicolor/${n}x${n}/apps/dev.vmux.Vmux.png
done
gtk4-update-icon-cache -q ~/.local/share/icons/hicolor 2>/dev/null || true
update-desktop-database ~/.local/share/applications 2>/dev/null || true
echo "installed: ~/.local/bin/vmux"
