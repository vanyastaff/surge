#!/usr/bin/env bash
# Package the desktop shell and its CLI/daemon siblings in a macOS .app bundle.
#
# A bundle gives the desktop shell a real CFBundleIdentifier, which the UI
# needs for Notification Center delivery (unbundled launches skip OS
# notifications, see crates/surge-ui/src/notifications.rs). It also lets the
# app be opened from Finder / `open` like any other application.
#
# Usage: scripts/bundle-macos-app.sh [--build] [--release] [--out DIR] [--surge-home DIR]
#   --build        freshly build all three executables before packaging
#   --release      select target/release (default: target/debug)
#   --out DIR      where to write "Surge.app" (default: target/<profile>/bundle)
#   --surge-home   bake SURGE_HOME into LSEnvironment (isolated acceptance runs)
set -euo pipefail

build=false
profile=debug
out=""
surge_home=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --build) build=true; shift ;;
        --release) profile=release; shift ;;
        --out)
            [[ $# -ge 2 && -n "$2" ]] || { echo "--out requires a directory" >&2; exit 2; }
            out="$2"; shift 2 ;;
        --surge-home)
            [[ $# -ge 2 && -n "$2" ]] || { echo "--surge-home requires a directory" >&2; exit 2; }
            surge_home="$2"; shift 2 ;;
        -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "bundle-macos-app.sh only runs on macOS" >&2
    exit 1
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
binary_dir="$root/target/$profile"
if [[ "$build" == true ]]; then
    if ! rustc_metadata="$(rustc -vV)"; then
        echo "cannot determine the native Rust host" >&2
        exit 1
    fi
    host=""
    host_count=0
    while IFS= read -r line; do
        case "$line" in
            host:*) host="${line#host: }"; host_count=$((host_count + 1)) ;;
        esac
    done <<< "$rustc_metadata"
    if [[ "$host_count" -ne 1 || ! "$host" =~ ^[A-Za-z0-9_]+-apple-darwin$ ]]; then
        echo "expected exactly one valid Darwin Rust host" >&2
        exit 1
    fi
    binary_dir="$root/target/$host/$profile"
    cargo_args=(build --locked --target "$host" --target-dir "$root/target" -p surge-ui --bin surge-ui -p surge-cli --bin surge -p surge-daemon --bin surge-daemon)
    if [[ "$profile" == release ]]; then
        cargo_args+=(--release)
    fi
    (
        cd "$root"
        cargo "${cargo_args[@]}"
    )
fi

binaries=(surge-ui surge surge-daemon)
for name in "${binaries[@]}"; do
    bin="$binary_dir/$name"
    if [[ ! -f "$bin" || ! -x "$bin" ]]; then
        echo "missing executable $bin — use --build to build all bundle binaries" >&2
        exit 1
    fi
done
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
out="${out:-$root/target/$profile/bundle}"
app="$out/Surge.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
for name in "${binaries[@]}"; do
    cp "$binary_dir/$name" "$app/Contents/MacOS/$name"
done

env_block=""
if [[ -n "$surge_home" ]]; then
    surge_home_xml="$(printf '%s' "$surge_home" | sed -e 's/\&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's/"/\&quot;/g' -e "s/'/\&apos;/g")"
    env_block="    <key>LSEnvironment</key>
    <dict>
        <key>SURGE_HOME</key>
        <string>$surge_home_xml</string>
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
