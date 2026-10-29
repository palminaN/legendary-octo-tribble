use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Stdio;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

mod site;
mod stream;

pub use site::{download_site, list_formats, FormatOption, SiteRequest};
pub use stream::download_stream;

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub destination: PathBuf,
    /// Extra request headers (Cookie, Referer, User-Agent…) captured from the
    /// browser, for servers that refuse requests without them.
    pub headers: Vec<(String, String)>,
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
    #[error("ffmpeg not found: install it or set DM_FFMPEG to its path")]
    FfmpegNotFound,
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("yt-dlp not found: install it or set DM_YTDLP to its path")]
    YtDlpNotFound,
    #[error("yt-dlp failed: {0}")]
    YtDlp(String),
}

/// A command for an external tool (ffmpeg, yt-dlp) whose output is parsed
/// rather than shown.
fn tool_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        // The caller's stdin/stdout may be a protocol channel (native
        // messaging), so the tool must not inherit them.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        // The native host runs without a console; don't pop one up for the tool.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
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
    let mut builder = reqwest::Client::new().get(&request.url);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    let response = builder.send().await?.error_for_status()?;
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
