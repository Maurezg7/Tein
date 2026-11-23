use super::{OnlineTrack, Resolved, Source, path_segments};
use crate::net::{self, Result};
use serde_json::Value;

pub fn is_link(s: &str) -> bool {
    s.contains("open.spotify.com/") || s.starts_with("spotify:")
}

fn parse(input: &str) -> Option<(String, String)> {
    if let Some(rest) = input.strip_prefix("spotify:") {
        let (kind, id) = rest.split_once(':')?;
        return Some((kind.to_string(), id.to_string()));
    }
    let segs: Vec<String> = path_segments(input)
        .into_iter()
        .filter(|s| !s.starts_with("intl-") && s != "embed")
        .collect();
    match segs.as_slice() {
        [kind, id, ..] => Some((kind.clone(), id.clone())),
        _ => None,
    }
}

/// Reads track metadata from Spotify's public embed page (no account needed).
pub fn resolve(input: &str) -> Result<Resolved> {
    let (kind, id) = parse(input).ok_or("Link de Spotify no reconocido")?;
    if !matches!(kind.as_str(), "track" | "album" | "playlist") {
        return Err("Solo se aceptan links de canciones, álbumes o playlists de Spotify".into());
    }
    let html = net::get_text(
        &format!("https://open.spotify.com/embed/{kind}/{id}"),
        net::BROWSER_UA,
    )?;
    let start_tag = "<script id=\"__NEXT_DATA__\" type=\"application/json\">";
    let start = html
        .find(start_tag)
        .ok_or("Spotify cambió su página, no se pudo leer")?
        + start_tag.len();
    let end = html[start..]
        .find("</script>")
        .ok_or("Respuesta de Spotify incompleta")?
        + start;
    let data: Value = serde_json::from_str(&html[start..end]).map_err(|e| e.to_string())?;
    let entity = &data["props"]["pageProps"]["state"]["data"]["entity"];
    let name = entity["name"]
        .as_str()
        .or(entity["title"].as_str())
        .unwrap_or("Spotify")
        .to_string();

    let tracks: Vec<OnlineTrack> = if kind == "track" {
        let artists: Vec<&str> = entity["artists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["name"].as_str())
            .collect();
        vec![track(&name, &artists.join(", "), &entity["duration"])]
    } else {
        entity["trackList"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| {
                let artist = t["subtitle"].as_str().unwrap_or("").replace('\u{a0}', " ");
                track(t["title"].as_str().unwrap_or(""), &artist, &t["duration"])
            })
            .filter(|t| !t.title.is_empty())
            .collect()
    };
    if tracks.is_empty() {
        return Err("No se encontraron canciones en ese link".into());
    }
    Ok(Resolved {
        label: format!("Spotify: {name}"),
        tracks,
    })
}

fn track(title: &str, artist: &str, duration_ms: &Value) -> OnlineTrack {
    OnlineTrack {
        title: title.to_string(),
        artist: artist.to_string(),
        duration_secs: (duration_ms.as_u64().unwrap_or(0) / 1000) as u32,
        source: Source::Match,
        origin: "Spotify",
    }
}
