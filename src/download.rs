use crate::library::{self, Track};
use crate::online::{self, OnlineTrack, Source, audius, soundcloud, youtube};
use std::sync::mpsc::{SyncSender, sync_channel};

pub const QUEUE_LIMIT: usize = 512;

pub struct Job {
    pub id: u64,
    pub track: OnlineTrack,
    /// Playlist/album name: saved in a subfolder with that name.
    pub folder: Option<String>,
}

pub enum Update {
    Progress {
        id: u64,
        status: String,
        progress: f32,
    },
    Done {
        id: u64,
        track: Track,
    },
    Failed {
        id: u64,
        error: String,
    },
}

pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> SyncSender<Job> {
    let (tx, rx) = sync_channel::<Job>(QUEUE_LIMIT);
    std::thread::Builder::new()
        .name("downloads".into())
        .spawn(move || {
            for job in rx {
                let id = job.id;
                match run(&job, &on_update) {
                    Ok(track) => on_update(Update::Done { id, track }),
                    Err(error) => on_update(Update::Failed { id, error }),
                }
            }
        })
        .expect("download thread");
    tx
}

pub(crate) fn run(job: &Job, on_update: &dyn Fn(Update)) -> Result<Track, String> {
    run_in(job, on_update, crate::paths::download_dir())
}

pub(crate) fn run_in(
    job: &Job,
    on_update: &dyn Fn(Update),
    mut dir: std::path::PathBuf,
) -> Result<Track, String> {
    let id = job.id;
    let status = |s: &str, p: f32| {
        on_update(Update::Progress {
            id,
            status: s.to_string(),
            progress: p,
        })
    };
    status("Buscando audio…", 0.0);
    let original = match &job.track.source {
        Source::SoundCloud { url, .. } => Some(soundcloud::original(url)?),
        _ => None,
    };
    let track = original.as_ref().map(|(t, _)| t).unwrap_or(&job.track);
    let (artist, title) = clean_names(track);
    if let Some(folder) = &job.folder {
        dir = dir.join(sanitize(folder));
    }
    let dir = &dir;
    std::fs::create_dir_all(dir).map_err(|e| format!("No se pudo crear {}: {e}", dir.display()))?;
    let ext = match &track.source {
        Source::SoundCloud { .. } => "mp3",
        Source::Audius { ext, .. } => ext.as_str(),
        _ => "m4a",
    };
    let dest = dir.join(format!(
        "{}.{ext}",
        sanitize(&format!("{artist} - {title}"))
    ));
    if dest.exists() {
        return library::read_track(&dest).ok_or_else(|| "Archivo existente ilegible".into());
    }
    let part = dest.with_file_name(format!(
        ".{}.part.{ext}",
        dest.file_stem().unwrap_or_default().to_string_lossy()
    ));

    let mut last = -1i32;
    let mut report = |p: f32, bytes: u64| {
        let pct = (p * 100.0) as i32;
        if pct != last {
            last = pct;
            let mb = bytes as f32 / 1_048_576.0;
            status(&format!("Descargando {pct}% · {mb:.1} MB"), p);
        }
    };
    let (result, art) = match &track.source {
        Source::Audius { .. } | Source::SoundCloud { .. } => {
            let (url, art) = match &track.source {
                Source::Audius { id, art, .. } => (audius::download_url(id), art),
                Source::SoundCloud { art, .. } => (
                    original
                        .as_ref()
                        .ok_or("Falta el enlace de descarga")?
                        .1
                        .clone(),
                    art,
                ),
                _ => unreachable!(),
            };
            let r =
                crate::net::download_to(&url, crate::net::BROWSER_UA, &part, &mut |n, total| {
                    report(
                        if total > 0 {
                            n as f32 / total as f32
                        } else {
                            0.0
                        },
                        n,
                    )
                });
            (r, art.clone())
        }
        _ => match online::video_id_for(&job.track) {
            Ok(video_id) => {
                let r = youtube::audio_stream(&video_id).and_then(|stream| {
                    let size = stream.size;
                    youtube::download(&stream, &part, &mut |p| report(p, (p * size as f32) as u64))
                });
                (r, format!("yt:{video_id}"))
            }
            Err(e) => (Err(e), String::new()),
        },
    };
    let result = result.and_then(|()| {
        let f = std::fs::File::open(&part).map_err(|e| e.to_string())?;
        rodio::Decoder::try_from(f)
            .map(|_| ())
            .map_err(|_| "La respuesta no contiene audio compatible".to_string())
    });
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }

    status("Guardando etiquetas…", 1.0);
    let album = job
        .folder
        .clone()
        .unwrap_or_else(|| job.track.origin.to_string());
    let cover = match art.strip_prefix("yt:") {
        Some(video_id) => youtube::thumbnail(video_id),
        None if art.is_empty() => None,
        None => crate::net::get_bytes(&art, crate::net::BROWSER_UA).ok(),
    };
    if let Err(e) = library::write_tags(&part, &title, &artist, &album, cover) {
        log::warn!("tagging failed: {e}");
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    library::read_track(&dest).ok_or_else(|| "No se pudo leer el archivo descargado".into())
}

/// Turns "Artist - Song (Official Video)" style YouTube titles into clean tags.
pub fn clean_names(t: &OnlineTrack) -> (String, String) {
    let mut artist = t.artist.trim_end_matches(" - Topic").trim().to_string();
    let mut title = t.title.clone();
    if t.origin == "YouTube" {
        if let Some((a, rest)) = title.split_once(" - ") {
            artist = a.trim().to_string();
            title = rest.trim().to_string();
        }
        const NOISE: &[&str] = &[
            "official",
            "oficial",
            "video",
            "lyric",
            "letra",
            "audio",
            "visualizer",
            "hd",
            "4k",
            "mv",
        ];
        for (open, close) in [('(', ')'), ('[', ']')] {
            while let (Some(s), Some(e)) = (title.rfind(open), title.rfind(close)) {
                if e < s {
                    break;
                }
                let inner = title[s + 1..e].to_lowercase();
                if !NOISE.iter().any(|n| inner.contains(n)) {
                    break;
                }
                title.replace_range(s..=e, "");
                title = title.trim().to_string();
            }
        }
    }
    if artist.is_empty() {
        artist = "Artista desconocido".into();
    }
    (artist, title.trim().to_string())
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut name: String = cleaned.trim().trim_matches('.').chars().take(120).collect();
    while name.len() > 220 {
        name.pop();
    }
    let name = name.trim_end_matches(['.', ' ']);
    if name.is_empty() {
        return "Sin nombre".into();
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "AUX" | "PRN" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|s| {
            matches!(
                s,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    }) {
        format!("_{name}")
    } else {
        name.into()
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;
    #[test]
    fn folder_is_a_single_normal_component() {
        use std::path::{Component, Path};
        for s in ["..", ".", "", "/ Indie / 2026", &"🎵".repeat(120)] {
            let name = sanitize(s);
            let mut parts = Path::new(&name).components();
            assert!(matches!(parts.next(), Some(Component::Normal(_))));
            assert!(parts.next().is_none() && name.len() <= 220);
        }
        for name in ["con", "Lpt5.demo", "COM¹", "NuL.mp3", "aux"] {
            assert!(sanitize(name).starts_with('_'));
        }
        assert_eq!(sanitize("Comfort"), "Comfort");
    }
}
