pub mod apple;
pub mod spotify;
pub mod youtube;

use crate::net::Result;

#[derive(Clone, Debug)]
pub enum Source {
    /// A YouTube video id, ready to download.
    YouTube(String),
    /// Metadata only (Spotify / Apple Music): the audio is matched on YouTube when downloading.
    Match,
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
}

/// Resolves free text (search) or a YouTube / Spotify / Apple Music link.
pub fn resolve(input: &str) -> Result<Resolved> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Escribí algo para buscar".into());
    }
    let is_link = |w: &str| spotify::is_link(w) || apple::is_link(w) || youtube::is_link(w);
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
    let tracks = youtube::search(input)?;
    Ok(Resolved {
        label: format!("Resultados para \"{input}\""),
        tracks,
    })
}

/// Finds the YouTube video id for a track, matching metadata when needed.
pub fn video_id_for(track: &OnlineTrack) -> Result<String> {
    match &track.source {
        Source::YouTube(id) => Ok(id.clone()),
        Source::Match => youtube::find_best(&track.artist, &track.title, track.duration_secs),
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
        acc * 60 + part.trim().parse::<u32>().unwrap_or(0)
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
}
