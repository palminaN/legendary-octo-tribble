// The window shown for each download: pick the destination folder, then
// follow the progress. The window owns the connection to the native host, so
// closing it (or Cancel) stops the download.

const NATIVE_HOST_NAME = "com.legendary_octo_tribble.native_host";

const id = new URLSearchParams(location.search).get("id");
const storageKey = `download:${id}`;

const $ = (selector) => document.querySelector(selector);

let request = null; // the native host request, opened by the background script
let folder = null;
let port = null;
let finished = false;
let savedPath = null;

// Speed is averaged over samples at least a second apart.
let sample = { time: 0, bytes: 0 };
let speed = null;

async function sendNative(message) {
  const response = await chrome.runtime.sendNativeMessage(NATIVE_HOST_NAME, message);
  if (response.type === "error") throw new Error(response.message);
  return response;
}

function formatBytes(bytes) {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

function setStatus(text, kind = "") {
  $("#status").textContent = text;
  $("#status").className = kind;
}

function showFolder() {
  $("#folder").value = folder || "";
  $("#folder").title = folder || "";
  $("#start").disabled = !folder;
}

async function init() {
  const stored = (await chrome.storage.session.get(storageKey))[storageKey];
  if (!stored) {
    $("#name").textContent = "Download not found";
    setStatus("This download window has expired. Start the download again.", "error");
    $("#setup").hidden = true;
    return;
  }
  request = stored.request;
  document.title = stored.label;
  $("#name").textContent = stored.label;
  $("#name").title = stored.label;
  $("#source").textContent = request.url;
  $("#source").title = request.url;

  try {
    folder = (await chrome.storage.local.get("lastFolder")).lastFolder || null;
  } catch {}
  if (!folder) {
    try {
      folder = (await sendNative({ type: "default_folder" })).path;
    } catch (err) {
      setStatus(`Download manager unavailable: ${err.message}`, "error");
    }
  }
  showFolder();
}

$("#browse").addEventListener("click", async () => {
  $("#browse").disabled = true;
  try {
    // The native host shows the system folder picker.
    const response = await sendNative({ type: "pick_folder", initial: folder });
    if (response.path) {
      folder = response.path;
      showFolder();
    }
  } catch (err) {
    setStatus(`Could not open the folder picker: ${err.message}`, "error");
  } finally {
    $("#browse").disabled = false;
  }
});

$("#start").addEventListener("click", () => {
  chrome.storage.local.set({ lastFolder: folder }).catch(() => {});
  chrome.storage.session.remove(storageKey).catch(() => {});

  $("#setup").hidden = true;
  $("#start").hidden = true;
  $("#transfer").hidden = false;
  setStatus("Starting…");

  port = chrome.runtime.connectNative(NATIVE_HOST_NAME);
  port.onMessage.addListener(onHostMessage);
  port.onDisconnect.addListener((p) => {
    if (finished) return;
    const reason = p.error?.message || chrome.runtime.lastError?.message;
    fail(reason ? `Download manager stopped: ${reason}` : "Download manager stopped unexpectedly");
  });
  port.postMessage({ ...request, directory: folder });
});

function onHostMessage(message) {
  switch (message.type) {
    case "progress":
      showProgress(message.downloaded, message.total);
      break;
    case "done":
      finished = true;
      savedPath = message.path;
      $("#bar").max = 1;
      $("#bar").value = 1;
      $("#speed").textContent = "";
      setStatus(`Saved to ${message.path}`, "done");
      $("#reveal").hidden = false;
      $("#cancel").textContent = "Close";
      port.disconnect();
      break;
    case "error":
      fail(message.message);
      break;
  }
}

function showProgress(downloaded, total) {
  const now = performance.now();
  // yt-dlp downloads video and audio one after the other: a drop means the
  // next track started.
  if (downloaded < sample.bytes) sample = { time: now, bytes: downloaded };
  if (now - sample.time >= 1000) {
    if (sample.time) speed = ((downloaded - sample.bytes) * 1000) / (now - sample.time);
    sample = { time: now, bytes: downloaded };
  }

  if (total) {
    $("#bar").max = total;
    $("#bar").value = downloaded;
    $("#amount").textContent = `${formatBytes(downloaded)} of ${formatBytes(total)} · ${Math.floor((downloaded / total) * 100)}%`;
  } else {
    // Streams have no known size: an indeterminate bar.
    $("#bar").removeAttribute("value");
    $("#amount").textContent = formatBytes(downloaded);
  }
  $("#speed").textContent = speed !== null ? `${formatBytes(speed)}/s` : "";
  setStatus("Downloading…");
}

function fail(message) {
  finished = true;
  $("#transfer").hidden = true;
  setStatus(`Error: ${message}`, "error");
  $("#cancel").textContent = "Close";
  port?.disconnect();
}

$("#reveal").addEventListener("click", () => {
  sendNative({ type: "reveal", path: savedPath }).catch((err) =>
    setStatus(`Could not open the folder: ${err.message}`, "error")
  );
});

// Cancel/Close: closing the window disconnects the port, which makes the
// browser stop the native host (and the host's job object its children).
$("#cancel").addEventListener("click", () => window.close());

window.addEventListener("pagehide", () => {
  chrome.storage.session.remove(storageKey).catch(() => {});
});

init();
