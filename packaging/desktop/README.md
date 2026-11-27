# Compilar Tein para PC

Desde la raíz, con Rust estable (compatible con `Cargo.lock`), compilador C/C++, `pkg-config` y, en Linux, los encabezados de ALSA y fontconfig:

```sh
cargo build --locked --release --bin tein-desktop
packaging/desktop/package.sh deb target/release/tein-desktop
```

Los archivos se escriben en `target/dist/`. Cambiá `deb` por `rpm`, `arch` o `void` para usar `rpmbuild`, `makepkg` (sin root) o `xbps-create`, respectivamente. La opción `deb` usa `dpkg-deb`. Se necesitan también Bash, `file`, `tar` y las herramientas habituales de compilación de paquetes de la distro. Al distribuir el mismo ELF para varias distros, compilalo en una base compatible con la versión mínima de glibc declarada (para 1.4.0: x86_64, glibc 2.35).

Para Windows x64 podés compilar en Windows con la toolchain MSVC y el mismo comando de Cargo, o usar MinGW-w64 desde Linux:

```sh
rustup target add x86_64-pc-windows-gnu
cargo build --locked --release --bin tein-desktop --target x86_64-pc-windows-gnu
packaging/desktop/package.sh windows target/x86_64-pc-windows-gnu/release/tein-desktop.exe
```

La opción `windows` se ejecuta desde Bash con `file` y `zip`. El ZIP contiene el ejecutable, instrucciones y licencias. No está firmado con Authenticode.

Después de instalar un paquete de Linux, ejecutá `packaging/desktop/smoke.sh` desde una sesión gráfica o con `xvfb-run -a`. La prueba crea su propia carpeta Música, inicia Tein y comprueba que el WAV de prueba aparece en la biblioteca; no modifica la configuración de la cuenta.

## Verificación de la primera edición

Se instaló el paquete con APT en Debian 12, DNF en Fedora 44, pacman en Arch y XBPS en Void glibc. En los cuatro contenedores el programa arrancó en X11 virtual y escaneó una canción de prueba. El ejecutable del ZIP abrió una ventana y encontró un WAV en la carpeta Música de Windows bajo Wine 11; no se probó en Windows nativo. Estas pruebas no verifican la salida de sonido en hardware real.
