//! HLS/DASH stream downloads, delegated to ffmpeg: it already handles playlist
//! parsing, variant selection, segment fetching, AES-128 decryption and muxing
//! separate audio/video tracks into a single file.

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::{tool_command, DownloadError, DownloadRequest, Progress};

/// Lets users point at an ffmpeg that isn't on PATH.
pub(crate) const FFMPEG_ENV: &str = "DM_FFMPEG";

/// Downloads the stream at `request.url` (an HLS or DASH manifest) and remuxes
/// it, without re-encoding, into `request.destination`. The container is
/// picked by ffmpeg from the destination's extension.
///
/// Progress reports bytes written so far; `total` is always `None` since the
/// final size of a stream isn't known in advance.
pub async fn download_stream(
    request: &DownloadRequest,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), DownloadError> {
    let ffmpeg = std::env::var_os(FFMPEG_ENV).unwrap_or_else(|| "ffmpeg".into());
    let mut command = tool_command(ffmpeg);
    command.args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error"]);

    let mut headers = String::new();
    for (name, value) in &request.headers {
        if name.eq_ignore_ascii_case("user-agent") {
            command.arg("-user_agent").arg(value);
        } else {
            headers.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    if !headers.is_empty() {
        command.arg("-headers").arg(headers);
    }

    command
        // 30s, in microseconds: fail instead of hanging on a dead connection.
        .args(["-rw_timeout", "30000000"])
        // Keeps a malicious playlist from pointing segments at local files.
        .args(["-protocol_whitelist", "http,https,tls,tcp,crypto,data"])
        .arg("-i")
        .arg(&request.url)
        .args(["-c", "copy", "-progress", "pipe:1", "-y"])
        .arg(&request.destination);

    let mut child = command.spawn().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => DownloadError::FfmpegNotFound,
        _ => err.into(),
    })?;

    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_task = tokio::spawn(async move {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output).await;
        output
    });

    // `-progress pipe:1` emits `key=value` lines; total_size is bytes written.
    let mut lines = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();
    while let Some(line) = lines.next_line().await? {
        if let Some(Ok(downloaded)) = line.strip_prefix("total_size=").map(|size| size.parse()) {
            on_progress(Progress {
                downloaded,
                total: None,
            });
        }
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        let _ = tokio::fs::remove_file(&request.destination).await;
        let message = match stderr.trim() {
            "" => status.to_string(),
            message => message.to_owned(),
        };
        return Err(DownloadError::Ffmpeg(message));
    }
    Ok(())
}
