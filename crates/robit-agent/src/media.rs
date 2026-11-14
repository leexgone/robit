//! Media handling utilities: download, encode, etc.

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose, Engine as _};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

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
    #[error("Media exceeds size limit ({limit} bytes)")]
    TooLarge { limit: u64 },
}

/// Options for [`download_media_with_opts`].
#[derive(Debug, Clone)]
pub struct MediaDownloadOptions {
    /// Maximum bytes to download (0 = unlimited). Enforced via Content-Length
    /// pre-check plus streaming accumulation, so memory never exceeds the cap.
    pub max_bytes: u64,
    /// Platform-provided MIME type, used to supply an extension when the
    /// filename has none (keeps extension-based MIME inference working).
    pub content_type: Option<String>,
}

impl Default for MediaDownloadOptions {
    fn default() -> Self {
        Self {
            max_bytes: crate::context::DEFAULT_MAX_MEDIA_BYTES,
            content_type: None,
        }
    }
}

/// Download media from URL and save to the specified directory with default
/// options (20MB cap). Returns the path to the saved file.
pub async fn download_media(
    url: &str,
    filename: Option<&str>,
    save_dir: &PathBuf,
) -> Result<PathBuf, MediaError> {
    download_media_with_opts(url, filename, save_dir, &MediaDownloadOptions::default()).await
}

/// Download media from URL and save to the specified directory.
///
/// The filename is sanitized (single path component, no control characters)
/// and collisions get a `-1`/`-2` suffix instead of silently overwriting.
/// The body is streamed to disk under `opts.max_bytes`; an oversized response
/// is rejected (Content-Length pre-check when present) and no file is left.
///
/// Returns the path to the saved file.
pub async fn download_media_with_opts(
    url: &str,
    filename: Option<&str>,
    save_dir: &PathBuf,
    opts: &MediaDownloadOptions,
) -> Result<PathBuf, MediaError> {
    tokio::fs::create_dir_all(save_dir).await?;

    let save_filename = sanitize_filename(filename, opts.content_type.as_deref());
    let save_path = resolve_collision(save_dir, &save_filename).await;

    let client = reqwest::Client::new();
    let mut response = client.get(url).send().await?;
    check_content_length(&response, opts.max_bytes)?;

    let mut file = tokio::fs::File::create(&save_path).await?;
    let mut total: u64 = 0;
    let result: Result<(), MediaError> = async {
        while let Some(chunk) = response.chunk().await? {
            total += chunk.len() as u64;
            if opts.max_bytes > 0 && total > opts.max_bytes {
                return Err(MediaError::TooLarge {
                    limit: opts.max_bytes,
                });
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(())
    }
    .await;

    if let Err(e) = result {
        drop(file);
        let _ = tokio::fs::remove_file(&save_path).await;
        return Err(e);
    }
    if total == 0 {
        let _ = tokio::fs::remove_file(&save_path).await;
        return Err(MediaError::InvalidContent);
    }

    Ok(save_path)
}

/// Reject early when the advertised Content-Length exceeds the cap.
fn check_content_length(response: &reqwest::Response, max_bytes: u64) -> Result<(), MediaError> {
    if max_bytes > 0 {
        if let Some(len) = response.content_length() {
            if len > max_bytes {
                return Err(MediaError::TooLarge { limit: max_bytes });
            }
        }
    }
    Ok(())
}

/// Sanitize a platform-provided filename into a single safe path component.
///
/// - Takes only the `file_name()` component (strips directories and `..`).
/// - Filters control characters and residual separators (`/`, `\`, `:`).
/// - Trims surrounding whitespace and dots.
/// - Empty result → UUID name; missing extension → mapped from content_type.
fn sanitize_filename(filename: Option<&str>, content_type: Option<&str>) -> String {
    let component = filename
        .and_then(|name| Path::new(name).file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut cleaned: String = component
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\' && *c != ':')
        .collect();
    cleaned = cleaned.trim().trim_matches('.').trim().to_string();

    if cleaned.is_empty() {
        cleaned = uuid::Uuid::new_v4().to_string();
    }
    if Path::new(&cleaned).extension().is_none() {
        if let Some(ext) = extension_from_content_type(content_type) {
            cleaned.push('.');
            cleaned.push_str(ext);
        }
    }
    cleaned
}

/// Map common MIME types to file extensions (for unnamed/extensionless files).
fn extension_from_content_type(content_type: Option<&str>) -> Option<&'static str> {
    let ct = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    let ext = match ct.as_str() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "text/markdown" => "md",
        "application/json" => "json",
        "application/zip" => "zip",
        "application/msword" => "doc",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        _ => return None,
    };
    Some(ext)
}

/// Pick a non-colliding path: `name.ext` → `name-1.ext` → `name-2.ext` …
/// Falls back to a UUID name (keeping the extension) after 100 attempts.
async fn resolve_collision(save_dir: &Path, filename: &str) -> PathBuf {
    let candidate = save_dir.join(filename);
    if !tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
        return candidate;
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| filename.to_string());
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    for i in 1..=100u32 {
        let name = match &ext {
            Some(e) => format!("{}-{}.{}", stem, i, e),
            None => format!("{}-{}", stem, i),
        };
        let p = save_dir.join(name);
        if !tokio::fs::try_exists(&p).await.unwrap_or(false) {
            return p;
        }
    }
    let name = match &ext {
        Some(e) => format!("{}.{}", uuid::Uuid::new_v4(), e),
        None => uuid::Uuid::new_v4().to_string(),
    };
    save_dir.join(name)
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
/// images first (see [`compress_image_bytes`]). The download is capped at
/// `max_bytes` (0 = unlimited), mirroring the save-to-disk path's guard.
pub async fn download_and_encode_base64(
    url: &str,
    content_type: &str,
    max_image_dim: u32,
    max_bytes: u64,
) -> Result<EncodedImage, MediaError> {
    let bytes = fetch_bytes_capped(url, max_bytes).await?;
    encode_image_bytes(&bytes, content_type, max_image_dim).await
}

/// Fetch a URL body into memory under a byte cap (Content-Length pre-check +
/// streaming accumulation). Empty bodies are rejected.
async fn fetch_bytes_capped(url: &str, max_bytes: u64) -> Result<Vec<u8>, MediaError> {
    let client = reqwest::Client::new();
    let mut response = client.get(url).send().await?;
    check_content_length(&response, max_bytes)?;

    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if max_bytes > 0 && (buf.len() + chunk.len()) as u64 > max_bytes {
            return Err(MediaError::TooLarge { limit: max_bytes });
        }
        buf.extend_from_slice(&chunk);
    }
    if buf.is_empty() {
        return Err(MediaError::InvalidContent);
    }
    Ok(buf)
}

/// Read a local file and encode it as a base64 data URL, compressing images
/// first (see [`compress_image_bytes`]). The MIME type is inferred from the
/// file extension.
pub async fn encode_file_base64(
    path: &Path,
    max_image_dim: u32,
) -> Result<EncodedImage, MediaError> {
    encode_file_base64_with_mime(path, "", max_image_dim).await
}

/// Like [`encode_file_base64`], but with an explicit MIME type (e.g. the
/// platform-provided `content_type` of an attachment). An empty `mime` falls
/// back to extension inference. Preferring the declared MIME avoids
/// misclassifying images whose saved filename has a wrong/missing extension.
pub async fn encode_file_base64_with_mime(
    path: &Path,
    mime: &str,
    max_image_dim: u32,
) -> Result<EncodedImage, MediaError> {
    let bytes = tokio::fs::read(path).await?;

    if bytes.is_empty() {
        return Err(MediaError::InvalidContent);
    }

    let mime_type = if mime.is_empty() {
        mime_from_extension(path)
    } else {
        mime
    };
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

    // ========================================================================
    // Filename sanitization (M4/M5)
    // ========================================================================

    #[test]
    fn sanitize_strips_traversal_and_directories() {
        assert_eq!(sanitize_filename(Some("../secret.txt"), None), "secret.txt");
        assert_eq!(
            sanitize_filename(Some("a/b/../../c.csv"), None),
            "c.csv"
        );
        assert_eq!(sanitize_filename(Some(".."), None).contains('.'), false);
        assert_eq!(sanitize_filename(Some("."), None).contains('.'), false);
    }

    #[test]
    fn sanitize_filters_separators_and_control_chars() {
        let out = sanitize_filename(Some("a\\b/c:d\0e.txt"), None);
        assert!(!out.contains('\\') && !out.contains('/') && !out.contains(':'));
        assert!(!out.chars().any(|c| c.is_control()));
        assert!(out.ends_with(".txt"));
    }

    #[test]
    fn sanitize_empty_falls_back_to_uuid_with_content_type_extension() {
        let out = sanitize_filename(None, Some("image/png"));
        assert!(out.ends_with(".png"), "got {}", out);
        assert!(out.len() > 4);
        let out = sanitize_filename(Some("   "), Some("text/csv; charset=utf-8"));
        assert!(out.ends_with(".csv"), "got {}", out);
        let out = sanitize_filename(None, Some("application/x-unknown"));
        assert!(!out.contains('.'), "unknown MIME must not invent an extension");
    }

    #[test]
    fn sanitize_adds_missing_extension_from_content_type() {
        assert_eq!(
            sanitize_filename(Some("report"), Some("application/pdf")),
            "report.pdf"
        );
        // Existing extension is kept even if content_type disagrees.
        assert_eq!(
            sanitize_filename(Some("report.bin"), Some("application/pdf")),
            "report.bin"
        );
    }

    #[tokio::test]
    async fn collision_gets_numeric_suffix_instead_of_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("dup.txt"), b"original")
            .await
            .unwrap();
        let p1 = resolve_collision(dir.path(), "dup.txt").await;
        assert_eq!(p1.file_name().unwrap(), "dup-1.txt");
        tokio::fs::write(&p1, b"second").await.unwrap();
        let p2 = resolve_collision(dir.path(), "dup.txt").await;
        assert_eq!(p2.file_name().unwrap(), "dup-2.txt");
        assert_eq!(tokio::fs::read(dir.path().join("dup.txt")).await.unwrap(), b"original");
    }

    // ========================================================================
    // Download guards (M6) — local one-shot HTTP server
    // ========================================================================

    /// Spawn a one-shot HTTP server returning `body`. `advertise_len` sets the
    /// Content-Length header (None → close-delimited body, no length header).
    async fn spawn_http_server(body: Vec<u8>, advertise_len: Option<u64>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let mut acc: Vec<u8> = Vec::new();
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            acc.extend_from_slice(&buf[..n]);
                            if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let header = match advertise_len {
                    Some(len) => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", len),
                    None => "HTTP/1.1 200 OK\r\n\r\n".to_string(),
                };
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{}/file", addr)
    }

    #[tokio::test]
    async fn download_saves_sanitized_file_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let url = spawn_http_server(b"a,b\n1,2\n".to_vec(), Some(8)).await;
        let path = download_media_with_opts(
            &url,
            Some("../evil.csv"),
            &dir.path().to_path_buf(),
            &MediaDownloadOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(path.file_name().unwrap(), "evil.csv");
        assert_eq!(path.parent().unwrap(), dir.path());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"a,b\n1,2\n");
    }

    #[tokio::test]
    async fn download_rejects_oversized_content_length_before_body() {
        let dir = tempfile::tempdir().unwrap();
        // Advertised length far above the cap; body never needs to arrive.
        let url = spawn_http_server(b"tiny".to_vec(), Some(100 * 1024 * 1024)).await;
        let opts = MediaDownloadOptions { max_bytes: 1024, content_type: None };
        let err = download_media_with_opts(&url, Some("big.bin"), &dir.path().to_path_buf(), &opts)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::TooLarge { limit: 1024 }));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn download_rejects_oversized_stream_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        // No Content-Length: the streaming accumulator must trip the cap.
        let url = spawn_http_server(vec![0u8; 4096], None).await;
        let opts = MediaDownloadOptions { max_bytes: 1024, content_type: None };
        let err = download_media_with_opts(&url, Some("big.bin"), &dir.path().to_path_buf(), &opts)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::TooLarge { limit: 1024 }));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn download_zero_cap_means_unlimited() {
        let dir = tempfile::tempdir().unwrap();
        let url = spawn_http_server(vec![7u8; 4096], None).await;
        let opts = MediaDownloadOptions { max_bytes: 0, content_type: None };
        let path = download_media_with_opts(&url, Some("ok.bin"), &dir.path().to_path_buf(), &opts)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap().len(), 4096);
    }

    #[tokio::test]
    async fn download_empty_body_is_invalid_content() {
        let dir = tempfile::tempdir().unwrap();
        let url = spawn_http_server(Vec::new(), Some(0)).await;
        let err = download_media(&url, Some("empty.bin"), &dir.path().to_path_buf())
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::InvalidContent));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn download_same_name_twice_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let url1 = spawn_http_server(b"first".to_vec(), Some(5)).await;
        let url2 = spawn_http_server(b"second".to_vec(), Some(6)).await;
        let d = dir.path().to_path_buf();
        let p1 = download_media(&url1, Some("same.txt"), &d).await.unwrap();
        let p2 = download_media(&url2, Some("same.txt"), &d).await.unwrap();
        assert_ne!(p1, p2);
        assert_eq!(tokio::fs::read(&p1).await.unwrap(), b"first");
        assert_eq!(tokio::fs::read(&p2).await.unwrap(), b"second");
    }

    #[tokio::test]
    async fn download_unnamed_attachment_gets_content_type_extension() {
        let dir = tempfile::tempdir().unwrap();
        let url = spawn_http_server(solid_png(4, 4), None).await;
        let opts = MediaDownloadOptions {
            max_bytes: 0,
            content_type: Some("image/png".to_string()),
        };
        let path = download_media_with_opts(&url, None, &dir.path().to_path_buf(), &opts)
            .await
            .unwrap();
        assert!(
            path.extension().map(|e| e == "png").unwrap_or(false),
            "unnamed PNG attachment must save with .png, got {:?}",
            path.file_name()
        );
    }

    #[tokio::test]
    async fn encode_download_is_capped_too() {
        let url = spawn_http_server(vec![0u8; 4096], None).await;
        let err = download_and_encode_base64(&url, "application/octet-stream", 1024, 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::TooLarge { limit: 1024 }));
    }

    // ========================================================================
    // Local-first encoding (M2)
    // ========================================================================

    #[tokio::test]
    async fn encode_with_mime_prefers_declared_type_over_extension() {
        let dir = tempfile::tempdir().unwrap();
        // PNG bytes under a wrong extension: extension inference would pass
        // them through as octet-stream; the declared MIME must compress them.
        let path = dir.path().join("img.bin");
        tokio::fs::write(&path, noise_png(256, 256)).await.unwrap();

        let by_ext = encode_file_base64(&path, 64).await.unwrap();
        assert!(by_ext.compression.kept_original);
        assert_eq!(by_ext.compression.mime, "application/octet-stream");

        let by_mime = encode_file_base64_with_mime(&path, "image/png", 64).await.unwrap();
        assert!(!by_mime.compression.kept_original);
        assert_eq!(by_mime.compression.mime, "image/jpeg");
        assert_eq!(by_mime.compression.new_dims, (64, 64));
        assert!(by_mime.data_url.starts_with("data:image/jpeg;base64,"));
    }

    #[tokio::test]
    async fn encode_with_empty_mime_falls_back_to_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img.png");
        tokio::fs::write(&path, noise_png(128, 128)).await.unwrap();
        let out = encode_file_base64_with_mime(&path, "", 64).await.unwrap();
        assert_eq!(out.compression.mime, "image/jpeg");
        assert_eq!(out.compression.new_dims, (64, 64));
    }
}
