use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

pub struct Paths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub download_dir: PathBuf,
    pub music_dirs: Vec<PathBuf>,
}

static PATHS: OnceLock<Paths> = OnceLock::new();

static DOWNLOAD_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Where downloads go: the folder chosen by the user, or the default one.
pub fn download_dir() -> PathBuf {
    DOWNLOAD_DIR
        .read()
        .ok()
        .and_then(|d| d.clone())
        .unwrap_or_else(|| get().download_dir.clone())
}

pub fn set_download_dir(dir: PathBuf) {
    if let Ok(mut d) = DOWNLOAD_DIR.write() {
        *d = Some(dir);
    }
}

pub fn get() -> &'static Paths {
    PATHS.get_or_init(desktop_paths)
}

fn desktop_paths() -> Paths {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let data_dir = home.join(".local/share/tein");
    let music = home.join("Music");
    finish(
        data_dir.clone(),
        data_dir.join("cache"),
        music.join("Tein"),
        vec![music],
    )
}

#[cfg(target_os = "android")]
pub fn init_android(app: &slint::android::AndroidApp) {
    let internal = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp/tein"));
    let storage = PathBuf::from("/storage/emulated/0");
    let mut download_dir = storage.join("Music/Tein");
    if std::fs::create_dir_all(&download_dir).is_err() || !is_writable(&download_dir) {
        download_dir = app
            .external_data_path()
            .unwrap_or_else(|| internal.clone())
            .join("Music");
    }
    let mut music_dirs = vec![storage.join("Music"), storage.join("Download")];
    if !download_dir.starts_with(storage.join("Music")) {
        music_dirs.push(download_dir.clone());
    }
    let _ = PATHS.set(finish(
        internal.clone(),
        internal.join("cache"),
        download_dir,
        music_dirs,
    ));
}

pub fn is_writable(dir: &std::path::Path) -> bool {
    // Android 11+ only lets apps create audio files in Music/, so the probe must look like one.
    let probe = dir.join(".tein_probe.m4a");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

fn finish(
    data_dir: PathBuf,
    cache_dir: PathBuf,
    download_dir: PathBuf,
    music_dirs: Vec<PathBuf>,
) -> Paths {
    for d in [&data_dir, &cache_dir, &download_dir] {
        let _ = std::fs::create_dir_all(d);
    }
    Paths {
        data_dir,
        cache_dir,
        download_dir,
        music_dirs,
    }
}
