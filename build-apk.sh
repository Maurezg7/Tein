#!/usr/bin/env bash
# Builds a signed Tein APK using only the Android SDK command-line tools (no Gradle).
# Usage: ./build-apk.sh [abi ...]   (default: arm64-v8a armeabi-v7a)
set -euo pipefail
cd "$(dirname "$0")"

SDK="${ANDROID_HOME:-$HOME/android-sdk}"
BT="$SDK/build-tools/${BUILD_TOOLS:-34.0.0}"
PLATFORM_JAR="$SDK/platforms/android-${COMPILE_SDK:-34}/android.jar"
export ANDROID_HOME="$SDK"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$(ls -d "$SDK"/ndk/* | sort -V | tail -1)}"
export ANDROID_JAR="$PLATFORM_JAR"
ABIS=("${@:-arm64-v8a armeabi-v7a}")
read -r -a ABIS <<< "${ABIS[*]}"

APP=android/app/src/main
OUT=target/apk
KEYSTORE="${TEIN_KEYSTORE:-android/tein-release.jks}"
KS_PASS="${TEIN_KEYSTORE_PASSWORD:-tein123}"
rm -rf "$OUT" && mkdir -p "$OUT"/{res,gen,classes,lib}

echo "==> Rust (${ABIS[*]})"
if [ -z "${SKIP_CARGO:-}" ]; then
    for abi in "${ABIS[@]}"; do
        if [ "$abi" = armeabi-v7a ]; then
            # rust-skia ships no 32-bit ARM binaries: build Skia from source, with the NDK's libclang for bindgen.
            FORCE_SKIA_BUILD=1 ANDROID_NDK="${ANDROID_NDK:-$ANDROID_NDK_HOME}" SKIA_NINJA_COMMAND="${SKIA_NINJA_COMMAND:-$(command -v ninja)}" \
                LIBCLANG_PATH="${LIBCLANG_PATH:-$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/lib}" \
                cargo ndk -P 26 -t "$abi" -o "$OUT/lib" build --release --lib
        else
            cargo ndk -P 26 -t "$abi" -o "$OUT/lib" build --release --lib
        fi
    done
else
    for abi in "${ABIS[@]}"; do
        mkdir -p "$OUT/lib/$abi"
        t=$(case $abi in arm64-v8a) echo aarch64-linux-android;; armeabi-v7a) echo armv7-linux-androideabi;; x86_64) echo x86_64-linux-android;; esac)
        cp "target/$t/release/libtein.so" "$OUT/lib/$abi/"
    done
fi

# Skia and Oboe link against the shared C++ runtime.
for abi in "${ABIS[@]}"; do
    t=$(case $abi in arm64-v8a) echo aarch64-linux-android;; armeabi-v7a) echo arm-linux-androideabi;; x86_64) echo x86_64-linux-android;; esac)
    cp "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$t/libc++_shared.so" "$OUT/lib/$abi/"
    "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip" "$OUT/lib/$abi/libc++_shared.so"
done

echo "==> Recursos"
sed 's|<manifest |<manifest package="com.tein.app" |' "$APP/AndroidManifest.xml" > "$OUT/AndroidManifest.xml"
"$BT/aapt2" compile --dir "$APP/res" -o "$OUT/res.zip"
"$BT/aapt2" link -o "$OUT/base.apk" -I "$PLATFORM_JAR" --manifest "$OUT/AndroidManifest.xml" \
    --java "$OUT/gen" --min-sdk-version 26 --target-sdk-version 34 \
    --version-code 6 --version-name 1.2.1 "$OUT/res.zip"

echo "==> Java"
javac -nowarn -Xlint:-options -source 8 -target 8 -bootclasspath "$PLATFORM_JAR:$BT/core-lambda-stubs.jar" \
    -d "$OUT/classes" $(find "$APP/java" "$OUT/gen" -name '*.java')
"$BT/d8" --release --min-api 26 --lib "$PLATFORM_JAR" --output "$OUT" $(find "$OUT/classes" -name '*.class')

echo "==> Empaquetado"
cp "$OUT/base.apk" "$OUT/unaligned.apk"
(cd "$OUT" && zip -q unaligned.apk classes.dex)
# Native libraries stay uncompressed so Android can map them directly (extractNativeLibs=false).
(cd "$OUT" && zip -q -0 -r unaligned.apk lib)
"$BT/zipalign" -f -p 4 "$OUT/unaligned.apk" "$OUT/aligned.apk"

if [ ! -f "$KEYSTORE" ]; then
    keytool -genkeypair -keystore "$KEYSTORE" -alias tein -keyalg RSA -keysize 2048 -validity 10000 \
        -storepass "$KS_PASS" -keypass "$KS_PASS" -dname "CN=Tein" >/dev/null 2>&1
fi
"$BT/apksigner" sign --ks "$KEYSTORE" --ks-pass "pass:$KS_PASS" --ks-key-alias "${TEIN_KEY_ALIAS:-tein}" \
    --out "$OUT/tein.apk" "$OUT/aligned.apk"
ls -lh "$OUT/tein.apk"
