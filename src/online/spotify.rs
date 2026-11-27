use super::{OnlineTrack, Resolved, Source, path_segments};
use crate::net::{self, Result};
use serde_json::{Value, json};

pub fn is_link(s: &str) -> bool {
    s.starts_with("https://open.spotify.com/") || s.starts_with("spotify:")
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

    let mut tracks: Vec<OnlineTrack> = if kind == "track" {
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
    if kind == "playlist" && tracks.len() >= EMBED_LIMIT {
        let token = data["props"]["pageProps"]["state"]["settings"]["session"]["accessToken"]
            .as_str()
            .unwrap_or("");
        if let Err(e) = more_playlist_tracks(&id, token, &mut tracks) {
            log::warn!("spotify playlist paging: {e}");
        }
    }
    if tracks.is_empty() {
        return Err("No se encontraron canciones en ese link".into());
    }
    Ok(Resolved {
        label: format!("Spotify: {name}"),
        tracks,
        collection: (kind != "track").then_some(name),
    })
}

const EMBED_LIMIT: usize = 100;
const MAX_TRACKS: usize = 5000;
const FETCH_PLAYLIST_HASH: &str =
    "19ff1327c29e99c208c86d7a9d8f1929cfdf3d3202a0ff4253c821f1901aa94d";

/// The embed page lists only the first 100 tracks; the rest come from the web player API.
fn more_playlist_tracks(id: &str, token: &str, tracks: &mut Vec<OnlineTrack>) -> Result<()> {
    if token.is_empty() {
        return Err("sin token".into());
    }
    let auth = format!("Bearer {token}");
    let ext = json!({ "persistedQuery": { "version": 1, "sha256Hash": FETCH_PLAYLIST_HASH } });
    let mut offset = tracks.len();
    let mut fetched = Vec::new();
    while offset < MAX_TRACKS {
        let vars =
            json!({ "uri": format!("spotify:playlist:{id}"), "offset": offset, "limit": 100 });
        let url = format!(
            "https://api-partner.spotify.com/pathfinder/v1/query?operationName=fetchPlaylist&variables={}&extensions={}",
            net::url_encode(&vars.to_string()),
            net::url_encode(&ext.to_string())
        );
        let text = net::get_text_with(
            &url,
            &[("User-Agent", net::BROWSER_UA), ("Authorization", &auth)],
        )?;
        let resp: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let content = &resp["data"]["playlistV2"]["content"];
        let items = content["items"]
            .as_array()
            .ok_or("respuesta sin canciones")?;
        if items.is_empty() {
            break;
        }
        offset += items.len();
        for item in items {
            let t = &item["itemV2"]["data"];
            let artists: Vec<&str> = t["artists"]["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a["profile"]["name"].as_str())
                .collect();
            let name = t["name"].as_str().unwrap_or("");
            if !name.is_empty() {
                fetched.push(track(
                    name,
                    &artists.join(", "),
                    &t["trackDuration"]["totalMilliseconds"],
                ));
            }
        }
        if offset >= content["totalCount"].as_u64().unwrap_or(0) as usize {
            break;
        }
    }
    tracks.extend(fetched);
    Ok(())
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
