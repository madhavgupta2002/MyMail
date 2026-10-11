#!/usr/bin/env bash
# Builds the Rust server and the SwiftUI client, then assembles build/MyMail.app.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/build/MyMail.app"

echo "==> Building backend (cargo, release)"
cargo build --release --quiet --manifest-path "$ROOT/backend/Cargo.toml"

# With only Command Line Tools installed, the macOS 27 SDK expands @State via a
# macro plugin that CLT doesn't ship. Fall back to the newest macOS 26 SDK.
if [[ -z "${SDKROOT:-}" && "$(xcode-select -p)" == *CommandLineTools* ]]; then
    SDK26="$(ls -d /Library/Developer/CommandLineTools/SDKs/MacOSX26.[0-9]*.sdk 2>/dev/null | sort -V | tail -1)"
    [[ -n "$SDK26" ]] && export SDKROOT="$SDK26"
fi

echo "==> Building frontend (swift, release${SDKROOT:+, SDK $(basename "$SDKROOT")})"
swift build -c release --package-path "$ROOT/app" 2>&1 | grep -Ev '^\[[0-9]+ ?/ ?[0-9]+\]|search path .* not found|^Building for' || true
BIN_DIR="$(swift build -c release --package-path "$ROOT/app" --show-bin-path)"
[[ -x "$BIN_DIR/MyMail" ]] || { echo "frontend build failed" >&2; exit 1; }

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$ROOT/data"
cp "$BIN_DIR/MyMail" "$APP/Contents/MacOS/MyMail"
cp "$ROOT/backend/target/release/mymail-server" "$APP/Contents/Resources/mymail-server"

xml_escape() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' <<<"$1"; }

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>MyMail</string>
    <key>CFBundleDisplayName</key><string>MyMail</string>
    <key>CFBundleIdentifier</key><string>local.mymail.MyMail</string>
    <key>CFBundleExecutable</key><string>MyMail</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>NSAppTransportSecurity</key>
    <dict><key>NSAllowsLocalNetworking</key><true/></dict>
    <key>MyMailDataDir</key><string>$(xml_escape "$ROOT/data")</string>
</dict>
</plist>
PLIST

if [[ -f "$ROOT/assets/AppIcon.icns" ]]; then
    cp "$ROOT/assets/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
    /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string AppIcon" "$APP/Contents/Info.plist"
fi

codesign --force --sign - "$APP/Contents/Resources/mymail-server" >/dev/null 2>&1
codesign --force --sign - "$APP" >/dev/null 2>&1

echo "==> Built $APP"
