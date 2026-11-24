//! Fills in missing tags and cover art from public music databases.
//!
//! Lookup order: iTunes Search, Deezer and MusicBrainz by title/artist (no key
//! needed), then AcoustID audio fingerprint when the name gives no match and
//! the app was built with `ACOUSTID_KEY`.

use crate::library::Track;
use crate::net::{self, Result};
use serde_json::Value;
use std::path::Path;

const MB_UA: &str = "Tein/1.0 ( https://github.com/Maurezg7/Tein )";
const MIN_SCORE: f32 = 0.5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<u16>,
    pub cover_url: Option<String>,
    pub source: &'static str,
    /// Subtracted from the match score (e.g. compilation-only releases).
    pub penalty: f32,
}

/// Finds metadata for a track and writes it, with the cover, into the file.
pub fn enrich(track: &Track) -> Result<Meta> {
    let meta = lookup(track)?;
    let cover = meta
        .cover_url
        .as_deref()
        .and_then(|u| net::get_bytes(u, MB_UA).ok())
        .filter(|d| d.len() > 1024);
    crate::library::write_tags_full(Path::new(&track.path), &meta, cover)?;
    Ok(meta)
}

pub fn lookup(track: &Track) -> Result<Meta> {
    let title = clean(&track.title);
    let artist = known(&track.artist).map(clean).unwrap_or_default();
    let mut best: Option<(f32, Meta)> = None;
    if !title.is_empty() {
        let query = format!("{artist} {title}");
        for provider in [itunes, itunes_title, deezer, musicbrainz] {
            let Ok(found) = provider(&title, &artist) else {
                continue;
            };
            for m in found {
                let s = score(&query, &m) - m.penalty;
                if best.as_ref().is_none_or(|(b, _)| s > *b) {
                    best = Some((s, m));
                }
            }
            if best.as_ref().is_some_and(|(s, _)| *s >= 0.8) {
                break;
            }
        }
    }
    if let Some((s, m)) = best.take() {
        if s >= MIN_SCORE {
            return Ok(with_cover(m));
        }
    }
    match acoustid_key() {
        Some(key) => by_sound(Path::new(&track.path), track.duration_secs, key).map(with_cover),
        None => Err("No encontré datos para esta canción".into()),
    }
}

fn acoustid_key() -> Option<&'static str> {
    option_env!("ACOUSTID_KEY").filter(|k| !k.is_empty())
}

/// Adds a cover from iTunes or the Cover Art Archive when the match has none.
fn with_cover(mut m: Meta) -> Meta {
    if m.cover_url.is_none() {
        m.cover_url = itunes(&m.title, &m.artist)
            .ok()
            .and_then(|v| {
                v.into_iter()
                    .find(|c| norm(&c.artist) == norm(&m.artist) && c.cover_url.is_some())
            })
            .and_then(|c| c.cover_url);
    }
    m
}

fn known(artist: &str) -> Option<&str> {
    (!artist.is_empty() && artist != "Artista desconocido").then_some(artist)
}

/// Strips video noise like "(Official Video)", "[HD]" or "Lyrics" from names.
pub fn clean(s: &str) -> String {
    const NOISE: &[&str] = &[
        "official",
        "video",
        "audio",
        "lyric",
        "lyrics",
        "letra",
        "visualizer",
        "hd",
        "4k",
        "mv",
        "videoclip",
        "oficial",
    ];
    let mut out = String::new();
    let mut depth = 0;
    let mut group = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                group.clear();
            }
            ')' | ']' if depth > 0 => {
                depth -= 1;
                let g = norm(&group);
                if !g.split(' ').any(|w| NOISE.contains(&w)) {
                    out.push_str(&format!(" ({group})"));
                }
            }
            _ if depth > 0 => group.push(c),
            _ => out.push(c),
        }
    }
    let out = out.replace('_', " ");
    let words: Vec<&str> = out
        .split_whitespace()
        .filter(|w| !NOISE.contains(&norm(w).as_str()) || w.chars().any(char::is_uppercase))
        .collect();
    words
        .join(" ")
        .trim_matches(|c: char| c == '-' || c.is_whitespace())
        .to_string()
}

/// Lowercase, accent-free, alphanumeric words.
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.to_lowercase().chars() {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            '&' => ' ',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn words(s: &str) -> std::collections::HashSet<String> {
    norm(s)
        .split(' ')
        .filter(|w| {
            !w.is_empty() && !matches!(*w, "the" | "feat" | "ft" | "y" | "and" | "el" | "la")
        })
        .map(str::to_string)
        .collect()
}

/// Dice similarity between the query words and the candidate's title+artist;
/// zero when no title word matches.
fn score(query: &str, m: &Meta) -> f32 {
    let q = words(query);
    let title = words(&m.title);
    if q.is_empty() || title.is_disjoint(&q) {
        return 0.0;
    }
    let c: std::collections::HashSet<String> = title.union(&words(&m.artist)).cloned().collect();
    let common = q.intersection(&c).count() as f32;
    2.0 * common / (q.len() + c.len()) as f32
}

fn year_of(date: &str) -> Option<u16> {
    date.get(..4)?.parse().ok().filter(|y| *y > 1800)
}

fn itunes(title: &str, artist: &str) -> Result<Vec<Meta>> {
    itunes_term(format!("{artist} {title}").trim(), 10)
}

/// iTunes ranks popular songs of the artist above the exact title, so also
/// search the title alone and let the scoring pick the artist.
fn itunes_title(title: &str, artist: &str) -> Result<Vec<Meta>> {
    if artist.is_empty() {
        return Ok(Vec::new());
    }
    itunes_term(title, 50)
}

fn itunes_term(term: &str, limit: u32) -> Result<Vec<Meta>> {
    let url = format!(
        "https://itunes.apple.com/search?entity=song&limit={limit}&term={}",
        net::url_encode(term)
    );
    Ok(parse_itunes(&net::get_json(&url, MB_UA)?))
}

fn parse_itunes(v: &Value) -> Vec<Meta> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(Meta {
                title: r["trackName"].as_str()?.to_string(),
                artist: r["artistName"].as_str()?.to_string(),
                album: r["collectionName"].as_str().unwrap_or_default().to_string(),
                year: r["releaseDate"].as_str().and_then(year_of),
                cover_url: r["artworkUrl100"]
                    .as_str()
                    .map(|u| u.replace("100x100bb", "600x600bb")),
                source: "iTunes",
                penalty: 0.0,
            })
        })
        .collect()
}

fn deezer(title: &str, artist: &str) -> Result<Vec<Meta>> {
    let q = if artist.is_empty() {
        title.to_string()
    } else {
        format!("artist:\"{artist}\" track:\"{title}\"")
    };
    let url = format!(
        "https://api.deezer.com/search?limit=10&q={}",
        net::url_encode(&q)
    );
    let v = net::get_json(&url, MB_UA)?;
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(Meta {
                title: r["title"].as_str()?.to_string(),
                artist: r["artist"]["name"].as_str()?.to_string(),
                album: r["album"]["title"].as_str().unwrap_or_default().to_string(),
                year: None,
                cover_url: r["album"]["cover_xl"]
                    .as_str()
                    .or(r["album"]["cover_big"].as_str())
                    .map(str::to_string),
                source: "Deezer",
                penalty: 0.0,
            })
        })
        .collect())
}

fn musicbrainz(title: &str, artist: &str) -> Result<Vec<Meta>> {
    let esc = |s: &str| s.replace('"', "");
    let mut q = format!("recording:\"{}\"", esc(title));
    if !artist.is_empty() {
        q += &format!(" AND artist:\"{}\"", esc(artist));
    }
    let url = format!(
        "https://musicbrainz.org/ws/2/recording?fmt=json&limit=10&query={}",
        net::url_encode(&q)
    );
    let v = musicbrainz_get(&url)?;
    Ok(v["recordings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(mb_recording)
        .collect())
}

/// MusicBrainz allows one request per second per client.
fn musicbrainz_get(url: &str) -> Result<Value> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    for attempt in 0..3 {
        {
            let mut last = LAST.lock().map_err(|e| e.to_string())?;
            if let Some(t) = *last {
                let wait = Duration::from_millis(1100).saturating_sub(t.elapsed());
                std::thread::sleep(wait);
            }
            *last = Some(Instant::now());
        }
        match net::get_json(url, MB_UA) {
            Err(e) if attempt < 2 && e.contains("503") => continue,
            r => return r,
        }
    }
    Err("MusicBrainz ocupado".into())
}

fn mb_recording(r: &Value) -> Option<Meta> {
    let artist = r["artist-credit"]
        .as_array()?
        .iter()
        .map(|a| {
            format!(
                "{}{}",
                a["name"].as_str().unwrap_or_default(),
                a["joinphrase"].as_str().unwrap_or_default()
            )
        })
        .collect::<String>();
    // Prefer the original official album/single over compilations.
    let release = r["releases"].as_array().and_then(|rs| {
        rs.iter().min_by_key(|x| {
            let group = &x["release-group"];
            (
                x["status"].as_str() != Some("Official"),
                group["secondary-types"]
                    .as_array()
                    .is_some_and(|t| !t.is_empty()),
                x["date"].as_str().unwrap_or("9999").to_string(),
            )
        })
    });
    let original = release.is_some_and(|x| {
        x["status"].as_str() == Some("Official")
            && x["release-group"]["secondary-types"]
                .as_array()
                .is_none_or(|t| t.is_empty())
    });
    Some(Meta {
        title: r["title"].as_str()?.to_string(),
        artist,
        album: release
            .and_then(|x| x["title"].as_str())
            .unwrap_or_default()
            .to_string(),
        year: r["first-release-date"]
            .as_str()
            .or(release.and_then(|x| x["date"].as_str()))
            .and_then(year_of),
        cover_url: release
            .and_then(|x| x["id"].as_str())
            .map(|id| format!("https://coverartarchive.org/release/{id}/front-500")),
        source: "MusicBrainz",
        penalty: if original { 0.0 } else { 0.1 },
    })
}

/// Identifies the song from its sound with a Chromaprint fingerprint.
fn by_sound(path: &Path, duration: u32, key: &str) -> Result<Meta> {
    let (fp, secs) = fingerprint(path)?;
    let url = format!(
        "https://api.acoustid.org/v2/lookup?client={key}&meta=recordings+releases&duration={}&fingerprint={fp}",
        if duration > 0 { duration } else { secs }
    );
    let v = net::get_json(&url, MB_UA).map_err(|e| {
        if e.contains("400") {
            "AcoustID rechazó la consulta (revisá la clave)".to_string()
        } else {
            e
        }
    })?;
    if v["status"].as_str() != Some("ok") {
        return Err(format!(
            "AcoustID: {}",
            v["error"]["message"].as_str().unwrap_or("error")
        ));
    }
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["score"].as_f64().unwrap_or(0.0) > 0.5)
        .flat_map(|r| r["recordings"].as_array().cloned().unwrap_or_default())
        .find_map(|r| mb_recording(&r))
        .map(|m| Meta {
            source: "AcoustID",
            ..m
        })
        .ok_or_else(|| "No reconocí la canción por el sonido".into())
}

/// Compressed, URL-safe base64 Chromaprint fingerprint of the first 2 minutes,
/// plus the decoded length in seconds.
pub fn fingerprint(path: &Path) -> Result<(String, u32)> {
    use rodio::Source;
    use rusty_chromaprint::{Configuration, FingerprintCompressor, Fingerprinter};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let decoder = rodio::Decoder::try_from(file).map_err(|e| e.to_string())?;
    let rate = decoder.sample_rate().get();
    let channels = decoder.channels().get() as u32;
    let config = Configuration::preset_test2();
    let mut printer = Fingerprinter::new(&config);
    printer
        .start(rate, channels)
        .map_err(|e| format!("{e:?}"))?;
    let max = (rate * channels * 120) as usize;
    let mut buf = Vec::with_capacity(8192);
    let mut total = 0usize;
    for s in decoder.take(max) {
        buf.push((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        if buf.len() == 8192 {
            printer.consume(&buf);
            total += buf.len();
            buf.clear();
        }
    }
    total += buf.len();
    printer.consume(&buf);
    printer.finish();
    if total < (rate * channels * 10) as usize {
        return Err("El audio es demasiado corto para reconocerlo".into());
    }
    let raw = FingerprintCompressor::from(&config).compress(printer.fingerprint());
    Ok((
        base64_url(&raw),
        (total / (rate * channels) as usize) as u32,
    ))
}

fn base64_url(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 3);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str) -> Track {
        Track {
            path: String::new(),
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            duration_secs: 0,
            mtime: 0,
        }
    }

    #[test]
    fn cleans_video_noise() {
        assert_eq!(
            clean("Queen - Bohemian Rhapsody (Official Video Remastered)"),
            "Queen - Bohemian Rhapsody"
        );
        assert_eq!(
            clean("Bad Bunny - Tití Me Preguntó [Lyrics]"),
            "Bad Bunny - Tití Me Preguntó"
        );
        assert_eq!(clean("Song (Live at Wembley)"), "Song (Live at Wembley)");
    }

    #[test]
    fn base64() {
        assert_eq!(base64_url(b"Ma"), "TWE");
        assert_eq!(base64_url(b"Man"), "TWFu");
        assert_eq!(base64_url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn scoring() {
        let m = Meta {
            title: "Bohemian Rhapsody".into(),
            artist: "Queen".into(),
            ..Default::default()
        };
        assert!(score("Queen Bohemian Rhapsody", &m) > 0.9);
        assert_eq!(score("Another One Bites", &m), 0.0);
    }

    #[test]
    #[ignore = "network"]
    fn finds_by_name() {
        for (t, a) in [
            ("Bohemian Rhapsody", "Queen"),
            (
                "Soda Stereo - De Música Ligera (Official Video)",
                "Artista desconocido",
            ),
            ("tusa", "karol g"),
        ] {
            let m = lookup(&track(t, a)).unwrap();
            println!("{m:?}");
            assert!(m.cover_url.is_some());
        }
    }

    /// Needs a downloaded file in `TEIN_TEST_AUDIO`.
    #[test]
    #[ignore = "network"]
    fn writes_tags_and_cover() {
        let src = std::env::var("TEIN_TEST_AUDIO").unwrap();
        let dest = std::env::temp_dir().join("tein_meta_test.m4a");
        std::fs::copy(&src, &dest).unwrap();
        let t = track(
            "Soda Stereo - De Música Ligera (Official Video)",
            "Artista desconocido",
        );
        let m = enrich(&Track {
            path: dest.to_string_lossy().into(),
            ..t
        })
        .unwrap();
        println!("{m:?}");
        let back = crate::library::read_track(&dest).unwrap();
        assert_eq!(back.artist, m.artist);
        assert!(crate::library::cover(&back.path).unwrap().len() > 10_000);
    }

    #[test]
    #[ignore = "network"]
    fn fingerprints() {
        let (fp, secs) =
            fingerprint(Path::new(&std::env::var("TEIN_TEST_AUDIO").unwrap())).unwrap();
        println!("{secs}s {} {}", fp.len(), &fp[..40]);
        std::fs::write(std::env::temp_dir().join("tein_fp.txt"), &fp).unwrap();
        if let Ok(key) = std::env::var("ACOUSTID_TEST_KEY") {
            println!(
                "{:?}",
                by_sound(
                    Path::new(&std::env::var("TEIN_TEST_AUDIO").unwrap()),
                    0,
                    &key
                )
            );
        }
    }
}
