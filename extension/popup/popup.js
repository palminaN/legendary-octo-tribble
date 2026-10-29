const NATIVE_HOST_NAME = "com.legendary_octo_tribble.native_host";

chrome.runtime.sendNativeMessage(NATIVE_HOST_NAME, { type: "ping" }, (response) => {
  const status = document.getElementById("status");
  if (chrome.runtime.lastError || !response) {
    status.textContent = "native host unreachable";
    return;
  }
  status.textContent = "connected: " + response.type;
});
