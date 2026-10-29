//! Native Messaging host: the bridge between the browser extension and the
//! Rust download engine. Chrome/Firefox launch this binary as a subprocess
//! and exchange length-prefixed JSON messages over stdin/stdout.
//!
//! See https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging
//! and extension/native-messaging-manifests/ for the manifest templates that
//! register this binary with the browser.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use dm_core::{DownloadError, DownloadRequest, FormatOption, Progress, SiteRequest};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Ping,
    /// A regular file. `destination` is set for downloads intercepted from the
    /// browser (it already picked a path); otherwise the file is saved as
    /// `filename` in the user's Downloads folder.
    Download {
        url: String,
        destination: Option<PathBuf>,
        filename: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
    /// An HLS/DASH manifest, downloaded through ffmpeg into the Downloads folder.
    Stream {
        url: String,
        filename: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
    /// A page on a video site (YouTube…) or a manifest, downloaded through
    /// yt-dlp into the Downloads folder. `format` comes from a `formats`
    /// response; `title` names the file (defaults to the video's title).
    Site {
        url: String,
        format: Option<String>,
        title: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
    /// Lists the qualities yt-dlp can download for a page or manifest.
    Formats {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response<'a> {
    Pong,
    Progress { downloaded: u64, total: Option<u64> },
    Done { path: &'a Path },
    Formats { options: &'a [FormatOption] },
    Error { message: &'a str },
}

fn read_message(stdin: &mut impl Read) -> io::Result<Option<Request>> {
    let mut len_buf = [0u8; 4];
    if let Err(err) = stdin.read_exact(&mut len_buf) {
        if err.kind() == io::ErrorKind::UnexpectedEof {
            return Ok(None);
        }
        return Err(err);
    }
    let len = u32::from_ne_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stdin.read_exact(&mut payload)?;

    let request = serde_json::from_slice(&payload)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(Some(request))
}

fn write_message(stdout: &mut impl Write, response: &Response) -> io::Result<()> {
    let payload = serde_json::to_vec(response)?;
    stdout.write_all(&(payload.len() as u32).to_ne_bytes())?;
    stdout.write_all(&payload)?;
    stdout.flush()
}

fn report_progress(progress: Progress) {
    let _ = write_message(
        &mut io::stdout().lock(),
        &Response::Progress {
            downloaded: progress.downloaded,
            total: progress.total,
        },
    );
}

/// Reports the end of a download: the finished file's path, or the error.
fn write_result(out: &mut impl Write, result: Result<&Path, &DownloadError>) -> io::Result<()> {
    match result {
        Ok(path) => write_message(out, &Response::Done { path }),
        Err(err) => write_message(
            out,
            &Response::Error {
                message: &err.to_string(),
            },
        ),
    }
}

fn download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Picks a free path in the user's Downloads folder, so an existing file is
/// never overwritten.
fn default_destination(filename: Option<&str>) -> PathBuf {
    let dir = download_dir();
    let name = sanitize_filename(filename.unwrap_or("download"));
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name.as_str(), String::new()),
    };

    let mut candidate = dir.join(&name);
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}){extension}"));
        n += 1;
    }
    candidate
}

/// Filenames come from page titles, so strip what Windows rejects.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        "download".to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn main() -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start tokio runtime");

    let mut stdin = io::stdin().lock();
    let stdout = io::stdout();

    while let Some(request) = read_message(&mut stdin)? {
        let mut out = stdout.lock();
        match request {
            Request::Ping => write_message(&mut out, &Response::Pong)?,
            Request::Download { url, .. }
            | Request::Stream { url, .. }
            | Request::Site { url, .. }
            | Request::Formats { url, .. }
                if !is_http(&url) =>
            {
                write_message(
                    &mut out,
                    &Response::Error {
                        message: "only http(s) URLs can be downloaded",
                    },
                )?
            }
            Request::Download {
                url,
                destination,
                filename,
                headers,
            } => {
                let destination =
                    destination.unwrap_or_else(|| default_destination(filename.as_deref()));
                let req = DownloadRequest {
                    url,
                    destination,
                    headers: headers.into_iter().collect(),
                };
                let result = runtime.block_on(dm_core::download(&req, report_progress));
                write_result(
                    &mut out,
                    result.as_ref().map(|()| req.destination.as_path()),
                )?;
            }
            Request::Stream {
                url,
                filename,
                headers,
            } => {
                let req = DownloadRequest {
                    url,
                    destination: default_destination(Some(
                        filename.as_deref().unwrap_or("video.mp4"),
                    )),
                    headers: headers.into_iter().collect(),
                };
                let result = runtime.block_on(dm_core::download_stream(&req, report_progress));
                write_result(
                    &mut out,
                    result.as_ref().map(|()| req.destination.as_path()),
                )?;
            }
            Request::Site {
                url,
                format,
                title,
                headers,
            } => {
                let req = SiteRequest {
                    url,
                    directory: download_dir(),
                    format,
                    // Page titles can be long; leave room for the quality suffix.
                    title: title.map(|title| {
                        sanitize_filename(&title.chars().take(120).collect::<String>())
                    }),
                    headers: headers.into_iter().collect(),
                };
                let result = runtime.block_on(dm_core::download_site(&req, report_progress));
                write_result(&mut out, result.as_deref())?;
            }
            Request::Formats { url, headers } => {
                let headers: Vec<_> = headers.into_iter().collect();
                match runtime.block_on(dm_core::list_formats(&url, &headers)) {
                    Ok(options) => {
                        write_message(&mut out, &Response::Formats { options: &options })?
                    }
                    Err(err) => write_result(&mut out, Err(&err))?,
                }
            }
        }
    }

    Ok(())
}
