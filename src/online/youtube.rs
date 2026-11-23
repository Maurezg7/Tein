use super::{OnlineTrack, Resolved, Source, parse_clock, path_segments};
use crate::net::{self, Result};
use serde_json::{Value, json};
use std::io::{Read, Write};

const VR_UA: &str = "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";
const WEB_VERSION: &str = "2.20240726.00.00";
const CHUNK: u64 = 4 * 1024 * 1024;

pub fn is_link(s: &str) -> bool {
    s.contains("youtube.com/") || s.contains("youtu.be/")
}

fn web_context() -> Value {
    json!({ "client": { "clientName": "WEB", "clientVersion": WEB_VERSION, "hl": "es", "gl": "US" } })
}

pub fn search(query: &str) -> Result<Vec<OnlineTrack>> {
    let body = json!({ "context": web_context(), "query": query, "params": "EgIQAQ%3D%3D" });
    let resp = net::post_json(
        "https://www.youtube.com/youtubei/v1/search?prettyPrint=false",
        &[("User-Agent", net::BROWSER_UA)],
        &body,
    )?;
    let mut out = Vec::new();
    collect(&resp, "videoRenderer", &mut |v| {
        let Some(id) = v["videoId"].as_str() else {
            return;
        };
        let Some(len) = v["lengthText"]["simpleText"].as_str() else {
            return;
        };
        out.push(OnlineTrack {
            title: runs_text(&v["title"]),
            artist: runs_text(&v["ownerText"]),
            duration_secs: parse_clock(len),
            source: Source::YouTube(id.to_string()),
            origin: "YouTube",
        });
    });
    Ok(out)
}

/// Picks the search result that best matches the given metadata.
pub fn find_best(artist: &str, title: &str, duration_secs: u32) -> Result<String> {
    let results = search(&format!("{artist} - {title} audio"))?;
    let close =
        |t: &&OnlineTrack| duration_secs == 0 || t.duration_secs.abs_diff(duration_secs) <= 12;
    let pick = results.iter().take(10).find(close).or(results.first());
    match pick.map(|t| &t.source) {
        Some(Source::YouTube(id)) => Ok(id.clone()),
        _ => Err(format!("No se encontró \"{title}\" en YouTube")),
    }
}

pub fn resolve_link(url: &str) -> Result<Resolved> {
    if let Some(list) = net::query_param(url, "list") {
        if net::query_param(url, "v").is_none() || url.contains("/playlist") {
            return playlist(&list);
        }
    }
    let id = video_id_from_url(url).ok_or("Link de YouTube no reconocido")?;
    let (info, _) = player(&id)?;
    let details = &info["videoDetails"];
    let track = OnlineTrack {
        title: details["title"].as_str().unwrap_or(&id).to_string(),
        artist: details["author"]
            .as_str()
            .unwrap_or("")
            .trim_end_matches(" - Topic")
            .to_string(),
        duration_secs: details["lengthSeconds"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        source: Source::YouTube(id),
        origin: "YouTube",
    };
    Ok(Resolved {
        label: "Video de YouTube".into(),
        tracks: vec![track],
        collection: None,
    })
}

fn video_id_from_url(url: &str) -> Option<String> {
    if let Some(v) = net::query_param(url, "v") {
        return Some(v);
    }
    let segs = path_segments(url);
    if url.contains("youtu.be/") {
        return segs.first().cloned();
    }
    match segs.as_slice() {
        [kind, id, ..] if kind == "shorts" || kind == "embed" || kind == "live" => Some(id.clone()),
        _ => None,
    }
}

fn playlist(list_id: &str) -> Result<Resolved> {
    const URL: &str = "https://www.youtube.com/youtubei/v1/browse?prettyPrint=false";
    let headers = [("User-Agent", net::BROWSER_UA)];
    let first = json!({ "context": web_context(), "browseId": format!("VL{list_id}") });
    let mut resp = net::post_json(URL, &headers, &first)?;
    let title = resp["header"]["playlistHeaderRenderer"]["title"]["simpleText"]
        .as_str()
        .or(resp["metadata"]["playlistMetadataRenderer"]["title"].as_str())
        .unwrap_or("Playlist de YouTube")
        .to_string();
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..MAX_PAGES {
        playlist_items(&resp, &mut tracks, &mut seen);
        let Some(token) = continuation(&resp) else {
            break;
        };
        let next = json!({ "context": web_context(), "continuation": token });
        match net::post_json(URL, &headers, &next) {
            Ok(r) => resp = r,
            Err(e) => {
                log::warn!("youtube playlist paging: {e}");
                break;
            }
        }
    }
    if tracks.is_empty() {
        return Err("La playlist está vacía o es privada".into());
    }
    Ok(Resolved {
        label: title.clone(),
        tracks,
        collection: Some(title),
    })
}

/// Pages of ~100 videos; caps huge playlists at about 5000 tracks.
const MAX_PAGES: usize = 50;

fn playlist_items(
    resp: &Value,
    tracks: &mut Vec<OnlineTrack>,
    seen: &mut std::collections::HashSet<String>,
) {
    collect(resp, "playlistVideoRenderer", &mut |v| {
        let Some(id) = v["videoId"]
            .as_str()
            .filter(|id| seen.insert(id.to_string()))
        else {
            return;
        };
        tracks.push(OnlineTrack {
            title: runs_text(&v["title"]),
            artist: runs_text(&v["shortBylineText"]),
            duration_secs: v["lengthSeconds"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            source: Source::YouTube(id.to_string()),
            origin: "YouTube",
        });
    });
    collect(resp, "lockupViewModel", &mut |v| {
        if v["contentType"].as_str() != Some("LOCKUP_CONTENT_TYPE_VIDEO") {
            return;
        }
        let Some(id) = v["contentId"]
            .as_str()
            .filter(|id| seen.insert(id.to_string()))
        else {
            return;
        };
        let meta = &v["metadata"]["lockupMetadataViewModel"];
        let artist = meta["metadata"]["contentMetadataViewModel"]["metadataRows"][0]["metadataParts"][0]["text"]
            ["content"]
            .as_str()
            .unwrap_or("");
        let mut duration = 0;
        collect(&v["contentImage"], "thumbnailBadgeViewModel", &mut |b| {
            if let Some(t) = b["text"].as_str().filter(|t| t.contains(':')) {
                duration = parse_clock(t);
            }
        });
        tracks.push(OnlineTrack {
            title: meta["title"]["content"].as_str().unwrap_or(id).to_string(),
            artist: artist.to_string(),
            duration_secs: duration,
            source: Source::YouTube(id.to_string()),
            origin: "YouTube",
        });
    });
}

/// Token of the "load more" entry that sits in the same list as the playlist videos.
fn continuation(v: &Value) -> Option<String> {
    match v {
        Value::Array(items) => {
            let is_video_list = items.iter().any(|i| {
                i.get("lockupViewModel").is_some() || i.get("playlistVideoRenderer").is_some()
            });
            if is_video_list {
                return items.iter().find_map(|i| {
                    i.get("continuationItemViewModel")
                        .or(i.get("continuationItemRenderer"))
                        .and_then(continuation_token)
                });
            }
            items.iter().find_map(continuation)
        }
        Value::Object(map) => map.values().find_map(continuation),
        _ => None,
    }
}

fn continuation_token(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => map
            .get("continuationCommand")
            .and_then(|c| c["token"].as_str())
            .map(str::to_string)
            .or_else(|| map.values().find_map(continuation_token)),
        Value::Array(items) => items.iter().find_map(continuation_token),
        _ => None,
    }
}

const IOS_UA: &str =
    "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";

struct Client {
    ua: &'static str,
    id: &'static str,
    version: &'static str,
    context: fn() -> Value,
}

/// Player clients tried in order; some videos are only served to one of them.
const CLIENTS: &[Client] = &[
    Client {
        ua: VR_UA,
        id: "28",
        version: "1.65.10",
        context: || {
            json!({ "clientName": "ANDROID_VR", "clientVersion": "1.65.10", "deviceMake": "Oculus",
                "deviceModel": "Quest 3", "androidSdkVersion": 32, "osName": "Android", "osVersion": "12L",
                "hl": "es", "gl": "US" })
        },
    },
    Client {
        ua: IOS_UA,
        id: "5",
        version: "20.10.4",
        context: || {
            json!({ "clientName": "IOS", "clientVersion": "20.10.4", "deviceMake": "Apple",
                "deviceModel": "iPhone16,2", "osName": "iPhone", "osVersion": "18.3.2.22D82",
                "hl": "es", "gl": "US" })
        },
    },
];

/// Anonymous visitor id; without it YouTube often answers "confirm you're not a bot".
fn visitor_data() -> Option<String> {
    static CACHE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().ok()?;
    if cache.is_none() {
        let body = json!({ "context": web_context() });
        *cache = net::post_json(
            "https://www.youtube.com/youtubei/v1/visitor_id?prettyPrint=false",
            &[("User-Agent", net::BROWSER_UA)],
            &body,
        )
        .ok()
        .and_then(|r| {
            r["responseContext"]["visitorData"]
                .as_str()
                .map(str::to_string)
        });
    }
    cache.clone()
}

/// Player response plus the user agent its stream URLs must be fetched with.
fn player(video_id: &str) -> Result<(Value, &'static str)> {
    let mut last_err = String::from("YouTube no respondió");
    let visitor = visitor_data();
    for client in CLIENTS {
        let mut ctx = (client.context)();
        let mut headers = vec![
            ("User-Agent", client.ua),
            ("X-YouTube-Client-Name", client.id),
            ("X-YouTube-Client-Version", client.version),
        ];
        if let Some(vd) = visitor.as_deref() {
            ctx["visitorData"] = json!(vd);
            headers.push(("X-Goog-Visitor-Id", vd));
        }
        let body = json!({
            "context": { "client": ctx },
            "videoId": video_id, "contentCheckOk": true, "racyCheckOk": true
        });
        let resp = match net::post_json(
            "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
            &headers,
            &body,
        ) {
            Ok(r) => r,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let status = resp["playabilityStatus"]["status"].as_str().unwrap_or("");
        if status == "OK" {
            return Ok((resp, client.ua));
        }
        let reason = resp["playabilityStatus"]["reason"]
            .as_str()
            .unwrap_or(status);
        last_err = format!("YouTube no permite este video: {reason}");
    }
    Err(last_err)
}

pub struct AudioStream {
    pub url: String,
    pub size: u64,
    ua: &'static str,
}

/// Best AAC (m4a) audio stream, which the player decodes natively.
pub fn audio_stream(video_id: &str) -> Result<AudioStream> {
    let (info, ua) = player(video_id)?;
    let formats = info["streamingData"]["adaptiveFormats"]
        .as_array()
        .ok_or("Sin formatos de audio")?;
    formats
        .iter()
        .filter(|f| {
            f["mimeType"]
                .as_str()
                .is_some_and(|m| m.starts_with("audio/mp4"))
        })
        .filter(|f| f["url"].is_string())
        .max_by_key(|f| {
            (
                f["itag"].as_i64() == Some(140),
                f["bitrate"].as_i64().unwrap_or(0),
            )
        })
        .map(|f| AudioStream {
            url: f["url"].as_str().unwrap_or_default().to_string(),
            size: f["contentLength"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            ua,
        })
        .ok_or_else(|| "No hay audio m4a disponible para este video".into())
}

/// Downloads the audio stream in ranged chunks, reporting progress in 0.0..=1.0.
pub fn download(
    stream: &AudioStream,
    dest: &std::path::Path,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let mut file =
        std::fs::File::create(dest).map_err(|e| format!("No se pudo crear el archivo: {e}"))?;
    let total = stream.size;
    let mut done: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let end = if total > 0 {
            (done + CHUNK).min(total) - 1
        } else {
            done + CHUNK - 1
        };
        let resp = net::agent()
            .get(&stream.url)
            .header("User-Agent", stream.ua)
            .header("Range", &format!("bytes={done}-{end}"))
            .call()
            .map_err(|e| match e {
                ureq::Error::StatusCode(403) => {
                    "YouTube bloqueó la descarga (403). Probá otra red o más tarde.".to_string()
                }
                e => format!("Error descargando: {e}"),
            })?;
        let mut reader = resp.into_body().into_reader();
        let mut got = 0u64;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("Error descargando: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("Error escribiendo: {e}"))?;
            got += n as u64;
            done += n as u64;
            if total > 0 {
                progress(done as f32 / total as f32);
            }
        }
        if got == 0 || (total > 0 && done >= total) || (total == 0 && got < CHUNK) {
            break;
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    if done == 0 {
        return Err("La descarga quedó vacía".into());
    }
    Ok(())
}

pub fn thumbnail(video_id: &str) -> Option<Vec<u8>> {
    ["maxresdefault", "hqdefault"].iter().find_map(|q| {
        net::get_bytes(
            &format!("https://i.ytimg.com/vi/{video_id}/{q}.jpg"),
            net::BROWSER_UA,
        )
        .ok()
    })
}

fn runs_text(v: &Value) -> String {
    if let Some(s) = v["simpleText"].as_str() {
        return s.to_string();
    }
    v["runs"]
        .as_array()
        .map(|runs| {
            runs.iter()
                .filter_map(|r| r["text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn collect(v: &Value, key: &str, f: &mut dyn FnMut(&Value)) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if k == key {
                    f(child);
                } else {
                    collect(child, key, f);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|c| collect(c, key, f)),
        _ => {}
    }
}
