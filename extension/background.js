// Must match the "name" field in the installed native messaging manifest
// (see native-messaging-manifests/).
const NATIVE_HOST_NAME = "com.legendary_octo_tribble.native_host";

// Request headers worth replaying from the native engine: stream servers often
// check the session cookie, the referring page or the browser's User-Agent.
const FORWARDED_HEADERS = ["cookie", "referer", "origin", "user-agent", "authorization"];

// Smaller "videos" are almost always segments of a stream, previews or ads.
const MIN_VIDEO_SIZE = 1024 * 1024;

// The native host handles one request at a time, so each download gets its
// own port (and therefore its own host process) to run in parallel.
function sendToHost(message) {
  const port = chrome.runtime.connectNative(NATIVE_HOST_NAME);
  port.onMessage.addListener((response) => {
    // TODO: forward progress/done/error events to the popup UI.
    console.log("native host:", message.url, response);
    if (response.type === "done" || response.type === "error") port.disconnect();
  });
  port.postMessage(message);
}

// --- Media detection -------------------------------------------------------
//
// Detected media is kept per tab in storage.session: the background script is
// an event page / service worker and loses in-memory state when suspended.

const requestHeaders = new Map(); // requestId -> forwarded headers
let storageQueue = Promise.resolve();

function storageKey(tabId) {
  return `media:${tabId}`;
}

async function loadTabMedia(tabId) {
  const key = storageKey(tabId);
  const stored = await chrome.storage.session.get(key);
  return stored[key] || { items: [], variants: [] };
}

// Serializes read-modify-write cycles so concurrent requests don't drop items.
function updateTabMedia(tabId, update) {
  storageQueue = storageQueue.then(async () => {
    const media = await loadTabMedia(tabId);
    if (update(media) === false) return;
    await chrome.storage.session.set({ [storageKey(tabId)]: media });
    const count = visibleItems(media).length;
    setBadge(tabId, count ? String(count) : "");
  });
  return storageQueue.catch((err) => console.error("media storage:", err));
}

function clearTabMedia(tabId) {
  chrome.storage.session.remove(storageKey(tabId));
  setBadge(tabId, "");
}

function setBadge(tabId, text) {
  // Rejects when the tab has just closed (e.g. from tabs.onRemoved); harmless.
  chrome.action.setBadgeText({ tabId, text }).catch(() => {});
}

function header(headers, name) {
  return headers?.find((h) => h.name.toLowerCase() === name)?.value;
}

function classify(details) {
  const contentType = (header(details.responseHeaders, "content-type") || "")
    .split(";")[0]
    .trim()
    .toLowerCase();
  const url = new URL(details.url);
  const path = url.pathname.toLowerCase();

  if (path.endsWith(".m3u8") || contentType.includes("mpegurl")) return "hls";
  if (path.endsWith(".mpd") || contentType === "application/dash+xml") return "dash";

  const isVideo =
    /^video\/(mp4|webm|x-matroska|quicktime|ogg)$/.test(contentType) ||
    (details.type === "media" && /\.(mp4|webm|mkv|mov)$/.test(path));
  // googlevideo.com (YouTube) and `range=` URLs serve fragments of a file,
  // not the whole video.
  if (isVideo && !url.hostname.endsWith("googlevideo.com") && !url.searchParams.has("range")) {
    return "video";
  }
  return null;
}

function responseSize(details) {
  // A 206 response's Content-Length is the chunk; Content-Range has the total.
  const range = header(details.responseHeaders, "content-range")?.match(/\/(\d+)$/);
  if (range) return Number(range[1]);
  const length = header(details.responseHeaders, "content-length");
  return length ? Number(length) : null;
}

// Master playlists list their variants (one per quality). Fetching the master
// lets us hide the variant playlists the player loads afterwards, and label
// the master with its best resolution.
async function inspectHlsPlaylist(item) {
  try {
    const response = await fetch(item.url, { credentials: "include" });
    const text = await response.text();
    if (!text.includes("#EXT-X-STREAM-INF")) return null;

    const variants = [];
    let bestHeight = 0;
    const lines = text.split(/\r?\n/);
    lines.forEach((line, i) => {
      if (line.startsWith("#EXT-X-STREAM-INF")) {
        const height = Number(line.match(/RESOLUTION=\d+x(\d+)/)?.[1] || 0);
        bestHeight = Math.max(bestHeight, height);
        const uri = lines.slice(i + 1).find((l) => l && !l.startsWith("#"));
        if (uri) variants.push(new URL(uri, item.url).href);
      }
      const mediaUri = line.startsWith("#EXT-X-MEDIA") && line.match(/URI="([^"]+)"/)?.[1];
      if (mediaUri) variants.push(new URL(mediaUri, item.url).href);
    });
    return { variants, label: bestHeight ? `up to ${bestHeight}p` : null };
  } catch (err) {
    console.warn("could not inspect playlist", item.url, err);
    return null;
  }
}

// Items worth showing: once a page is known to stream, loose video files are
// almost always its segments, and variant playlists duplicate their master.
function visibleItems(media) {
  const hasManifest = media.items.some((item) => item.kind !== "video");
  return media.items.filter(
    (item) =>
      !media.variants.includes(item.url) &&
      !(hasManifest && item.kind === "video")
  );
}

const KIND_RANK = { hls: 0, dash: 1, video: 2 };

function bestItem(media) {
  return visibleItems(media).sort(
    (a, b) =>
      KIND_RANK[a.kind] - KIND_RANK[b.kind] ||
      Number(b.master) - Number(a.master) ||
      (b.size || 0) - (a.size || 0) ||
      b.detectedAt - a.detectedAt
  )[0];
}

const WATCHED = { urls: ["<all_urls>"], types: ["xmlhttprequest", "media", "other"] };

function addSendHeadersListener(extraInfoSpec) {
  chrome.webRequest.onSendHeaders.addListener(
    (details) => {
      if (details.tabId < 0) return;
      const headers = {};
      for (const { name, value } of details.requestHeaders || []) {
        if (FORWARDED_HEADERS.includes(name.toLowerCase())) headers[name] = value;
      }
      requestHeaders.set(details.requestId, headers);
    },
    WATCHED,
    extraInfoSpec
  );
}

function watchNetwork() {
  try {
    // Chrome only exposes Cookie/Referer with "extraHeaders"; Firefox rejects it.
    addSendHeadersListener(["requestHeaders", "extraHeaders"]);
  } catch {
    addSendHeadersListener(["requestHeaders"]);
  }

  chrome.webRequest.onHeadersReceived.addListener(
    (details) => {
      const headers = requestHeaders.get(details.requestId) || {};
      requestHeaders.delete(details.requestId);
      // tabId is -1 for requests not made by a page (including our own fetch).
      if (details.tabId < 0 || details.statusCode < 200 || details.statusCode >= 300) return;

      const kind = classify(details);
      if (!kind) return;
      const size = responseSize(details);
      if (kind === "video" && size !== null && size < MIN_VIDEO_SIZE) return;

      const item = {
        url: details.url,
        kind,
        size,
        headers,
        master: false,
        label: null,
        detectedAt: Date.now(),
      };
      // Live streams reload their playlists every few seconds; only inspect a
      // playlist the first time it shows up.
      let added = false;
      updateTabMedia(details.tabId, (media) => {
        if (media.items.some((existing) => existing.url === item.url)) return false;
        media.items.push(item);
        added = true;
        console.log(`detected ${kind} in tab ${details.tabId}:`, item.url);
      }).then(async () => {
        if (!added || kind !== "hls") return;
        const playlist = await inspectHlsPlaylist(item);
        if (!playlist) return;
        updateTabMedia(details.tabId, (media) => {
          const stored = media.items.find((existing) => existing.url === item.url);
          if (stored) {
            stored.master = true;
            stored.label = playlist.label;
          }
          media.variants.push(...playlist.variants);
        });
      });
    },
    WATCHED,
    ["responseHeaders"]
  );

  // Forget a tab's media when it navigates to a new page or closes.
  chrome.webRequest.onBeforeRequest.addListener(
    (details) => {
      if (details.tabId >= 0) clearTabMedia(details.tabId);
    },
    { urls: ["<all_urls>"], types: ["main_frame"] }
  );
}

// Without the webRequest permission (e.g. a temporary add-on reloaded from
// files but not its manifest) skip detection, but keep download
// interception and the overlay buttons working.
if (chrome.webRequest) {
  watchNetwork();
} else {
  console.error("webRequest is unavailable: reload the add-on so its manifest permissions apply");
}

chrome.tabs.onRemoved.addListener(clearTabMedia);

function fileNameFor(item, title) {
  const base = (title || "video").slice(0, 120);
  if (item.kind !== "video") return `${base}.mp4`;
  const last = decodeURIComponent(new URL(item.url).pathname.split("/").pop() || "");
  return /\.\w{2,4}$/.test(last) ? last : `${base}.mp4`;
}

async function downloadItem(tabId, item) {
  const tab = await chrome.tabs.get(tabId);
  sendToHost({
    type: item.kind === "video" ? "download" : "stream",
    url: item.url,
    filename: fileNameFor(item, tab.title),
    headers: item.headers,
  });
}

// --- Video sites (yt-dlp) ---------------------------------------------------
//
// Some sites never load a plain manifest (YouTube streams signed fragments),
// so their page URL is handed to yt-dlp instead, which knows how to extract
// the video. yt-dlp is also the fallback for any page where nothing was
// detected: it supports many other sites.

const SITE_PAGES = [
  /^https:\/\/(www\.|m\.|music\.)?youtube\.com\/(watch|shorts\/|embed\/|live\/)/,
  /^https:\/\/(www\.)?youtube-nocookie\.com\/embed\//,
  /^https:\/\/youtu\.be\//,
];

function isSitePage(url) {
  return SITE_PAGES.some((pattern) => pattern.test(url || ""));
}

function downloadSite(url) {
  sendToHost({ type: "site", url });
}

// --- Quality menu -----------------------------------------------------------

const BEST_QUALITY = { label: "Best quality", selector: null, size: null };

// What the overlay button on a streaming player downloads: the page itself on
// video sites, else the best stream detected in the tab, else the page as a
// yt-dlp fallback.
async function playerTarget(sender) {
  const tabId = sender.tab.id;
  // The player's own page: the tab for the top frame (its URL follows
  // in-page navigation, e.g. between YouTube videos), the frame for an
  // embedded player.
  const pageUrl = sender.frameId === 0 ? sender.tab.url : sender.url;
  if (!isSitePage(pageUrl)) {
    const item = bestItem(await loadTabMedia(tabId));
    if (item) return { tabId, item };
  }
  return { tabId, item: { kind: "site", url: pageUrl, headers: {} } };
}

async function listFormats(target) {
  // A plain file has a single quality.
  if (target.item.kind === "video") return { target, options: [BEST_QUALITY] };
  try {
    const response = await chrome.runtime.sendNativeMessage(NATIVE_HOST_NAME, {
      type: "formats",
      url: target.item.url,
      headers: target.item.headers,
    });
    if (response.type === "formats") return { target, options: response.options };
    return { target, options: [BEST_QUALITY], error: response.message };
  } catch (err) {
    return { target, options: [BEST_QUALITY], error: String(err.message || err) };
  }
}

async function downloadFormat(target, selector) {
  const { tabId, item } = target;
  if (item.kind === "site") {
    sendToHost({ type: "site", url: item.url, format: selector });
  } else if (item.kind === "video" || selector === null) {
    // The best quality of a detected stream goes through ffmpeg, which
    // doesn't need yt-dlp.
    downloadItem(tabId, item);
  } else {
    // A specific quality of a detected stream: yt-dlp reads the manifest.
    const tab = await chrome.tabs.get(tabId);
    sendToHost({
      type: "site",
      url: item.url,
      format: selector,
      title: tab.title,
      headers: item.headers,
    });
  }
}

// --- Messages from the content script and the popup -----------------------

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  switch (message.type) {
    // The overlay button on an image/video with a plain http(s) source.
    // Starting it as a regular browser download lets the onCreated listener
    // below pick it up, so the destination path is resolved the same way as
    // for any other download.
    case "download-media":
      chrome.downloads.download({ url: message.url });
      return false;

    // The overlay button on a streaming player (blob: source) opens a menu
    // of the qualities available for what it would download…
    case "list-formats":
      playerTarget(sender).then(listFormats).then(sendResponse);
      return true; // sendResponse is called asynchronously

    // …and this starts the one picked. `target` comes back from list-formats.
    case "download-format":
      downloadFormat(message.target, message.selector);
      return false;

    case "list-media":
      Promise.all([loadTabMedia(message.tabId), chrome.tabs.get(message.tabId)]).then(
        ([media, tab]) => {
          const items = visibleItems(media);
          const isHttp = /^https?:/.test(tab.url || "");
          if (isSitePage(tab.url) || (items.length === 0 && isHttp)) {
            items.unshift({ kind: "site", url: tab.url, title: tab.title, label: "via yt-dlp" });
          }
          sendResponse(items);
        }
      );
      return true;

    case "download-item":
      if (message.kind === "site") {
        downloadSite(message.url);
        return false;
      }
      loadTabMedia(message.tabId).then((media) => {
        const item = media.items.find((existing) => existing.url === message.url);
        if (item) downloadItem(message.tabId, item);
      });
      return false;
  }
  return false;
});

// --- Browser download interception -----------------------------------------

chrome.downloads.onCreated.addListener((downloadItem) => {
  // blob:/data:/file: URLs only exist inside the browser (e.g. saving from the
  // built-in PDF viewer), so the native engine can't fetch them — let the
  // browser handle those itself.
  if (!/^https?:/i.test(downloadItem.url)) return;

  chrome.downloads.cancel(downloadItem.id);
  chrome.downloads.erase({ id: downloadItem.id });

  sendToHost({
    type: "download",
    url: downloadItem.url,
    destination: downloadItem.filename,
  });
});
