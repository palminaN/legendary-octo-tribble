# legendary-octo-tribble

Download manager written in rust + browser extensions

## Structure

```
crates/
  core/         dm-core   — download engine (reqwest + tokio), used by both frontends
  cli/          dm        — CLI frontend
  native-host/  dm-native-host — native messaging host, bridges the browser extension to dm-core
extension/                — WebExtension (MV3): intercepts browser downloads, forwards them to the native host
```

The browser extension itself must be JS/HTML (WebExtensions API) — browsers don't run native
code inside the extension sandbox. It talks to `dm-native-host` over stdio using the
[Native Messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)
protocol; that binary links `dm-core` directly to run the actual download.

## Building

Requires a Rust toolchain ([rustup.rs](https://rustup.rs)) — not installed on this machine yet.

```
cargo build --workspace
```

## Trying the extension locally

1. `cargo build --release` and note the path to `dm-native-host(.exe)`.
2. Load `extension/` as an unpacked extension in Chrome/Edge (`chrome://extensions` → Developer
   mode → Load unpacked) and copy its generated extension ID.
3. Fill in `extension/native-messaging-manifests/chrome.json.example` with that ID and the
   binary's path, then register it with the browser:
   - **Windows/Chrome**: add a registry value at
     `HKEY_CURRENT_USER\Software\Google\Chrome\NativeMessagingHosts\com.legendary_octo_tribble.native_host`
     pointing to the manifest file.
   - **Firefox**: the add-on ID is fixed (`dm@legendary-octo-tribble`, set in `manifest.json`), so
     only the binary path needs filling in `firefox.json.example`. Save it anywhere (e.g.
     `%APPDATA%\Mozilla\NativeMessagingHosts\com.legendary_octo_tribble.native_host.json`), then on
     Windows add a registry key at
     `HKEY_CURRENT_USER\Software\Mozilla\NativeMessagingHosts\com.legendary_octo_tribble.native_host`
     whose default value is the manifest's path — Firefox on Windows only looks in the registry.
     Load the extension via `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** →
     pick `extension/manifest.json`.

## Download window

Every download (intercepted browser downloads, the overlay button, the popup) opens a small
window to choose the destination folder (the system folder picker, via the native host; the last
folder is remembered), then shows a progress bar, size and speed, and any error. Closing the
window or pressing Cancel stops the download: the native host exits as soon as the browser closes
its connection, and on Windows a job object takes ffmpeg/yt-dlp down with it. The partially
downloaded file is left behind.

## Streaming videos

The extension watches network requests (`webRequest`) for HLS (`.m3u8`), DASH (`.mpd`) and
direct video files loaded by players, and lists them in the toolbar popup (the badge shows how
many were found). The overlay button on a streaming player downloads the best one detected.
Cookie/Referer/User-Agent headers are captured and replayed so session-protected streams work.

Stream downloads are delegated to [ffmpeg](https://ffmpeg.org), which handles playlist parsing,
AES-128 decryption and audio/video muxing. It must be on `PATH`, or set `DM_FFMPEG` to its
path. Files go to the user's Downloads folder, named after the page title.

### Video sites (YouTube…)

YouTube never loads a plain manifest, so on YouTube pages (and embedded YouTube players) the
extension sends the *page* URL to [yt-dlp](https://github.com/yt-dlp/yt-dlp) instead. yt-dlp is
also the fallback when nothing was detected on a page, since it supports many other sites
(Vimeo, Twitch, Dailymotion…). Install it, plus [Deno](https://deno.com), which yt-dlp needs to
solve YouTube's player challenges:

```
winget install yt-dlp.yt-dlp DenoLand.Deno
```

Clicking the overlay button on a player opens a quality menu (resolutions with approximate
sizes, plus audio only), listed by yt-dlp for both video sites and detected HLS/DASH streams.
Without yt-dlp, detected streams still offer "Best quality" through ffmpeg.

yt-dlp must be on `PATH`, or set `DM_YTDLP` to its path. Keep it up to date (`yt-dlp -U`):
YouTube changes regularly and old versions stop working.

Not supported: DRM-protected streams (Netflix, Disney+…).

## Roadmap (not implemented yet)


- Parallel chunked downloads with `Range` requests and resume after interruption
- Persistent download queue (survive app restarts)
- Speed limiting / scheduling
- GUI frontend (Tauri or egui — undecided)
