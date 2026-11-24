//! Small square cover thumbnails for lists and grids, cached on disk as JPEG.

use crate::library::Track;
use image::imageops::FilterType;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::path::PathBuf;

const SIZE: u32 = 160;

pub type Thumb = SharedPixelBuffer<Rgba8Pixel>;

/// Tracks of the same album share one thumbnail.
pub fn key(t: &Track) -> String {
    if t.album.is_empty() || t.album == "Sin álbum" {
        t.path.clone()
    } else {
        format!("{}\u{1}{}", t.artist.to_lowercase(), t.album.to_lowercase())
    }
}

fn file(key: &str, ext: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    crate::paths::get()
        .cache_dir
        .join("thumbs")
        .join(format!("{:x}.{ext}", h.finish()))
}

/// Loads the cached thumbnail or builds it from the first file with a cover.
pub fn load(key: &str, paths: &[String]) -> Option<Thumb> {
    let jpg = file(key, "jpg");
    if let Ok(img) = image::open(&jpg) {
        return Some(buffer(img.to_rgba8()));
    }
    // Remember how many files had no cover so they are not re-read on every start.
    let none = file(key, "none");
    let tried: usize = std::fs::read_to_string(&none)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if tried >= paths.len() {
        return None;
    }
    let img = paths
        .iter()
        .filter_map(|p| crate::library::cover(p))
        .find_map(|data| image::load_from_memory(&data).ok());
    let Some(img) = img else {
        let _ = std::fs::create_dir_all(none.parent()?);
        let _ = std::fs::write(&none, paths.len().to_string());
        return None;
    };
    let small = img.resize_to_fill(SIZE, SIZE, FilterType::Triangle);
    let _ = std::fs::create_dir_all(jpg.parent()?);
    let _ = small
        .to_rgb8()
        .save_with_format(&jpg, image::ImageFormat::Jpeg);
    Some(buffer(small.to_rgba8()))
}

/// Square thumbnail from encoded image bytes (e.g. a YouTube preview).
pub fn from_bytes(data: &[u8]) -> Option<Thumb> {
    let img = image::load_from_memory(data).ok()?;
    let side = img.width().min(img.height());
    let img = img.crop_imm(
        (img.width() - side) / 2,
        (img.height() - side) / 2,
        side,
        side,
    );
    Some(buffer(
        img.resize_exact(96, 96, FilterType::Triangle).to_rgba8(),
    ))
}

pub fn forget(key: &str) {
    for ext in ["jpg", "none"] {
        let _ = std::fs::remove_file(file(key, ext));
    }
}

fn buffer(img: image::RgbaImage) -> Thumb {
    SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height())
}

/// A dark, saturated background color taken from the cover.
pub fn tint(t: &Thumb) -> slint::Color {
    let (mut r, mut g, mut b, mut w) = (0f32, 0f32, 0f32, 0f32);
    for p in t.as_slice() {
        let (pr, pg, pb) = (p.r as f32, p.g as f32, p.b as f32);
        let weight = pr.max(pg).max(pb) - pr.min(pg).min(pb) + 8.0;
        r += pr * weight;
        g += pg * weight;
        b += pb * weight;
        w += weight;
    }
    let (r, g, b) = (r / w, g / w, b / w);
    let peak = r.max(g).max(b).max(1.0);
    let scale = 120.0 / peak;
    slint::Color::from_rgb_u8((r * scale) as u8, (g * scale) as u8, (b * scale) as u8)
}
