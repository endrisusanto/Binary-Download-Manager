const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let downloadListEl;
let emptyStateEl;
let queueCountEl;
let listenerStatusEl;

// Dictionary keeping track of download elements by file name
const activeDownloads = {};

window.addEventListener("DOMContentLoaded", () => {
  // Elements
  downloadListEl = document.querySelector("#download-list");
  emptyStateEl = document.querySelector("#empty-state");
  queueCountEl = document.querySelector("#queue-count");
  listenerStatusEl = document.querySelector("#listener-status");

  // Open download folder button
  document.querySelector("#btn-open-folder").addEventListener("click", async () => {
    try {
      await invoke("open_download_folder");
    } catch (err) {
      alert("Failed to open downloads folder: " + err);
    }
  });

  // Clear queue button
  document.querySelector("#btn-clear-queue").addEventListener("click", () => {
    downloadListEl.innerHTML = "";
    Object.keys(activeDownloads).forEach(key => delete activeDownloads[key]);
    updateQueueUI();
  });

  // Setup Tauri Event Listeners
  setupTauriListeners();
});

function updateQueueUI() {
  const count = Object.keys(activeDownloads).length;
  queueCountEl.textContent = `${count} item${count === 1 ? "" : "s"}`;
  
  if (count === 0) {
    emptyStateEl.style.display = "flex";
    downloadListEl.style.display = "none";
  } else {
    emptyStateEl.style.display = "none";
    downloadListEl.style.display = "flex";
  }
}

function formatBytes(bytes, decimals = 2) {
  if (!bytes) return "0 Bytes";
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ["Bytes", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(dm)) + " " + sizes[i];
}

async function setupTauriListeners() {
  // 1. Connection status listener
  await listen("listener-status", (event) => {
    const payload = event.payload;
    const dot = listenerStatusEl.querySelector(".status-dot");
    const text = listenerStatusEl.querySelector(".status-text");

    if (payload.active) {
      dot.className = "status-dot online";
      text.textContent = `Active: ${payload.bind}`;
    } else {
      dot.className = "status-dot offline";
      text.textContent = "Offline (Port occupied)";
      console.error(payload.error);
    }
  });

  // 2. Download started listener
  await listen("download-started", (event) => {
    const file = event.payload;
    const fileName = file.fileName;

    // Create item DOM structure
    const itemEl = document.createElement("div");
    itemEl.className = "download-item downloading";
    itemEl.innerHTML = `
      <div class="item-main">
        <div class="file-info">
          <div class="file-name">${fileName}</div>
          <div class="file-meta">Path: ${file.serverPath} | Size: ${formatBytes(file.expectedSize)}</div>
        </div>
        <div class="item-status">
          <span class="status-pill downloading">Downloading</span>
        </div>
      </div>
      
      <div class="progress-section">
        <div class="progress-container">
          <div class="progress-bar" style="width: 0%"></div>
        </div>
        <div class="progress-percent">0%</div>
      </div>

      <div class="notes-accordion" style="display: none;">
        <div class="accordion-toggle">Show Details</div>
        <div class="accordion-content" style="display: none;"></div>
      </div>
    `;

    // Accordion setup
    const toggle = itemEl.querySelector(".accordion-toggle");
    const content = itemEl.querySelector(".accordion-content");
    toggle.addEventListener("click", () => {
      toggle.classList.toggle("active");
      const isVisible = content.style.display === "block";
      content.style.display = isVisible ? "none" : "block";
      toggle.textContent = isVisible ? "Show Details" : "Hide Details";
    });

    // Store reference
    activeDownloads[fileName] = {
      el: itemEl,
      progressBar: itemEl.querySelector(".progress-bar"),
      percentText: itemEl.querySelector(".progress-percent"),
      statusPill: itemEl.querySelector(".status-pill"),
      notesArea: itemEl.querySelector(".notes-accordion"),
      notesContent: content
    };

    downloadListEl.appendChild(itemEl);
    updateQueueUI();
  });

  // 3. Download progress listener
  await listen("download-progress", (event) => {
    const progress = event.payload;
    const item = activeDownloads[progress.file_name];
    if (!item) return;

    const percent = Math.min(
      100,
      Math.round((progress.bytes_downloaded / progress.total_bytes) * 100)
    );

    item.progressBar.style.width = `${percent}%`;
    item.percentText.textContent = `${percent}%`;
    
    // Update label with current bytes
    const expectedStr = formatBytes(progress.total_bytes);
    const downloadedStr = formatBytes(progress.bytes_downloaded);
    const metaEl = item.el.querySelector(".file-meta");
    const currentMetaText = metaEl.textContent.split(" | ")[0]; // Get Server Path
    metaEl.textContent = `${currentMetaText} | Size: ${downloadedStr} of ${expectedStr}`;
  });

  // 4. Download finished listener
  await listen("download-finished", (event) => {
    const report = event.payload;
    const fileName = report.fileName;
    const item = activeDownloads[fileName];
    if (!item) return;

    // Update state to success
    item.el.className = "download-item success";
    item.statusPill.className = "status-pill success";
    item.statusPill.textContent = report.status === "downloaded" ? "Completed" : "Replayed";
    item.progressBar.style.width = "100%";
    item.percentText.textContent = "100%";

    // Set notes
    item.notesContent.textContent = report.notes.join("\n");
    item.notesArea.style.display = "block";
    
    // Auto-scroll details box if visible
    item.notesContent.scrollTop = item.notesContent.scrollHeight;
  });

  // 5. Download failed listener (specific item)
  await listen("download-failed", (event) => {
    const data = event.payload;
    const item = activeDownloads[data.fileName];
    if (!item) return;

    // Update state to failed
    item.el.className = "download-item failed";
    item.statusPill.className = "status-pill failed";
    item.statusPill.textContent = "Failed";
    
    item.notesContent.textContent = data.error;
    item.notesArea.style.display = "block";
  });

  // 6. Global error listener
  await listen("download-error", (event) => {
    console.error("Global listener error:", event.payload);
    // Render a transient toast alert
    showToast(event.payload);
  });
}

function showToast(message) {
  const toast = document.createElement("div");
  toast.className = "global-toast";
  toast.textContent = message;
  
  // Quick styles for toast
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
