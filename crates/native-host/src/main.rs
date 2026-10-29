//! Native Messaging host: the bridge between the browser extension and the
//! Rust download engine. Chrome/Firefox launch this binary as a subprocess
//! and exchange length-prefixed JSON messages over stdin/stdout.
//!
//! See https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging
//! and extension/native-messaging-manifests/ for the manifest templates that
//! register this binary with the browser.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Ping,
    Download { url: String, destination: PathBuf },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response<'a> {
    Pong,
    Progress { downloaded: u64, total: Option<u64> },
    Done,
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
            Request::Download { url, destination } => {
                let req = dm_core::DownloadRequest { url, destination };
                let result = runtime.block_on(dm_core::download(&req, |progress| {
                    let _ = write_message(
                        &mut io::stdout().lock(),
                        &Response::Progress {
                            downloaded: progress.downloaded,
                            total: progress.total,
                        },
                    );
                }));

                match result {
                    Ok(()) => write_message(&mut out, &Response::Done)?,
                    Err(err) => write_message(
                        &mut out,
                        &Response::Error {
                            message: &err.to_string(),
                        },
                    )?,
                }
            }
        }
    }

    Ok(())
}
