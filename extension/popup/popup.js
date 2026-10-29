const NATIVE_HOST_NAME = "com.legendary_octo_tribble.native_host";

chrome.runtime.sendNativeMessage(NATIVE_HOST_NAME, { type: "ping" }, (response) => {
  const status = document.getElementById("status");
  if (chrome.runtime.lastError || !response) {
    status.textContent = "native host unreachable";
    return;
  }
  status.textContent = "connected: " + response.type;
});

// Firefox treats MV3 host permissions as opt-in: without them webRequest
// silently sees nothing, so offer to grant them (requires a user click).
const ALL_SITES = { origins: ["<all_urls>"] };
chrome.permissions.contains(ALL_SITES, (granted) => {
  document.getElementById("permission").hidden = granted;
});
document.getElementById("grant").addEventListener("click", () => {
  chrome.permissions.request(ALL_SITES, (granted) => {
    document.getElementById("permission").hidden = granted;
  });
});

function describe(item) {
  if (item.label) return item.label;
  if (item.size) return `${(item.size / (1024 * 1024)).toFixed(1)} MB`;
  return "";
}

function render(tabId, items) {
  const list = document.getElementById("media");
  document.getElementById("empty").hidden = items.length > 0;

  for (const item of items) {
    const row = document.createElement("li");

    const kind = document.createElement("span");
    kind.className = "kind";
    kind.textContent = item.kind.toUpperCase();

    const name = document.createElement("span");
    name.className = "name";
    name.title = item.url;
    const url = new URL(item.url);
    name.textContent = item.title || url.pathname.split("/").pop() || url.hostname;
    const detail = describe(item);
    if (detail) {
      const extra = document.createElement("span");
      extra.className = "detail";
      extra.textContent = ` · ${detail}`;
      name.append(extra);
    }

    const button = document.createElement("button");
    button.textContent = "Download";
    button.addEventListener("click", () => {
      chrome.runtime.sendMessage({ type: "download-item", tabId, url: item.url, kind: item.kind });
      button.textContent = "✓ Sent";
      button.disabled = true;
    });

    row.append(kind, name, button);
    list.append(row);
  }
}

chrome.tabs.query({ active: true, currentWindow: true }, ([tab]) => {
  chrome.runtime.sendMessage({ type: "list-media", tabId: tab.id }, (items) => {
    render(tab.id, items || []);
  });
});
