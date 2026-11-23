use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const FAVORITES: &str = "Favoritas";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            volume: 1.0,
            shuffle: false,
            repeat: 0,
        }
    }
}

fn file(name: &str) -> PathBuf {
    crate::paths::get().data_dir.join(name)
}

fn load<T: for<'de> Deserialize<'de>>(name: &str) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(file(name)).ok()?).ok()
}

fn save<T: Serialize>(name: &str, value: &T) {
    if let Ok(s) = serde_json::to_string_pretty(value) {
        let tmp = file(&format!("{name}.tmp"));
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(tmp, file(name));
        }
    }
}

pub fn load_playlists() -> Vec<Playlist> {
    let mut lists: Vec<Playlist> = load("playlists.json").unwrap_or_default();
    if !lists.iter().any(|p| p.name == FAVORITES) {
        lists.insert(
            0,
            Playlist {
                name: FAVORITES.into(),
                paths: vec![],
            },
        );
    }
    lists
}

pub fn save_playlists(lists: &[Playlist]) {
    save("playlists.json", &lists);
}

pub fn load_settings() -> Settings {
    load("settings.json").unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    save("settings.json", s);
}
