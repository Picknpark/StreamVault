import { invokeCmd } from "./ipc.js";
import { store } from "./state.js";
import { escapeHtml, formatTimeMs, showToast } from "./utils.js";

let switchViewFn = null;
let playMediaFn = null;

export function initHomeModule({ onSwitchView, onPlayMedia }) {
  switchViewFn = onSwitchView;
  playMediaFn = onPlayMedia;

  const form = document.getElementById("home-search-form");
  const input = document.getElementById("home-omnibox-input");

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const val = input.value.trim();
    if (!val) return;
    input.value = "";
    try {
      switchViewFn("browser");
      await invokeCmd("tab_navigate", { tabId: null, input: val });
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  document.querySelectorAll("[data-goto-view]").forEach((btn) => {
    btn.addEventListener("click", () => {
      switchViewFn(btn.getAttribute("data-goto-view"));
    });
  });
}

export async function renderHomePanel() {
  const recentBox = document.getElementById("home-recent-media-list");
  const bmBox = document.getElementById("home-bookmarks-list");
  const histBox = document.getElementById("home-history-list");
  if (!recentBox || !bmBox || !histBox) return;

  try {
    const freshRecent = await invokeCmd("recent_media_list", { limit: 20 });
    if (Array.isArray(freshRecent)) {
      store.recentMedia = freshRecent;
    }
  } catch (_) {}

  const recents = (store.recentMedia || []).slice(0, 4);
  if (recents.length === 0) {
    recentBox.innerHTML = `<div style="color:var(--text-muted);font-size:12px;">No media played yet.</div>`;
  } else {
    recentBox.innerHTML = recents
      .map((m) => {
        const resumeBadge =
          !m.completed && m.last_position_ms >= 5000
            ? `Resume ${formatTimeMs(m.last_position_ms)}`
            : "Play";
        return `
          <div class="data-row" style="padding:7px 4px;" data-home-rm="${m.id}">
            <div class="data-row-main">
              <div class="data-row-title">${escapeHtml(m.title)}</div>
              <div class="data-row-url">${escapeHtml(m.original_url)}</div>
            </div>
            <button class="btn btn-primary btn-sm">${escapeHtml(resumeBadge)}</button>
          </div>
        `;
      })
      .join("");

    recentBox.querySelectorAll("[data-home-rm]").forEach((row) => {
      const id = Number(row.getAttribute("data-home-rm"));
      const item = recents.find((x) => x.id === id);
      if (item) {
        row.querySelector("button").onclick = () =>
          playMediaFn(item.original_url, item.title, null);
      }
    });
  }

  const bms = (store.bookmarks?.bookmarks || []).slice(0, 5);
  if (bms.length === 0) {
    bmBox.innerHTML = `<div style="color:var(--text-muted);font-size:12px;">No saved bookmarks yet.</div>`;
  } else {
    bmBox.innerHTML = bms
      .map(
        (b) => `
        <div class="data-row" style="padding:7px 4px;" data-home-url="${escapeHtml(b.url)}">
          <div class="data-row-main">
            <div class="data-row-title">★ ${escapeHtml(b.title)}</div>
            <div class="data-row-url">${escapeHtml(b.url)}</div>
          </div>
          <button class="btn btn-sm">Open</button>
        </div>
      `
      )
      .join("");

    bmBox.querySelectorAll("[data-home-url]").forEach((row) => {
      const url = row.getAttribute("data-home-url");
      row.querySelector("button").onclick = () => {
        switchViewFn("browser");
        invokeCmd("tab_navigate", { tabId: null, input: url });
      };
    });
  }

  let hist = store.history || [];
  if (hist.length === 0) {
    try {
      hist = await invokeCmd("history_list", { query: null, limit: 5 });
      store.history = hist;
    } catch (_) {}
  }
  const topHist = hist.slice(0, 5);
  if (topHist.length === 0) {
    histBox.innerHTML = `<div style="color:var(--text-muted);font-size:12px;">No recent browsing history.</div>`;
  } else {
    histBox.innerHTML = topHist
      .map(
        (h) => `
        <div class="data-row" style="padding:7px 4px;" data-home-hurl="${escapeHtml(h.url)}">
          <div class="data-row-main">
            <div class="data-row-title">${escapeHtml(h.title)}</div>
            <div class="data-row-url">${escapeHtml(h.url)}</div>
          </div>
          <button class="btn btn-sm">Visit</button>
        </div>
      `
      )
      .join("");

    histBox.querySelectorAll("[data-home-hurl]").forEach((row) => {
      const url = row.getAttribute("data-home-hurl");
      row.querySelector("button").onclick = () => {
        switchViewFn("browser");
        invokeCmd("tab_navigate", { tabId: null, input: url });
      };
    });
  }
}
