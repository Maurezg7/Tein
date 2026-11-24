use lofty::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const EXTENSIONS: &[&str] = &["mp3", "m4a", "mp4", "aac", "flac", "ogg", "oga", "wav"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: u32,
    #[serde(default)]
    pub mtime: u64,
}

fn cache_file() -> PathBuf {
    crate::paths::get().data_dir.join("library.json")
}

pub fn load_cache() -> Vec<Track> {
    std::fs::read_to_string(cache_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_cache(tracks: &[Track]) {
    if let Ok(s) = serde_json::to_string(tracks) {
        let _ = std::fs::write(cache_file(), s);
    }
}

/// Walks the music folders, reusing cached tags for files that did not change.
pub fn scan(previous: &[Track]) -> Vec<Track> {
    let known: HashMap<&str, &Track> = previous.iter().map(|t| (t.path.as_str(), t)).collect();
    let mut files = Vec::new();
    let paths = crate::paths::get();
    for dir in paths
        .music_dirs
        .iter()
        .chain(std::iter::once(&paths.download_dir))
    {
        walk(dir, 0, &mut files);
    }
    files.sort();
    files.dedup();
    let mut tracks: Vec<Track> = files
        .into_iter()
        .filter_map(|p| {
            let mtime = mtime(&p);
            let key = p.to_string_lossy();
            match known.get(key.as_ref()) {
                Some(t) if t.mtime == mtime => Some((*t).clone()),
                _ => read_track(&p),
            }
        })
        .collect();
    sort(&mut tracks);
    tracks
}

pub fn sort(tracks: &mut [Track]) {
    tracks.sort_by_cached_key(|t| t.title.to_lowercase());
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(&path, depth + 1, out);
        } else if is_audio(&path) {
            out.push(path);
        }
    }
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn read_track(path: &Path) -> Option<Track> {
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let (mut title, mut artist, mut album, mut duration_secs) =
        (String::new(), String::new(), String::new(), 0);
    if let Ok(tagged) = lofty::read_from_path(path) {
        duration_secs = tagged.properties().duration().as_secs() as u32;
        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            title = tag.title().map(|s| s.to_string()).unwrap_or_default();
            artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
            album = tag.album().map(|s| s.to_string()).unwrap_or_default();
        }
    }
    if title.is_empty() {
        match stem.split_once(" - ") {
            Some((a, t)) if artist.is_empty() => {
                artist = a.trim().to_string();
                title = t.trim().to_string();
            }
            _ => title = stem,
        }
    }
    Some(Track {
        path: path.to_string_lossy().to_string(),
        title,
        artist: if artist.is_empty() {
            "Artista desconocido".into()
        } else {
            artist
        },
        album: if album.is_empty() {
            "Sin álbum".into()
        } else {
            album
        },
        duration_secs,
        mtime: mtime(path),
    })
}

/// Embedded cover art of a file, if any.
pub fn cover(path: &str) -> Option<Vec<u8>> {
    let tagged = lofty::read_from_path(path).ok()?;
    tagged
        .tags()
        .iter()
        .find_map(|t| t.pictures().first().map(|p| p.data().to_vec()))
}

/// Writes title, artist, album and cover into a downloaded file.
pub fn write_tags(
    path: &Path,
    title: &str,
    artist: &str,
    album: &str,
    cover: Option<Vec<u8>>,
) -> Result<(), String> {
    let meta = crate::meta::Meta {
        title: title.into(),
        artist: artist.into(),
        album: album.into(),
        ..Default::default()
    };
    write_tags_full(path, &meta, cover)
}

/// Writes title, artist, album, year and (replacing any old one) the cover.
pub fn write_tags_full(
    path: &Path,
    meta: &crate::meta::Meta,
    cover: Option<Vec<u8>>,
) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::tag::Tag;
    let mut tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    if tagged.primary_tag().is_none() {
        let kind = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(kind));
    }
    let tag = tagged.primary_tag_mut().ok_or("sin etiquetas")?;
    tag.set_title(meta.title.clone());
    tag.set_artist(meta.artist.clone());
    if !meta.album.is_empty() {
        tag.set_album(meta.album.clone());
    }
    if let Some(year) = meta.year {
        tag.set_date(lofty::tag::items::Timestamp {
            year,
            ..Default::default()
        });
    }
    if let Some(data) = cover {
        let mime = if data.starts_with(b"\x89PNG") {
            MimeType::Png
        } else {
            MimeType::Jpeg
        };
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(
            Picture::unchecked(data)
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build(),
        );
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(|e| e.to_string())
}
