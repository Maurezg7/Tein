pub mod apple;
pub mod audius;
pub mod soundcloud;
pub mod spotify;
pub mod youtube;

use crate::net::Result;

#[derive(Clone, Debug)]
pub enum Source {
    /// A YouTube video id, ready to download.
    YouTube(String),
    /// Metadata only (Spotify / Apple Music): the audio is matched on YouTube when downloading.
    Match,
    /// An Audius track id and its artwork URL.
    Audius {
        id: String,
        art: String,
        ext: String,
    },
    SoundCloud {
        url: String,
        art: String,
    },
}

impl OnlineTrack {
    pub fn thumb_key(&self) -> Option<&str> {
        match &self.source {
            Source::YouTube(id) => Some(id),
            Source::Audius { art, .. } | Source::SoundCloud { art, .. } if !art.is_empty() => {
                Some(art)
            }
            _ => None,
        }
    }
    /// Cache key and URL of a small preview image, when the source has one.
    pub fn thumb(&self) -> Option<(String, String)> {
        match &self.source {
            Source::YouTube(id) => Some((
                id.clone(),
                format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"),
            )),
            Source::Audius { art, .. } if !art.is_empty() => {
                Some((art.clone(), art.replace("480x480", "150x150")))
            }
            Source::SoundCloud { art, .. } if !art.is_empty() => Some((art.clone(), art.clone())),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OnlineTrack {
    pub title: String,
    pub artist: String,
    pub duration_secs: u32,
    pub source: Source,
    pub origin: &'static str,
}

pub struct Resolved {
    pub label: String,
    pub tracks: Vec<OnlineTrack>,
    /// Playlist or album name: its downloads go to a folder and a playlist with this name.
    pub collection: Option<String>,
}

/// Resolves a link from any supported service, or searches free text in `provider`
/// (0 YouTube, 1 SoundCloud, 2 Audius).
pub fn resolve_in(input: &str, provider: i32) -> Result<Resolved> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Escribí algo para buscar".into());
    }
    let is_link = |w: &str| {
        spotify::is_link(w)
            || apple::is_link(w)
            || youtube::is_link(w)
            || soundcloud::is_link(w)
            || audius::is_link(w)
    };
    let input = input
        .split_whitespace()
        .find(|w| is_link(w))
        .unwrap_or(input);
    if spotify::is_link(input) {
        return spotify::resolve(input);
    }
    if apple::is_link(input) {
        return apple::resolve(input);
    }
    if youtube::is_link(input) {
        return youtube::resolve_link(input);
    }
    if soundcloud::is_link(input) {
        return soundcloud::resolve(input);
    }
    if audius::is_link(input) {
        return audius::resolve(input);
    }
    if input.contains("://") {
        return Err("Pegá un enlace HTTPS de una plataforma compatible".into());
    }
    let tracks = match provider {
        1 => soundcloud::search(input)?,
        2 => audius::search(input)?,
        _ => youtube::search(input)?,
    };
    Ok(Resolved {
        label: format!("Resultados para \"{input}\""),
        tracks,
        collection: None,
    })
}

#[cfg(test)]
pub fn resolve(input: &str) -> Result<Resolved> {
    resolve_in(input, 0)
}

/// Finds the YouTube video id for a track, matching metadata when needed.
pub fn video_id_for(track: &OnlineTrack) -> Result<String> {
    match &track.source {
        Source::YouTube(id) => Ok(id.clone()),
        Source::Match => youtube::find_best(&track.artist, &track.title, track.duration_secs),
        _ => Err("Esta fuente no usa YouTube".into()),
    }
}

pub fn format_duration(secs: u32) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

pub(crate) fn parse_clock(text: &str) -> u32 {
    text.split(':').fold(0, |acc, part| {
        acc.saturating_mul(60)
            .saturating_add(part.trim().parse::<u32>().unwrap_or(0))
    })
}

/// Path segments of a URL, without scheme, host, query or fragment.
pub(crate) fn path_segments(url: &str) -> Vec<String> {
    let no_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let path = no_scheme.split(['?', '#']).next().unwrap_or("");
    path.split('/')
        .skip(1)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(75), "1:15");
        assert_eq!(format_duration(3725), "1:02:05");
    }

    /// Searches, downloads the first hit, then checks the file is readable and taggable.
    fn fetch_first(provider: i32, query: &str) {
        let r = resolve_in(query, provider).unwrap();
        assert!(!r.tracks.is_empty());
        let t = &r.tracks[0];
        println!(
            "{} - {} ({}s) {:?}",
            t.artist,
            t.title,
            t.duration_secs,
            t.thumb()
        );
        let dir = std::env::temp_dir().join(format!("tein-{}-{}", provider, fastrand::u64(..)));
        let got = std::sync::Mutex::new(None);
        let job = crate::download::Job {
            id: 1,
            track: t.clone(),
            folder: None,
        };
        let track = crate::download::run_in(
            &job,
            &|u| {
                if let crate::download::Update::Progress { status, .. } = u {
                    *got.lock().unwrap() = Some(status);
                }
            },
            dir.clone(),
        )
        .unwrap();
        println!(
            "{:?} {}s, last status {:?}",
            track.path,
            track.duration_secs,
            got.lock().unwrap()
        );
        assert!(track.duration_secs > 10);
        assert_eq!(track.title, crate::download::clean_names(t).1);
        assert!(crate::library::cover(&track.path).is_some());
        let file = std::fs::File::open(&track.path).unwrap();
        let dec = rodio::Decoder::try_from(file).unwrap();
        let mut dec = dec;
        assert!(dec.by_ref().take(200_000).count() == 200_000);
        use rodio::Source as _;
        dec.try_seek(std::time::Duration::from_secs(5)).unwrap();
        assert!(dec.take(200_000).count() == 200_000);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "network"]
    fn soundcloud_download() {
        assert!(!resolve_in("lofi", 1).unwrap().tracks.is_empty());
        fetch_first(
            1,
            "https://soundcloud.com/scifri/adam-aleksic-responds-to-callers-words",
        );
    }

    #[test]
    #[ignore = "network"]
    fn audius_download() {
        fetch_first(2, "Audius Wave Nichelle Wattz");
    }

    #[test]
    #[ignore = "network"]
    fn soundcloud_and_audius_links() {
        let r = resolve("https://soundcloud.com/forss/sets/soulhack").unwrap();
        println!("{} ({})", r.label, r.tracks.len());
        assert!(!r.tracks.is_empty() && r.collection.as_deref() == Some("Soulhack"));
        assert!(resolve("https://audius.co/sebbie2k/electronic-butterflies").is_err());
    }

    #[test]
    #[ignore = "network"]
    fn youtube_search_and_stream() {
        let r = resolve("daft punk get lucky").unwrap();
        assert!(!r.tracks.is_empty());
        let id = video_id_for(&r.tracks[0]).unwrap();
        let s = youtube::audio_stream(&id).unwrap();
        assert!(s.size > 0);
    }

    /// Full stream downloads get HTTP 403 from datacenter IPs; run from a home/mobile network.
    #[test]
    #[ignore = "network"]
    fn youtube_full_download() {
        let r = resolve("daft punk get lucky").unwrap();
        let s = youtube::audio_stream(&video_id_for(&r.tracks[0]).unwrap()).unwrap();
        let dest = std::env::temp_dir().join("tein-test.m4a");
        youtube::download(&s, &dest, &mut |_| {}).unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), s.size);
        let _ = std::fs::remove_file(dest);
    }

    #[test]
    #[ignore = "network"]
    fn spotify_track() {
        let r = resolve("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT").unwrap();
        assert_eq!(r.tracks.len(), 1);
        println!("{} - {}", r.tracks[0].artist, r.tracks[0].title);
        video_id_for(&r.tracks[0]).unwrap();
    }

    #[test]
    #[ignore = "network"]
    fn apple_album() {
        let r =
            resolve("https://music.apple.com/us/album/random-access-memories/617154241").unwrap();
        assert!(r.tracks.len() > 5);
        println!(
            "{}: {} - {}",
            r.label, r.tracks[0].artist, r.tracks[0].title
        );
    }

    #[test]
    #[ignore = "network"]
    fn youtube_playlist() {
        let r = resolve("https://www.youtube.com/playlist?list=PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI")
            .unwrap();
        println!("{} ({} tracks)", r.label, r.tracks.len());
        assert!(r.tracks.len() > 5);
    }

    #[test]
    #[ignore = "network"]
    fn spotify_playlist_over_100() {
        let r = resolve("https://open.spotify.com/playlist/37i9dQZF1DX4UtSsGT1Sbe").unwrap();
        println!("{} ({} tracks)", r.label, r.tracks.len());
        assert!(r.tracks.len() > 100);
        assert!(r.collection.is_some());
    }

    #[test]
    #[ignore = "network"]
    fn youtube_playlist_over_100() {
        let r = resolve("https://www.youtube.com/playlist?list=PLMC9KNkIncKtPzgY-5rmhvj7fax8fdxoj")
            .unwrap();
        println!("{} ({} tracks)", r.label, r.tracks.len());
        assert!(r.tracks.len() > 100);
    }

    #[test]
    #[ignore = "network"]
    fn youtube_download() {
        let id = std::env::var("YT_ID").unwrap_or("dQw4w9WgXcQ".into());
        let s = youtube::audio_stream(&id).unwrap();
        let dest = std::env::temp_dir().join("tein_yt_test.m4a");
        youtube::download(&s, &dest, &mut |_| {}).unwrap();
        let len = std::fs::metadata(&dest).unwrap().len();
        println!("downloaded {len} of {}", s.size);
        assert!(len > 1_000_000);
    }
}
