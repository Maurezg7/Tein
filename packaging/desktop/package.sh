#!/usr/bin/env bash
# Build one desktop package from the matching release executable.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
format=${1:?Uso: package.sh deb|rpm|arch|void|windows RUTA_AL_BINARIO [DIRECTORIO_DE_SALIDA]}
bin=$(realpath "${2:?Falta el ejecutable compilado}")
version=1.4.0
mkdir -p "${3:-$root/target/dist}"
out=$(realpath "${3:-$root/target/dist}")
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
cd "$work"

if [[ $format == windows ]]; then
    file "$bin" | grep -q 'PE32+ executable.*x86-64'
    install -m 755 "$bin" Tein.exe
    cp "$root/LICENSE" "$root/ui/fonts/VT323-OFL.txt" "$here/LEEME.txt" .
    zip -q -9 "$work/Tein-$version-windows-x64.zip" Tein.exe LICENSE VT323-OFL.txt LEEME.txt
    mv "$work/Tein-$version-windows-x64.zip" "$out/"
    exit
fi

file "$bin" | grep -q 'ELF 64-bit.*x86-64'
install -Dm755 "$bin" stage/usr/bin/tein
install -Dm644 "$here/tein.desktop" stage/usr/share/applications/tein.desktop
install -Dm644 "$root/docs/logo.png" stage/usr/share/icons/hicolor/512x512/apps/tein.png
install -Dm644 "$root/LICENSE" stage/usr/share/licenses/tein/LICENSE
install -Dm644 "$root/ui/fonts/VT323-OFL.txt" stage/usr/share/licenses/tein/VT323-OFL.txt

case "$format" in
deb)
    mkdir stage/DEBIAN
    cat > stage/DEBIAN/control <<EOF
Package: tein
Version: $version-1
Section: sound
Priority: optional
Architecture: amd64
Maintainer: deiko777 <deiko777@proton.me>
Depends: libc6 (>= 2.35), libgcc-s1, libasound2, libfontconfig1, libx11-6, libxkbcommon-x11-0, libxcursor1, libxi6, libgl1, libwayland-client0, zenity, xdg-utils
Homepage: https://github.com/Maurezg7/Tein
Description: Reproductor de música con biblioteca, descargas, ecualizador y skins
EOF
    dpkg-deb --root-owner-group -Zxz --build stage "$out/tein_${version}-1_amd64.deb"
    ;;
rpm)
    mkdir -p SOURCES
    tar czf SOURCES/payload.tar.gz -C stage usr
    rpmbuild -bb --define "_topdir $work" --define '_build_id_links none' "$here/tein.spec"
    cp RPMS/x86_64/*.rpm "$out/"
    ;;
arch)
    tar czf payload.tar.gz -C stage usr
    cp "$here/PKGBUILD" .
    makepkg -g >> PKGBUILD
    PKGEXT=.pkg.tar.zst makepkg
    cp ./*.pkg.tar.zst "$out/"
    ;;
void)
    xbps-create -A x86_64 -n "tein-${version}_1" -s 'Reproductor de música con biblioteca, descargas y skins' \
        -l 'MIT, OFL-1.1' -H 'https://github.com/Maurezg7/Tein' -m 'deiko777 <deiko777@proton.me>' \
        -D 'glibc>=2.35_1 libgcc>=0 alsa-lib>=0 fontconfig>=0 libX11>=0 libxkbcommon-x11>=0 libXcursor>=0 libXi>=0 libglvnd>=0 mesa-dri>=0 wayland>=0 zenity>=0 xdg-utils>=0' stage
    cp ./*.xbps "$out/"
    ;;
*) echo "Formato desconocido: $format" >&2; exit 2 ;;
esac
