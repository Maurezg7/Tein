"""Install the built app and verify a fresh local library on iPhone and iPad."""
import json
import pathlib
import subprocess
import sys
import time
import wave


def ctl(*args):
    return subprocess.check_output(["xcrun", "simctl", *args], text=True).strip()


app = pathlib.Path(sys.argv[1]).resolve()
bundle = "io.github.maurezg7.tein"
devices = json.loads(ctl("list", "devices", "available", "-j"))["devices"]
for family in ["iPhone", "iPad"]:
    device = next(d["udid"] for rt, ds in reversed(list(devices.items()))
                  if ".iOS-" in rt for d in ds if d["name"].startswith(family))
    ctl("boot", device)
    try:
        ctl("bootstatus", device, "-b")
        ctl("install", device, str(app))
        home = pathlib.Path(ctl("get_app_container", device, bundle, "data"))
        root = home / "Documents" / "Música"
        root.mkdir(exist_ok=True, parents=True)
        with wave.open(str(root / "Demo local.wav"), "wb") as w:
            w.setparams((1, 2, 8000, 0, "NONE", ""))
            w.writeframes(bytes(16000))
        ctl("launch", device, bundle)
        library = home / "Library/Application Support/Tein/library.json"
        for _ in range(90):
            if library.exists():
                tracks = json.loads(library.read_text())
                if any(t["path"] == "Documents/Música/Demo local.wav" for t in tracks):
                    break
            time.sleep(1)
        else:
            raise RuntimeError(f"{family}: no se encontró la canción en la biblioteca")
        if bundle not in ctl("spawn", device, "launchctl", "list"):
            raise RuntimeError(f"{family}: el proceso no sigue activo")
        ctl("io", device, "screenshot", f"Tein-{family}.png")
        print(f"{family}: arranque y escaneo local correctos", flush=True)
    finally:
        ctl("shutdown", device)
