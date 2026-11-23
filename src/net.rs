use std::sync::OnceLock;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, String>;

pub const BROWSER_UA: &str = "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Mobile Safari/537.36";

pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .http_status_as_error(true)
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
