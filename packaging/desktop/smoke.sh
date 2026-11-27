#!/usr/bin/env bash
# Run inside a graphical Linux session (or Xvfb), using only disposable user data.
set -euo pipefail
work=$(mktemp -d)
pid=
cleanup() {
    if [[ -n $pid ]]; then kill "$pid" 2>/dev/null || :; wait "$pid" 2>/dev/null || :; fi
    rm -rf -- "$work"
}
trap cleanup EXIT
export HOME=$work XDG_DATA_HOME=$work/data XDG_CONFIG_HOME=$work/config XDG_CACHE_HOME=$work/cache
mkdir -p "$work/Música de prueba" "$XDG_CONFIG_HOME"
printf 'XDG_MUSIC_DIR="$HOME/Música de prueba"\n' > "$XDG_CONFIG_HOME/user-dirs.dirs"
{
    printf 'RIFF\x64\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x40\x1f\x00\x00\x80\x3e\x00\x00\x02\x00\x10\x00data\x40\x00\x00\x00'
    head -c 64 /dev/zero
} > "$work/Música de prueba/Smoke.wav"
"${1:-tein}" > "$work/app.log" 2>&1 &
pid=$!
for ((i=0; i<100; i++)); do
    kill -0 "$pid" 2>/dev/null || { cat "$work/app.log" >&2; exit 1; }
    if grep -q 'Smoke.wav' "$XDG_DATA_HOME/tein/library.json" 2>/dev/null; then
        echo "OK: Tein abrió y encontró el WAV en la carpeta XDG de música"
        exit 0
    fi
    sleep 0.2
done
cat "$work/app.log" >&2
exit 1
