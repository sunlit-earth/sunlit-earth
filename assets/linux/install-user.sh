#!/usr/bin/env bash
# Put the desktop entry and the icon set where a Linux session looks for them.
#
# There is no package yet, so this installs into the user-local half of the
# freedesktop layout: $XDG_DATA_HOME/applications for the entry, and
# .../icons/hicolor for the rasters plus the master SVG in the scalable slot.
# Nothing here needs root, and nothing outside $XDG_DATA_HOME is touched.
#
# The entry ships with `Exec=sunlit-earth`, which wants the binary on PATH.
# `--exec <path>` rewrites that line for a build that is not installed, which
# is what a checkout has.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
assets="$(cd "$here/.." && pwd)"
baked="$assets/icon/baked/hicolor"
master="$assets/icon/sunlit-earth.svg"

exec_line=""
prefix="${XDG_DATA_HOME:-$HOME/.local/share}"

usage() {
    cat <<'USAGE'
usage: install-user.sh [--exec PATH] [--prefix DIR]

  --exec PATH    what the entry runs; default is `sunlit-earth` on PATH
  --prefix DIR   the data directory; default is $XDG_DATA_HOME or ~/.local/share
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --exec) exec_line="${2:?--exec needs a path}"; shift 2 ;;
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ ! -d "$baked" ]; then
    echo "no baked icons at $baked; run \`cargo xtask bake-icon\` first" >&2
    exit 1
fi

applications="$prefix/applications"
theme="$prefix/icons/hicolor"
entry="$applications/sunlit-earth.desktop"

mkdir -p "$applications"
cp "$here/sunlit-earth.desktop" "$entry"
if [ -n "$exec_line" ]; then
    # A desktop entry's Exec is a command line, so an absolute path with spaces
    # in it has to arrive quoted or the launcher splits it into arguments.
    case "$exec_line" in
        *[[:space:]]*) exec_line="\"$exec_line\"" ;;
    esac
    sed -i "s|^Exec=.*|Exec=$exec_line|" "$entry"
fi
echo "installed $entry"

installed=0
for dir in "$baked"/*x*/; do
    size="$(basename "$dir")"
    target="$theme/$size/apps"
    mkdir -p "$target"
    cp "$dir/apps/sunlit-earth.png" "$target/sunlit-earth.png"
    installed=$((installed + 1))
done
mkdir -p "$theme/scalable/apps"
cp "$master" "$theme/scalable/apps/sunlit-earth.svg"
echo "installed $installed rasters and the SVG under $theme"

# Both caches are optional. A session reads the files directly when neither
# exists, so a missing tool is a slower first lookup and not a failure; the
# icon cache additionally refuses to run on a theme with no index.theme, which
# a user-local hicolor tree normally has none of.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$theme/index.theme" ]; then
    gtk-update-icon-cache -f -t "$theme" || true
fi
# KDE serves its menu out of sycoca rather than reading the directory, so
# Kickoff finds a new entry only once that cache has been rebuilt. Plasma 6
# renamed the tool; a session with neither is one that is not KDE.
for sycoca in kbuildsycoca6 kbuildsycoca5; do
    if command -v "$sycoca" >/dev/null 2>&1; then
        "$sycoca" >/dev/null 2>&1 || true
        break
    fi
done

echo
echo "to remove it again:"
echo "  rm -f $entry"
echo "  rm -f $theme/*/apps/sunlit-earth.png $theme/scalable/apps/sunlit-earth.svg"
