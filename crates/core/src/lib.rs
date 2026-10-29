use std::path::PathBuf;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub destination: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Single-connection streaming download. Writes directly to `request.destination`.
///
/// This is the minimal working path; parallel chunked downloads, resume via
/// `Range` headers, and a persistent queue are not implemented yet (see
/// README roadmap).
pub async fn download(
    request: &DownloadRequest,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), DownloadError> {
    let response = reqwest::get(&request.url).await?.error_for_status()?;
    let total = response.content_length();

    let mut file = tokio::fs::File::create(&request.destination).await?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        on_progress(Progress { downloaded, total });
    }

    file.flush().await?;
    Ok(())
}
