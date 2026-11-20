#!/usr/bin/env bash
# Builds target/release/Latent.app (ad-hoc signed). Usage: scripts/bundle-macos.sh [version]
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' apps/desktop/Cargo.toml | head -1)}"
version="${version#v}"

cargo build --release -p latent

app=target/release/Latent.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/latent "$app/Contents/MacOS/latent"
sed "s/@VERSION@/$version/g" apps/desktop/macos/Info.plist > "$app/Contents/Info.plist"

iconset="$(mktemp -d)/latent.iconset"
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
    sips -z $s $s apps/desktop/assets/icon.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
    sips -z $((s * 2)) $((s * 2)) apps/desktop/assets/icon.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/latent.icns"

codesign --force --deep -s - "$app"
echo "$app ($version)"
