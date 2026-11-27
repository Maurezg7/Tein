#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export MACOSX_DEPLOYMENT_TARGET=13.0
case "${1:-$(uname -m)}" in
    arm64) target=aarch64-apple-darwin ;;
    x86_64) target=x86_64-apple-darwin ;;
    *) echo 'Se requiere arm64 o x86_64' >&2; exit 2 ;;
esac
cargo build --release --locked --target "$target" --bin tein-desktop
out=target/apple/$target
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/Tein.app/Contents"
mkdir -p "$out" "$app/MacOS" "$app/Resources" "$work/Tein.iconset"
install -m 755 "target/$target/release/tein-desktop" "$app/MacOS/Tein"
install -m 644 packaging/apple/macos.plist "$app/Info.plist"
cp LICENSE ui/fonts/VT323-OFL.txt "$app/Resources/"
for size in 16 32 128 256 512; do
    for scale in 1 2; do
        suffix=''; [[ $scale == 1 ]] || suffix='@2x'
        sips -z "$((size*scale))" "$((size*scale))" docs/logo.png \
          --out "$work/Tein.iconset/icon_${size}x${size}${suffix}.png" >/dev/null
    done
done
iconutil -c icns "$work/Tein.iconset" -o "$app/Resources/Tein.icns"
plutil -lint "$app/Info.plist"
codesign --force --sign - "$work/Tein.app"
codesign --verify --strict "$work/Tein.app"
ditto -c -k --keepParent "$work/Tein.app" "$out/Tein-macOS-${target%%-apple-*}.zip"
