//! Media handling utilities: download, encode, etc.

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose, Engine as _};
use thiserror::Error;

/// Errors that can occur while handling media.
#[derive(Debug, Error)]
pub enum MediaError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid media content: empty or corrupted")]
    InvalidContent,
    #[error("Image processing error: {0}")]
    Image(String),
}

/// Download media from URL and save to the specified directory.
///
/// Returns the path to the saved file.
pub async fn download_media(
    url: &str,
    filename: Option<&str>,
    save_dir: &PathBuf,
) -> Result<PathBuf, MediaError> {
    // Create directory if it doesn't exist
    tokio::fs::create_dir_all(save_dir).await?;

    // Determine filename
    let save_filename = match filename {
        Some(s) => s.to_string(),
        None => uuid::Uuid::new_v4().to_string(),
    };
    let save_path = save_dir.join(save_filename);

    // Download
    let client = reqwest::Client::new();
    let response = client.get(url).send().await?;
    let bytes = response.bytes().await?;

    if bytes.is_empty() {
        return Err(MediaError::InvalidContent);
    }

    tokio::fs::write(&save_path, &bytes).await?;

    Ok(save_path)
}

/// An image (or other media) encoded as a base64 data URL, plus info about
/// any compression applied. `compression.bytes` is emptied after the data
/// URL is built to avoid holding a second copy of the payload.
#[derive(Debug)]
pub struct EncodedImage {
    pub data_url: String,
    pub compression: CompressedImage,
}

/// Download media from URL and encode as base64 data URL, compressing
/// images first (see [`compress_image_bytes`]).
pub async fn download_and_encode_base64(
    url: &str,
    content_type: &str,
    max_image_dim: u32,
) -> Result<EncodedImage, MediaError> {
    let client = reqwest::Client::new();
    let bytes = client.get(url).send().await?.bytes().await?;

    if bytes.is_empty() {
        return Err(MediaError::InvalidContent);
    }

    encode_image_bytes(&bytes, content_type, max_image_dim).await
}

/// Read a local file and encode it as a base64 data URL, compressing images
/// first (see [`compress_image_bytes`]). The MIME type is inferred from the
/// file extension.
pub async fn encode_file_base64(
    path: &Path,
    max_image_dim: u32,
) -> Result<EncodedImage, MediaError> {
    let bytes = tokio::fs::read(path).await?;

    if bytes.is_empty() {
        return Err(MediaError::InvalidContent);
    }

    let mime_type = mime_from_extension(path);
    encode_image_bytes(&bytes, mime_type, max_image_dim).await
}

/// Compress (if an image and enabled) and base64-encode raw media bytes.
async fn encode_image_bytes(
    bytes: &[u8],
    mime: &str,
    max_image_dim: u32,
) -> Result<EncodedImage, MediaError> {
    let mut compression = if mime.starts_with("image/") {
        let owned = bytes.to_vec();
        let mime = mime.to_string();
        // Decoding/resizing/encoding is pure CPU work on multi-MB payloads —
        // keep it off the async runtime threads.
        tokio::task::spawn_blocking(move || compress_image_bytes(&owned, &mime, max_image_dim))
            .await
            .map_err(|e| MediaError::Image(format!("compression task failed: {}", e)))??
    } else {
        CompressedImage {
            bytes: bytes.to_vec(),
            mime: mime.to_string(),
            orig_dims: (0, 0),
            new_dims: (0, 0),
            kept_original: true,
        }
    };

    let data_url = format!(
        "data:{};base64,{}",
        compression.mime,
        general_purpose::STANDARD.encode(&compression.bytes)
    );
    // The bytes now only exist inside the data URL; drop the copy.
    compression.bytes = Vec::new();
    Ok(EncodedImage {
        data_url,
        compression,
    })
}

/// Infer a MIME type from the file extension.
///
/// Falls back to `application/octet-stream` for unknown extensions.
fn mime_from_extension(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Result of preparing an image for the vision-model context.
#[derive(Debug)]
pub struct CompressedImage {
    /// Encoded image bytes (possibly the untouched original).
    pub bytes: Vec<u8>,
    /// MIME type of `bytes`.
    pub mime: String,
    /// Original (width, height); (0, 0) when unknown (passthrough).
    pub orig_dims: (u32, u32),
    /// Final (width, height); (0, 0) when unknown (passthrough).
    pub new_dims: (u32, u32),
    /// True when the original bytes were kept (GIF, compression disabled,
    /// or re-encoding would have grown the file).
    pub kept_original: bool,
}

/// Downscale and re-encode an image for the vision-model context.
///
/// Base64 image payloads are re-sent with every LLM call, so multi-MB 2K
/// images quickly blow past provider-gateway request-body limits (HTTP 413).
/// This function proportionally downscales the image so its longest side is
/// at most `max_dim`, flattens any alpha channel onto white, and re-encodes
/// as JPEG (quality 85) — typically a 10x size reduction for generated 2K
/// PNGs with no practical loss for vision models, which ingest images at
/// roughly this resolution anyway.
///
/// Pass-through cases (original bytes kept unchanged):
/// - `max_dim == 0` (compression disabled)
/// - GIF (re-encoding would keep only the first frame of an animation)
/// - re-encoding produced a LARGER file (image was already well optimized)
/// Decode guards: images whose width or height exceeds this are rejected
/// before any pixel buffer is allocated. File size says nothing about the
/// decoded size — a tiny, highly compressible PNG can decompress to hundreds
/// of MB (decode bomb), enough to OOM a resident bot process.
const MAX_DECODE_DIMENSION: u32 = 16384;
/// Hard cap on total allocation during decode (256MB).
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;

fn compress_image_bytes(bytes: &[u8], mime: &str, max_dim: u32) -> Result<CompressedImage, MediaError> {
    if max_dim == 0 || mime == "image/gif" {
        return Ok(CompressedImage {
            bytes: bytes.to_vec(),
            mime: mime.to_string(),
            orig_dims: (0, 0),
            new_dims: (0, 0),
            kept_original: true,
        });
    }

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);

    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| MediaError::Image(format!("format detection failed: {}", e)))?;
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| MediaError::Image(format!("decode failed: {}", e)))?;
    let orig_dims = (img.width(), img.height());

    // JPEG has no alpha channel — flatten transparency onto white.
    let rgb = flatten_alpha_to_white(img);

    let final_img = if orig_dims.0.max(orig_dims.1) > max_dim {
        let longest = orig_dims.0.max(orig_dims.1) as f32;
        let scale = max_dim as f32 / longest;
        let nw = ((orig_dims.0 as f32 * scale).round() as u32).max(1);
        let nh = ((orig_dims.1 as f32 * scale).round() as u32).max(1);
        image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Lanczos3)
    } else {
        rgb
    };

    let mut jpeg = Vec::new();
    {
        use image::ImageEncoder as _;
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85);
        encoder
            .write_image(
                final_img.as_raw(),
                final_img.width(),
                final_img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| MediaError::Image(format!("JPEG encode failed: {}", e)))?;
    }

    // Keep the original when re-encoding grew the file.
    if jpeg.len() >= bytes.len() {
        return Ok(CompressedImage {
            bytes: bytes.to_vec(),
            mime: mime.to_string(),
            orig_dims,
            new_dims: orig_dims,
            kept_original: true,
        });
    }

    Ok(CompressedImage {
        bytes: jpeg,
        mime: "image/jpeg".to_string(),
        orig_dims,
        new_dims: (final_img.width(), final_img.height()),
        kept_original: false,
    })
}

/// Composite an image's alpha channel onto a white background, returning RGB.
fn flatten_alpha_to_white(img: image::DynamicImage) -> image::RgbImage {
    use image::GenericImageView;
    let (w, h) = img.dimensions();
    let mut out = image::RgbImage::new(w, h);
    for (x, y, p) in img.pixels() {
        let a = p.0[3] as f32 / 255.0;
        let blend = |c: u8| ((c as f32 * a) + (255.0 * (1.0 - a))).round() as u8;
        out.put_pixel(x, y, image::Rgb([blend(p.0[0]), blend(p.0[1]), blend(p.0[2])]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random RGB PNG. Noise barely compresses in PNG,
    /// so the downscaled JPEG is guaranteed to be much smaller — this keeps
    /// the size assertions meaningful and reproducible.
    fn noise_png(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbImage::new(w, h);
        let mut seed: u32 = 0x1234_5678;
        for (_, _, p) in img.enumerate_pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *p = image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn solid_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn noise_gif(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbImage::new(w, h);
        let mut seed: u32 = 0xDEAD_BEEF;
        for (_, _, p) in img.enumerate_pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *p = image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Gif)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn big_image_is_downscaled_and_reencoded_as_jpeg() {
        let orig = noise_png(2048, 2048);
        let out = compress_image_bytes(&orig, "image/png", 1024).unwrap();
        assert_eq!(out.mime, "image/jpeg");
        assert_eq!(out.orig_dims, (2048, 2048));
        assert_eq!(out.new_dims, (1024, 1024));
        assert!(!out.kept_original);
        // JPEG magic bytes
        assert_eq!(&out.bytes[0..2], &[0xFF, 0xD8]);
        assert!(
            out.bytes.len() * 10 < orig.len(),
            "2048px noise PNG should shrink >10x as a 1024px JPEG: {} -> {} bytes",
            orig.len(),
            out.bytes.len()
        );
    }

    #[test]
    fn small_image_keeps_dimensions_still_reencodes() {
        let orig = noise_png(800, 600);
        let out = compress_image_bytes(&orig, "image/png", 1024).unwrap();
        assert_eq!(out.new_dims, (800, 600));
        assert_eq!(out.mime, "image/jpeg");
        assert!(!out.kept_original);
    }

    #[test]
    fn gif_passes_through_unchanged() {
        // Re-encoding a GIF keeps only the first frame, so GIFs must never
        // go through the compression path.
        let orig = noise_gif(32, 32);
        let out = compress_image_bytes(&orig, "image/gif", 1024).unwrap();
        assert!(out.kept_original);
        assert_eq!(out.bytes, orig);
        assert_eq!(out.mime, "image/gif");
    }

    #[test]
    fn tiny_image_keeps_original_when_reencoding_would_grow() {
        let orig = solid_png(8, 8);
        let out = compress_image_bytes(&orig, "image/png", 1024).unwrap();
        assert!(out.kept_original, "tiny PNG should not be replaced by a larger JPEG");
        assert_eq!(out.bytes, orig);
        assert_eq!(out.mime, "image/png");
    }

    #[test]
    fn transparent_pixels_flatten_to_white() {
        // 256px RGBA noise under full transparency: the PNG is large (noise)
        // while the flattened JPEG is tiny, so the compression path runs.
        let mut img = image::RgbaImage::new(256, 256);
        let mut seed: u32 = 0x0BAD_C0DE;
        for (_, _, p) in img.enumerate_pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *p = image::Rgba([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8, 0]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let out = compress_image_bytes(&buf.get_ref(), "image/png", 1024).unwrap();
        assert_eq!(out.mime, "image/jpeg");
        assert!(!out.kept_original);
        let decoded = image::load_from_memory(&out.bytes).unwrap();
        use image::GenericImageView as _;
        let p = decoded.get_pixel(0, 0);
        assert!(
            p.0[0] >= 250 && p.0[1] >= 250 && p.0[2] >= 250,
            "transparent pixels should flatten to white, got {:?}",
            p
        );
    }

    #[test]
    fn zero_max_dimension_disables_compression() {
        let orig = noise_png(2048, 2048);
        let out = compress_image_bytes(&orig, "image/png", 0).unwrap();
        assert!(out.kept_original);
        assert_eq!(out.bytes, orig);
        assert_eq!(out.mime, "image/png");
    }

    #[test]
    fn non_square_images_scale_by_longest_side() {
        // Portrait: 600×2000 → longest side 2000 → 307×1024
        let out = compress_image_bytes(&noise_png(600, 2000), "image/png", 1024).unwrap();
        assert_eq!(out.new_dims, (307, 1024));
        assert_eq!(out.orig_dims, (600, 2000));

        // Extreme aspect: 10000×10 → 1024×1
        let out = compress_image_bytes(&noise_png(10000, 10), "image/png", 1024).unwrap();
        assert_eq!(out.new_dims, (1024, 1));
    }

    #[test]
    fn oversized_dimensions_are_rejected_before_decoding() {
        // A highly compressible 20000×10 PNG is a tiny file, but decoding
        // must refuse it (decode bomb guard) instead of trusting file size.
        let bomb = solid_png(20000, 10);
        let result = compress_image_bytes(&bomb, "image/png", 1024);
        assert!(
            result.is_err(),
            "images wider/taller than the decode cap must be rejected, got Ok"
        );
    }
}
