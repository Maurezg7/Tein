Name: tein
Version: 1.4.0
Release: 1
Summary: Reproductor de música en Rust con biblioteca, descargas y skins
License: MIT AND OFL-1.1
URL: https://github.com/Maurezg7/Tein
Source0: payload.tar.gz
ExclusiveArch: x86_64
Requires: alsa-lib, fontconfig, libX11, libxkbcommon-x11, libXcursor, libXi, libglvnd-glx, libwayland-client, zenity, xdg-utils

%description
Tein reproduce música local, organiza listas y permite buscar y descargar
canciones. Incluye ecualizador y diseños seleccionables.

%install
mkdir -p "%{buildroot}"
tar xzf "%{SOURCE0}" -C "%{buildroot}"

%files
/usr/bin/tein
/usr/share/applications/tein.desktop
/usr/share/icons/hicolor/512x512/apps/tein.png
%license /usr/share/licenses/tein
