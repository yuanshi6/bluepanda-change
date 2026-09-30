#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --workspace --release
app_dir="$PWD/dist/BluePandaChange.app/Contents"
mkdir -p "$app_dir/MacOS"
cp target/release/bluepanda-gui "$app_dir/MacOS/BluePandaChange"
cat > "$app_dir/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>BluePandaChange</string>
  <key>CFBundleDisplayName</key><string>BluePandaChange</string>
  <key>CFBundleIdentifier</key><string>dev.bluepanda.change</string>
  <key>CFBundleExecutable</key><string>BluePandaChange</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
PLIST
printf 'Built CLI: %s\n' "$PWD/target/release/bluepanda-change"
printf 'Built GUI: %s\n' "$PWD/dist/BluePandaChange.app"
