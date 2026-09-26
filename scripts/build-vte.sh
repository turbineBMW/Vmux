#!/usr/bin/env bash
# Build the Vmux fork of libvte (with SIXEL, kitty graphics protocol and
# iTerm2 inline image support) into vendor/vte, where .cargo/config.toml
# points pkg-config and the runtime linker.
#
# Usage: scripts/build-vte.sh [--clean]
#
# Environment:
#   VTE_SRC   use an existing checkout instead of cloning (e.g. ../vte)
#   VTE_REPO  git URL to clone (default: the Vmux fork on GitHub)
#   VTE_REF   branch or tag to build (default: image-support)
set -euo pipefail
cd "$(dirname "$0")/.."

VTE_REPO=${VTE_REPO:-https://github.com/turbineBMW/vte.git}
VTE_REF=${VTE_REF:-image-support}
PREFIX=$PWD/vendor/vte
SRC=${VTE_SRC:-$PWD/vendor/vte-src}
BUILD=$PWD/vendor/vte-build

if [[ ${1:-} == --clean ]]; then
  rm -rf "$PREFIX" "$BUILD"
  [[ -z ${VTE_SRC:-} ]] && rm -rf "$SRC"
fi

for tool in meson ninja glib-mkenums glib-genmarshal pkg-config; do
  if ! command -v "$tool" >/dev/null; then
    echo "build-vte: missing '$tool'." >&2
    echo "  Arch: sudo pacman -S --needed base-devel meson ninja glib2-devel gtk4 pkgconf" >&2
    exit 1
  fi
done

if [[ -z ${VTE_SRC:-} ]]; then
  if [[ ! -d $SRC/.git ]]; then
    git clone --depth 1 --branch "$VTE_REF" "$VTE_REPO" "$SRC"
  else
    git -C "$SRC" fetch --depth 1 origin "$VTE_REF"
    git -C "$SRC" checkout -q FETCH_HEAD
  fi
fi

# Build only the GTK4 widget; Vmux needs neither the GTK3 widget, bindings
# nor docs. Subprojects (simdutf, fast_float) are fetched by meson if the
# system lacks them.
if [[ ! -f $BUILD/build.ninja ]]; then
  meson setup "$BUILD" "$SRC" \
    --prefix="$PREFIX" --libdir=lib \
    --buildtype=release \
    -Dgtk3=false -Dgtk4=true -Dsixel=true \
    -Dgir=false -Dvapi=false -Ddocs=false -Dglade=false -Dapp=false
fi
ninja -C "$BUILD"
meson install -C "$BUILD" --quiet

echo "build-vte: installed to $PREFIX"
echo "  $(PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig pkg-config --modversion vte-2.91-gtk4) at $PREFIX/lib/libvte-2.91-gtk4.so"
