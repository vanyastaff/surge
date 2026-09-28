#!/usr/bin/env bash
# Wrap the surge-ui binary in a minimal macOS .app bundle.
#
# A bundle gives the desktop shell a real CFBundleIdentifier, which the UI
# needs for Notification Center delivery (unbundled launches skip OS
# notifications, see crates/surge-ui/src/notifications.rs). It also lets the
# app be opened from Finder / `open` like any other application.
#
# Usage: scripts/bundle-macos-app.sh [--release] [--out DIR] [--surge-home DIR]
#   --release      bundle target/release/surge-ui (default: target/debug)
#   --out DIR      where to write "Surge.app" (default: target/<profile>/bundle)
#   --surge-home   bake SURGE_HOME into LSEnvironment (isolated acceptance runs)
set -euo pipefail

profile=debug
out=""
surge_home=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --release) profile=release; shift ;;
        --out) out="$2"; shift 2 ;;
        --surge-home) surge_home="$2"; shift 2 ;;
        -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "bundle-macos-app.sh only runs on macOS" >&2
    exit 1
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/$profile/surge-ui"
if [[ ! -x "$bin" ]]; then
    flag=""; [[ "$profile" == release ]] && flag=" --release"
    echo "missing $bin — build it first (cargo build -p surge-ui$flag)" >&2
    exit 1
fi
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
out="${out:-$root/target/$profile/bundle}"
app="$out/Surge.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$bin" "$app/Contents/MacOS/surge-ui"

env_block=""
if [[ -n "$surge_home" ]]; then
    env_block="    <key>LSEnvironment</key>
    <dict>
        <key>SURGE_HOME</key>
        <string>$surge_home</string>
    </dict>"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDisplayName</key>
    <string>Surge</string>
    <key>CFBundleName</key>
    <string>Surge</string>
    <key>CFBundleExecutable</key>
    <string>surge-ui</string>
    <key>CFBundleIdentifier</key>
    <string>dev.surge.desktop</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
$env_block
</dict>
</plist>
PLIST

codesign --force --sign - "$app" >/dev/null 2>&1 || echo "warning: ad-hoc codesign failed" >&2
echo "$app"
