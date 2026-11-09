import { invokeCmd } from "./ipc.js";
import { store } from "./state.js";
import {
  closeModal,
  copyTextToClipboard,
  escapeHtml,
  formatBytes,
  formatSpeed,
  openModal,
  showToast,
} from "./utils.js";

export function initDownloadsModule() {
  const input = document.getElementById("direct-dl-url-input");
  const btnStart = document.getElementById("btn-start-direct-dl");
  const btnClear = document.getElementById("btn-clear-completed-dl");

  const submitDirectDownload = () => {
    const url = input.value.trim();
    if (!url) {
      showToast("Enter a direct media URL to download", "error");
      return;
    }
    input.value = "";
    triggerPermanentDownload(url, true);
  };

  btnStart.addEventListener("click", submitDirectDownload);
  input.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      submitDirectDownload();
    }
  });

  btnClear.addEventListener("click", async () => {
    try {
      await invokeCmd("download_clear_completed");
      store.downloads = await invokeCmd("download_list");
      renderDownloadsPanel();
      showToast("Cleared finished download history");
    } catch (e) {
      showToast(String(e), "error");
    }
  });
}

/// Initiates permanent media download (independent of temporary streaming cache).
/// Uses native Windows Save As dialog pre-filled with the exact server filename when enabled or requested.
export async function triggerPermanentDownload(url, forceSaveAs = false) {
  try {
    const askSaveAs = forceSaveAs || store.settings?.ask_save_as_every_time !== false;
    if (askSaveAs) {
      const rec = await invokeCmd("download_prompt_save_as_and_start", { url });
      if (!rec) {
        // User cancelled Windows Save As dialog cleanly
        return;
      }
      if (store.settings?.download_notifications !== false) {
        showToast(`Downloading: ${rec.filename}`, "success");
      }
    } else {
      const rec = await invokeCmd("download_start_default_dir", {
        url,
        customFilename: null,
      });
      if (store.settings?.download_notifications !== false) {
        showToast(`Downloading: ${rec.filename}`, "success");
      }
    }
    store.downloads = await invokeCmd("download_list");
    renderDownloadsPanel();
    updateSidebarDownloadBadge();
  } catch (e) {
    // Fallback web modal if native Save As had an issue
    openFallbackSaveAsModal(url, String(e));
  }
}

function openFallbackSaveAsModal(url, errDetail) {
  invokeCmd("download_prepare_save_as", { url })
    .then((sug) => {
      openModal(
        `
          <h3>Save Media As</h3>
          <div style="font-size:12px;color:var(--text-secondary);">
            Destination folder: <code>${escapeHtml(sug.default_directory)}</code>
          </div>
          <label style="display:flex;flex-direction:column;gap:4px;">
            <span style="font-size:12px;color:var(--text-secondary);">Original Server Filename</span>
            <input id="fb-save-name" class="input-field" value="${escapeHtml(sug.suggested_filename)}" />
          </label>
          <div class="modal-actions">
            <button id="fb-save-cancel" class="btn">Cancel</button>
            <button id="fb-save-ok" class="btn btn-primary">⬇ Save Permanently</button>
          </div>
        `,
        (card) => {
          card.querySelector("#fb-save-cancel").onclick = closeModal;
          card.querySelector("#fb-save-ok").onclick = async () => {
            const fname = card.querySelector("#fb-save-name").value.trim();
            closeModal();
            try {
              const rec = await invokeCmd("download_start_default_dir", {
                url,
                customFilename: fname || sug.suggested_filename,
              });
              store.downloads = await invokeCmd("download_list");
              renderDownloadsPanel();
              showToast(`Downloading: ${rec.filename}`, "success");
            } catch (err) {
              showToast(String(err), "error");
            }
          };
        }
      );
    })
    .catch(() => {
      showToast(`Download error: ${errDetail}`, "error");
    });
}

export function updateSidebarDownloadBadge() {
  const badge = document.getElementById("nav-dl-badge");
  if (!badge) return;
  const activeCount = (store.downloads || []).filter(
    (d) => d.status === "downloading" || d.status === "queued"
  ).length;
  badge.textContent = String(activeCount);
  badge.classList.toggle("hidden", activeCount === 0);
}

export function renderDownloadsPanel() {
  updateSidebarDownloadBadge();
  const container = document.getElementById("downloads-list-container");
  if (!container) return;

  const list = store.downloads || [];
  if (list.length === 0) {
    container.innerHTML = `
      <div class="empty-state">
        <strong>No Permanent Downloads Yet</strong>
        <p style="margin:6px 0 0;">Right-click any media link on a webpage or use the Detected Media bar and choose "Download Media" to save files permanently.</p>
      </div>
    `;
    return;
  }

  container.innerHTML = list
    .map((d) => {
      const pct =
        d.total_bytes && d.total_bytes > 0
          ? Math.min(100, (d.downloaded_bytes / d.total_bytes) * 100).toFixed(1)
          : d.status === "completed"
          ? "100.0"
          : "0.0";

      const sizeText = d.total_bytes
        ? `${formatBytes(d.downloaded_bytes)} / ${formatBytes(d.total_bytes)} (${pct}%)`
        : `${formatBytes(d.downloaded_bytes)}`;

      const speedText =
        d.status === "downloading" ? ` • ${formatSpeed(d.speed_bps)}` : "";
      const rangeBadge = d.supports_range ? "Resumable (HTTP Range)" : "Standard Stream";

      return `
        <div class="download-card" data-dl-id="${escapeHtml(d.id)}">
          <div class="download-hdr">
            <strong style="word-break:break-all;">${escapeHtml(d.filename)}</strong>
            <span class="status-pill ${escapeHtml(d.status)}">${escapeHtml(d.status)}</span>
          </div>
          <div style="font-size:12px;color:var(--text-secondary);">
            ${escapeHtml(sizeText)}${escapeHtml(speedText)} • ${rangeBadge}
          </div>
          <div class="progress-bar-bg">
            <div class="progress-bar-fill" style="width:${pct}%;"></div>
          </div>
          <div style="font-size:11.5px;color:var(--text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="${escapeHtml(d.destination_path)}">
            Saved to: ${escapeHtml(d.destination_path)}
          </div>
          <div style="font-size:11px;color:var(--text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="${escapeHtml(d.original_url)}">
            Source: ${escapeHtml(d.original_url)}
          </div>
          ${
            d.error_message
              ? `<div style="font-size:12px;color:var(--danger);">${escapeHtml(d.error_message)}</div>`
              : ""
          }
          <div class="media-card-actions">
            ${
              d.status === "downloading"
                ? `<button class="btn btn-sm" data-dl-act="pause">⏸ Pause</button>`
                : ""
            }
            ${
              (d.status === "paused" || d.status === "interrupted") && d.supports_range
                ? `<button class="btn btn-primary btn-sm" data-dl-act="resume">▶ Resume</button>`
                : ""
            }
            ${
              d.status === "failed" || d.status === "cancelled" || ((d.status === "paused" || d.status === "interrupted") && !d.supports_range)
                ? `<button class="btn btn-primary btn-sm" data-dl-act="retry">↻ Retry</button>`
                : ""
            }
            ${
              d.status === "downloading" ||
              d.status === "paused" ||
              d.status === "interrupted" ||
              d.status === "queued"
                ? `<button class="btn btn-danger btn-sm" data-dl-act="cancel">✕ Cancel</button>`
                : ""
            }
            ${
              d.status === "completed"
                ? `<button class="btn btn-primary btn-sm" data-dl-act="open-file">▶ Open File</button>`
                : ""
            }
            <button class="btn btn-sm" data-dl-act="open-folder">📂 Show in Folder</button>
            <button class="btn btn-sm" data-dl-act="save-copy">⬇ Save As...</button>
            <button class="btn btn-sm" data-dl-act="copy-url">⧉ Copy URL</button>
            ${
              d.status !== "downloading" && d.status !== "queued"
                ? `<button class="btn btn-sm" data-dl-act="remove" style="margin-left:auto;">Remove Entry</button>`
                : ""
            }
          </div>
        </div>
      `;
    })
    .join("");

  container.querySelectorAll(".download-card").forEach((card) => {
    const id = card.getAttribute("data-dl-id");
    const rec = list.find((x) => x.id === id);
    card.querySelectorAll("[data-dl-act]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const act = btn.getAttribute("data-dl-act");
        try {
          if (act === "pause") await invokeCmd("download_pause", { id });
          else if (act === "resume") await invokeCmd("download_resume", { id });
          else if (act === "retry") await invokeCmd("download_retry", { id });
          else if (act === "cancel") await invokeCmd("download_cancel", { id });
          else if (act === "open-file") await invokeCmd("download_open_file", { id });
          else if (act === "open-folder") await invokeCmd("download_open_folder", { id });
          else if (act === "save-copy" && rec) await triggerPermanentDownload(rec.original_url, true);
          else if (act === "copy-url" && rec) await copyTextToClipboard(rec.original_url);
          else if (act === "remove") await invokeCmd("download_remove", { id });

          store.downloads = await invokeCmd("download_list");
          renderDownloadsPanel();
        } catch (e) {
          showToast(String(e), "error");
        }
      });
    });
  });
}
