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
    let chosen = crate::paths::download_dir();
    for dir in paths
        .music_dirs
        .iter()
        .chain([&paths.download_dir, &chosen])
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
        .map(|d| d.as_nanos() as u64)
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
    let mut pictures = tagged.tags().iter().flat_map(|t| t.pictures());
    pictures
        .clone()
        .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
        .or_else(|| pictures.next())
        .map(|p| p.data().to_vec())
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
    use lofty::picture::{MimeType, Picture, PictureType};
    update_tag(path, |tag| {
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
    })
}

pub fn edit_tags(path: &Path, title: &str, artist: &str, album: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("Escribí un título".into());
    }
    update_tag(path, |tag| {
        tag.set_title(title.trim().into());
        if artist.trim().is_empty() {
            tag.remove_artist()
        } else {
            tag.set_artist(artist.trim().into())
        }
        if album.trim().is_empty() {
            tag.remove_album()
        } else {
            tag.set_album(album.trim().into())
        }
    })
}

/// Renames the album and/or artist of a file, keeping every other tag.
pub fn retag(path: &Path, album: Option<&str>, artist: Option<&str>) -> Result<(), String> {
    update_tag(path, |tag| {
        if let Some(album) = album {
            tag.set_album(album.into());
        }
        if let Some(artist) = artist {
            tag.set_artist(artist.into());
        }
    })
}

/// Replaces the embedded cover (JPEG bytes), or removes it with `None`.
pub fn set_cover(path: &Path, cover: Option<&[u8]>) -> Result<(), String> {
    use lofty::picture::{MimeType, Picture, PictureType};
    update_tag(path, |tag| {
        while tag.picture_count() > 0 {
            tag.remove_picture(0);
        }
        if let Some(data) = cover {
            tag.push_picture(
                Picture::unchecked(data.to_vec())
                    .pic_type(PictureType::CoverFront)
                    .mime_type(MimeType::Jpeg)
                    .build(),
            );
        }
    })
}

fn update_tag(path: &Path, edit: impl FnOnce(&mut lofty::tag::Tag)) -> Result<(), String> {
    use lofty::tag::Tag;
    let mut tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tagged.primary_tag_type()));
    }
    let tag = tagged.primary_tag_mut().ok_or("sin etiquetas")?;
    edit(tag);
    tag.save_to_path(path, lofty::config::WriteOptions::default())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_round_trips_and_keeps_art_after_a_thumbnail_miss() {
        let path = std::env::temp_dir().join(format!("tein-{}.wav", fastrand::u64(..)));
        let mut wav = b"RIFF\x26\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(b"\x02\0\x10\0data\x02\0\0\0\0\0");
        std::fs::write(&path, wav).unwrap();
        let before = read_track(&path).unwrap();
        let key = crate::art::key(&before);
        assert!(crate::art::load(&key, &before.path).is_none());
        let mut data = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(8, 8)
            .write_to(&mut data, image::ImageFormat::Png)
            .unwrap();
        let png = data.into_inner();
        write_tags(&path, "Antes", "Prueba", "Disco", Some(png.clone())).unwrap();
        edit_tags(&path, " Después ", "Otro", "").unwrap();
        let after = read_track(&path).unwrap();
        assert_eq!(
            (&*after.title, &*after.artist, &*after.album),
            ("Después", "Otro", "Sin álbum")
        );
        assert_eq!(cover(&after.path), Some(png));
        assert!(crate::art::load(&key, &after.path).is_some());
        assert!(edit_tags(&path, "  ", "", "").is_err());
        crate::art::forget(&key);
        std::fs::remove_file(path).unwrap();
    }
}
