//! Audius public search and creator-enabled downloads.
use super::{OnlineTrack, Resolved, Source};
use crate::net::{self, Result};
use serde_json::Value;

const API: &str = "https://api.audius.co/v1";
const APP: &str = "app_name=Tein";

pub fn is_link(s: &str) -> bool {
    s.starts_with("https://audius.co/") || s.starts_with("https://www.audius.co/")
}

fn track(v: &Value) -> Option<OnlineTrack> {
    if v["is_downloadable"] != true
        || v["is_download_gated"] != false
        || v["access"]["download"] != true
    {
        return None;
    }
    let ext = if v["is_original_available"] == true {
        let name = std::path::Path::new(v["orig_filename"].as_str()?);
        if !crate::library::is_audio(name) {
            return None;
        }
        name.extension()?.to_str()?.to_ascii_lowercase()
    } else {
        "mp3".into()
    };
    Some(OnlineTrack {
        title: v["title"].as_str()?.to_string(),
        artist: v["user"]["name"].as_str().unwrap_or_default().to_string(),
        duration_secs: v["duration"].as_u64().unwrap_or(0) as u32,
        source: Source::Audius {
            id: v["id"].as_str()?.to_string(),
            ext,
            art: v["artwork"]["480x480"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        },
        origin: "Audius",
    })
}

fn tracks(v: &Value) -> Vec<OnlineTrack> {
    v.as_array()
        .map(|a| a.iter().filter_map(track).collect())
        .unwrap_or_default()
}

pub fn search(query: &str) -> Result<Vec<OnlineTrack>> {
    let url = format!(
        "{API}/tracks/search?query={}&limit=30&{APP}",
        net::url_encode(query)
    );
    Ok(tracks(&net::get_json(&url, net::BROWSER_UA)?["data"]))
}

pub fn resolve(url: &str) -> Result<Resolved> {
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let v = net::get_json(
        &format!("{API}/resolve?url={}&{APP}", net::url_encode(url)),
        net::BROWSER_UA,
    )?;
    let data = match &v["data"] {
        Value::Array(a) => a.first().cloned().unwrap_or_default(),
        other => other.clone(),
    };
    if let Some(name) = data["playlist_name"].as_str() {
        let id = data["id"].as_str().unwrap_or_default();
        let list = net::get_json(
            &format!("{API}/playlists/{}/tracks?{APP}", net::url_encode(id)),
            net::BROWSER_UA,
        )?;
        return Ok(Resolved {
            label: name.to_string(),
            tracks: tracks(&list["data"]),
            collection: Some(name.to_string()),
        });
    }
    let t = track(&data).ok_or("La pista no ofrece una descarga pública compatible en Audius")?;
    Ok(Resolved {
        label: format!("{} - {}", t.artist, t.title),
        tracks: vec![t],
        collection: None,
    })
}

pub fn download_url(id: &str) -> String {
    format!("{API}/tracks/{}/download?{APP}", net::url_encode(id))
}

#[cfg(test)]
mod tests {
    #[test]
    fn requires_explicit_public_download() {
        let song = serde_json::json!({"id":"sample","title":"Example","is_downloadable":true,"is_download_gated":false,"access":{"download":true}});
        assert!(super::track(&song).is_some());
        for key in ["is_downloadable", "is_download_gated", "access"] {
            let mut incomplete = song.clone();
            incomplete.as_object_mut().unwrap().remove(key);
            assert!(super::track(&incomplete).is_none());
        }
    }
}
