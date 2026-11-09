import { getActiveTab, renderBookmarkBar, renderBrowserChrome } from "./browser.js";
import { invokeCmd } from "./ipc.js";
import { store } from "./state.js";
import { closeModal, escapeHtml, openModal, showToast } from "./utils.js";

let switchViewFn = null;
let searchFilter = "";

export function urlsEquivalent(a, b) {
  if (!a || !b) return false;
  const norm = (raw) => {
    const s = String(raw).trim();
    if (!s) return "";
    try {
      const u = new URL(s.includes("://") ? s : `https://${s}`);
      u.hash = "";
      const p = u.pathname === "/" ? "" : u.pathname.replace(/\/+$/, "");
      return `${u.protocol.toLowerCase()}//${u.host.toLowerCase()}${p}${u.search}`;
    } catch (_) {
      return s.replace(/\/+$/, "").toLowerCase();
    }
  };
  return norm(a) === norm(b);
}

export function initBookmarksModule({ onSwitchView }) {
  switchViewFn = onSwitchView;

  const searchInput = document.getElementById("bookmarks-search-input");
  const btnNewFolder = document.getElementById("btn-new-bm-folder");
  const btnNewBookmark = document.getElementById("btn-new-bookmark");

  searchInput.addEventListener("input", () => {
    searchFilter = searchInput.value.trim().toLowerCase();
    renderBookmarksPanel();
  });

  btnNewFolder.addEventListener("click", () => openFolderModal(null));
  btnNewBookmark.addEventListener("click", () => openBookmarkEditorModal(null));
}

export async function refreshBookmarks() {
  try {
    store.bookmarks = await invokeCmd("bookmarks_tree");
  } catch (err) {
    showToast(`Bookmark load error: ${err}`, "error");
    store.bookmarks = store.bookmarks || { folders: [], bookmarks: [] };
  }
  renderBookmarksPanel();
  renderBookmarkBar();
}

export function renderBookmarksPanel() {
  const folderPane = document.getElementById("bookmark-folders-pane");
  const itemsPane = document.getElementById("bookmarks-items-pane");
  if (!folderPane || !itemsPane) return;

  const folders = store.bookmarks?.folders || [];
  const allBookmarks = store.bookmarks?.bookmarks || [];
  const sel = store.selectedBookmarkFolderId;

  folderPane.innerHTML = `
    <div class="folder-row ${sel === "all" ? "active" : ""}" data-folder-sel="all">
      <span>📚 All Bookmarks</span>
      <span style="font-size:11px;">${allBookmarks.length}</span>
    </div>
    <div class="folder-row ${sel === "root" ? "active" : ""}" data-folder-sel="root">
      <span>★ Unsorted / Root</span>
      <span style="font-size:11px;">${allBookmarks.filter((b) => !b.folder_id).length}</span>
    </div>
    <div style="height:1px;background:var(--border-subtle);margin:4px 0;"></div>
    ${folders
      .map(
        (f) => `
      <div class="folder-row ${sel === f.id ? "active" : ""}" data-folder-sel="${f.id}">
        <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">📁 ${escapeHtml(f.name)}</span>
        <span style="display:inline-flex;gap:4px;">
          <button class="tab-close-btn" data-rename-folder="${f.id}" title="Rename Folder">✎</button>
          <button class="tab-close-btn" data-del-folder="${f.id}" title="Delete Folder">✕</button>
        </span>
      </div>
    `
      )
      .join("")}
  `;

  folderPane.querySelectorAll("[data-folder-sel]").forEach((row) => {
    row.addEventListener("click", (ev) => {
      if (ev.target.closest("button")) return;
      const raw = row.getAttribute("data-folder-sel");
      store.selectedBookmarkFolderId = raw === "all" || raw === "root" ? raw : Number(raw);
      renderBookmarksPanel();
    });
  });

  folderPane.querySelectorAll("[data-rename-folder]").forEach((btn) => {
    btn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      const fid = Number(btn.getAttribute("data-rename-folder"));
      const folder = folders.find((f) => f.id === fid);
      if (folder) openFolderModal(folder);
    });
  });

  folderPane.querySelectorAll("[data-del-folder]").forEach((btn) => {
    btn.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      try {
        const fid = Number(btn.getAttribute("data-del-folder"));
        await invokeCmd("bookmark_folder_delete", { id: fid, deleteBookmarks: false });
        if (store.selectedBookmarkFolderId === fid) store.selectedBookmarkFolderId = "all";
        await refreshBookmarks();
        showToast("Folder deleted (bookmarks moved to Unsorted)");
      } catch (e) {
        showToast(String(e), "error");
      }
    });
  });

  let filtered = allBookmarks;
  if (searchFilter) {
    filtered = filtered.filter(
      (b) =>
        b.title.toLowerCase().includes(searchFilter) ||
        b.url.toLowerCase().includes(searchFilter)
    );
  } else if (sel === "root") {
    filtered = filtered.filter((b) => !b.folder_id);
  } else if (typeof sel === "number") {
    filtered = filtered.filter((b) => b.folder_id === sel);
  }

  if (filtered.length === 0) {
    itemsPane.innerHTML = `
      <div class="empty-state">
        <strong>No Bookmarks in This View</strong>
        <p style="margin:6px 0 0;">Click "+ Add Bookmark" or click the ☆ icon in the Browser address bar to bookmark webpages.</p>
      </div>
    `;
    return;
  }

  itemsPane.innerHTML = `
    <div class="data-list">
      ${filtered
        .map((b) => {
          const folderName = folders.find((f) => f.id === b.folder_id)?.name;
          return `
            <div class="data-row" data-bm-row="${b.id}">
              <div class="data-row-main">
                <div class="data-row-title">
                  ${escapeHtml(b.title)}
                  ${b.show_on_bar ? `<span style="font-size:10.5px;color:var(--accent);margin-left:6px;">[Bar]</span>` : ""}
                  ${folderName ? `<span style="font-size:11px;color:var(--text-muted);margin-left:6px;">📁 ${escapeHtml(folderName)}</span>` : ""}
                </div>
                <div class="data-row-url">${escapeHtml(b.url)}</div>
              </div>
              <div class="panel-toolbar">
                <button class="btn btn-primary btn-sm" data-bma="open">Open</button>
                <button class="btn btn-sm" data-bma="newtab">New Tab</button>
                <button class="btn btn-sm" data-bma="edit">Edit / Move</button>
                <button class="btn btn-danger btn-sm" data-bma="del">Delete</button>
              </div>
            </div>
          `;
        })
        .join("")}
    </div>
  `;

  itemsPane.querySelectorAll("[data-bm-row]").forEach((row) => {
    const id = Number(row.getAttribute("data-bm-row"));
    const bm = allBookmarks.find((x) => x.id === id);
    if (!bm) return;

    const mainArea = row.querySelector(".data-row-main");
    if (mainArea) {
      mainArea.style.cursor = "pointer";
      mainArea.title = "Click to open bookmark in Browser";
      mainArea.addEventListener("click", async () => {
        switchViewFn("browser");
        try {
          await invokeCmd("tab_navigate", { tabId: null, input: bm.url });
        } catch (e) {
          showToast(String(e), "error");
        }
      });
    }

    row.querySelectorAll("[data-bma]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const act = btn.getAttribute("data-bma");
        try {
          if (act === "open") {
            switchViewFn("browser");
            await invokeCmd("tab_navigate", { tabId: null, input: bm.url });
          } else if (act === "newtab") {
            switchViewFn("browser");
            await invokeCmd("tab_create", { url: bm.url, active: true });
          } else if (act === "edit") {
            openBookmarkEditorModal(bm);
          } else if (act === "del") {
            await invokeCmd("bookmarks_delete", { id: bm.id });
            await refreshBookmarks();
            renderBrowserChrome();
            showToast("Bookmark deleted");
          }
        } catch (e) {
          showToast(String(e), "error");
        }
      });
    });
  });
}

function openFolderModal(existingFolder) {
  openModal(
    `
      <h3>${existingFolder ? "Rename Bookmark Folder" : "New Bookmark Folder"}</h3>
      <input id="bm-folder-name-inp" class="input-field" placeholder="Folder name (e.g. Local Media Servers)" value="${escapeHtml(existingFolder?.name || "")}" />
      <div class="modal-actions">
        <button id="bm-f-cancel" class="btn">Cancel</button>
        <button id="bm-f-save" class="btn btn-primary">Save</button>
      </div>
    `,
    (card) => {
      const inp = card.querySelector("#bm-folder-name-inp");
      inp.focus();
      const doSave = async () => {
        const name = inp.value.trim();
        if (!name) return;
        try {
          if (existingFolder) {
            await invokeCmd("bookmark_folder_rename", { id: existingFolder.id, name });
          } else {
            await invokeCmd("bookmark_folder_create", { name, parentId: null });
          }
          closeModal();
          await refreshBookmarks();
          showToast(existingFolder ? "Folder renamed" : "Folder created", "success");
        } catch (e) {
          showToast(String(e), "error");
        }
      };
      inp.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter") {
          ev.preventDefault();
          doSave();
        } else if (ev.key === "Escape") {
          closeModal();
        }
      });
      card.querySelector("#bm-f-cancel").onclick = closeModal;
      card.querySelector("#bm-f-save").onclick = doSave;
    }
  );
}

export function openBookmarkEditorModal(existing, prefill = null) {
  const folders = store.bookmarks?.folders || [];
  const defaultFolderId =
    existing?.folder_id ??
    (typeof store.selectedBookmarkFolderId === "number" ? store.selectedBookmarkFolderId : null);

  const activeTab = getActiveTab();
  const initialTitle = existing?.title ?? prefill?.title ?? "";
  const initialUrl = existing?.url ?? prefill?.url ?? "https://";

  const folderOptions = [
    `<option value="">(Root / Unsorted)</option>`,
    ...folders.map(
      (f) =>
        `<option value="${f.id}" ${defaultFolderId === f.id ? "selected" : ""}>${escapeHtml(f.name)}</option>`
    ),
  ].join("");

  const canFillFromTab =
    !existing && !prefill && activeTab?.url && activeTab.url !== "about:blank";

  openModal(
    `
      <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
        <h3 style="margin:0;">${existing ? "Edit / Move Bookmark" : "Add Bookmark"}</h3>
        ${
          canFillFromTab
            ? `<button type="button" id="bme-fill-tab" class="btn btn-sm">Use Current Tab</button>`
            : ""
        }
      </div>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Name</span>
        <input id="bme-title" class="input-field" value="${escapeHtml(initialTitle)}" placeholder="e.g. Media Index Server" />
      </label>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">URL</span>
        <input id="bme-url" class="input-field" value="${escapeHtml(initialUrl)}" />
      </label>
      <label style="display:flex;flex-direction:column;gap:4px;">
        <span style="font-size:12px;color:var(--text-secondary);">Folder</span>
        <select id="bme-folder" class="select-field">${folderOptions}</select>
      </label>
      <label style="display:flex;align-items:center;gap:8px;cursor:pointer;">
        <input id="bme-bar" type="checkbox" ${!existing || existing.show_on_bar ? "checked" : ""} />
        <span>Show on Bookmark Bar</span>
      </label>
      <div class="modal-actions">
        ${
          existing
            ? `<button id="bme-delete" class="btn btn-danger" style="margin-right:auto;">Remove</button>`
            : ""
        }
        <button id="bme-cancel" class="btn">Cancel</button>
        <button id="bme-save" class="btn btn-primary">Save Bookmark</button>
      </div>
    `,
    (card) => {
      const titleInp = card.querySelector("#bme-title");
      const urlInp = card.querySelector("#bme-url");
      titleInp.focus();

      const fillBtn = card.querySelector("#bme-fill-tab");
      if (fillBtn && activeTab) {
        fillBtn.onclick = () => {
          titleInp.value = activeTab.title || activeTab.url || "";
          urlInp.value = activeTab.url || "https://";
        };
      }

      const delBtn = card.querySelector("#bme-delete");
      if (delBtn && existing) {
        delBtn.onclick = async () => {
          try {
            await invokeCmd("bookmarks_delete", { id: existing.id });
            closeModal();
            await refreshBookmarks();
            renderBrowserChrome();
            showToast("Bookmark removed");
          } catch (e) {
            showToast(String(e), "error");
          }
        };
      }

      const doSave = async () => {
        const title = titleInp.value.trim();
        const url = urlInp.value.trim();
        if (!url || url === "https://" || url === "http://") {
          showToast("Please enter a valid bookmark URL", "error");
          urlInp.focus();
          return;
        }
        const folderVal = card.querySelector("#bme-folder").value;
        const folderId = folderVal ? Number(folderVal) : null;
        const showOnBar = card.querySelector("#bme-bar").checked;
        try {
          if (existing) {
            await invokeCmd("bookmarks_update", {
              id: existing.id,
              title,
              url,
              folderId,
              showOnBar,
            });
          } else {
            await invokeCmd("bookmarks_add", { title, url, folderId, showOnBar });
          }
          closeModal();
          await refreshBookmarks();
          renderBrowserChrome();
          showToast("Bookmark saved", "success");
        } catch (e) {
          showToast(String(e), "error");
        }
      };

      [titleInp, urlInp].forEach((el) => {
        el.addEventListener("keydown", (ev) => {
          if (ev.key === "Enter") {
            ev.preventDefault();
            doSave();
          } else if (ev.key === "Escape") {
            closeModal();
          }
        });
      });

      card.querySelector("#bme-cancel").onclick = closeModal;
      card.querySelector("#bme-save").onclick = doSave;
    }
  );
}
