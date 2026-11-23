use super::{OnlineTrack, Resolved, Source, path_segments};
use crate::net::{self, Result};
use serde_json::Value;

pub fn is_link(s: &str) -> bool {
    s.contains("music.apple.com/") || s.contains("itunes.apple.com/")
}

/// Uses the public iTunes lookup API (and the playlist page JSON-LD) to read metadata.
pub fn resolve(url: &str) -> Result<Resolved> {
    let segs = path_segments(url);
    let country = segs
        .first()
        .filter(|s| s.len() == 2)
        .cloned()
        .unwrap_or_else(|| "us".into());
    let kind_pos = segs
        .iter()
        .position(|s| matches!(s.as_str(), "album" | "song" | "playlist"));
    let kind = kind_pos
        .map(|i| segs[i].as_str())
        .ok_or("Link de Apple Music no reconocido")?;
    let last_id = segs.last().cloned().unwrap_or_default();

    if kind == "playlist" {
        return playlist(url, &country);
    }
    let (lookup, label) = match (kind, net::query_param(url, "i")) {
        (_, Some(track_id)) => (format!("id={track_id}"), "canción"),
        ("song", None) => (format!("id={last_id}"), "canción"),
        _ => (format!("id={last_id}&entity=song"), "álbum"),
    };
    let data = net::get_json(
        &format!("https://itunes.apple.com/lookup?{lookup}&country={country}"),
        net::BROWSER_UA,
    )?;
    let tracks = tracks_from(&data);
    if tracks.is_empty() {
        return Err("Apple Music no devolvió canciones para ese link".into());
    }
    let name = data["results"][0]["collectionName"]
        .as_str()
        .unwrap_or(label);
    Ok(Resolved {
        label: format!("Apple Music: {name}"),
        tracks,
    })
}

fn playlist(url: &str, country: &str) -> Result<Resolved> {
    let html = net::get_text(url, net::BROWSER_UA)?;
    let marker = "<script type=\"application/ld+json\"";
    let mut name = "Playlist".to_string();
    let mut ids = Vec::new();
    let mut rest = html.as_str();
    while let Some(pos) = rest.find(marker) {
        let after = &rest[pos..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let body = &after[open_end + 1..];
        let Some(close) = body.find("</script>") else {
            break;
        };
        if let Ok(v) = serde_json::from_str::<Value>(&body[..close]) {
            if v["@type"] == "MusicPlaylist" {
                name = v["name"].as_str().unwrap_or("Playlist").to_string();
                for t in v["track"].as_array().into_iter().flatten() {
                    if let Some(id) = t["url"]
                        .as_str()
                        .and_then(|u| path_segments(u).last().cloned())
                    {
                        ids.push(id);
                    }
                }
            }
        }
        rest = &body[close..];
    }
    if ids.is_empty() {
        return Err("No se pudo leer la playlist de Apple Music".into());
    }
    let mut tracks = Vec::new();
    for chunk in ids.chunks(100) {
        let data = net::get_json(
            &format!(
                "https://itunes.apple.com/lookup?id={}&country={country}",
                chunk.join(",")
            ),
            net::BROWSER_UA,
        )?;
        tracks.extend(tracks_from(&data));
    }
    Ok(Resolved {
        label: format!("Apple Music: {name}"),
        tracks,
    })
}

fn tracks_from(data: &Value) -> Vec<OnlineTrack> {
    data["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["wrapperType"] == "track")
        .map(|r| OnlineTrack {
            title: r["trackName"].as_str().unwrap_or("").to_string(),
            artist: r["artistName"].as_str().unwrap_or("").to_string(),
            duration_secs: (r["trackTimeMillis"].as_u64().unwrap_or(0) / 1000) as u32,
            source: Source::Match,
            origin: "Apple Music",
        })
        .collect()
}
