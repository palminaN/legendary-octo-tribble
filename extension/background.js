// Must match the "name" field in the installed native messaging manifest
// (see native-messaging-manifests/).
const NATIVE_HOST_NAME = "com.legendary_octo_tribble.native_host";

let port = null;

function connect() {
  port = chrome.runtime.connectNative(NATIVE_HOST_NAME);
  port.onMessage.addListener(onNativeMessage);
  port.onDisconnect.addListener(() => {
    port = null;
  });
}

function onNativeMessage(message) {
  // TODO: forward progress/done/error events to the popup UI.
  console.log("native host:", message);
}

// The content script's overlay button asks for a media download. Starting it
// as a regular browser download lets the onCreated listener below pick it up,
// so the destination path is resolved the same way as for any other download.
chrome.runtime.onMessage.addListener((message) => {
  if (message.type === "download-media") {
    chrome.downloads.download({ url: message.url });
  }
});

// Intercept browser downloads and hand them to the native engine instead.
chrome.downloads.onCreated.addListener((downloadItem) => {
  // blob:/data:/file: URLs only exist inside the browser (e.g. saving from the
  // built-in PDF viewer), so the native engine can't fetch them — let the
  // browser handle those itself.
  if (!/^https?:/i.test(downloadItem.url)) return;

  if (!port) connect();

  chrome.downloads.cancel(downloadItem.id);
  chrome.downloads.erase({ id: downloadItem.id });

  port.postMessage({
    type: "download",
    url: downloadItem.url,
    destination: downloadItem.filename,
  });
});
