//! Vision input (§§33–35, 72–74): images as first-class attachments.
//!
//! Flow: bytes → decode (dimensions) → downscale to a token-sane size →
//! JPEG data URL for llama-server's OpenAI-compatible image_url parts.
//! When the loaded model has no vision support, images degrade honestly:
//! the model gets dimensions/filename context plus OCR-if-available — never
//! a silent drop, never a fake "I see it" (§73 fallback rule).

use serde::{Deserialize, Serialize};

/// Longest side after downscale: balances detail vs vision-token cost (§166).
pub const MAX_SIDE_PX: u32 = 1568;
pub const MAX_IMAGE_BYTES: usize = 8_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedImage {
    pub width: u32,
    pub height: u32,
    /// `data:image/jpeg;base64,...` ready for a message content part.
    pub data_url: String,
}

pub fn is_image_mime(mime: &str) -> bool {
    mime.to_lowercase().starts_with("image/")
}

/// Decode for metadata + normalize to a bounded JPEG data URL.
/// Pure-Rust (`image` crate): no Pillow/OpenCV sidecars needed locally.
pub fn prepare_image(bytes: &[u8]) -> Result<PreparedImage, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "image too large ({} bytes, max {MAX_IMAGE_BYTES})",
            bytes.len()
        ));
    }
    let img = image::load_from_memory(bytes).map_err(|e| format!("cannot decode image: {e}"))?;
    let (w, h) = (img.width(), img.height());
    let scaled = {
        let longest = w.max(h);
        if longest <= MAX_SIDE_PX {
            img.to_rgb8()
        } else {
            let scale = MAX_SIDE_PX as f32 / longest as f32;
            let (nw, nh) = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
            image::imageops::resize(
                &img.to_rgb8(),
                nw.max(1),
                nh.max(1),
                image::imageops::FilterType::Triangle,
            )
        }
    };
    let mut jpeg = vec![];
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80);
    enc.encode(
        scaled.as_raw(),
        scaled.width(),
        scaled.height(),
        image::ExtendedColorType::Rgb8,
    )
    .map_err(|e| format!("jpeg encode failed: {e}"))?;
    Ok(PreparedImage {
        width: w,
        height: h,
        data_url: format!(
            "data:image/jpeg;base64,{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &jpeg)
        ),
    })
}

/// Excerpt placeholder so text-only models still know what was attached.
pub fn image_excerpt(
    filename: &str,
    width: u32,
    height: u32,
    bytes: usize,
    vision: bool,
) -> String {
    if vision {
        format!(
            "[image: {filename} {width}x{height}, {} KB — attached below]",
            bytes / 1024
        )
    } else {
        format!(
            "[image: {filename} {width}x{height} — this model has no vision. Describe it from the filename, or switch to a vision-capable model.]"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200u8, 10, 10]));
        let mut buf = vec![];
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        buf
    }

    #[test]
    fn prepares_and_downscales() {
        let big = test_png(3000, 100);
        let p = prepare_image(&big).unwrap();
        assert_eq!((p.width, p.height), (3000, 100)); // original dims reported
        assert!(p.data_url.starts_with("data:image/jpeg;base64,"));
        let small = test_png(40, 30);
        assert!(prepare_image(&small).unwrap().data_url.len() > 100);
    }

    #[test]
    fn rejects_garbage_and_oversize() {
        assert!(prepare_image(b"not an image").is_err());
        assert!(prepare_image(&vec![0u8; MAX_IMAGE_BYTES + 1]).is_err());
    }

    #[test]
    fn fallback_excerpt_is_honest() {
        let t = image_excerpt("shot.png", 800, 600, 2048, false);
        assert!(t.contains("no vision"), "{t}");
        assert!(image_excerpt("shot.png", 800, 600, 2048, true).contains("attached below"));
    }
}
