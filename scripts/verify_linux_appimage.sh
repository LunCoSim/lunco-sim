#!/usr/bin/env bash
# Verify the root AppImage desktop/icon contract emitted by Velopack.

set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <appimage>" >&2
    exit 2
fi

appimage="$(realpath "$1")"
if [ ! -f "$appimage" ]; then
    echo "AppImage does not exist: $appimage" >&2
    exit 1
fi

extract_dir="$(mktemp -d "${TMPDIR:-/tmp}/luncosim-appimage-verify.XXXXXX")"
cleanup() {
    rm -rf -- "$extract_dir"
}
trap cleanup EXIT

if ! (cd "$extract_dir" && "$appimage" --appimage-extract >/dev/null); then
    echo "AppImage extraction failed: $appimage" >&2
    exit 1
fi

appdir="$extract_dir/squashfs-root"
if [ ! -d "$appdir" ]; then
    echo "AppImage extraction did not produce squashfs-root: $appimage" >&2
    exit 1
fi

mapfile -t desktop_files < <(find "$appdir" -maxdepth 1 -type f -name '*.desktop' -print)
if [ "${#desktop_files[@]}" -ne 1 ]; then
    echo "expected exactly one root desktop file, found ${#desktop_files[@]}" >&2
    exit 1
fi

desktop_file="${desktop_files[0]}"
icon_name="$(sed -n 's/^Icon=//p' "$desktop_file" | head -n 1)"
if [ -z "$icon_name" ] || [[ "$icon_name" == */* || "$icon_name" == *.* ]]; then
    echo "root desktop file must declare an extensionless root Icon: $desktop_file" >&2
    exit 1
fi
if [ ! -f "$appdir/$icon_name.png" ] && [ ! -f "$appdir/$icon_name.svg" ]; then
    echo "root desktop Icon has no matching root image: $icon_name" >&2
    exit 1
fi
if [ ! -e "$appdir/.DirIcon" ]; then
    echo "AppImage is missing root .DirIcon" >&2
    exit 1
fi

startup_wm_class="$(sed -n 's/^StartupWMClass=//p' "$desktop_file" | head -n 1)"
if [ "$startup_wm_class" != "luncosim" ]; then
    echo "StartupWMClass=$startup_wm_class does not match luncosim" >&2
    exit 1
fi

echo "Verified Linux AppImage: $(basename "$appimage") (luncosim)"
