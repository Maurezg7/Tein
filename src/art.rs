//! Small square cover thumbnails for lists and grids, cached on disk as JPEG.

use crate::library::Track;
use image::imageops::FilterType;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::path::PathBuf;

const SIZE: u32 = 112;

pub type Thumb = SharedPixelBuffer<Rgba8Pixel>;

pub fn decode(data: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

pub fn key(t: &Track) -> String {
    format!("{}\0{}", t.path, t.mtime)
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

pub fn load(key: &str, path: &str) -> Option<Thumb> {
    let jpg = file(key, "jpg");
    if let Ok(img) = image::open(&jpg) {
        return Some(buffer(img.to_rgba8()));
    }
    let img = crate::player::cached_cover(path)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| decode(&b))
        .or_else(|| decode(&crate::library::cover(path)?))?;
    let small = img.resize_to_fill(SIZE, SIZE, FilterType::Triangle);
    let _ = std::fs::create_dir_all(jpg.parent()?);
    let _ = small
        .to_rgb8()
        .save_with_format(&jpg, image::ImageFormat::Jpeg);
    Some(buffer(small.to_rgba8()))
}

/// Square thumbnail from encoded image bytes (e.g. a YouTube preview).
pub fn from_bytes(data: &[u8]) -> Option<Thumb> {
    let img = decode(data)?;
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
    let _ = std::fs::remove_file(file(key, "jpg"));
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
