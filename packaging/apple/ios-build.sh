#!/usr/bin/env bash
set -euo pipefail
cd "$SRCROOT/../.."
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export CARGO_TARGET_DIR="${TEIN_CARGO_TARGET_DIR:-$DERIVED_FILE_DIR/cargo}"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
# Cargo also compiles host tools; do not pass the target SDK to their linker.
unset SDKROOT CC CXX AR LD CFLAGS CXXFLAGS CPPFLAGS LDFLAGS
profile=debug
args=()
if [[ $CONFIGURATION != Debug ]]; then
    profile=release; args+=(--release)
    export CARGO_PROFILE_RELEASE_DEBUG=1 CARGO_PROFILE_RELEASE_STRIP=none
fi
binaries=()
for arch in $ARCHS; do
    case "$PLATFORM_NAME/$arch" in
        iphoneos/arm64) target=aarch64-apple-ios ;;
        iphonesimulator/arm64) target=aarch64-apple-ios-sim ;;
        iphonesimulator/x86_64) target=x86_64-apple-ios ;;
        *) echo "Arquitectura no admitida: $PLATFORM_NAME/$arch" >&2; exit 1 ;;
    esac
    cargo build --locked ${args[@]+"${args[@]}"} --target "$target" --bin tein-desktop
    binaries+=("$CARGO_TARGET_DIR/$target/$profile/tein-desktop")
done
exe="$TARGET_BUILD_DIR/$EXECUTABLE_PATH"
mkdir -p "$(dirname "$exe")"
lipo -create "${binaries[@]}" -output "$exe"
if [[ $profile == release ]]; then
    if [[ -n ${DWARF_DSYM_FOLDER_PATH:-} && -n ${DWARF_DSYM_FILE_NAME:-} ]]; then
        mkdir -p "$DWARF_DSYM_FOLDER_PATH"
        dsymutil "$exe" -o "$DWARF_DSYM_FOLDER_PATH/$DWARF_DSYM_FILE_NAME"
    fi
    strip -S "$exe"
fi
