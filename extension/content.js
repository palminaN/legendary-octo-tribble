// Shows a "Download" button over images and videos under the mouse, IDM-style.
// Clicking it asks the background script to start the download, which then
// goes through the normal interception path to the native host.

// Ignore icons, avatars, tracking pixels, etc.
const MIN_WIDTH = 120;
const MIN_HEIGHT = 80;

const host = document.createElement("div");
const shadow = host.attachShadow({ mode: "closed" });
shadow.innerHTML = `
  <style>
    button {
      position: fixed;
      z-index: 2147483647;
      display: none;
      padding: 4px 10px;
      border: none;
      border-radius: 4px;
      background: #1a73e8;
      color: #fff;
      font: 600 12px/1.4 system-ui, sans-serif;
      box-shadow: 0 1px 4px rgba(0, 0, 0, 0.4);
      cursor: pointer;
      opacity: 0.9;
    }
    button:hover { opacity: 1; }
  </style>
  <button type="button">⬇ Download</button>
`;
const button = shadow.querySelector("button");
document.documentElement.appendChild(host);

let current = null; // { element, url }

function mediaUrl(element) {
  let url;
  if (element instanceof HTMLImageElement) {
    url = element.currentSrc || element.src;
  } else {
    url =
      element.currentSrc ||
      element.src ||
      element.querySelector("source[src]")?.src;
  }
  // blob:/data: media (e.g. streaming players using MediaSource) can't be
  // fetched by the native engine.
  return url && /^https?:/i.test(url) ? url : null;
}

function findMedia(x, y) {
  // elementsFromPoint sees through overlays that video players put on top of
  // the <video> element.
  for (const element of document.elementsFromPoint(x, y)) {
    if (element === host) continue;
    if (!(element instanceof HTMLImageElement || element instanceof HTMLVideoElement)) continue;
    const rect = element.getBoundingClientRect();
    if (rect.width < MIN_WIDTH || rect.height < MIN_HEIGHT) continue;
    const url = mediaUrl(element);
    if (url) return { element, url };
  }
  return null;
}

function place() {
  if (!current || !current.element.isConnected) {
    hide();
    return;
  }
  const rect = current.element.getBoundingClientRect();
  button.style.top = `${Math.max(rect.top, 0) + 8}px`;
  button.style.left = `${rect.right - button.offsetWidth - 8}px`;
}

function hide() {
  current = null;
  button.style.display = "none";
}

let pending = false;
document.addEventListener(
  "mousemove",
  (event) => {
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      // Keep the button while hovering it, even though it covers the media.
      if (event.composedPath().includes(host)) return;
      const found = findMedia(event.clientX, event.clientY);
      if (!found) {
        hide();
        return;
      }
      if (found.element !== current?.element || found.url !== current?.url) {
        current = found;
        button.textContent = "⬇ Download";
        button.style.display = "block";
      }
      place();
    });
  },
  { passive: true }
);

document.addEventListener("scroll", place, { passive: true, capture: true });

button.addEventListener("click", (event) => {
  event.preventDefault();
  event.stopPropagation();
  if (!current) return;
  chrome.runtime.sendMessage({ type: "download-media", url: current.url });
  button.textContent = "✓ Sent";
});
