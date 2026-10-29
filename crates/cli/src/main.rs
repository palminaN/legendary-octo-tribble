use std::path::PathBuf;

use clap::Parser;
use dm_core::{download, DownloadRequest};

/// Minimal CLI frontend for the download engine.
#[derive(Parser)]
struct Args {
    url: String,
    #[arg(short, long)]
    output: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let request = DownloadRequest {
        url: args.url,
        destination: args.output,
    };

    download(&request, |progress| {
        match progress.total {
            Some(total) => print!("\r{} / {} bytes", progress.downloaded, total),
            None => print!("\r{} bytes", progress.downloaded),
        }
    })
    .await?;

    println!("\ndone");
    Ok(())
}
