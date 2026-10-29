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

## Roadmap (not implemented yet)

- Parallel chunked downloads with `Range` requests and resume after interruption
- Persistent download queue (survive app restarts)
- Speed limiting / scheduling
- GUI frontend (Tauri or egui — undecided)
