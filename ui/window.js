"use strict";

if (window.__TAURI__ && navigator.platform.startsWith("Mac")) {
  document.documentElement.dataset.windowChrome = "overlay";
  document.addEventListener("DOMContentLoaded", () => {
    // Keep WebKit from selecting the empty area when Tauri handles a double click.
    document.querySelector(".window-drag-region").addEventListener("mousedown", event => {
      if (event.button === 0) event.preventDefault();
    });
  }, { once: true });
}
