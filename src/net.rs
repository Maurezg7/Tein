use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::{Instant, SystemTime};

pub type Result<T> = std::result::Result<T, String>;
pub const MAX_AUDIO: u64 = 512 * 1024 * 1024;

pub const BROWSER_UA: &str = "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Mobile Safari/537.36";

fn cooldown(header: Option<&str>, now: SystemTime) -> Duration {
    let seconds = header
        .and_then(|s| {
            s.parse::<u64>().ok().or_else(|| {
                httpdate::parse_http_date(s)
                    .ok()
                    .map(|t| t.duration_since(now).unwrap_or_default().as_secs())
            })
        })
        .unwrap_or(30);
    Duration::from_secs(seconds.clamp(5, 86_400))
}

#[derive(Default)]
struct Backoff(Mutex<Vec<(String, Instant)>>);

impl ureq::middleware::Middleware for Backoff {
    fn handle(
        &self,
        req: ureq::http::Request<ureq::SendBody>,
        next: ureq::middleware::MiddlewareNext,
    ) -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        let host = req.uri().host().unwrap_or_default().to_ascii_lowercase();
        {
            let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
            entries.retain(|(_, until)| *until > Instant::now());
            if entries.iter().any(|(h, _)| h == &host) {
                return Err(ureq::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "El proveedor está saturado; esperá antes de reintentar",
                )));
            }
        }
        let res = next.handle(req)?;
        let status = res.status();
        if matches!(status.as_u16(), 429 | 503) {
            let until = Instant::now()
                + cooldown(
                    res.headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok()),
                    SystemTime::now(),
                );
            let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, old)) = entries.iter_mut().find(|(h, _)| h == &host) {
                *old = (*old).max(until);
            } else {
                if entries.len() == 64 {
                    entries.remove(0);
                }
                entries.push((host, until));
            }
        }
        if status.is_client_error() || status.is_server_error() {
            return Err(ureq::Error::StatusCode(status.as_u16()));
        }
        Ok(res)
    }
}

pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(5)
            .timeout_global(Some(Duration::from_secs(90)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .middleware(Backoff::default())
            .build()
            .into()
    })
}

pub fn get_text(url: &str, ua: &str) -> Result<String> {
    get_text_with(url, &[("User-Agent", ua)])
}

pub fn get_text_with(url: &str, headers: &[(&str, &str)]) -> Result<String> {
    let mut req = agent()
        .get(url)
        .header("Accept-Language", "es-ES,es;q=0.9,en;q=0.8");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.call()
        .map_err(|e| format!("Error de red: {e}"))?
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("Error leyendo respuesta: {e}"))
}

pub fn get_json(url: &str, ua: &str) -> Result<serde_json::Value> {
    serde_json::from_str(&get_text(url, ua)?).map_err(|e| format!("JSON inválido: {e}"))
}

pub fn get_bytes(url: &str, ua: &str) -> Result<Vec<u8>> {
    agent()
        .get(url)
        .header("User-Agent", ua)
        .call()
        .map_err(|e| format!("Error de red: {e}"))?
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("Error leyendo respuesta: {e}"))
}

/// Streams a fresh public audio file to disk, reporting received and expected bytes.
pub fn download_to(
    url: &str,
    ua: &str,
    dest: &std::path::Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    use std::io::{Read, Write};
    let mut resp = agent()
        .get(url)
        .header("User-Agent", ua)
        .call()
        .map_err(|e| format!("Error de red: {e}"))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if total > MAX_AUDIO
        || resp
            .headers()
            .get("content-type")
            .and_then(|s| s.to_str().ok())
            .is_some_and(|s| s.starts_with("text/") || s.starts_with("application/json"))
    {
        return Err("La respuesta no es audio o supera los 512 MiB".into());
    }
    let mut file = std::fs::File::create(dest)
        .map_err(|e| format!("No se pudo escribir {}: {e}", dest.display()))?;
    let mut reader = resp.body_mut().with_config().limit(MAX_AUDIO).reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Error de red: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(done, total);
    }
    if done == 0 || (total > 0 && total != done) {
        return Err("La descarga está vacía o incompleta".into());
    }
    Ok(())
}

pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let mut req = agent().post(url).header("Content-Type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let text = req
        .send(body.to_string())
        .map_err(|e| format!("Error de red: {e}"))?
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("Error leyendo respuesta: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON inválido: {e}"))
}

pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Returns the value of `key` in the query string of `url`.
pub fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_stops_only_the_busy_host_without_network() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .middleware(Backoff::default())
            .middleware(
                move |req: ureq::http::Request<ureq::SendBody>,
                      _: ureq::middleware::MiddlewareNext| {
                    count.fetch_add(1, Ordering::Relaxed);
                    let status = match req.uri().host() {
                        Some("busy.invalid") => 429,
                        Some("maintenance.invalid") => 503,
                        _ => 200,
                    };
                    Ok(ureq::http::Response::builder()
                        .status(status)
                        .header("retry-after", "60")
                        .body(ureq::Body::builder().data(vec![]))?)
                },
            )
            .build()
            .into();
        for (url, status) in [
            ("https://busy.invalid/", 429),
            ("https://maintenance.invalid/", 503),
        ] {
            assert!(
                matches!(agent.get(url).call(), Err(ureq::Error::StatusCode(n)) if n == status)
            );
            assert!(
                matches!(agent.get(url).call(), Err(ureq::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
        assert!(agent.get("https://available.invalid/").call().is_ok());
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn cooldown_accepts_seconds_and_http_dates() {
        let now = SystemTime::UNIX_EPOCH;
        for (header, secs) in [
            (None, 30),
            (Some("120"), 120),
            (Some("0"), 5),
            (Some("bad"), 30),
            (Some("90000"), 86400),
            (Some("Thu, 01 Jan 1970 00:02:01 GMT"), 121),
        ] {
            assert_eq!(cooldown(header, now), Duration::from_secs(secs));
        }
    }
}
