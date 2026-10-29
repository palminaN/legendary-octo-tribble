// Shows a "Download" button over images and videos under the mouse, IDM-style.
// For an image or a plain video file, clicking it asks the background script
// to start the download, which then goes through the normal interception path
// to the native host. For a streaming player, it opens a menu of the
// available qualities.

// Ignore icons, avatars, tracking pixels, etc.
const MIN_WIDTH = 120;
const MIN_HEIGHT = 80;

const host = document.createElement("div");
const shadow = host.attachShadow({ mode: "closed" });
shadow.innerHTML = `
  <style>
    #download, .menu {
      position: fixed;
      z-index: 2147483647;
      display: none;
      font: 600 12px/1.4 system-ui, sans-serif;
      box-shadow: 0 1px 4px rgba(0, 0, 0, 0.4);
    }
    #download {
      padding: 4px 10px;
      border: none;
      border-radius: 4px;
      background: #1a73e8;
      color: #fff;
      cursor: pointer;
      opacity: 0.9;
    }
    #download:hover { opacity: 1; }
    .menu {
      min-width: 180px;
      max-width: 280px;
      max-height: 320px;
      overflow-y: auto;
      padding: 4px 0;
      border-radius: 6px;
      background: #202124;
      color: #fff;
    }
    .menu button {
      display: flex;
      justify-content: space-between;
      gap: 16px;
      width: 100%;
      padding: 6px 12px;
      border: none;
      background: none;
      color: inherit;
      font: inherit;
      text-align: left;
      cursor: pointer;
    }
    .menu button:hover { background: #1a73e8; }
    .size { color: #bdc1c6; font-weight: 400; }
    .note { padding: 6px 12px; color: #bdc1c6; font-weight: 400; overflow-wrap: anywhere; }
  </style>
  <button id="download" type="button">⬇ Download</button>
  <div class="menu" role="menu"></div>
`;
const button = shadow.querySelector("#download");
const menu = shadow.querySelector(".menu");
document.documentElement.appendChild(host);

let current = null; // { element, url }
let menuOpen = false;

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
    // A video without a fetchable source is fed by a streaming player; the
    // background script knows its manifest from the network requests.
    if (element instanceof HTMLVideoElement && (element.currentSrc || element.srcObject)) {
      return { element, url: null };
    }
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
  if (menuOpen) {
    const anchor = button.getBoundingClientRect();
    menu.style.top = `${anchor.bottom + 4}px`;
    menu.style.left = `${Math.max(anchor.right - menu.offsetWidth, 0)}px`;
  }
}

function hide() {
  closeMenu();
  current = null;
  button.style.display = "none";
}

function closeMenu() {
  menuOpen = false;
  menu.style.display = "none";
}

function note(text) {
  const element = document.createElement("div");
  element.className = "note";
  element.textContent = text;
  return element;
}

function formatSize(bytes) {
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) return `~${(mb / 1024).toFixed(1)} GB`;
  return `~${mb.toFixed(mb < 10 ? 1 : 0)} MB`;
}

function menuItem(option, target) {
  const item = document.createElement("button");
  item.type = "button";
  const label = document.createElement("span");
  label.textContent = option.label;
  const size = document.createElement("span");
  size.className = "size";
  size.textContent = option.size ? formatSize(option.size) : "";
  item.append(label, size);
  item.addEventListener("click", (event) => {
    event.stopPropagation();
    chrome.runtime.sendMessage({
      type: "download-format",
      target,
      selector: option.selector,
      label: option.label,
    });
    closeMenu();
    button.textContent = "✓ Sent";
  });
  return item;
}

function openMenu() {
  menuOpen = true;
  menu.replaceChildren(note("Loading qualities…"));
  menu.style.display = "block";
  place();

  const requested = current;
  chrome.runtime.sendMessage({ type: "list-formats" }, (response) => {
    // The menu may have been closed, or moved to another video, meanwhile.
    if (!menuOpen || current !== requested) return;
    if (!response) {
      menu.replaceChildren(note("Download manager unavailable"));
      return;
    }
    const rows = response.options.map((option) => menuItem(option, response.target));
    if (response.error) rows.push(note(`Other qualities unavailable: ${response.error}`));
    menu.replaceChildren(...rows);
    place();
  });
}

let pending = false;
document.addEventListener(
  "mousemove",
  (event) => {
    if (pending || menuOpen) return;
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

// Clicking anywhere else, or Escape, dismisses the menu.
document.addEventListener(
  "mousedown",
  (event) => {
    if (menuOpen && !event.composedPath().includes(host)) hide();
  },
  true
);
document.addEventListener("keydown", (event) => {
  if (menuOpen && event.key === "Escape") hide();
});

button.addEventListener("click", (event) => {
  event.preventDefault();
  event.stopPropagation();
  if (!current) return;
  if (current.url) {
    chrome.runtime.sendMessage({ type: "download-media", url: current.url });
    button.textContent = "✓ Sent";
    return;
  }
  if (menuOpen) closeMenu();
  else openMenu();
});
