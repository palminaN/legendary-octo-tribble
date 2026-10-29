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
    // Downloads are saved in `directory`, or the user's Downloads folder when
    // it's missing.
    /// A regular file, saved as `filename`.
    Download {
        url: String,
        directory: Option<PathBuf>,
        filename: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
    /// An HLS/DASH manifest, downloaded through ffmpeg.
    Stream {
        url: String,
        directory: Option<PathBuf>,
        filename: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
    /// A page on a video site (YouTube…) or a manifest, downloaded through
    /// yt-dlp. `format` comes from a `formats` response; `title` names the
    /// file (defaults to the video's title).
    Site {
        url: String,
        directory: Option<PathBuf>,
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
    /// The folder downloads go to when none is chosen.
    DefaultFolder,
    /// Shows the system folder picker, starting in `initial`.
    PickFolder {
        initial: Option<PathBuf>,
    },
    /// Opens the file manager on a downloaded file.
    Reveal {
        path: PathBuf,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response<'a> {
    Pong,
    Progress {
        downloaded: u64,
        total: Option<u64>,
    },
    Done {
        path: &'a Path,
    },
    Formats {
        options: &'a [FormatOption],
    },
    /// `path` is `None` when the folder picker was cancelled.
    Folder {
        path: Option<&'a Path>,
    },
    Ok,
    Error {
        message: &'a str,
    },
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

/// Picks a free path in `directory` (by default the user's Downloads
/// folder), so an existing file is never overwritten.
fn destination_in(directory: Option<PathBuf>, filename: Option<&str>) -> PathBuf {
    let dir = directory.unwrap_or_else(download_dir);
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

/// Makes the processes this host starts (ffmpeg, yt-dlp and their own
/// children) end with it. The browser kills the host when the extension
/// cancels a download by closing its port; without this they would keep
/// downloading in the background.
#[cfg(windows)]
fn end_children_with_host() {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: plain Win32 calls on a handle we own and on the current-process
    // pseudo-handle. The job handle is deliberately never closed: the system
    // closes it when this process exits, which is what ends the processes in
    // the job. Failures (e.g. a browser job that forbids nesting) are ignored:
    // the download still works, it just isn't cleaned up on cancel.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        AssignProcessToJobObject(job, GetCurrentProcess());
    }
}

#[cfg(not(windows))]
fn end_children_with_host() {}

/// A download is the last request on its connection: from then on stdin is
/// only watched for the browser closing it, which is how the extension
/// cancels. Exit right away then, rather than when the browser kills the host
/// a few seconds later, taking ffmpeg/yt-dlp down too. The partially
/// downloaded file is left behind.
fn cancel_on_disconnect() {
    end_children_with_host();
    std::thread::spawn(|| {
        let mut buf = [0u8; 256];
        loop {
            match io::stdin().read(&mut buf) {
                Ok(0) | Err(_) => std::process::exit(1),
                Ok(_) => {}
            }
        }
    });
}

fn reveal(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "file not found"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer expects `/select,"path"` as one argument, quoted its own
        // way; paths can't contain quotes on Windows.
        std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()?;
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(path.parent().unwrap_or(path))
            .spawn()?;
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start tokio runtime");

    // Not locked for the whole loop: the cancel watcher reads it too.
    let mut stdin = io::stdin();
    let stdout = io::stdout();

    while let Some(request) = read_message(&mut stdin)? {
        let is_download = matches!(
            request,
            Request::Download { .. } | Request::Stream { .. } | Request::Site { .. }
        );
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
                directory,
                filename,
                headers,
            } => {
                cancel_on_disconnect();
                let req = DownloadRequest {
                    url,
                    destination: destination_in(directory, filename.as_deref()),
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
                directory,
                filename,
                headers,
            } => {
                cancel_on_disconnect();
                let req = DownloadRequest {
                    url,
                    destination: destination_in(
                        directory,
                        Some(filename.as_deref().unwrap_or("video.mp4")),
                    ),
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
                directory,
                format,
                title,
                headers,
            } => {
                cancel_on_disconnect();
                let req = SiteRequest {
                    url,
                    directory: directory.unwrap_or_else(download_dir),
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
                end_children_with_host();
                let headers: Vec<_> = headers.into_iter().collect();
                match runtime.block_on(dm_core::list_formats(&url, &headers)) {
                    Ok(options) => {
                        write_message(&mut out, &Response::Formats { options: &options })?
                    }
                    Err(err) => write_result(&mut out, Err(&err))?,
                }
            }
            Request::DefaultFolder => write_message(
                &mut out,
                &Response::Folder {
                    path: Some(&download_dir()),
                },
            )?,
            Request::PickFolder { initial } => {
                let mut dialog = rfd::FileDialog::new().set_title("Save downloads to");
                if let Some(initial) = initial.filter(|dir| dir.is_dir()) {
                    dialog = dialog.set_directory(initial);
                }
                let picked = dialog.pick_folder();
                write_message(
                    &mut out,
                    &Response::Folder {
                        path: picked.as_deref(),
                    },
                )?
            }
            Request::Reveal { path } => match reveal(&path) {
                Ok(()) => write_message(&mut out, &Response::Ok)?,
                Err(err) => write_message(
                    &mut out,
                    &Response::Error {
                        message: &err.to_string(),
                    },
                )?,
            },
        }
        if is_download {
            break;
        }
    }

    Ok(())
}
