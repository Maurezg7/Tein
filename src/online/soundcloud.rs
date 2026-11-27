//! Public SoundCloud results. Offline audio comes only from publisher-enabled RSS enclosures.
use super::{OnlineTrack, Resolved, Source};
use crate::net::{self, Result};
use serde_json::Value;

const SITE: &str = "https://soundcloud.com";
const UA: &str = "Tein/1.4 (+https://github.com/Maurezg7/Tein)";

pub fn is_link(s: &str) -> bool {
    [
        "https://soundcloud.com/",
        "https://m.soundcloud.com/",
        "https://on.soundcloud.com/",
    ]
    .iter()
    .any(|prefix| s.starts_with(prefix))
}

fn from_sound(v: &Value) -> Option<OnlineTrack> {
    let url = v["permalink_url"].as_str().filter(|s| is_link(s))?;
    Some(OnlineTrack {
        title: v["title"].as_str()?.into(),
        artist: v["user"]["username"].as_str()?.into(),
        duration_secs: (v["duration"].as_u64().unwrap_or_default() / 1000) as u32,
        origin: "SoundCloud",
        source: Source::SoundCloud {
            url: url.into(),
            art: v["artwork_url"]
                .as_str()
                .filter(|a| a.starts_with("https://i1.sndcdn.com/"))
                .unwrap_or_default()
                .into(),
        },
    })
}

fn page(url: &str) -> Result<Value> {
    if !is_link(url) {
        return Err("Enlace de SoundCloud no válido".into());
    }
    let canonical = url.replacen("https://m.soundcloud.com/", "https://soundcloud.com/", 1);
    let html = net::get_text(&canonical, UA)?;
    let payload = html
        .split_once("window.__sc_hydration = ")
        .ok_or("SoundCloud no entregó los datos del enlace")?
        .1;
    let rows = serde_json::Deserializer::from_str(payload)
        .into_iter::<Vec<Value>>()
        .next()
        .and_then(|v| v.ok())
        .ok_or("Respuesta de SoundCloud no válida")?;
    rows.into_iter()
        .find(|r| matches!(r["hydratable"].as_str(), Some("sound" | "playlist")))
        .map(|mut r| r["data"].take())
        .ok_or_else(|| "El enlace no contiene una canción o lista pública".into())
}

pub fn resolve(url: &str) -> Result<Resolved> {
    let data = page(url)?;
    let title = data["title"].as_str().unwrap_or("SoundCloud").to_string();
    let (tracks, collection) = if let Some(items) = data["tracks"].as_array() {
        (
            items.iter().filter_map(from_sound).collect(),
            Some(title.clone()),
        )
    } else {
        (
            vec![from_sound(&data).ok_or("Canción no disponible")?],
            None,
        )
    };
    let total = data["track_count"].as_u64().unwrap_or_default() as usize;
    let label = if total > tracks.len() {
        format!(
            "{title} ({} de {total} visibles en SoundCloud)",
            tracks.len()
        )
    } else {
        title
    };
    Ok(Resolved {
        label,
        tracks,
        collection,
    })
}

pub fn search(query: &str) -> Result<Vec<OnlineTrack>> {
    let html = net::get_text(
        &format!("{SITE}/search/sounds?q={}", net::url_encode(query)),
        UA,
    )?;
    Ok(html
        .split("<h2><a href=\"/")
        .skip(1)
        .filter_map(|s| {
            let (slug, rest) = s.split_once("\">")?;
            let (artist, song) = slug.split_once('/')?;
            if artist.is_empty()
                || song.is_empty()
                || !slug
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_/".contains(&b))
            {
                return None;
            }
            let title = rest
                .split_once("</a>")?
                .0
                .replace("&amp;", "&")
                .replace("&#x27;", "'")
                .replace("&quot;", "\"")
                .replace("&lt;", "<")
                .replace("&gt;", ">");
            Some(OnlineTrack {
                title,
                artist: artist.into(),
                duration_secs: 0,
                origin: "SoundCloud",
                source: Source::SoundCloud {
                    url: format!("{SITE}/{slug}"),
                    art: String::new(),
                },
            })
        })
        .take(30)
        .collect())
}

fn enclosure(xml: &str, id: u64) -> Option<String> {
    let target = format!("tag:soundcloud,2010:tracks/{id}");
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            nodes_limit: 200_000,
            ..Default::default()
        },
    )
    .ok()?;
    let item = doc.descendants().find(|n| {
        n.has_tag_name("item")
            && n.children()
                .any(|c| c.has_tag_name("guid") && c.text() == Some(&target))
    })?;
    item.children()
        .filter(|c| c.has_tag_name("enclosure") && c.attribute("type") == Some("audio/mpeg"))
        .filter_map(|c| c.attribute("url"))
        .find(|u| u.starts_with("https://feeds.soundcloud.com/stream/"))
        .map(str::to_owned)
}

pub fn original(url: &str) -> Result<(OnlineTrack, String)> {
    let data = page(url)?;
    let t = from_sound(&data).ok_or("Canción de SoundCloud no disponible")?;
    let user = data["user"]["id"]
        .as_u64()
        .ok_or("Artista sin identificador")?;
    let id = data["id"].as_u64().ok_or("Canción sin identificador")?;
    let xml = net::get_text(
        &format!("https://feeds.soundcloud.com/users/soundcloud:users:{user}/sounds.rss"),
        net::BROWSER_UA,
    )?;
    let file = enclosure(&xml, id)
        .ok_or("El artista no publicó esta pista para descarga por RSS en SoundCloud")?;
    Ok((t, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_matching_published_audio() {
        let feed = "<rss><item><guid>tag:soundcloud,2010:tracks/73</guid><enclosure type='audio/mpeg' url='https://feeds.soundcloud.com/stream/73-song.mp3'/></item></rss>";
        assert!(enclosure(feed, 73).is_some());
        assert!(enclosure(feed, 7).is_none());
        assert!(enclosure(&feed.replace("feeds.soundcloud.com", "example.org"), 73).is_none());
    }
}
