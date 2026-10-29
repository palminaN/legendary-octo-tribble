//! Downloads from video sites (YouTube, Vimeo, …), delegated to yt-dlp: these
//! sites don't serve plain manifests, and yt-dlp keeps up with their frequent
//! player changes. yt-dlp also reads HLS/DASH manifests, which makes it the
//! source of the quality list for those too.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitStatus;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::stream::FFMPEG_ENV;
use crate::{tool_command, DownloadError, Progress};

/// Lets users point at a yt-dlp that isn't on PATH.
const YTDLP_ENV: &str = "DM_YTDLP";

const PROGRESS_PREFIX: &str = "progress ";

#[derive(Debug, Clone)]
pub struct SiteRequest {
    /// A video page (YouTube…) or a manifest URL.
    pub url: String,
    pub directory: PathBuf,
    /// A yt-dlp format selector, from [`FormatOption::selector`]; `None`
    /// picks the best quality.
    pub format: Option<String>,
    /// File name without extension; defaults to the video's title. Must
    /// already be a valid file name.
    pub title: Option<String>,
    pub headers: Vec<(String, String)>,
}

/// One choice in the quality menu.
#[derive(Debug, Clone, Serialize)]
pub struct FormatOption {
    pub label: String,
    /// yt-dlp format selector; `None` for the best quality.
    pub selector: Option<String>,
    /// Approximate size in bytes, when the site reports it.
    pub size: Option<u64>,
}

fn ytdlp_command(headers: &[(String, String)]) -> Command {
    let ytdlp = std::env::var_os(YTDLP_ENV).unwrap_or_else(|| "yt-dlp".into());
    let mut command = tool_command(ytdlp);
    command.args(["--no-playlist", "--no-colors", "--encoding", "utf-8"]);
    for (name, value) in headers {
        command.arg("--add-headers").arg(format!("{name}:{value}"));
    }
    command
}

fn spawn(command: &mut Command) -> Result<Child, DownloadError> {
    command.spawn().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => DownloadError::YtDlpNotFound,
        _ => err.into(),
    })
}

fn failure(status: ExitStatus, stderr: &str) -> DownloadError {
    // yt-dlp prints warnings too; the ERROR lines say what went wrong.
    let errors: Vec<&str> = stderr.lines().filter(|l| l.starts_with("ERROR")).collect();
    let message = match (errors.is_empty(), stderr.trim()) {
        (false, _) => errors.join("\n"),
        (true, "") => status.to_string(),
        (true, all) => all.to_owned(),
    };
    DownloadError::YtDlp(message)
}

/// Lists the qualities available for the video at `url`: best first, then
/// one entry per resolution (highest first), then audio only.
pub async fn list_formats(
    url: &str,
    headers: &[(String, String)],
) -> Result<Vec<FormatOption>, DownloadError> {
    let mut command = ytdlp_command(headers);
    command.arg("--dump-single-json").arg("--").arg(url);
    let output = spawn(&mut command)?.wait_with_output().await?;
    if !output.status.success() {
        return Err(failure(
            output.status,
            &String::from_utf8_lossy(&output.stderr),
        ));
    }
    let info: Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| DownloadError::YtDlp(format!("unexpected output: {err}")))?;
    Ok(format_options(&info))
}

fn format_options(info: &Value) -> Vec<FormatOption> {
    let formats = info["formats"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    // A codec is "none" when the track is absent, null when unknown (common
    // for HLS variants without a CODECS attribute).
    let lacks = |f: &Value, key: &str| f[key].as_str() == Some("none");
    let bitrate = |f: &Value| f["tbr"].as_f64().unwrap_or(0.0);
    let duration = info["duration"].as_f64();
    let size = |f: &Value| {
        f["filesize"]
            .as_f64()
            .or_else(|| f["filesize_approx"].as_f64())
            // Streams rarely report a size; estimate it (tbr is in kbit/s).
            .or_else(|| {
                f["tbr"]
                    .as_f64()
                    .zip(duration)
                    .map(|(tbr, secs)| tbr * 125.0 * secs)
            })
            .map(|bytes| bytes as u64)
    };

    let audio: Vec<&Value> = formats
        .iter()
        .filter(|f| lacks(f, "vcodec") && !lacks(f, "acodec"))
        .collect();
    let audio_size = audio.iter().filter_map(|f| size(f)).max();

    // The highest-bitrate video format of each height, roughly what the
    // `[height=N]` selector below picks. Formats with a known size win, since
    // yt-dlp also prefers plain downloads over streamed copies (which report
    // none).
    let rank = |f: &Value| {
        (
            f["filesize"].is_number() || f["filesize_approx"].is_number(),
            bitrate(f),
        )
    };
    let mut by_height: BTreeMap<u64, &Value> = BTreeMap::new();
    for f in formats.iter().filter(|f| !lacks(f, "vcodec")) {
        let Some(height) = f["height"].as_u64() else {
            continue;
        };
        by_height
            .entry(height)
            .and_modify(|best| {
                if rank(f) > rank(best) {
                    *best = f;
                }
            })
            .or_insert(f);
    }

    let mut options = vec![FormatOption {
        label: "Best quality".to_owned(),
        selector: None,
        size: None,
    }];
    for (height, f) in by_height.iter().rev() {
        let fps = f["fps"].as_f64().unwrap_or(0.0);
        let label = if fps >= 50.0 {
            format!("{height}p{}", fps.round())
        } else {
            format!("{height}p")
        };
        // Video-only formats get the best audio track merged in.
        let with_audio = if lacks(f, "acodec") {
            audio_size
        } else {
            Some(0)
        };
        options.push(FormatOption {
            label,
            selector: Some(format!("bv*[height={height}]+ba/b[height={height}]")),
            size: size(f).zip(with_audio).map(|(video, audio)| video + audio),
        });
    }
    if !audio.is_empty() {
        options.push(FormatOption {
            label: "Audio only".to_owned(),
            selector: Some("ba[ext=m4a]/ba".to_owned()),
            size: audio_size,
        });
    }
    options
}

/// Downloads the video at `request.url` into `request.directory` and returns
/// the path of the final file.
///
/// Separate video and audio tracks are downloaded one after the other (so
/// progress restarts once) and merged into an mp4 by ffmpeg.
pub async fn download_site(
    request: &SiteRequest,
    mut on_progress: impl FnMut(Progress),
) -> Result<PathBuf, DownloadError> {
    let stem = match &request.title {
        // A literal in yt-dlp's output template, where % starts a field.
        Some(title) => title.replace('%', "%%"),
        // Byte-limited title: long titles would exceed Windows' path limit.
        None => "%(title).150B [%(id)s]".to_owned(),
    };
    // Different qualities of the same video mustn't share a name: yt-dlp
    // would consider the second one already downloaded.
    let suffix = if request.format.is_some() {
        " %(resolution)s"
    } else {
        ""
    };

    let mut command = ytdlp_command(&request.headers);
    command
        .args(["--newline", "--merge-output-format", "mp4"])
        // --print implies --quiet; --progress brings the progress lines back,
        // in a format that's easy to parse.
        .args(["--print", "after_move:filepath", "--progress"])
        .args([
            "--progress-template",
            "download:progress %(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s",
        ])
        .arg("-o")
        .arg(format!("{stem}{suffix}.%(ext)s"))
        .arg("-P")
        .arg(&request.directory);
    if let Some(format) = &request.format {
        command.arg("-f").arg(format);
    }
    if let Some(ffmpeg) = std::env::var_os(FFMPEG_ENV) {
        command.arg("--ffmpeg-location").arg(ffmpeg);
    }
    command.arg("--").arg(&request.url);

    let mut child = spawn(&mut command)?;

    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_task = tokio::spawn(async move {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output).await;
        output
    });

    // stdout carries progress lines, then the final path once the file is
    // complete (after merging).
    let mut path = None;
    let mut lines = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();
    while let Some(line) = lines.next_line().await? {
        match line.strip_prefix(PROGRESS_PREFIX) {
            Some(progress) => {
                let mut values = progress.split_whitespace().map(|v| v.parse::<f64>().ok());
                if let Some(Some(downloaded)) = values.next() {
                    on_progress(Progress {
                        downloaded: downloaded as u64,
                        total: values.next().flatten().map(|total| total as u64),
                    });
                }
            }
            None if !line.trim().is_empty() => path = Some(PathBuf::from(line.trim())),
            None => {}
        }
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();
    match path {
        Some(path) if status.success() => Ok(path),
        _ => Err(failure(status, &stderr)),
    }
}
