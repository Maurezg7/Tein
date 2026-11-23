use crate::library::{self, Track};
use crate::online::{self, OnlineTrack, youtube};
use std::sync::mpsc::{Sender, channel};

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

pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> Sender<Job> {
    let (tx, rx) = channel::<Job>();
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

fn run(job: &Job, on_update: &dyn Fn(Update)) -> Result<Track, String> {
    let id = job.id;
    let status = |s: &str, p: f32| {
        on_update(Update::Progress {
            id,
            status: s.to_string(),
            progress: p,
        })
    };
    let (artist, title) = clean_names(&job.track);
    let mut dir = crate::paths::get().download_dir.clone();
    if let Some(folder) = &job.folder {
        dir = dir.join(sanitize(folder));
    }
    let dir = &dir;
    std::fs::create_dir_all(dir).map_err(|e| format!("No se pudo crear {}: {e}", dir.display()))?;
    let dest = dir.join(format!("{}.m4a", sanitize(&format!("{artist} - {title}"))));
    if dest.exists() {
        return library::read_track(&dest).ok_or_else(|| "Archivo existente ilegible".into());
    }

    status("Buscando audio…", 0.0);
    let video_id = online::video_id_for(&job.track)?;
    let stream = youtube::audio_stream(&video_id)?;
    let part = dest.with_extension("m4a.part");
    let mb = stream.size as f32 / 1_048_576.0;
    let mut last = -1i32;
    let result = youtube::download(&stream, &part, &mut |p| {
        let pct = (p * 100.0) as i32;
        if pct != last {
            last = pct;
            status(&format!("Descargando {pct}% · {mb:.1} MB"), p);
        }
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
    if let Err(e) = library::write_tags(
        &part,
        &title,
        &artist,
        &album,
        youtube::thumbnail(&video_id),
    ) {
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
    cleaned.trim().chars().take(120).collect()
}
