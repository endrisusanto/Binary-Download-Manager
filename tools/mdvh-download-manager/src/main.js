const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// Element references
let listProgressEl;
let listCompletedEl;
let listFailedEl;
let emptyStateEl;
let accordionStackEl;

let metricTotalEl;
let subTotalEl;
let metricActiveEl;
let subActiveEl;
let metricCompletedEl;
let subCompletedEl;

let folderTitleEl;
let folderPathEl;
let listenerStatusEl;

// Dictionary keeping track of active/completed download details
const downloadsDb = {};

window.addEventListener("DOMContentLoaded", () => {
  // Bind UI Elements
  listProgressEl = document.querySelector("#list-progress");
  listCompletedEl = document.querySelector("#list-completed");
  listFailedEl = document.querySelector("#list-failed");
  emptyStateEl = document.querySelector("#empty-state");
  accordionStackEl = document.querySelector("#accordion-stack");

  metricTotalEl = document.querySelector("#metric-total");
  subTotalEl = document.querySelector("#sub-total");
  metricActiveEl = document.querySelector("#metric-active");
  subActiveEl = document.querySelector("#sub-active");
  metricCompletedEl = document.querySelector("#metric-completed");
  subCompletedEl = document.querySelector("#sub-completed");

  folderTitleEl = document.querySelector(".folder-title");
  folderPathEl = document.querySelector("#sub-folder");
  listenerStatusEl = document.querySelector("#listener-status");

  // Setup Accordion Headings Toggles
  document.querySelectorAll(".task-accordion").forEach(accordion => {
    const heading = accordion.querySelector(".accordion-heading");
    heading.addEventListener("click", () => {
      accordion.classList.toggle("open");
    });
  });

  // Open download folder button
  document.querySelector("#btn-open-folder").addEventListener("click", async () => {
    try {
      await invoke("open_download_folder");
    } catch (err) {
      alert("Failed to open downloads folder: " + err);
    }
  });

  // Pick download folder card trigger
  document.querySelector("#btn-pick-folder").addEventListener("click", async () => {
    try {
      const picked = await invoke("pick_download_dir");
      if (picked) {
        updateFolderDisplay(picked);
      }
    } catch (err) {
      alert("Failed to select folder: " + err);
    }
  });

  // Clear queue button
  document.querySelector("#btn-clear-queue").addEventListener("click", () => {
    listCompletedEl.innerHTML = "";
    listFailedEl.innerHTML = "";
    
    // Remove completed and failed downloads from database
    Object.keys(downloadsDb).forEach(fileName => {
      const status = downloadsDb[fileName].status;
      if (status === "completed" || status === "failed") {
        delete downloadsDb[fileName];
      }
    });

    updateMetrics();
  });

  // Query Initial Configs from Tauri Backend
  queryInitialBackendState();

  // Setup Tauri Event Listeners
  setupTauriListeners();
});

async function queryInitialBackendState() {
  // Get active listener status
  try {
    const status = await invoke("get_listener_status");
    updateListenerStatusUI(status);
  } catch (err) {
    console.error("Failed to query listener status:", err);
  }

  // Get active download folder
  try {
    const folder = await invoke("get_download_dir");
    if (folder) {
      updateFolderDisplay(folder);
    }
  } catch (err) {
    console.error("Failed to query download directory:", err);
  }

  // Get app version
  try {
    const version = await invoke("get_app_version");
    const subtitleEl = document.querySelector(".subtitle");
    if (subtitleEl) {
      subtitleEl.textContent = `SSCM / MDVH Binary Bridging Tool v${version}`;
    }
  } catch (err) {
    console.error("Failed to query app version:", err);
  }
}

function updateFolderDisplay(fullPath) {
  folderPathEl.textContent = fullPath;
  // Extract last folder name for title
  const parts = fullPath.replace(/\\/g, "/").split("/");
  const lastFolder = parts[parts.length - 1] || parts[parts.length - 2] || "downloads";
  folderTitleEl.textContent = lastFolder;
}

function formatBytes(bytes, decimals = 2) {
  if (!bytes) return "0 Bytes";
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ["Bytes", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(dm)) + " " + sizes[i];
}

function updateMetrics() {
  const items = Object.values(downloadsDb);
  const total = items.length;
  const active = items.filter(d => d.status === "downloading" || d.status === "paused").length;
  const completed = items.filter(d => d.status === "completed").length;
  const failed = items.filter(d => d.status === "failed").length;

  metricTotalEl.textContent = total;
  subTotalEl.textContent = `${items.filter(d => d.status === "downloading").length} active downloads`;

  metricActiveEl.textContent = active;
  subActiveEl.textContent = active > 0 ? `${active} thread${active === 1 ? "" : "s"}` : "0 threads";

  metricCompletedEl.textContent = completed;
  subCompletedEl.textContent = `${failed} failed`;

  // Toggle empty states
  if (total === 0) {
    emptyStateEl.style.display = "flex";
    accordionStackEl.style.display = "none";
  } else {
    emptyStateEl.style.display = "none";
    accordionStackEl.style.display = "grid";
  }

  // Update accordion badges
  document.querySelector("#count-progress").textContent = items.filter(d => d.status === "downloading" || d.status === "paused").length;
  document.querySelector("#count-completed").textContent = completed;
  document.querySelector("#count-failed").textContent = failed;

  // Toggle individual accordions visibility / empty indicators
  toggleAccordionPlaceholder("progress", items.filter(d => d.status === "downloading" || d.status === "paused").length);
  toggleAccordionPlaceholder("completed", completed);
  toggleAccordionPlaceholder("failed", failed);
}

function toggleAccordionPlaceholder(key, count) {
  const list = document.querySelector(`#list-${key}`);
  const parent = document.querySelector(`#sec-${key}`);
  let placeholder = parent.querySelector(".accordion-empty");
  
  if (count === 0) {
    if (!placeholder) {
      placeholder = document.createElement("div");
      placeholder.className = "accordion-empty";
      placeholder.textContent = `No ${key === "progress" ? "in-progress" : key} downloads.`;
      list.after(placeholder);
    }
    list.style.display = "none";
  } else {
    if (placeholder) placeholder.remove();
    list.style.display = "grid";
  }
}

function updateListenerStatusUI(payload) {
  const dot = listenerStatusEl.querySelector(".status-dot");
  const text = listenerStatusEl.querySelector(".status-text");

  if (payload.active) {
    dot.className = "status-dot online";
    text.textContent = `Active: ${payload.bind}`;
  } else if (payload.error) {
    dot.className = "status-dot offline";
    text.textContent = "Offline (Port occupied)";
    console.error(payload.error);
  } else {
    dot.className = "status-dot offline";
    text.textContent = "Connecting...";
  }
}

async function setupTauriListeners() {
  // 1. Connection status listener
  await listen("listener-status", (event) => {
    updateListenerStatusUI(event.payload);
  });

  // 2. Download started listener
  await listen("download-started", (event) => {
    const file = event.payload;
    const fileName = file.fileName;

    // Avoid duplicates if already in db
    if (downloadsDb[fileName]) return;

    // Create item DOM structure
    const itemEl = document.createElement("div");
    itemEl.className = "download-item";
    itemEl.id = `dl-${fileName.replace(/[^a-zA-Z0-9]/g, "-")}`;
    itemEl.innerHTML = `
      <div class="item-left">
        <div class="file-info">
          <div class="file-name" title="${fileName}">${fileName}</div>
          <div class="file-meta" id="meta-${itemEl.id}">Size: ${formatBytes(file.expectedSize)}</div>
        </div>
        <div class="progress-section" id="prog-${itemEl.id}">
          <div class="progress-container">
            <div class="progress-bar" style="width: 0%"></div>
          </div>
          <div class="progress-percent">0%</div>
        </div>
      </div>
      <div class="item-right">
        <span class="status-pill downloading">Downloading</span>
        <div class="card-actions">
          <button class="action-btn pause-resume-btn" title="Pause download">⏸</button>
          <button class="action-btn danger cancel-btn" title="Cancel download">✖</button>
        </div>
      </div>
    `;

    // Store in downloadsDb
    downloadsDb[fileName] = {
      status: "downloading",
      el: itemEl,
      progressBar: itemEl.querySelector(".progress-bar"),
      percentText: itemEl.querySelector(".progress-percent"),
      statusPill: itemEl.querySelector(".status-pill"),
      metaText: itemEl.querySelector(`#meta-${itemEl.id}`),
      expectedSize: file.expectedSize,
      downloadedSize: 0,
      paused: false
    };

    // Setup Pause/Resume local toggle
    const pauseResumeBtn = itemEl.querySelector(".pause-resume-btn");
    pauseResumeBtn.addEventListener("click", async () => {
      const dbItem = downloadsDb[fileName];
      if (!dbItem) return;

      if (dbItem.paused) {
        // Resume download
        try {
          await invoke("resume_download", { fileName });
          dbItem.paused = false;
          dbItem.status = "downloading";
          dbItem.statusPill.className = "status-pill downloading";
          dbItem.statusPill.textContent = "Downloading";
          pauseResumeBtn.textContent = "⏸";
          pauseResumeBtn.title = "Pause download";
        } catch (err) {
          alert("Failed to resume: " + err);
        }
      } else {
        // Pause download
        try {
          // Send pause command. UI will update when 'download-finished' (cancelled) is emitted
          pauseResumeBtn.disabled = true; // prevent spamming
          await invoke("pause_download", { fileName });
          setTimeout(() => pauseResumeBtn.disabled = false, 1000);
        } catch (err) {
          alert("Failed to pause: " + err);
          pauseResumeBtn.disabled = false;
        }
      }
      updateMetrics();
    });

    // Setup Cancel button trigger
    const cancelBtn = itemEl.querySelector(".cancel-btn");
    cancelBtn.addEventListener("click", () => {
      itemEl.remove();
      delete downloadsDb[fileName];
      updateMetrics();
    });

    listProgressEl.appendChild(itemEl);
    updateMetrics();
  });

  // 3. Download progress listener
  await listen("download-progress", (event) => {
    const progress = event.payload;
    const dbItem = downloadsDb[progress.file_name];
    if (!dbItem) return;

    dbItem.downloadedSize = progress.bytes_downloaded;
    const total = progress.total_bytes || dbItem.expectedSize;

    const percent = Math.min(
      100,
      Math.round((progress.bytes_downloaded / total) * 100)
    );

    dbItem.progressBar.style.width = `${percent}%`;
    dbItem.percentText.textContent = `${percent}%`;
    
    // Update label with current bytes
    const expectedStr = formatBytes(total);
    const downloadedStr = formatBytes(progress.bytes_downloaded);
    dbItem.metaText.textContent = `Size: ${downloadedStr} of ${expectedStr}`;
  });

  // 4. Download finished listener
  await listen("download-finished", (event) => {
    const report = event.payload;
    const fileName = report.fileName;
    const dbItem = downloadsDb[fileName];
    if (!dbItem) return;

    if (report.status === "cancelled") {
      dbItem.paused = true;
      dbItem.status = "paused";
      dbItem.statusPill.className = "status-pill paused";
      dbItem.statusPill.textContent = "Paused";
      
      const pauseResumeBtn = dbItem.el.querySelector(".pause-resume-btn");
      if (pauseResumeBtn) {
        pauseResumeBtn.textContent = "▶";
        pauseResumeBtn.title = "Resume download";
        pauseResumeBtn.disabled = false;
      }
      updateMetrics();
      return;
    }

    // Update state to completed
    dbItem.status = "completed";
    dbItem.statusPill.className = "status-pill success";
    dbItem.statusPill.textContent = report.status === "downloaded" ? "Completed" : "Replayed";

    // Hide progress bar section
    const progressSec = dbItem.el.querySelector(`#prog-${dbItem.el.id}`);
    if (progressSec) progressSec.style.display = "none";

    // Update metadata label
    dbItem.metaText.textContent = `Completed | Size: ${formatBytes(report.actualSize || dbItem.expectedSize)}`;

    // Update action buttons (Add open directory button, remove pause/resume)
    const actionsArea = dbItem.el.querySelector(".card-actions");
    actionsArea.innerHTML = `
      <button class="action-btn open-file-btn" title="Open download directory">📁</button>
    `;
    actionsArea.querySelector(".open-file-btn").addEventListener("click", async () => {
      try {
        await invoke("open_download_folder");
      } catch (err) {
        alert("Failed to open downloads folder: " + err);
      }
    });

    // Move card to completed list
    listCompletedEl.appendChild(dbItem.el);
    updateMetrics();
  });

  // 5. Download failed listener
  await listen("download-failed", (event) => {
    const data = event.payload;
    const dbItem = downloadsDb[data.fileName];
    if (!dbItem) return;

    // Update state to failed
    dbItem.status = "failed";
    dbItem.statusPill.className = "status-pill failed";
    dbItem.statusPill.textContent = "Failed";

    // Hide progress bar section
    const progressSec = dbItem.el.querySelector(`#prog-${dbItem.el.id}`);
    if (progressSec) progressSec.style.display = "none";

    // Update metadata label
    dbItem.metaText.textContent = `Error: ${data.error}`;

    // Update action buttons (Add retry/refresh button, remove pause/resume)
    const actionsArea = dbItem.el.querySelector(".card-actions");
    actionsArea.innerHTML = `
      <button class="action-btn retry-btn" title="Retry download">🔄</button>
      <button class="action-btn danger cancel-btn" title="Remove card">🗑️</button>
    `;
    actionsArea.querySelector(".retry-btn").addEventListener("click", () => {
      // For simplicity, we can let user click the checkbox in browser again to retry,
      // or we can remove the card and let them import again.
      dbItem.el.remove();
      delete downloadsDb[data.fileName];
      updateMetrics();
    });
    actionsArea.querySelector(".cancel-btn").addEventListener("click", () => {
      dbItem.el.remove();
      delete downloadsDb[data.fileName];
      updateMetrics();
    });

    // Move card to failed list
    listFailedEl.appendChild(dbItem.el);
    updateMetrics();
  });

  // 6. Global error listener
  await listen("download-error", (event) => {
    console.error("Global listener error:", event.payload);
    showToast(event.payload);
  });
}

function showToast(message) {
  const toast = document.createElement("div");
  toast.className = "global-toast";
  toast.textContent = message;
  
  Object.assign(toast.style, {
    position: "fixed",
    bottom: "20px",
    left: "50%",
    transform: "translateX(-50%)",
    background: "#ef4444",
    color: "#ffffff",
    padding: "12px 24px",
    borderRadius: "8px",
    fontSize: "0.85rem",
    fontWeight: "600",
    boxShadow: "0 4px 12px rgba(0,0,0,0.3)",
    zIndex: "9999",
    transition: "opacity 0.3s ease"
  });

  document.body.appendChild(toast);
  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => toast.remove(), 300);
  }, 4000);
}
