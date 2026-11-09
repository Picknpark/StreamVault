import { invokeCmd } from "./ipc.js";

export function escapeHtml(str) {
  if (str === null || str === undefined) return "";
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export function formatTimeMs(ms) {
  const clamped = Math.max(0, Math.floor((ms || 0) / 1000));
  const h = Math.floor(clamped / 3600);
  const m = Math.floor((clamped % 3600) / 60);
  const s = clamped % 60;
  const pad = (n) => String(n).padStart(2, "0");
  return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

export function formatBytes(bytes) {
  if (bytes === null || bytes === undefined || isNaN(bytes)) return "Unknown size";
  const b = Math.max(0, Number(bytes));
  if (b < 1024) return `${b} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let val = b / 1024;
  let u = 0;
  while (val >= 1024 && u < units.length - 1) {
    val /= 1024;
    u++;
  }
  return `${val.toFixed(val >= 100 ? 0 : val >= 10 ? 1 : 2)} ${units[u]}`;
}

export function formatSpeed(bps) {
  if (!bps || bps <= 0) return "0 KB/s";
  return `${formatBytes(bps)}/s`;
}

export function formatCompactNetRate(bps) {
  const b = Math.max(0, Number(bps) || 0);
  if (b <= 0) return "0 KB/s";
  if (b < 1024) return `${Math.round(b)} B/s`;
  const kb = b / 1024;
  if (kb < 1024) {
    return `${kb >= 100 ? Math.round(kb) : kb.toFixed(1)} KB/s`;
  }
  const mb = kb / 1024;
  if (mb < 1024) {
    return `${mb >= 100 ? Math.round(mb) : mb.toFixed(1)} MB/s`;
  }
  const gb = mb / 1024;
  return `${gb.toFixed(2)} GB/s`;
}

export function formatTimestamp(ms) {
  if (!ms) return "";
  try {
    return new Date(ms).toLocaleString();
  } catch (_) {
    return "";
  }
}

export function showToast(message, kind = "info") {
  const container = document.getElementById("toast-container");
  if (!container) return;
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  container.appendChild(el);
  setTimeout(() => {
    if (el.parentNode) el.parentNode.removeChild(el);
  }, 3600);
}

export function openModal(innerHtml, onMount) {
  const root = document.getElementById("modal-root");
  if (!root) return;
  root.innerHTML = `<div class="modal-card">${innerHtml}</div>`;
  root.classList.remove("hidden");
  document.dispatchEvent(new CustomEvent("sv:modal-visibility", { detail: { open: true } }));

  const closeOnBackdrop = (ev) => {
    if (ev.target === root) {
      closeModal();
    }
  };
  root.onclick = closeOnBackdrop;
  if (typeof onMount === "function") {
    onMount(root.querySelector(".modal-card"));
  }
}

export function closeModal() {
  const root = document.getElementById("modal-root");
  if (!root) return;
  root.innerHTML = "";
  root.classList.add("hidden");
  document.dispatchEvent(new CustomEvent("sv:modal-visibility", { detail: { open: false } }));
}

export async function copyTextToClipboard(text) {
  const str = String(text ?? "");
  if (!str) return;

  // Primary path: native Win32 clipboard via Rust (`CF_UNICODETEXT`) — works 100% reliably
  // regardless of WebView2 origin/focus and preserves percent-encoded media URLs verbatim.
  try {
    await invokeCmd("clipboard_write_text", { text: str });
    showToast("Copied link to clipboard", "success");
    return;
  } catch (_) {}

  // Fallback path: browser Clipboard API + DOM execCommand
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(str);
      showToast("Copied link to clipboard", "success");
      return;
    }
  } catch (_) {}

  try {
    const ta = document.createElement("textarea");
    ta.value = str;
    ta.setAttribute("readonly", "");
    ta.style.cssText = "position:fixed;top:-9999px;left:-9999px;opacity:0;";
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    document.body.removeChild(ta);
    showToast("Copied link to clipboard", "success");
  } catch (err) {
    showToast(`Failed to copy link: ${err}`, "error");
  }
}
