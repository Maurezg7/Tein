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
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return PATHS.get_or_init(desktop_paths);
    #[cfg(any(target_os = "android", target_os = "ios"))]
    PATHS.get().expect("mobile paths not initialized")
}

#[cfg(target_os = "ios")]
pub fn init_ios() -> std::io::Result<()> {
    // An update can change the container UUID. Save track and playlist paths relative to it.
    std::env::set_current_dir(dirs::home_dir().ok_or(std::io::ErrorKind::NotFound)?)?;
    let docs = PathBuf::from("Documents");
    PATHS.get_or_init(|| {
        finish(
            "Library/Application Support/Tein".into(),
            "Library/Caches/Tein".into(),
            docs.join("Música"),
            vec![docs],
        )
    });
    Ok(())
}

#[cfg(any(target_os = "ios", test))]
pub fn documents_subdir(docs: &std::path::Path, chosen: &std::path::Path) -> Option<PathBuf> {
    let root = docs.canonicalize().ok()?;
    let path = chosen.canonicalize().ok()?;
    path.is_dir().then_some(())?;
    Some(docs.join(path.strip_prefix(root).ok()?))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn desktop_paths() -> Paths {
    let home = dirs::home_dir().expect("No se encontró la carpeta de usuario");
    let data = dirs::data_local_dir()
        .unwrap_or_else(|| home.join(".local/share"))
        .join("tein");
    let cache = dirs::cache_dir()
        .unwrap_or_else(|| home.join(".cache"))
        .join("tein/cache");
    let music = dirs::audio_dir().unwrap_or_else(|| home.join("Music"));
    finish(data, cache, music.join("Tein"), vec![music])
}

#[cfg(test)]
mod tests {
    #[test]
    fn folder_picker_accepts_only_real_descendants() {
        let root = std::env::temp_dir().join(format!("tein-folders-{:016x}", fastrand::u64(..)));
        let docs = root.join("Documents");
        let music = docs.join("Mi música");
        let other = root.join("Documents-fuera");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir(&other).unwrap();
        assert_eq!(super::documents_subdir(&docs, &music), Some(music));
        assert_eq!(super::documents_subdir(&docs, &docs), Some(docs.clone()));
        assert_eq!(super::documents_subdir(&docs, &other), None);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&other, docs.join("enlace")).unwrap();
            assert_eq!(super::documents_subdir(&docs, &docs.join("enlace")), None);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
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
    let probe = dir.join(format!(".tein_probe_{:016x}.m4a", fastrand::u64(..)));
    if std::fs::File::create_new(&probe).is_err() {
        return false;
    }
    std::fs::remove_file(probe).is_ok()
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
