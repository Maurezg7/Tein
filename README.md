# Ritmo

Reproductor de música liviano para Android, escrito en Rust (UI con [Slint](https://slint.dev), audio con [Rodio](https://github.com/RustAudio/rodio)).

## Funciones

- Biblioteca local (MP3, M4A/AAC, FLAC, OGG, WAV) con vistas por canciones, artistas, álbumes y listas.
- Buscador en la biblioteca y buscador online.
- Reproductor completo: play/pausa, anterior/siguiente, barra de progreso, volumen, aleatorio, repetir (todo / una), cola editable.
- Favoritas y listas de reproducción propias.
- Reproducción en segundo plano con notificación y controles multimedia (auriculares, pantalla de bloqueo).
- Descargas desde YouTube (búsqueda o link de video/playlist) en M4A con título, artista y portada.
- Links de Spotify y Apple Music (canción, álbum o playlist): se leen los datos públicos (título/artista) y se descarga el audio equivalente desde YouTube. Spotify y Apple Music usan DRM, así que su audio no se descarga directamente.

Las descargas se guardan en `Música/Ritmo`.

## Compilar

Requisitos: Rust, Android SDK (platform 34, build-tools 34), **Android NDK r30** (`30.0.16248370`), Java 17 y `cargo-ndk`.

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
export ANDROID_HOME=~/android-sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/30.0.16248370
./build-apk.sh                 # arm64-v8a + x86_64 (emulador)
./build-apk.sh arm64-v8a       # solo celulares modernos (APK más chico)
```

El APK queda en `target/apk/ritmo.apk`. Instalar con `adb install -r target/apk/ritmo.apk`.

También se puede compilar con Gradle (`cd android && ./gradlew assembleRelease`).

> Slint usa Skia en Android y no hay binarios precompilados de Skia para `armeabi-v7a` (celulares de 32 bits), por eso no se incluye esa arquitectura.
>
> Con NDK anteriores a r30 la app compila pero se cierra al abrir (`cannot locate symbol __hash_memory`).

## Desarrollo en PC

```bash
cargo run --bin ritmo-desktop
cargo test --lib                       # tests offline
cargo test --lib -- --include-ignored  # incluye tests de red
```

## Licencia

MIT
