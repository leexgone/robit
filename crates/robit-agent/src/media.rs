//! Media handling utilities: download, encode, etc.

use std::path::PathBuf;

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

/// Download media from URL and encode as base64 data URL.
///
/// Returns a string like "data:image/jpeg;base64,...".
pub async fn download_and_encode_base64(
    url: &str,
    content_type: &str,
) -> Result<String, MediaError> {
    let client = reqwest::Client::new();
    let bytes = client.get(url).send().await?.bytes().await?;

    if bytes.is_empty() {
        return Err(MediaError::InvalidContent);
    }

    let base64 = general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", content_type, base64))
}
